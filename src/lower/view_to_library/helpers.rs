//! View-helper call construction. Translates classified view-helper
//! kinds (`link_to`, `dom_id`, `pluralize`, …) into spinel-shape
//! `ViewHelpers.*` / `RouteHelpers.*` / `Inflector.*` Sends, and
//! handles URL-position argument lowering.

use crate::expr::{Expr, ExprNode, InterpPart, Literal};
use crate::ident::Symbol;
use crate::naming::{singularize, snake_case};
use crate::span::Span;

use crate::lower::view::{
    classify_nested_url_element, classify_view_url_arg, NestedUrlElement, ViewHelperKind,
    ViewUrlArg,
};

use super::attr_parts::{
    append_attr_parts, default_form_class, default_method_sym, lit_str_coerce, string_interp,
    take_opt,
};
use super::walker::rewrite_helpers_in_expr;
use super::{
    bare_record_name, lit_str, lit_sym, route_helpers_call, send, var_ref,
    view_helpers_call, ViewCtx,
};

/// `turbo_stream.<action>(target[, record])` → `Broadcasts
/// .turbo_stream_fragment("<action>", <target>, <html>)`.
///
/// The markup itself stays in each target's hand-written `Broadcasts`
/// (the same composer the model-side `broadcast_append_to` path uses),
/// so a change to the `<turbo-stream>` element has one owner rather than
/// a second copy here.
///
/// TARGET: a bare record is its `dom_id`, which is what Rails does — a
/// Symbol or String is the id verbatim, and anything else (typically an
/// explicit `dom_id(...)`) is already a String.
///
/// HTML: a bare record renders ITS partial, by the same dir/singular
/// convention `render @record` uses — `@message` → `Views::Messages
/// .message(message)`. `remove` carries no `<template>`, so it gets "".
/// Is this receiver the `turbo_stream` builder? Used to tell "not a
/// turbo_stream call at all" from "a turbo_stream call we don't lower",
/// so only the latter files a residue line.
pub(super) fn is_turbo_stream_builder(recv: Option<&Expr>) -> bool {
    match recv.map(|r| &*r.node) {
        Some(ExprNode::Send { recv: None, method, args, block: None, .. }) => {
            method.as_str() == "turbo_stream" && args.is_empty()
        }
        Some(ExprNode::Var { name, .. }) => name.as_str() == "turbo_stream",
        _ => false,
    }
}

/// `turbo_stream.<action> target, partial: "…"` with optional `locals:`,
/// `collection:` and `as:`: the option form, rendering a partial.
pub(super) struct TurboStreamPartial<'a> {
    pub target: &'a Expr,
    pub partial: &'a str,
    pub locals: Option<&'a [(Expr, Expr)]>,
    pub collection: Option<&'a Expr>,
    pub as_name: Option<&'a str>,
}

/// The option form the walker lowers, for it and for the render graph
/// alike, so the graph has an edge exactly where a partial is rendered.
/// None for an option it does not read (`layout:`), a `partial:` or
/// `locals:` that is not a literal, or `as:` without `collection:` —
/// dropping any of them would render the partial wrong.
pub(super) fn turbo_stream_partial<'a>(
    recv: Option<&Expr>,
    method: &str,
    args: &'a [Expr],
) -> Option<TurboStreamPartial<'a>> {
    let renders = matches!(method, "append" | "prepend" | "replace" | "update" | "before" | "after");
    if !renders || !is_turbo_stream_builder(recv) {
        return None;
    }
    let [target, opts] = args else { return None };
    let ExprNode::Hash { entries, .. } = &*opts.node else { return None };
    let (mut partial, mut locals, mut collection, mut as_name) = (None, None, None, None);
    for (k, v) in entries {
        let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else { return None };
        match (key.as_str(), &*v.node) {
            ("partial", ExprNode::Lit { value: Literal::Str { value } }) => partial = Some(value.as_str()),
            ("locals", ExprNode::Hash { entries, .. }) => locals = Some(entries.as_slice()),
            ("collection", _) => collection = Some(v),
            ("as", ExprNode::Lit { value: Literal::Sym { value } }) => as_name = Some(value.as_str()),
            ("as", ExprNode::Lit { value: Literal::Str { value } }) => as_name = Some(value.as_str()),
            _ => return None,
        }
    }
    if as_name.is_some() && collection.is_none() {
        return None;
    }
    Some(TurboStreamPartial { target, partial: partial?, locals, collection, as_name })
}

/// The `<turbo-stream target=…>` value for a `turbo_stream.<action>`
/// call's first argument. Shared by all three spellings — positional,
/// option-hash and block — so they cannot disagree about what a bare
/// record means.
///
/// A bare record is its `dom_id`, which is what Rails does; a Symbol is
/// the id verbatim; anything else (typically an explicit `dom_id(...)`)
/// is already a String.
pub(super) fn turbo_stream_target(target: &Expr, ctx: &ViewCtx) -> Expr {
    use crate::expr::Literal;
    match &*target.node {
        ExprNode::Lit { value: Literal::Sym { value } } => lit_str(value.as_str().to_string()),
        ExprNode::Var { .. } | ExprNode::Ivar { .. } => {
            view_helpers_call("dom_id", vec![rewrite_helpers_in_expr(target, ctx)])
        }
        _ => rewrite_helpers_in_expr(target, ctx),
    }
}

/// `Broadcasts.turbo_stream_fragment("<action>", <target>, <html>)`.
/// The markup lives in each target's hand-written `Broadcasts` — the
/// same composer the model-side `broadcast_append_to` path uses — so a
/// change to the `<turbo-stream>` element has one owner.
pub(super) fn turbo_stream_fragment_call(action: &str, target: Expr, html: Expr) -> Expr {
    send(
        Some(Expr::new(
            Span::synthetic(),
            ExprNode::Const { path: vec![Symbol::from("Broadcasts")] },
        )),
        "turbo_stream_fragment",
        vec![lit_str(action.to_string()), target, html],
        None,
        true,
    )
}

pub(super) fn emit_turbo_stream_fragment(
    ts: &crate::lower::view::TurboStreamCall<'_>,
    ctx: &ViewCtx,
) -> Option<Expr> {
    use crate::naming::pluralize_snake;

    let record_name = |e: &Expr| match &*e.node {
        ExprNode::Var { name, .. } | ExprNode::Ivar { name } => Some(name.as_str().to_string()),
        _ => None,
    };

    let target = turbo_stream_target(ts.target, ctx);

    let html = match ts.content {
        None => lit_str(String::new()),
        // Markup the controller already rendered
        // (`turbo_stream.append target, @message_html`). turbo-rails'
        // `render_template` renders a record's partial only when the
        // content has a `to_partial_path`; anything else goes into the
        // template as given, marked html_safe, so a String is not
        // escaped.
        Some(content)
            if matches!(content.ty, Some(crate::ty::Ty::Str))
                || record_name(content).is_some_and(|name| ctx.str_ivars.contains(&name)) =>
        {
            rewrite_helpers_in_expr(content, ctx)
        }
        Some(content) => {
            // Only the render-this-record form. Anything else (a literal
            // string, a nested call) would need the partial machinery a
            // `render` call site gets, so decline and leave the source
            // shape for the residue ledger.
            // turbo-rails renders it `formats: :html` (`render_record`),
            // whatever template the builder is written in.
            let name = record_name(content)?;
            let partial = format!("{}/{}", pluralize_snake(&name), name);
            super::partial::named_partial_call(
                &partial,
                Some(&rewrite_helpers_in_expr(content, ctx)),
                None,
                &ctx.in_formats(&["html"]),
            )?
        }
    };

    Some(turbo_stream_fragment_call(ts.action, target, html))
}

pub(super) fn emit_view_helper_call(kind: &ViewHelperKind<'_>, ctx: &ViewCtx) -> Option<Expr> {
    use ViewHelperKind::*;
    match kind {
        TurboStreamFrom { streamables } => {
            let (name, channel) = view_stream_from(streamables, ctx)?;
            Some(view_helpers_call("turbo_stream_from", vec![name, channel]))
        }
        // `attrs` is a Hash literal whose VALUES can themselves be a
        // helper call (the HAML compiler's shortcut-class merge emits
        // `render_attrs({ class: haml_class("g", k), … })`) — thread it
        // back through the walk first, so a nested helper reaches its
        // own `ActionView::ViewHelpers.*` emit instead of surviving as a
        // bare, unqualified call the Views module has no method for.
        RenderAttrs { attrs } => {
            Some(view_helpers_call("render_attrs", vec![rewrite_helpers_in_expr(attrs, ctx)]))
        }
        HamlClass { static_classes, value } => Some(view_helpers_call(
            "haml_class",
            vec![rewrite_helpers_in_expr(static_classes, ctx), rewrite_helpers_in_expr(value, ctx)],
        )),
        DomId { record, prefix } => {
            let mut args = vec![(*record).clone()];
            if let Some(p) = prefix {
                args.push((*p).clone());
            }
            Some(view_helpers_call("dom_id", args))
        }
        // Rails' `pluralize` returns a String that isn't html-safe (the
        // word is the caller's), so the template escapes it, as it does
        // `truncate`'s.
        Pluralize { count, word } => {
            let pluralized = crate::lower::view::pluralize_helper_call(
                (*count).clone(), (*word).clone(),
            );
            Some(view_helpers_call("html_escape", vec![pluralized]))
        }
        Truncate { text, opts } => {
            // Spinel-blog convention: `truncate` returns a plain
            // (non-html-safe) string, so its output gets wrapped in
            // `html_escape` before going to `io`. Other helpers
            // (link_to / button_to / dom_id / turbo_stream_from /
            // pluralize / content_for_get) return strings that are
            // already escape-correct and pass through raw.
            // `.to_s` on the text for the same reason as link_to's
            // label: it is routinely a nullable column read
            // (`truncate(article.body, length: 100)`) and the runtime
            // helper is monomorphic `(String, …) -> String`. Rails
            // renders nil as the empty string here too.
            let mut args = vec![lit_str_coerce((*text).clone())];
            if let Some(o) = opts {
                args.push((*o).clone());
            }
            let truncated = view_helpers_call("truncate", args);
            Some(view_helpers_call("html_escape", vec![truncated]))
        }
        // Rails' `simple_format` with `sanitize: false` writes the text as
        // it is (`raw`) inside each paragraph's tag and returns an html-safe
        // buffer: appended as it is. The text gets `.to_s` as `truncate`'s
        // does (a nullable column read; Rails renders nil as one empty
        // paragraph, as it does ""). A literal `nil` is that `""` already.
        SimpleFormat { text, html_opts, wrapper_tag } => {
            let text = match &*text.node {
                ExprNode::Lit { value: Literal::Nil } => lit_str(String::new()),
                _ => lit_str_coerce((*text).clone()),
            };
            let mut args = vec![text, lit_sym(wrapper_tag.clone())];
            if let Some(o) = html_opts {
                args.push((*o).clone());
            }
            Some(view_helpers_call("simple_format", args))
        }
        WordWrap { text, opts } => {
            let mut args = vec![lit_str_coerce((*text).clone())];
            if let Some(o) = opts {
                args.push((*o).clone());
            }
            let wrapped = view_helpers_call("word_wrap", args);
            Some(view_helpers_call("html_escape", vec![wrapped]))
        }
        // `content_tag_html` with the content escaped here: the runtime's
        // `content_tag` is that, but its optional `content` makes Go's
        // signature variadic `interface{}`, which takes no options map.
        // The name is checked and the options written through
        // `render_attrs` there, so the String is appended unescaped. The
        // options take the block form's shape: a positional Hash (Crystal
        // binds no keyword to `opts`), with a literal `class:` Array
        // space-joined.
        ContentTag { name, content, opts } => {
            // Rails escapes the content unless it is html-safe: an
            // `x.html_safe` or `raw(x)` (`x.to_s.html_safe`), an escape the
            // template applied (`h(x)`), or a helper
            // whose call here is markup (`link_to`, a nested `content_tag`,
            // `turbo_stream_from`'s element, `content_for`'s capture). A
            // `truncate` or `word_wrap` (an html-safe buffer, alone or in a
            // `+`) is written as it is below, by `safe_buffer_value`; a
            // `pluralize`, a plain String, is escaped once. The other kinds
            // (`dom_id`) are escaped like any String.
            let is_helper_markup = |e: &Expr| matches!(&*e.node,
                ExprNode::Send { recv: None, method, args, block: None, parenthesized }
                    if matches!(
                        super::walker::classify_template_helper(method.as_str(), args, *parenthesized, ctx),
                        Some(LinkTo { .. } | LinkToIf { .. } | ButtonTo { .. } | ContentTag { .. }
                            | CsrfMetaTags | CspMetaTag | StylesheetLinkTag { .. }
                            | JavascriptImportmapTags { .. } | EngineImportmapTags { .. }
                            | TurboStreamFrom { .. } | ContentForGetter { .. })
                    ));
            let helper_markup = is_helper_markup(content);
            let raw_value = match &*content.node {
                ExprNode::Send { recv: None, method, args, block: None, .. }
                    if method.as_str() == "raw" && args.len() == 1 && !ctx.is_local("raw") =>
                {
                    Some(args[0].clone())
                }
                _ => None,
            };
            // An escape the template already applied (`h(x)`,
            // `ERB::Util.html_escape(x)`) is html-safe in Rails too: escaped
            // here once, through the view helpers' `html_escape`.
            let escaped_value = match &*content.node {
                ExprNode::Send { recv: None, method, args, block: None, .. }
                    if method.as_str() == "h" && args.len() == 1 && !ctx.is_local("h") =>
                {
                    Some(&args[0])
                }
                ExprNode::Send { recv: Some(r), method, args, block: None, .. }
                    if matches!(method.as_str(), "html_escape" | "h")
                        && args.len() == 1
                        && matches!(&*r.node, ExprNode::Const { path } if path.iter().map(|p| p.as_str()).eq(["ERB", "Util"])) =>
                {
                    Some(&args[0])
                }
                _ => None,
            };
            let content = match raw_value.or_else(|| super::walker::html_safe_value(content, ctx)) {
                Some(safe) => lit_str_coerce(rewrite_helpers_in_expr(&safe, ctx)),
                None if helper_markup => lit_str_coerce(rewrite_helpers_in_expr(content, ctx)),
                None => {
                    // `h` leaves an html-safe argument as it is
                    // (`h(x.html_safe)`, `h(raw(x))`, `h(content_tag(…))`,
                    // a helper's markup as the bare content takes it).
                    let safe_arg = escaped_value.and_then(|v| match &*v.node {
                        ExprNode::Send { recv: None, method, args, block: None, .. }
                            if method.as_str() == "raw" && args.len() == 1 && !ctx.is_local("raw") =>
                        {
                            Some(args[0].clone())
                        }
                        _ if is_helper_markup(v) => Some(v.clone()),
                        _ => super::walker::html_safe_value(v, ctx),
                    });
                    match safe_arg {
                        Some(safe) => lit_str_coerce(rewrite_helpers_in_expr(&safe, ctx)),
                        None => {
                            let lowered = rewrite_helpers_in_expr(escaped_value.unwrap_or(content), ctx);
                            // Content Rails holds as an html-safe buffer
                            // (`truncate(x)`, `truncate(x) + "!"`) is written
                            // as it is, and `h` leaves one as it is too; a
                            // `pluralize` or any other String is escaped.
                            match super::walker::safe_buffer_value(&lowered, ctx) {
                                Some(safe) if matches!(&*safe.node, ExprNode::Send { method, .. } if matches!(method.as_str(), "html_escape" | "+")) => safe,
                                Some(safe) => lit_str_coerce(safe),
                                None => view_helpers_call("html_escape", vec![lit_str_coerce(lowered)]),
                            }
                        }
                    }
                }
            };
            let mut args = vec![rewrite_helpers_in_expr(name, ctx), content];
            if let Some(o) = opts {
                let o = match &*o.node {
                    ExprNode::Hash { entries, kwargs } => Expr::new(o.span, ExprNode::Hash {
                        entries: entries.iter().map(|(k, v)| (k.clone(), super::form_with::simplify_opts_value(k, v))).collect(),
                        kwargs: *kwargs,
                    }),
                    _ => (*o).clone(),
                };
                args.push(rewrite_helpers_in_expr(&o, ctx));
            }
            Some(view_helpers_call("content_tag_html", args))
        }
        ContentForGetter { slot } => Some(view_helpers_call(
            "content_for_get",
            vec![lit_sym(Symbol::from(*slot))],
        )),
        LinkTo { text, url, opts } => emit_link_to_inline(text, url, *opts, ctx),
        // Inlined as an `if` over the two `link_to` halves rather than
        // dispatched to the runtime helper, so the label is escaped
        // ONCE: a `truncate(…)` label lowers to `html_escape(truncate)`
        // on its own and a runtime `link_to` would escape it again,
        // where Rails' `truncate` is already an html_safe buffer.
        //
        // A url the lowering cannot resolve statically (campfire's
        // `opengraph_embed.href`, a reader on the local) goes to the
        // runtime `link_to`, which escapes its label itself — so a
        // label the Truncate arm already wrapped is handed over
        // unwrapped, and escaped once either way.
        LinkToIf { cond, text, url, opts, negate } => {
            // The label as the template's own helpers lower it: a
            // `truncate(…)` arrives as `html_escape(truncate(…))`,
            // already escape-correct, which the two branches below
            // each take exactly once. Only the view helpers' own
            // `html_escape`: an app's may not escape. `ERB::Util`'s
            // escape (that exact constant) is the same escape, and is
            // written as the view helpers' one.
            // Its argument is kept as the pre-escaped text before the
            // escape is built, since a literal's escape folds to text that
            // no longer looks escaped.
            let erb_util_arg = match &*text.node {
                ExprNode::Send { recv: Some(r), method, args, block: None, .. }
                    if matches!(method.as_str(), "html_escape" | "h")
                        && args.len() == 1
                        && matches!(&*r.node, ExprNode::Const { path } if path.iter().map(|p| p.as_str()).eq(["ERB", "Util"])) =>
                {
                    Some(lit_str_coerce(rewrite_helpers_in_expr(&args[0], ctx)))
                }
                _ => None,
            };
            let (lowered, pre_escaped) = match erb_util_arg {
                Some(arg) => (view_helpers_call("html_escape", vec![arg.clone()]), Some(arg)),
                None => {
                    let lowered = rewrite_helpers_in_expr(text, ctx);
                    let pre_escaped = match &*lowered.node {
                        ExprNode::Send { recv: Some(r), method, args, .. }
                            if method.as_str() == "html_escape"
                                && args.len() == 1
                                && matches!(&*r.node, ExprNode::Const { path } if path.iter().map(|p| p.as_str()).eq(["ActionView", "ViewHelpers"])) =>
                        {
                            Some(args[0].clone())
                        }
                        _ => None,
                    };
                    (lowered, pre_escaped)
                }
            };
            let linked = match emit_link_to_inline(text, url, *opts, ctx) {
                Some(inline) => inline,
                None => {
                    let label = pre_escaped.clone().unwrap_or_else(|| lit_str_coerce(lowered.clone()));
                    // `:back` is Rails' `url_for(:back)`, the referrer, not a String to escape.
                    let href = if matches!(&*url.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "back") {
                        view_helpers_call("back_url", Vec::new())
                    } else {
                        rewrite_helpers_in_expr(url, ctx)
                    };
                    let mut args = vec![label, href];
                    if let Some(o) = opts {
                        args.push((*o).clone());
                    }
                    view_helpers_call("link_to", args)
                }
            };
            // The label as a bare `<%= %>` shows it: a buffer (`x.html_safe`,
            // a marked app method's call, a `truncate`, any of them on the
            // left of a `+`) as it is, the `+`'s plain operand escaped;
            // anything else escaped whole.
            let bare = if pre_escaped.is_some() {
                lowered
            } else {
                match super::walker::safe_buffer_value(&lowered, ctx) {
                    Some(safe) if matches!(&*safe.node, ExprNode::Send { method, .. } if matches!(method.as_str(), "html_escape" | "+")) => safe,
                    Some(safe) => lit_str_coerce(safe),
                    None => view_helpers_call("html_escape", vec![lit_str_coerce(lowered)]),
                }
            };
            // `link_to_unless` links when the condition doesn't hold: the
            // same branches, swapped.
            let (then_branch, else_branch) = if *negate { (bare, linked) } else { (linked, bare) };
            Some(Expr::new(
                Span::synthetic(),
                ExprNode::If {
                    // The condition is a template condition in all but
                    // spelling: the same predicate grounding an `if`
                    // gets (`href.present?` on a nilable reader is a
                    // nil test, not a dispatch no compiled target has).
                    cond: super::predicates::rewrite_predicates(
                        &rewrite_helpers_in_expr(cond, ctx),
                        &ctx.nullable_locals,
                        &ctx.reference_reads,
                        &ctx.nilable_scalar_reads,
                    ),
                    then_branch,
                    else_branch,
                },
            ))
        }
        ButtonTo { text, target, opts } => emit_button_to_inline(text, target, *opts, ctx),
        // Layout-`<head>` helpers — bare zero-arg ViewHelpers calls.
        CsrfMetaTags => Some(view_helpers_call("csrf_meta_tags", Vec::new())),
        CspMetaTag => Some(view_helpers_call("csp_meta_tag", Vec::new())),
        // `javascript_importmap_tags` consumes per-app importmap data:
        // emit `Importmap.pins` and `Importmap.entry` as args. Both
        // are class methods on the generated `Importmap` module
        // (lower_importmap_to_library_functions). The runtime helper
        // iterates pins to emit modulepreload links + the importmap-
        // script JSON, matching Rails' shape. Other targets (Rust /
        // Python / Crystal / Go) still consume `Importmap::PINS` as
        // a constant; their per-target emit will migrate to the
        // method form when each target retires its lowered_importmap
        // emitter.
        JavascriptImportmapTags { entry: Some(entry) } => {
            let pins = super::send(
                Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Const { path: vec![Symbol::from("Importmap")] },
                )),
                "pins",
                vec![],
                None,
                true,
            );
            Some(view_helpers_call("javascript_importmap_tags", vec![pins, (*entry).clone()]))
        }
        EngineImportmapTags { entry, module } => {
            let pins = super::send(
                Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Const { path: vec![Symbol::from("Importmap")] },
                )),
                &crate::lower::importmap_to_library::engine_pins_name(module),
                vec![],
                None,
                true,
            );
            Some(view_helpers_call("javascript_importmap_tags", vec![pins, (*entry).clone()]))
        }
        JavascriptImportmapTags { entry: None } => {
            let pins = super::send(
                Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Const { path: vec![Symbol::from("Importmap")] },
                )),
                "pins",
                vec![],
                None,
                true,
            );
            let entry = super::send(
                Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Const { path: vec![Symbol::from("Importmap")] },
                )),
                "entry",
                vec![],
                None,
                true,
            );
            Some(view_helpers_call("javascript_importmap_tags", vec![pins, entry]))
        }
        // `<%= stylesheet_link_tag :app, "data-turbo-track":
        // "reload" %>` — first arg is the stylesheet group. A SYMBOL
        // names a group rather than a file, and Propshaft resolves a
        // group to one `<link>` per stylesheet on the load path
        // (`app/assets/stylesheets/` + `app/assets/builds/`, which is
        // what `ctx.stylesheets` holds); we mirror that expansion so the
        // rendered head matches structurally. A STRING names one file
        // and passes through.
        //
        // Any symbol, not a hard-coded `:app`. The blog fixture writes
        // `:app` and campfire writes `:all` — Propshaft's own spelling
        // for the same thing — and keying on the blog's spelling meant
        // campfire's twenty-six stylesheets collapsed to a single
        // `<link href="/assets/all.css">` that nothing produced: a chat
        // application served entirely unstyled, at 200.
        //
        // Excluded: a symbol that IS one of the app's stylesheet stems.
        // An app with `all.css` writing `stylesheet_link_tag :all` means
        // that one file, and expanding it would be wrong in the
        // direction that is hard to notice (every stylesheet still
        // arrives, just also the others).
        StylesheetLinkTag { name, opts } => {
            if let ExprNode::Lit { value: Literal::Sym { value } } = &*name.node {
                if !ctx.stylesheets.is_empty()
                    && !ctx.stylesheets.iter().any(|s| s == value.as_str())
                {
                    let mut calls: Vec<Expr> = Vec::new();
                    for sheet in &ctx.stylesheets {
                        let mut args = vec![lit_str(sheet.clone())];
                        if let Some(o) = opts {
                            args.push((*o).clone());
                        }
                        calls.push(view_helpers_call("stylesheet_link_tag", args));
                    }
                    // Chain calls with " + \"\\n\" + " so subsequent links
                    // render flush-left — matches Rails' helper output where
                    // only the first stylesheet gets the source indent and
                    // the rest are at column 0. (Same shape as Rails'
                    // `javascript_importmap_tags` modulepreload list.)
                    let sep = lit_str("\n".to_string());
                    let mut chain = calls.remove(0);
                    for call in calls {
                        chain = send(Some(chain), "+", vec![sep.clone()], None, false);
                        chain = send(Some(chain), "+", vec![call], None, false);
                    }
                    return Some(chain);
                }
            }
            let name_expr = match &*name.node {
                ExprNode::Lit { value: Literal::Sym { value } } => {
                    lit_str(value.as_str().to_string())
                }
                _ => (*name).clone(),
            };
            let mut args = vec![name_expr];
            if let Some(o) = opts {
                args.push((*o).clone());
            }
            Some(view_helpers_call("stylesheet_link_tag", args))
        }
        // ContentForSetter is statement-level (handled in walk_stmt);
        // returning None here forwards a TODO so any unexpected
        // `<%= content_for :slot, body %>` form surfaces as a no-op
        // append rather than silent-passing through.
        ContentForSetter { .. } => None,
    }
}

/// Spell the stream name for `turbo_stream_from <streamables…>`. The
/// SUBSCRIBE half of the wire whose PUBLISH half is the model-side
/// `broadcast_*_to` lowering; both call `lower::broadcasts::stream_name`
/// so the two names cannot drift apart.
///
/// A bare name matching a model singular is that record (`room` →
/// `room_#{room.id}`) — the same name signal `apply_route_param_lowering`
/// and the view lowerer's `ivar_ty` already commit to. A literal is its
/// own text. Anything else declines the whole call rather than
/// subscribing to a name we guessed at; a `channel:` kwarg is dropped
/// with a ledger line, since our cable transport has no per-channel
/// classes to route to.
/// The whole `turbo_stream_from` call: its stream name and the channel
/// class the subscriber will name.
///
/// TWO HALVES OF ONE ATTRIBUTE SET, kept together because the channel
/// decides whether the stream name is even reachable. turbo-rails 2.0.16
/// spells the default as
/// `attributes[:channel]&.to_s || "Turbo::StreamsChannel"`, and an app
/// naming a custom one is routing the subscription AWAY from the stock
/// channel deliberately: campfire prepends a guard onto
/// `Turbo::StreamsChannel` that refuses its `:messages` streams, so
/// `RoomMessagesChannel` is the only door onto them. Dropping the option
/// used to point the client at the door the app had nailed shut.
fn view_stream_from(streamables: &[Expr], ctx: &ViewCtx) -> Option<(Expr, Expr)> {
    let name = view_stream_name(streamables, ctx)?;
    let channel = streamables
        .iter()
        .find_map(channel_option)
        .unwrap_or_else(|| "Turbo::StreamsChannel".to_string());
    Some((name, str_lit(channel)))
}

/// `channel:` out of a trailing options hash, as a String.
///
/// Rails accepts a String or a class name (`channel: RoomChannel`) and
/// calls `.to_s` on either, so a constant contributes its own spelling.
/// Anything else — a local, a method call — is a value only the request
/// knows, and there is nothing to fold at lower time, so it declines and
/// the default stands.
fn channel_option(arg: &Expr) -> Option<String> {
    let ExprNode::Hash { entries, .. } = &*arg.node else { return None };
    for (k, v) in entries {
        let key = match &*k.node {
            ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
            ExprNode::Lit { value: Literal::Str { value } } => value.clone(),
            _ => continue,
        };
        if key != "channel" {
            continue;
        }
        return match &*v.node {
            ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
            // `channel: RoomChannel` — a constant path contributes its
            // own spelling, which is what Rails' `.to_s` on the class
            // produces. Joined with `::` so a namespaced channel keeps
            // the name the subscriber will actually send.
            ExprNode::Const { path } => Some(
                path.iter().map(|p| p.as_str()).collect::<Vec<_>>().join("::"),
            ),
            _ => None,
        };
    }
    None
}

fn str_lit(value: String) -> Expr {
    Expr::new(
        crate::span::Span::synthetic(),
        ExprNode::Lit { value: Literal::Str { value } },
    )
}

fn view_stream_name(streamables: &[Expr], ctx: &ViewCtx) -> Option<Expr> {
    use crate::lower::broadcasts::Streamable;
    // ONE streamable that is not a bare record name is the app spelling
    // the stream itself — `turbo_stream_from "article_#{@article.id}
    // _comments"` is the blog's, and the name it builds is already the
    // whole convention. Pass it through untouched; only the multi-part
    // form needs a spelling, because only it has parts to join.
    if let [only] = streamables {
        let bare = matches!(&*only.node,
            ExprNode::Lit { value: Literal::Sym { .. } })
            || streamable_record_name(only).is_some_and(|n| ctx.model_singulars.contains(&n));
        if !bare {
            return Some((*only).clone());
        }
    }
    let mut parts = Vec::new();
    for arg in streamables {
        match &*arg.node {
            ExprNode::Lit { value: Literal::Sym { value } } => {
                parts.push(Streamable::Literal(value.as_str().to_string()))
            }
            ExprNode::Lit { value: Literal::Str { value } } => {
                parts.push(Streamable::Literal(value.clone()))
            }
            // The trailing options hash (`channel: "RoomMessagesChannel"`),
            // read by the caller — it is an attribute of the whole call,
            // not one more element of the stream name.
            ExprNode::Hash { .. } => {}
            _ => {
                let name = streamable_record_name(arg)?;
                if !ctx.model_singulars.contains(&name) {
                    return None;
                }
                parts.push(Streamable::Record {
                    record: arg.clone(),
                });
            }
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(crate::lower::broadcasts::stream_name(&parts))
}

/// Inline-expand `link_to text, url, opts` into a single
/// StringInterp Expr: `<a href="<escaped_href>"<opts>>
/// <html_escape(text)></a>`. Retires the runtime `ViewHelpers.link_to`
/// call (HashMap-shaped opts) — same architectural rationale as the
/// form_with macro-inline (Wedges 1b-i + 1b-ii). The URL position
/// goes through `emit_url_arg` so path-helpers + record refs
/// resolve to `RouteHelpers.<x>_path(...)` calls before lowering
/// into the interp.
fn emit_link_to_inline(
    text: &Expr,
    url: &Expr,
    opts: Option<&Expr>,
    ctx: &ViewCtx,
) -> Option<Expr> {
    let (prefix, suffix) = link_to_wrapper_parts(url, opts, ctx, inert_label(text, ctx))?;
    let mut parts = prefix;
    parts.push(InterpPart::Expr { expr: escaped_label(text, ctx) });
    parts.extend(suffix);
    Some(string_interp(parts))
}

/// Whether a `link_to` / `button_to` label runs no code when evaluated (a
/// literal, a local, an ivar, a constant, or an interpolation of those),
/// so writing it after the url can't change what either does.
fn inert_label(text: &Expr, ctx: &ViewCtx) -> bool {
    match &*text.node {
        ExprNode::Lit { .. } | ExprNode::Var { .. } | ExprNode::Ivar { .. } | ExprNode::Const { .. } | ExprNode::SelfRef => true,
        ExprNode::Send { recv: None, method, args, block: None, parenthesized: false } => {
            args.is_empty() && ctx.binds_local(method.as_str())
        }
        ExprNode::StringInterp { parts } => parts.iter().all(|p| match p {
            InterpPart::Text { .. } => true,
            InterpPart::Expr { expr } => inert_label(expr, ctx),
        }),
        _ => false,
    }
}

/// A `link_to` / `button_to` label, escaped once. Its helpers are lowered
/// as the template's own are, so a `pluralize(…)` or `truncate(…)` label
/// reaches its runtime call on every target; one that lowers to
/// `ActionView::ViewHelpers.html_escape(…)` already is escape-correct and is
/// taken as it is. Only that receiver: an app's own `html_escape` may not
/// escape. Else the same `.to_s` the bare-interpolation path applies: the
/// label can be a nullable column read (`link_to article.title, …`), and
/// `html_escape` is deliberately monomorphic `(String) -> String`.
/// `nil.to_s == ""` is what Rails renders.
fn escaped_label(text: &Expr, ctx: &ViewCtx) -> Expr {
    // An escape the template applied (`h(x)`, `ERB::Util.html_escape(x)`)
    // is html-safe in Rails: escaped here once, before the rewrite folds a
    // literal one to text that no longer looks escaped.
    let escaped_arg = match &*text.node {
        ExprNode::Send { recv: None, method, args, block: None, .. }
            if method.as_str() == "h" && args.len() == 1 && !ctx.is_local("h") =>
        {
            Some(&args[0])
        }
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if matches!(method.as_str(), "html_escape" | "h")
                && args.len() == 1
                && matches!(&*r.node, ExprNode::Const { path } if path.iter().map(|p| p.as_str()).eq(["ERB", "Util"])) =>
        {
            Some(&args[0])
        }
        _ => None,
    };
    if let Some(arg) = escaped_arg {
        return view_helpers_call("html_escape", vec![lit_str_coerce(rewrite_helpers_in_expr(arg, ctx))]);
    }
    // A label Rails holds as an html-safe buffer (an escape the view helpers
    // applied, `truncate(x) + "!"`, `x.html_safe`) is written as it is.
    let lowered = rewrite_helpers_in_expr(text, ctx);
    match super::walker::safe_buffer_value(&lowered, ctx) {
        // The view helpers' escape and a `+` of buffers are Strings already.
        Some(safe) if matches!(&*safe.node, ExprNode::Send { method, .. } if matches!(method.as_str(), "html_escape" | "+")) => safe,
        Some(safe) => lit_str_coerce(safe),
        None => view_helpers_call("html_escape", vec![lit_str_coerce(lowered)]),
    }
}

/// The anchor both spellings share, split where the link's CONTENT
/// goes. Same division of labour as [`button_to_wrapper_parts`].
fn link_to_wrapper_parts(
    url: &Expr,
    opts: Option<&Expr>,
    ctx: &ViewCtx,
    string_ok: bool,
) -> Option<(Vec<InterpPart>, Vec<InterpPart>)> {
    let url_expr = emit_url_arg(url, ctx, string_ok)?;
    Some(link_to_wrapper_markup(url_expr, hash_entries(opts)))
}

/// The `<a …>` … `</a>` markup, with the URL already resolved —
/// everything in [`link_to_wrapper_parts`] that does not need a view
/// context.
///
/// Split for the same reason its `button_to` sibling is: `link_to url,
/// opts do … end` is written in HELPER MODULES too, where there is no
/// template to walk, and `lower::tag_builder` expands those. One owner
/// for the markup, so the two spellings cannot drift.
pub(crate) fn link_to_wrapper_markup(
    url_expr: Expr,
    opts_entries: Vec<(Expr, Expr)>,
) -> (Vec<InterpPart>, Vec<InterpPart>) {
    // Rails' attribute order: the html options first, `href` LAST
    // (`link_to` adds it with `html_options["href"] ||= url` once the
    // options are in hand). The runtime `ViewHelpers.link_to` renders
    // the same order, so the two spellings agree byte for byte.
    let mut parts: Vec<InterpPart> = Vec::new();
    parts.push(InterpPart::Text {
        value: "<a".to_string(),
    });
    append_attr_parts(&mut parts, &opts_entries);
    parts.push(InterpPart::Text {
        value: " href=\"".to_string(),
    });
    parts.push(InterpPart::Expr {
        expr: view_helpers_call("html_escape", vec![url_expr]),
    });
    parts.push(InterpPart::Text {
        value: "\">".to_string(),
    });
    let suffix = vec![InterpPart::Text { value: "</a>".to_string() }];
    (parts, suffix)
}

/// Inline-expand `button_to text, url, opts` into the wrapping
/// `<form action="..." method="post" class="<form_class>">...</form>`
/// + method override hidden input + `<button>` + CSRF token hidden
/// input shape Rails' runtime button_to produces. `method:` and
/// `form_class:` are peeled off `opts` at lower time; the rest
/// flow as `<button>` element attributes. CSRF + _method override
/// go through the same runtime primitives form_with uses.
fn emit_button_to_inline(
    text: &Expr,
    url: &Expr,
    opts: Option<&Expr>,
    ctx: &ViewCtx,
) -> Option<Expr> {
    let (prefix, suffix) = button_to_wrapper_parts(url, opts, ctx, inert_label(text, ctx))?;
    let mut parts = prefix;
    parts.push(InterpPart::Expr { expr: escaped_label(text, ctx) });
    parts.extend(suffix);
    Some(string_interp(parts))
}

/// The BLOCK spelling of a helper whose positional form inline-expands
/// — `<%= button_to url, class: "…" do %> …markup… <% end %>`
/// (campfire's join-code regenerate button) and the same for `link_to`
/// (five campfire templates, `messages/_actions` among them).
///
/// Emits STATEMENTS rather than one expression, the shape
/// `emit_tag_builder_inline` uses: the opening markup appends, the block
/// body walks into the SAME accumulator, then the closing appends. That
/// is not a stylistic choice — the block body is template markup
/// (`image_tag`, a literal `<span>`), and Rails treats what the block
/// yields as the element's HTML content. Routing it through the
/// positional form's `html_escape` would render the markup as visible
/// text.
///
/// `None` when the name is not one of those, or when the URL position
/// is a shape `emit_url_arg` declines — the caller then leaves the site
/// to the generic block arm, exactly as before.
pub(super) fn emit_inline_helper_block(
    method: &str,
    args: &[Expr],
    body: &Expr,
    block_params: &[Symbol],
    ctx: &ViewCtx,
) -> Option<Vec<Expr>> {
    // Both helpers put the URL first and an optional attributes hash
    // second; the block replaces the positional form's leading label.
    let url = args.first()?;
    let opts = args.get(1);
    let (prefix, suffix) = match method {
        // The url is evaluated before the block's content, as the
        // markup writes them.
        "button_to" => button_to_wrapper_parts(url, opts, ctx, true)?,
        "link_to" => link_to_wrapper_parts(url, opts, ctx, true)?,
        _ => return None,
    };
    let mut out = vec![super::accumulator_append_call(string_interp(prefix), ctx)];
    let inner_ctx = ctx.with_locals(block_params.iter().map(|p| p.as_str().to_string()));
    out.extend(super::walker::walk_body(body, &inner_ctx));
    out.push(super::accumulator_append_call(string_interp(suffix), ctx));
    Some(out)
}

/// The wrapper both spellings share, split where the button's CONTENT
/// goes: everything up to the open `<button …>` and everything from
/// `</button>` on. One owner for the markup, so the two forms cannot
/// drift.
fn button_to_wrapper_parts(
    url: &Expr,
    opts: Option<&Expr>,
    ctx: &ViewCtx,
    string_ok: bool,
) -> Option<(Vec<InterpPart>, Vec<InterpPart>)> {
    let url_expr = emit_url_arg(url, ctx, string_ok)?;
    Some(button_to_wrapper_markup(url_expr, hash_entries(opts)))
}

/// The markup itself, with the URL already resolved — everything in
/// [`button_to_wrapper_parts`] that does not need a view context.
///
/// Split out because the same `button_to url, opts do … end` is
/// written in HELPER MODULES too, where there is no `ViewCtx` and no
/// template to walk; `lower::tag_builder` expands those beside its
/// `button_tag` sibling and calls in here for the bytes. One owner for
/// the markup, so a `<form>` shape measured against Rails once cannot
/// drift between the two spellings.
pub(crate) fn button_to_wrapper_markup(
    url_expr: Expr,
    mut opts_entries: Vec<(Expr, Expr)>,
) -> (Vec<InterpPart>, Vec<InterpPart>) {
    let method_expr = take_opt(&mut opts_entries, "method").unwrap_or_else(default_method_sym);
    let form_class_expr =
        take_opt(&mut opts_entries, "form_class").unwrap_or_else(default_form_class);
    // Remaining entries become `<button>` attributes.
    let button_opts = opts_entries;

    let mut parts: Vec<InterpPart> = Vec::new();
    // <form action="<href>" method="post" class="<form_class>">
    parts.push(InterpPart::Text {
        value: "<form action=\"".to_string(),
    });
    parts.push(InterpPart::Expr {
        expr: view_helpers_call("html_escape", vec![url_expr]),
    });
    parts.push(InterpPart::Text {
        value: "\" method=\"post\" class=\"".to_string(),
    });
    parts.push(InterpPart::Expr {
        expr: view_helpers_call("html_escape", vec![form_class_expr]),
    });
    parts.push(InterpPart::Text {
        value: "\">".to_string(),
    });
    // _method hidden input (empty string when method is :get/:post).
    parts.push(InterpPart::Expr {
        expr: view_helpers_call("method_override_input", vec![method_expr]),
    });
    // <button type="submit" <button_opts>>
    parts.push(InterpPart::Text {
        value: "<button type=\"submit\"".to_string(),
    });
    append_attr_parts(&mut parts, &button_opts);
    parts.push(InterpPart::Text {
        value: ">".to_string(),
    });
    let suffix = vec![
        InterpPart::Text { value: "</button>".to_string() },
        // CSRF authenticity_token hidden input.
        InterpPart::Expr { expr: view_helpers_call("csrf_token_hidden_input", Vec::new()) },
        InterpPart::Text { value: "</form>".to_string() },
    ];
    (parts, suffix)
}

/// Extract the entries Vec from a `Hash` literal opts arg, or empty
/// when no opts were passed. Real-fixture call sites always pass
/// literal Hash kwargs; a non-Hash opts arg falls through as empty
/// (the helper renders with no extra attrs).
fn hash_entries(opts: Option<&Expr>) -> Vec<(Expr, Expr)> {
    let Some(o) = opts else {
        return Vec::new();
    };
    let ExprNode::Hash { entries, .. } = &*o.node else {
        return Vec::new();
    };
    entries.clone()
}

/// `RouteHelpers.<model>_path(record.<param>)` for a record whose TYPE
/// (not name) says which model it is — when that model has a one-member
/// route. Projected here rather than left to the ruby emit's route-param
/// pass: a view's analyzer types do not survive to that pass, and a
/// whole record interpolated into the path renders `#<User…>`. The
/// projection is Rails' `to_param`: the model's own override when it
/// has one (lobsters' User → username), else the id.
fn typed_record_path(ctx: &ViewCtx, model: &str, record: &Expr) -> Option<Expr> {
    let singular = snake_case(model);
    let helper = format!("{singular}_path");
    if ctx.route_helper_arity.get(helper.as_str()).is_none_or(|n| *n != 1) {
        return None;
    }
    let param = if ctx.slug_models.contains(&singular) { "to_param" } else { "id" };
    let member = send(Some(record.clone()), param, Vec::new(), None, false);
    Some(super::member_path_call(ctx, &helper, member))
}

/// Translate the URL-position argument (`link_to text, URL, opts`)
/// into spinel shape: literal strings pass through, path-helper calls
/// rewrite to `RouteHelpers.<name>(...)`, bare local records rewrite
/// to `RouteHelpers.<singular>_path(name.id)`. Nested arrays defer
/// to a later slice (form_with's nested-resource fixture forces them).
fn emit_url_arg(url: &Expr, ctx: &ViewCtx, string_ok: bool) -> Option<Expr> {
    // A CONDITIONAL url (`@root_path ? "/page/#{n}" : {controller: …}`
    // — every lobsters index's pagination link) is two urls, not one.
    // Resolve each branch on its own; the classifier below sees only the
    // whole `If` and gives up on it, which is how the options-hash
    // branch reached the tag renderer as attributes.
    if let ExprNode::If { cond, then_branch, else_branch } = &*url.node {
        let then_url = emit_url_arg(then_branch, ctx, string_ok)?;
        let else_url = emit_url_arg(else_branch, ctx, string_ok)?;
        return Some(Expr::new(
            url.span,
            ExprNode::If {
                cond: cond.clone(),
                then_branch: then_url,
                else_branch: else_url,
            },
        ));
    }
    // An INTERPOLATED literal path (`"/page/#{@page - 1}"`) is already
    // the url — there is nothing to resolve. The classifier only knows
    // the plain-`Lit::Str` case, so this shape fell all the way through
    // to the runtime `link_to`, taking its ternary sibling with it.
    if matches!(&*url.node, ExprNode::StringInterp { .. }) {
        return Some(rewrite_helpers_in_expr(url, ctx));
    }
    // An Active Storage Blob, Attachment or Attached links where Rails'
    // `resolve("ActiveStorage::Blob")` / `resolve("ActiveStorage::Attachment")`
    // route it: `rails_blob_path`, the blob's redirect route. Keyed off
    // the type, ahead of the name convention below, which read a `blob`
    // local as `blob_path(blob.id)`. A nilable one (`Attached#blob` is
    // `Blob?`) too: the runtime answers "" for nil rather than raising.
    if let Some(crate::ty::Ty::Class { id, .. }) = url.ty.as_ref().map(crate::ty::Ty::peel_nilable)
        && matches!(id.0.as_str(), "ActiveStorage::Blob" | "ActiveStorage::Attachment" | "ActiveStorage::Attached")
    {
        return Some(route_helpers_call("rails_blob_path", vec![rewrite_helpers_in_expr(url, ctx)]));
    }
    // The `active_storage/blobs/_blob` override's own record local, which
    // stays untyped (`record_arg_ty`): a Blob from the app's own render,
    // or Action Text's Attachment, which answers `blob` as Rails' delegates
    // to it. Either links to the blob's redirect route.
    let is_record_local = match &*url.node {
        ExprNode::Var { name, .. } => name.as_str() == ctx.arg_name,
        ExprNode::Send { recv: None, method, args, .. } => args.is_empty() && method.as_str() == ctx.arg_name,
        _ => false,
    };
    if ctx.resource_dir == "active_storage/blobs" && !ctx.arg_name.is_empty() && is_record_local {
        return Some(route_helpers_call("rails_blob_path", vec![rewrite_helpers_in_expr(url, ctx)]));
    }
    // A read `case_class_narrow` pinned to a model (`ma.item` inside
    // `when ModMail`) links through that model's route.
    if let ExprNode::Cast { target_ty: crate::ty::Ty::Class { id, .. }, .. } = &*url.node {
        if let Some(call) = typed_record_path(ctx, id.0.as_str(), url) {
            return Some(call);
        }
    }
    // `{controller: …, action: …, page: …}` — Rails' url-options hash.
    // Resolves through the generated route-table lookup (see
    // `lower_url_option_helpers`).
    if let Some(resolved) = emit_url_options_hash(url, ctx) {
        return Some(resolved);
    }
    // A Hash known only at run time resolves there, against this request
    // (`routes_to_library::lower_dynamic_url_helper`).
    if super::dynamic_url_hash(url) {
        let local = |name: &str| var_ref(Symbol::from(name));
        let recall = if ctx.locals.iter().any(|l| l == "path_parameters") {
            local("path_parameters")
        } else {
            Expr::new(Span::synthetic(), ExprNode::Hash { entries: vec![], kwargs: false })
        };
        return Some(route_helpers_call(
            "path_for_params",
            vec![local("controller_name"), local("action_name"), rewrite_helpers_in_expr(url, ctx), recall],
        ));
    }
    // Association-reader record URL (`link_to text,
    // showing_user.invited_by_user`) — Rails resolves the record
    // polymorphically through its named route. The reader's target
    // model comes from `reference_targets`; the record rides WHOLE
    // into the route helper (not `.id`) so a custom `to_param`
    // (lobsters' User#to_param = username) shapes the segment exactly
    // as Rails does. Without this arm the call fell back to the
    // runtime `link_to`, which interpolated the record as
    // `#<User:0x…>` into href.
    if let ExprNode::Send { recv: Some(_), method, args, block: None, .. } = &*url.node {
        if args.is_empty() {
            // A POLYMORPHIC reader's declared target is the association
            // name (`item`), which names no route; the analyzer's type
            // for the read — narrowed by a `case ma.item when ModMail`
            // arm — names the one that does (lobsters' mod activity log).
            if let Some(crate::ty::Ty::Class { id, .. }) =
                url.ty.as_ref().map(crate::ty::Ty::peel_nilable)
            {
                if let Some(call) = typed_record_path(ctx, id.0.as_str(), url) {
                    return Some(call);
                }
            }
            if let Some(target) = ctx.reference_targets.get(method.as_str()) {
                return Some(super::member_path_call(
                    ctx,
                    &format!("{target}_path"),
                    url.clone(),
                ));
            }
        }
    }
    // Bare `<x>_url` absolute helpers (`button_to "Verify",
    // twofa_verify_url`) — RouteHelpers has no `_url` functions;
    // ground to the shared absolute interp.
    if let ExprNode::Send { recv: None, method, args, block: None, .. } = &*url.node {
        if let Some(stem) = method.as_str().strip_suffix("_url") {
            return Some(super::absolute_url_interp(stem, args.clone()));
        }
    }
    // A url the template computes as a String (`"/x" + "y"`, a String
    // local, `"/X".downcase`, `articles_path + "?p=1"`) is the url, as an
    // interpolated one is: Rails' `url_for` returns a String unchanged.
    // Without this it fell through to a `link_to` no compiled runtime
    // defines (#1865). A record local's name convention doesn't apply to
    // a String either. Only where the label before it is inert
    // (`string_ok`): the markup writes the href first, and Rails
    // evaluates the label first, so a label and a url that both run code
    // keep the runtime helper, which evaluates them in order.
    let string_url = || (string_ok && matches!(url.ty, Some(crate::ty::Ty::Str))).then(|| rewrite_helpers_in_expr(url, ctx));
    let is_local = |n: &str| ctx.is_local(n);
    let Some(kind) = classify_view_url_arg(url, &is_local) else {
        return string_url();
    };
    if matches!(kind, ViewUrlArg::RecordRef { .. })
        && let Some(url) = string_url()
    {
        return Some(url);
    }
    match kind {
        ViewUrlArg::Literal { value } => Some(lit_str(value.to_string())),
        // The local already holds the url — read it.
        ViewUrlArg::LocalUrl { name } => Some(var_ref(Symbol::from(name))),
        ViewUrlArg::PathHelper { name, args } => {
            let route_args = rewrite_path_args(name, args, ctx);
            Some(route_helpers_call(name, route_args))
        }
        ViewUrlArg::RecordRef { name } => {
            let singular = singularize(name);
            // The NAME is the convention; when it names no route
            // (lobsters' `link_to recipient.username, recipient` over
            // `mod_mail.recipients`), the analyzer's type for the local
            // names the one Rails' polymorphic_path would pick. The
            // record rides whole, like the association-reader arm above.
            if !ctx.route_helper_arity.contains_key(format!("{singular}_path").as_str()) {
                if let Some(crate::ty::Ty::Class { id, .. }) =
                    url.ty.as_ref().map(crate::ty::Ty::peel_nilable)
                {
                    if let Some(call) = typed_record_path(ctx, id.0.as_str(), url) {
                        return Some(call);
                    }
                }
            }
            let param = if ctx.slug_models.contains(&singular) { "to_param" } else { "id" };
            let id_expr = send(
                Some(var_ref(Symbol::from(name))),
                param,
                Vec::new(),
                None,
                false,
            );
            Some(super::member_path_call(
                ctx,
                &format!("{singular}_path"),
                id_expr,
            ))
        }
        // `[comment.article, comment]` — nested-resource array. Each
        // element resolves to a (singular_name, id_expr) pair via
        // `classify_nested_url_element`; the path-helper name is the
        // underscore-joined singulars + `_path`, and the args are
        // each element's id expression. So `[comment.article,
        // comment]` → `RouteHelpers.article_comment_path
        // (comment.article_id, comment.id)`. Returns None if any
        // element doesn't classify (literals, complex chains).
        ViewUrlArg::NestedArray { elements } => {
            let is_local = |n: &str| ctx.is_local(n);
            let mut singulars: Vec<String> = Vec::new();
            let mut path_args: Vec<Expr> = Vec::new();
            for el in elements {
                let kind = classify_nested_url_element(el, &is_local)?;
                let (singular, id_expr) = nested_element_parts(&kind);
                singulars.push(singular);
                path_args.push(id_expr);
            }
            let path_name = format!("{}_path", singulars.join("_"));
            Some(route_helpers_call(&path_name, path_args))
        }
    }
}

/// `{controller: controller_name, action: action_name, page: @page + 1}`
/// → `RouteHelpers.path_for_controller_action_page(controller_name,
/// action_name, (@page + 1).to_s)`. The resolver is generated from the
/// app's own route table (`lower_url_option_helpers`), keyed on the same
/// extra-key set, so the two sides agree by construction.
///
/// Every value goes through `.to_s`: the resolver's params are String
/// (a path segment is text) while `page:` arrives as an Integer
/// expression. Returns None for any hash that isn't this form, so an
/// ordinary opts hash in url position still falls through.
fn emit_url_options_hash(url: &Expr, ctx: &ViewCtx) -> Option<Expr> {
    // `{controller:, action:, page:}.merge(extra)` — current lobsters
    // carries its last-read marker across pages this way
    // (`.merge(@next_page_params || {})`). The literal keys resolve
    // exactly as below; the merged keys are known only at run time,
    // and none of them is a segment of any route this shape reaches
    // (those take `page` alone), so Rails renders them as the query
    // string — `RouteHelpers.query_suffix`, the same helper a
    // `**splat` into a route helper goes through.
    if let ExprNode::Send { recv: Some(base), method, args, block: None, .. } = &*url.node {
        if method.as_str() == "merge" && args.len() == 1 {
            let path = emit_url_options_hash(base, ctx)?;
            let extra = rewrite_helpers_in_expr(&args[0], ctx);
            return Some(send(
                Some(path),
                "+",
                vec![route_helpers_call("query_suffix", vec![extra])],
                None,
                false,
            ));
        }
    }
    let ExprNode::Hash { entries, .. } = &*url.node else {
        return None;
    };
    let mut by_key: Vec<(String, Expr)> = Vec::new();
    for (k, v) in entries {
        let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node else {
            return None;
        };
        by_key.push((value.as_str().to_string(), v.clone()));
    }
    let take = |name: &str| -> Option<Expr> {
        by_key.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())
    };
    let controller = take("controller")?;
    let action = take("action")?;
    let mut extras: Vec<(String, Expr)> = by_key
        .iter()
        .filter(|(k, _)| k != "controller" && k != "action")
        .cloned()
        .collect();
    extras.sort_by(|a, b| a.0.cmp(&b.0));
    let extra_names: Vec<String> = extras.iter().map(|(k, _)| k.clone()).collect();
    let to_s = |e: Expr| send(Some(e), "to_s", Vec::new(), None, false);
    let mut args = vec![to_s(controller), to_s(action)];
    args.extend(extras.into_iter().map(|(_, v)| to_s(v)));
    let mut args: Vec<Expr> = args
        .into_iter()
        .map(|a| rewrite_helpers_in_expr(&a, ctx))
        .collect();
    // The request's path parameters, for the segments the hash leaves
    // out (Rails' recall). An action view takes them as a param
    // (`extra_params`); anywhere else they are not in scope, and an
    // empty Hash recalls nothing.
    args.push(if ctx.locals.iter().any(|l| l == "path_parameters") {
        var_ref(Symbol::from("path_parameters"))
    } else {
        Expr::new(Span::synthetic(), ExprNode::Hash { entries: Vec::new(), kwargs: false })
    });
    Some(route_helpers_call(
        &crate::lower::url_options_helper_name(&extra_names),
        args,
    ))
}

/// Each element of a nested URL array resolves to `(singular, id_expr)`.
/// `DirectLocal { name: "comment" }` → `("comment", comment.id)`.
/// `Association { owner: "comment", assoc: "article" }` →
/// `("article", comment.article_id)` — the FK column on the owner is
/// the load-bearing source so we don't have to dereference the
/// belongs_to read just to get the id.
fn nested_element_parts(kind: &NestedUrlElement<'_>) -> (String, Expr) {
    match kind {
        NestedUrlElement::DirectLocal { name } => {
            let id_expr = send(
                Some(var_ref(Symbol::from(*name))),
                "id",
                Vec::new(),
                None,
                false,
            );
            ((*name).to_string(), id_expr)
        }
        NestedUrlElement::Association { owner, assoc } => {
            let fk = format!("{assoc}_id");
            let id_expr = send(
                Some(var_ref(Symbol::from(*owner))),
                &fk,
                Vec::new(),
                None,
                false,
            );
            ((*assoc).to_string(), id_expr)
        }
    }
}

/// `link_to`'s `edit_article_path(article)` argument: the bare local
/// `article` should pass as `article.id`, mirroring how nav links flow
/// through Rails url-for. Accepts both `Var` (the post-ivar-rewrite
/// shape) and the bareword `Send { recv: None, args: [], block: None }`
/// shape Prism produces for partial-scope locals. Anything else
/// passes through unchanged.
/// [`rewrite_path_arg`] over a helper's arguments. A lone record handed
/// to the member helper of a model that overrides `to_param` (lobsters'
/// `user_path(user)`, a username) projects to that, as Rails renders it.
/// An argument that isn't a bound local projects by its type, as a nested
/// call's does (`walker::record_segment_method`): a local the template
/// assigns (`<% rec = @article %>`) is typed as the record. Not for a
/// helper whose body the app wrote, an app helper of the name or a
/// `direct` route, which reads the record.
pub(super) fn rewrite_path_args(helper: &str, args: &[Expr], ctx: &ViewCtx) -> Vec<Expr> {
    if ctx.direct_route_helpers.contains(helper) {
        return args.to_vec();
    }
    let resource = helper.trim_end_matches("_path").trim_end_matches("_url");
    let resource = resource.strip_prefix("edit_").unwrap_or(resource);
    let param = if args.len() == 1 && ctx.slug_models.iter().any(|m| m == resource) { "to_param" } else { "id" };
    let typed = !(ctx.app_helper_names.contains(helper) || ctx.direct_route_helpers.contains(helper));
    args.iter().map(|a| project_path_arg(a, ctx, param, typed)).collect()
}

pub(super) fn rewrite_path_arg(arg: &Expr, ctx: &ViewCtx) -> Expr {
    project_path_arg(arg, ctx, "id", false)
}

fn project_path_arg(arg: &Expr, ctx: &ViewCtx, param: &str, typed: bool) -> Expr {
    let local_name = match &*arg.node {
        ExprNode::Var { name, .. } if ctx.is_local(name.as_str()) => Some(name.clone()),
        ExprNode::Send {
            recv: None,
            method,
            args,
            block: None,
            ..
        } if args.is_empty() && ctx.is_local(method.as_str()) => Some(method.clone()),
        _ => None,
    };
    match local_name {
        Some(name) => {
            // The record's own `to_param` whatever the helper (`edit_mod_domain_path(domain)` is a domain name).
            let own = ctx.slug_models.iter().any(|m| *m == singularize(name.as_str()));
            send(Some(var_ref(name)), if own { "to_param" } else { param }, Vec::new(), None, false)
        }
        None if typed => match super::walker::record_segment_method(arg, ctx) {
            Some(method) => send(Some(arg.clone()), method, Vec::new(), None, false),
            None => arg.clone(),
        },
        None => arg.clone(),
    }
}

/// The model singular a `turbo_stream_from` streamable names, if it
/// names one.
///
/// `bare_record_name`'s three shapes (local, ivar, bare call) plus
/// `Current.<name>` — campfire's sidebar subscribes with
/// `turbo_stream_from Current.user, :rooms`, and `Current` is the app's
/// own `CurrentAttributes`, so the METHOD is the model singular there
/// the way the variable name is elsewhere. Declining it declined the
/// whole call, and an unlowered `turbo_stream_from` in a view body is a
/// NoMethodError at render — the subscribe half of the wire simply
/// missing while its publish half worked.
///
/// Kept local rather than folded into `bare_record_name`: this asks
/// "what record does this expression denote", which is the streamable
/// question. `bare_record_name`'s other callers ask narrower ones.
fn streamable_record_name(e: &Expr) -> Option<String> {
    if let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*e.node {
        if args.is_empty() {
            if let ExprNode::Const { path } = &*r.node {
                if path.last().map(|s| s.as_str()) == Some("Current") {
                    return Some(method.as_str().to_string());
                }
            }
        }
    }
    bare_record_name(e)
}
