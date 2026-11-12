//! Walker: traverse a compiled-ERB body and produce the corresponding
//! spinel-shape statement list. Dispatches output-position expressions
//! to the helper / partial / form-with / form-builder sub-modules.

use crate::expr::{BlockStyle, BoolOpKind, BoolOpSurface, Expr, ExprNode, InterpPart, LValue, Literal};
use crate::ident::{Symbol, VarId};
use crate::span::Span;

use crate::lower::view::{
    classify_form_builder_method, classify_render_partial, classify_view_helper,
    extract_sym_or_str, FormBuilderMethod, ViewHelperKind,
};

use super::form_builder::{
    emit_button_tag, emit_button_tag_block, emit_check_box_tag, emit_form_builder_block_inline, emit_form_builder_inline, emit_label_tag,
    emit_submit_tag,
};
use super::form_with::{
    emit_form_tag_inline, emit_form_with_blockless, emit_form_with_inline, emit_tag_builder_inline, emit_tag_builder_void_or_content, is_errors_each,
    rewrite_errors_each_body,
};
use super::helpers::{emit_inline_helper_block, emit_view_helper_call};
use super::partial::{emit_flagged_collection_render, emit_render_partial, emit_yield};
use super::turbo_drive::emit_turbo_drive_directive;
use super::predicates::rewrite_predicates;
use super::{
    accumulator_append_call, accumulator_result_ref, assign_accumulator_string_new, lit_str, lit_sym,
    nil_lit, seq, noop_io_append, send, todo_io_append, var_ref, view_helpers_call, ViewCtx,
};

/// Walk a compiled-ERB body (`Seq` of `_buf = …` statements + control-
/// flow) and produce the corresponding spinel-shape statement list:
/// `io << ...` / `if cond ... end` / `coll.each { |x| ... }` / bare
/// helper-call statements (content_for setter), in source order.
pub(super) fn walk_body(body: &Expr, ctx: &ViewCtx) -> Vec<Expr> {
    let stmts: Vec<&Expr> = match &*body.node {
        ExprNode::Seq { exprs } => exprs.iter().collect(),
        _ => vec![body],
    };
    let mut out = Vec::new();
    // A statement assigning a local over a flash param shadows it for the
    // rest of the body (`ViewCtx::after_assigning`).
    let mut shadowed: Option<ViewCtx> = None;
    for stmt in &stmts {
        if let Some(next) = shadowed.as_ref().unwrap_or(ctx).after_assigning(stmt) {
            shadowed = Some(next);
        }
        let ctx = shadowed.as_ref().unwrap_or(ctx);
        let mut lowered = walk_stmt(stmt, ctx);
        // Synthesis choke point: everything walk_stmt invented for this
        // source statement (helper sends, io appends, TODO markers, …)
        // attributes back to the statement it was derived from. Inner
        // recursions (if-branches, each-bodies, form_with bodies) have
        // already stamped their own, finer spans — those win.
        for e in &mut lowered {
            e.inherit_span(stmt.span);
        }
        out.extend(lowered);
        // A template local assigned here is in scope for the statements
        // after it (`ViewCtx::assigned_locals`), and so is one assigned
        // inside this statement's nested bodies (`nested_assigned_locals`).
        let (top, nested) = assigned_names(stmt);
        let top: Vec<String> = top.into_iter().filter(|n| !ctx.assigned_locals.contains(n)).collect();
        let nested: Vec<String> = nested.into_iter().filter(|n| !ctx.nested_assigned_locals.contains(n)).collect();
        if !top.is_empty() || !nested.is_empty() {
            let mut next = ctx.clone();
            next.assigned_locals.extend(top);
            next.nested_assigned_locals.extend(nested);
            shadowed = Some(next);
        }
    }
    out
}

/// The template locals `stmt` assigns that stay in scope after it: those
/// it assigns itself (`x = …`, `x ||= …`, `a, x = …`), and those its
/// nested statement bodies assign (an `if` branch, a `case` arm, a
/// `while`). A block's or lambda's body is its own scope, as in Ruby, and
/// is left out. `_buf` is the ERB buffer, not a template local.
fn assigned_names(stmt: &Expr) -> (Vec<String>, Vec<String>) {
    fn own(e: &Expr) -> Vec<String> {
        let names: Vec<&Symbol> = match &*e.node {
            ExprNode::Assign { target: LValue::Var { name, .. }, .. }
            | ExprNode::OpAssign { target: LValue::Var { name, .. }, .. } => vec![name],
            ExprNode::MultiAssign { targets, .. } => targets
                .iter()
                .filter_map(|t| match t {
                    LValue::Var { name, .. } => Some(name),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        names.into_iter().filter(|n| n.as_str() != "_buf").map(|n| n.as_str().to_string()).collect()
    }
    fn nested(e: &Expr, out: &mut Vec<String>) {
        e.node.for_each_child(&mut |c| {
            if matches!(&*c.node, ExprNode::Lambda { .. }) {
                return;
            }
            for n in own(c) {
                if !out.contains(&n) {
                    out.push(n);
                }
            }
            nested(c, out);
        });
    }
    let top = own(stmt);
    let mut inner = Vec::new();
    nested(stmt, &mut inner);
    inner.retain(|n| !top.contains(n));
    (top, inner)
}

/// The `link_to_if` / `link_to_unless` blocks in `body` whose parameter
/// shadows a template local assigned only inside an earlier statement's
/// nested body (`<% if c %><% x = 1 %><% end %>… do |x|`). Ruby keeps
/// that local after the block; the lowering's save and restore would read
/// it where a compiled target's scope doesn't have it, so the block is
/// refused instead (`emit_link_to_if_block`). The same statement-order
/// walk as `walk_body`'s.
pub(crate) fn link_to_if_params_shadowing_nested_locals(body: &Expr) -> Vec<(Span, String)> {
    fn visit(e: &Expr, top: &[String], nested: &[String], out: &mut Vec<(Span, String)>) {
        if let ExprNode::Seq { exprs } = &*e.node {
            let (mut top, mut nested) = (top.to_vec(), nested.to_vec());
            for stmt in exprs {
                visit(stmt, &top, &nested, out);
                let (t, n) = assigned_names(stmt);
                top.extend(t);
                nested.extend(n);
            }
            return;
        }
        if let ExprNode::Send { recv: None, method, block: Some(block), .. } = &*e.node
            && matches!(method.as_str(), "link_to_if" | "link_to_unless")
            && let ExprNode::Lambda { params, .. } = &*block.node
            && let Some(p) = params.first()
            && refuses_param(p.as_str(), top, nested)
        {
            out.push((e.span, method.as_str().to_string()));
        }
        e.node.for_each_child(&mut |c| visit(c, top, nested, out));
    }
    let mut out = Vec::new();
    visit(body, &[], &[], &mut out);
    out
}

/// A block parameter shadowing a local assigned only in an earlier nested
/// body (`link_to_if_params_shadowing_nested_locals`).
fn refuses_param(p: &str, top: &[String], nested: &[String]) -> bool {
    !top.iter().any(|n| n == p) && nested.iter().any(|n| n == p)
}

fn walk_stmt(stmt: &Expr, ctx: &ViewCtx) -> Vec<Expr> {
    match &*stmt.node {
        // Prologue `_buf = ""` — drop; we inject `io = String.new` once
        // at method-body construction time.
        ExprNode::Assign { target: LValue::Var { name, .. }, value }
            if name.as_str() == "_buf" =>
        {
            if let ExprNode::Lit { value: Literal::Str { value: s } } = &*value.node {
                if s.is_empty() {
                    return Vec::new();
                }
            }
            // `_buf = _buf + X` — the working shape.
            if let ExprNode::Send { recv: Some(recv), method, args, .. } = &*value.node {
                if method.as_str() == "+" && args.len() == 1 {
                    if let ExprNode::Var { name: rn, .. } = &*recv.node {
                        if rn.as_str() == "_buf" {
                            // Stamp with the appended chunk's span — one
                            // notch tighter than the enclosing
                            // `_buf = _buf + …` statement walk_body uses.
                            let mut out = emit_io_append(&args[0], ctx);
                            for e in &mut out {
                                e.inherit_span(args[0].span);
                            }
                            return out;
                        }
                    }
                }
            }
            // Unrecognized `_buf = …` shape — emit as TODO comment-style
            // io append so the file still parses; the test asserts on
            // recognized shapes only.
            vec![todo_io_append("unknown _buf shape", stmt.span)]
        }
        // Epilogue `_buf` read — drop; the explicit trailing `io` is
        // appended once at method-body construction.
        ExprNode::Var { name, .. } if name.as_str() == "_buf" => Vec::new(),
        // A local rebound to itself, which is what `<% article = @article %>`
        // becomes once the ivar is the view's parameter `article`
        // (`rewrite_ivars_to_locals`). Rails' two are one object, so the
        // assignment changes nothing; written out, TypeScript refuses the
        // second declaration and Crystal the no-op. Only for a local the
        // view binds: Ruby's `x = x` with no local `x` before it is nil.
        ExprNode::Assign { target: LValue::Var { name, .. }, value }
            if ctx.is_local(name.as_str())
                && matches!(&*value.node, ExprNode::Var { name: read, .. } if read == name) =>
        {
            Vec::new()
        }
        // Template-local `||=` (`<% is_unread ||= false %>`) — same
        // statement treatment as plain assignment; Ruby's `x ||= v`
        // defines the local when unset.
        ExprNode::OpAssign { target: LValue::Var { name, id }, op, value } => {
            vec![Expr::new(
                stmt.span,
                ExprNode::OpAssign {
                    target: LValue::Var { name: name.clone(), id: *id },
                    op: op.clone(),
                    value: rewrite_predicates(
                        value,
                        &ctx.nullable_locals,
                        &ctx.reference_reads,
                        &ctx.nilable_scalar_reads,
                    ),
                },
            )]
        }
        // `<% @page_title = room_display_name(@room) %>` — a write to
        // the VIEW CONTEXT, which Rails shares with the layout and
        // with every helper mixed into it. Two statements: the local,
        // so a later read in the SAME template resolves the way every
        // other template local does, and the ivar write-through, which
        // the emit routes to the per-dispatch controller seam that
        // already answers the helper-side read (`ApplicationHelper
        // .page_title_tag` reads `@page_title`). Emitting only the
        // local — which is what happened until now — left the write
        // going nowhere and campfire titled every page "Campfire".
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            let local = Symbol::from(ctx.ivar_local(name.as_str()));
            let rewritten = rewrite_predicates(
                value,
                &ctx.nullable_locals,
                &ctx.reference_reads,
                &ctx.nilable_scalar_reads,
            );
            vec![
                Expr::new(
                    stmt.span,
                    ExprNode::Assign {
                        target: LValue::Var { name: local.clone(), id: VarId(0) },
                        value: rewritten,
                    },
                ),
                Expr::new(
                    stmt.span,
                    ExprNode::Assign {
                        target: LValue::Ivar { name: name.clone() },
                        value: Expr::new(
                            stmt.span,
                            ExprNode::Var { id: VarId(0), name: local },
                        ),
                    },
                ),
            ]
        }
        // Template-local assignment (`<% flagged = comment.current_vote
        // && … %>`) — a real statement, not output; later
        // interpolations read the local by name. The value gets the
        // same predicate rewriting a condition does (`<% x =
        // list.any? %>`). Was silently swallowed by the catch-all
        // TODO append, leaving every read a NameError.
        ExprNode::Assign { target: LValue::Var { name, id }, value } => {
            vec![Expr::new(
                stmt.span,
                ExprNode::Assign {
                    target: LValue::Var { name: name.clone(), id: *id },
                    value: rewrite_predicates(
                        value,
                        &ctx.nullable_locals,
                        &ctx.reference_reads,
                        &ctx.nilable_scalar_reads,
                    ),
                },
            )]
        }
        // `<% type, value = node.first %>` (lobsters' search parse
        // tree) — the multi-local form of the arm above. Locals only:
        // an ivar target would need the view-context write-through the
        // single-ivar arm builds, so that shape keeps the residue line.
        ExprNode::MultiAssign { targets, value }
            if targets.iter().all(|t| matches!(t, LValue::Var { .. })) =>
        {
            vec![Expr::new(
                stmt.span,
                ExprNode::MultiAssign {
                    targets: targets.clone(),
                    value: rewrite_predicates(
                        value,
                        &ctx.nullable_locals,
                        &ctx.reference_reads,
                        &ctx.nilable_scalar_reads,
                    ),
                },
            )]
        }
        // Conditional branching at the template level. Cond goes
        // through `rewrite_predicates`: `.blank?` / `.present?` become
        // `(recv || "").empty?`, and `.any?` / `.none?` / `.empty?`
        // become collection `empty?`.
        //
        // A branch that is a nil literal stays nil: an `if` with no
        // `else` has one, and so does an `unless` with no `else`, as its
        // `then` branch. Walked, it would be an append of nothing, which
        // the Go, TypeScript, Crystal and Python buffers don't take.
        ExprNode::If { cond, then_branch, else_branch } => {
            let walk_branch = |branch: &Expr| {
                if matches!(&*branch.node, ExprNode::Lit { value: Literal::Nil }) {
                    return nil_lit();
                }
                let s = walk_body(branch, ctx);
                if s.len() == 1 {
                    s.into_iter().next().unwrap()
                } else {
                    seq(s)
                }
            };
            let then_body = walk_branch(then_branch);
            let else_body = walk_branch(else_branch);
            vec![Expr::new(
                Span::synthetic(),
                ExprNode::If {
                    cond: rewrite_predicates(cond, &ctx.nullable_locals, &ctx.reference_reads, &ctx.nilable_scalar_reads),
                    then_branch: then_body,
                    else_branch: else_body,
                },
            )]
        }
        // `<% cache <key> do %> … <% end %>` — Rails fragment caching,
        // served through `ActionView::ViewHelpers.fragment_read/_write`
        // (the runtime's store, or on the ruby family and spinel the
        // controller's) when the key can be built, and rendered
        // transparently when it cannot:
        //
        //   __cache_hit_1 = ActionView::ViewHelpers.fragment_read_record("messages/_message", message)
        //   if __cache_hit_1.nil?
        //     __cache_io_1 = String.new
        //     … body …
        //     io << ActionView::ViewHelpers.fragment_write_record(<same key>, __cache_io_1, 0)
        //   else
        //     io << __cache_hit_1
        //   end
        //
        // `cache record` hands the controller `[template, record]`, as
        // Rails does (`record_cache_key`); any other key is folded to
        // text and read with `fragment_read("views/…")`.
        //
        // TWO APPENDS, not one through a reassigned local. `read_str`
        // answers `String?` and the body produces a `String`; a single
        // local holding both is a type that changes mid-method, which
        // the strict targets have to widen. Each arm here appends a
        // value of one type, and the else arm reads the local where it
        // is already narrowed non-nil — the shape `attached.rs` uses.
        //
        // NO BLOCK, for the reason `Rails::Cache#read_str`'s note
        // records: a block would capture the accumulator, and on the
        // AOT lane a captured block dissolves into a heap poly proc
        // (matz/spinel#4245).
        //
        // Transparent when `cache_key_parts` declines the key, which is
        // also what this arm did for every site before the store
        // existed. Transparent and not DROPPED, because dropped is what
        // it was before that: `cache` fell through to the catch-all and
        // took the whole BODY with it, so lobsters' `users/tree.html.erb`
        // (44 lines, entirely wrapped) and `users/list.html.erb` each
        // emitted `io << ""` — two blank pages, reported by nothing.
        ExprNode::Send { recv: None, method, args, block: Some(block), .. }
            if method.as_str() == "cache" =>
        {
            let ExprNode::Lambda { body, .. } = &*block.node else {
                return vec![todo_io_append("cache block shape", stmt.span)];
            };
            let key = match record_cache_key(args, ctx) {
                Some(key) => key,
                None => match cache_key_parts(args, ctx) {
                    Some(parts) => FragmentKey::Text(parts),
                    None => return walk_body(body, ctx),
                },
            };
            emit_cached_fragment(&key, cache_ttl(args), body, ctx, stmt.span)
        }
        // Block-form `<% content_for :subnav do %> … <% end %>` —
        // slot capture (lobsters' subnav pattern: pages and partials
        // deposit nav markup, the layout gates on `content_for? :subnav`
        // and splices via `yield :subnav`). Render the block body into
        // its own accumulator and register it in the slot store; the
        // statement itself contributes no template output. The
        // accumulator name carries the slot so nested captures of
        // different slots can't shadow each other.
        ExprNode::Send { recv: None, method, args, block: Some(block), .. }
            if method.as_str() == "content_for" && args.len() == 1 =>
        {
            let Some(slot) = extract_sym_or_str(&args[0]) else {
                return vec![todo_io_append("dynamic content_for slot", stmt.span)];
            };
            let ExprNode::Lambda { body, .. } = &*block.node else {
                return vec![todo_io_append("content_for block shape", stmt.span)];
            };
            let cap = format!("cf_{slot}");
            let cap_ctx = ViewCtx { accumulator: cap.clone(), ..ctx.clone() };
            let mut out = vec![assign_accumulator_string_new(&cap)];
            out.extend(walk_body(body, &cap_ctx));
            // The capture is an ACCUMULATOR, and on a strict target
            // that is a builder, not a String — Crystal rejected
            // `content_for_set(:head, String::Builder)` the moment a
            // fixture first wrote the block form. Same tagged read the
            // tail of a view function uses, so each emitter finishes
            // the builder its own way.
            out.push(view_helpers_call(
                "content_for_set",
                vec![lit_sym(Symbol::from(slot)), accumulator_result_ref(&cap)],
            ));
            out
        }
        // Statement-form view-helper calls. Today: `<% content_for
        // :title, "Articles" %>` lowers to `ViewHelpers.content_for_set
        // (:title, "Articles")` — a bare call, not appended to `io`.
        // Other recognized statement-form helpers fall through to a
        // TODO append until a fixture exercises them.
        ExprNode::Send { recv: None, method, args, block: None, .. } => {
            // `<% turbo_page_requires_reload %>` — a turbo Drive
            // directive. Deposits into `:head`, appends nothing.
            if let Some(out) = emit_turbo_drive_directive(method.as_str(), args, stmt.span) {
                return out;
            }
            if let Some(ViewHelperKind::ContentForSetter { slot, body }) =
                classify_view_helper(method.as_str(), args)
            {
                return vec![view_helpers_call(
                    "content_for_set",
                    vec![lit_sym(Symbol::from(slot)), body.clone()],
                )];
            }
            // Statement-position render (`<% render partial:
            // 'stories/subnav' %>` — no `=`): Rails evaluates the
            // partial for its side effects (content_for deposits) and
            // DISCARDS the returned markup. Route the append into a
            // throwaway accumulator so the call still happens but
            // nothing lands on the page.
            let is_local = |n: &str| ctx.is_local(n);
            let is_options_ivar = |n: &str| ctx.is_options_ivar(n);
            if let Some(rp) = classify_render_partial(
                None,
                method.as_str(),
                args,
                None,
                &is_local,
                &is_options_ivar,
            ) {
                let disc = "discard";
                let disc_ctx = ViewCtx { accumulator: disc.to_string(), ..ctx.for_render(args) };
                if let Some(stmt) = emit_render_partial(&rp, &disc_ctx) {
                    return vec![assign_accumulator_string_new(disc), stmt];
                }
            }
            vec![todo_io_append("unknown stmt", stmt.span)]
        }
        // `<% case action_name when "all" %>…<% when … %>…<% end %>` at
        // the template level (lobsters' replies help text switches copy
        // on the action). Same treatment as If: recurse each arm's body
        // so inner `_buf` appends land on the accumulator; scrutinee
        // and guards pass through untouched. Was swallowed by the
        // catch-all TODO append — the whole case emitted `io << ""`.
        ExprNode::Case { scrutinee, arms } => {
            let new_arms = arms
                .iter()
                .map(|arm| crate::expr::Arm {
                    pattern: arm.pattern.clone(),
                    guard: arm.guard.clone(),
                    body: {
                        let stmts = walk_body(&arm.body, ctx);
                        if stmts.len() == 1 {
                            stmts.into_iter().next().unwrap()
                        } else {
                            seq(stmts)
                        }
                    },
                })
                .collect();
            vec![Expr::new(
                stmt.span,
                ExprNode::Case { scrutinee: scrutinee.clone(), arms: new_arms },
            )]
        }
        // `<% while cond %>…<% end %>` at the template level —
        // lobsters' users/tree.html.erb walks the invitation tree with
        // an explicit stack (`while subtree`, `ancestors << subtree`,
        // `subtree = ancestors.pop`). Recurse on the body so inner
        // `_buf` appends land on the outer io; cond rides the same
        // predicate rewriting an If cond gets.
        ExprNode::While { cond, body, until_form } => {
            let inner = walk_body(body, ctx);
            let inner_body =
                if inner.len() == 1 { inner.into_iter().next().unwrap() } else { seq(inner) };
            vec![Expr::new(
                Span::synthetic(),
                ExprNode::While {
                    cond: rewrite_predicates(
                        cond,
                        &ctx.nullable_locals,
                        &ctx.reference_reads,
                        &ctx.nilable_scalar_reads,
                    ),
                    body: inner_body,
                    until_form: *until_form,
                },
            )]
        }
        // Block-form `<% coll.each do |x| %>...<% end %>` at the
        // template level (rare — usually the each is inside a `<%= %>`
        // wrapper for collection partial render). When it shows up, we
        // recurse on the block body so inner `_buf = _buf + …` lines
        // become `io << …` against the outer io.
        ExprNode::Send {
            recv: Some(recv),
            method,
            args,
            block: Some(block),
            ..
        } if method.as_str() == "each" && args.is_empty() => {
            let ExprNode::Lambda { params, rest_param, extra_params, body, block_style, .. } = &*block.node else {
                return vec![todo_io_append("each block shape", stmt.span)];
            };
            let var_name = params
                .first()
                .map(|p| p.as_str().to_string())
                .unwrap_or_else(|| "item".into());
            // Spinel's `errors` is a `Vec<String>`, not a Vec of error
            // objects. Real Rails templates iterate via `e.full_message`;
            // rewrite that bareword projection back to the local so it
            // type-checks against spinel's runtime.
            let body = if is_errors_each(recv) {
                rewrite_errors_each_body(body, &var_name)
            } else {
                body.clone()
            };
            let inner_ctx = ctx.with_locals([var_name.clone()]);
            let inner_stmts = walk_body(&body, &inner_ctx);
            let inner_body = if inner_stmts.len() == 1 {
                inner_stmts.into_iter().next().unwrap()
            } else {
                seq(inner_stmts)
            };
            let block_lambda = Expr::new(
                Span::synthetic(),
                ExprNode::Lambda { rest_param: rest_param.clone(), extra_params: extra_params.clone(),
                    params: params.clone(),
                    block_param: None,
                    body: inner_body,
                    block_style: *block_style,
                },
            );
            vec![Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(recv.clone()),
                    method: method.clone(),
                    args: Vec::new(),
                    block: Some(block_lambda),
                    parenthesized: false,
                },
            )]
        }
        // A receiver'd, blockless call in statement position (`<%
        // ancestors << subtree %>`) — a side-effecting mutation whose
        // value is discarded. Pass it through verbatim: template
        // OUTPUT always arrives `_buf`-shaped, so anything here is
        // genuinely a statement (the prior TODO-append swallowed the
        // side effect and broke stack-walking templates).
        ExprNode::Send { recv: Some(_), block: None, .. } => vec![stmt.clone()],
        ExprNode::Raise { .. } => vec![stmt.clone()],
        // A bare literal — most often the synthesized `nil` arm `<% unless
        // %>` lowers to — renders nothing and does nothing: nothing is
        // dropped, so no residue line.
        ExprNode::Lit { .. } => vec![noop_io_append()],
        // The branch an `if` whose condition the analysis settled leaves in its place (`<% if @resource.try(:x?) %>`).
        ExprNode::Seq { .. } => walk_body(stmt, ctx),
        _ => vec![todo_io_append("unknown stmt", stmt.span)],
    }
}

/// Rebuild a form-wrapper helper's call site as the call it wraps:
/// `composer_form_tag(room) do |form| … end` → `form_with(model: …,
/// url: room_messages_path(room), …) do |form| … end`.
///
/// SUBSTITUTION, not inlining — the wrapper's body is one call by
/// construction ([`FormWrapperHelper`]), so the only thing to carry
/// across is each parameter's argument. Declines (leaving the site to
/// its loud NoMethodError) when an argument is anything but a
/// side-effect-free read: a parameter used twice would evaluate it
/// twice, and moving a call across the boundary changes when it runs.
/// campfire passes a plain local.
fn splice_form_wrapper(
    w: &super::FormWrapperHelper,
    args: &[Expr],
    block: &Expr,
) -> Option<Expr> {
    // Fewer arguments than parameters is the NORMAL call when the
    // wrapper's tail is an options hash: `**options` ingests as a
    // trailing positional defaulting to `{}`, so `profile_form_with
    // @user` supplies one argument to a two-parameter wrapper. Bind
    // what was passed and fall back to each remaining parameter's own
    // default; a parameter with no default and no argument is a call
    // this pass cannot reconstruct.
    let (positionals, keywords) = if w.keyword_rest {
        match args.last() {
            Some(last) => match &*last.node {
                ExprNode::Hash { kwargs: true, .. } => (&args[..args.len() - 1], Some(last)),
                ExprNode::KeywordSplat { value } if matches!(&*value.node, ExprNode::Hash { .. }) => {
                    (&args[..args.len() - 1], Some(value))
                }
                ExprNode::KeywordSplat { .. } => return None,
                _ => (args, None),
            },
            None => (args, None),
        }
    } else {
        (args, None)
    };
    let positional_count = w.params.len() - usize::from(w.keyword_rest);
    if positionals.len() > positional_count {
        return None;
    }
    if !positionals.iter().all(is_pure_read) || keywords.is_some_and(|arg| !is_pure_read(arg)) {
        return None;
    }
    let mut binding: crate::det::HashMap<&Symbol, &Expr> = crate::det::HashMap::default();
    for (i, (name, default)) in w.params.iter().enumerate() {
        let argument = if w.keyword_rest && i == positional_count { keywords } else { positionals.get(i) };
        match argument {
            Some(a) => {
                binding.insert(name, a);
            }
            None => match default {
                Some(d) => {
                    binding.insert(name, d);
                }
                None => return None,
            },
        }
    }
    let ExprNode::Send { method, args: wargs, parenthesized, .. } = &*w.call.node else {
        return None;
    };
    let args = wargs
        .iter()
        .map(|a| fold_static_merge(&substitute_vars(a, &binding)))
        .collect::<Option<Vec<_>>>()?;
    Some(Expr::new(
        w.call.span,
        ExprNode::Send {
            recv: None,
            method: method.clone(),
            args,
            block: Some(block.clone()),
            parenthesized: *parenthesized,
        },
    ))
}

/// `{a: 1}.merge({b: 2})` → `{a: 1, b: 2}`, once both sides are
/// literal.
///
/// A wrapper whose tail is `**options` has already been desugared by
/// `lower::kwsplat` into exactly that merge, so substituting the
/// caller's options hash leaves a `.merge` chain where `form_with`'s
/// lowering wants ONE literal hash to walk. Given the chain it declines
/// — and declining after the splice is worse than declining before it:
/// the call is gone and nothing replaces it, so the form silently
/// vanishes from the page instead of failing loudly.
///
/// Later keys win, which is Ruby's `merge` and also the point of the
/// wrapper: the caller's options override the helper's defaults.
fn fold_static_merge(e: &Expr) -> Option<Expr> {
    let ExprNode::Send { recv: Some(base), method, args, block: None, .. } = &*e.node else {
        return Some(e.clone());
    };
    if method.as_str() != "merge" {
        return Some(e.clone());
    }
    if args.len() != 1 {
        return None;
    }
    let (ExprNode::Hash { entries: base_entries, kwargs }, ExprNode::Hash { entries: over, .. }) =
        (&*base.node, &*args[0].node)
    else {
        return None;
    };
    let key_of = |k: &Expr| match &*k.node {
        ExprNode::Lit { value: value @ (Literal::Sym { .. } | Literal::Str { .. }) } => Some(value.clone()),
        _ => None,
    };
    if base_entries.iter().chain(over.iter()).any(|(k, _)| key_of(k).is_none()) {
        return None;
    }
    let mut merged: Vec<(Expr, Expr)> = Vec::new();
    for (k, v) in base_entries {
        match over.iter().find(|(ok, _)| key_of(ok) == key_of(k)) {
            Some((_, ov)) => merged.push((k.clone(), ov.clone())),
            None => merged.push((k.clone(), v.clone())),
        }
    }
    for (k, v) in over {
        if !base_entries.iter().any(|(bk, _)| key_of(bk) == key_of(k)) {
            merged.push((k.clone(), v.clone()));
        }
    }
    Some(Expr::new(e.span, ExprNode::Hash { entries: merged, kwargs: *kwargs }))
}

/// A read with no side effect and no cost worth worrying about, so
/// substituting it for each use of a parameter is safe however many
/// times the body names it.
///
/// The zero-arg block-less bare send belongs here for a reason specific
/// to templates: prism cannot prove `room` is a local inside an
/// ERB-ingested body, so a template local arrives as a `Send`, not a
/// `Var`. `composer_form_tag(room)` passes exactly that, and reading it
/// as impure would decline the only site this pass exists for. Same
/// judgment `scope_chain::owner_reads_once` makes about the same shape.
fn is_pure_read(e: &Expr) -> bool {
    match &*e.node {
        ExprNode::Var { .. } | ExprNode::Ivar { .. } | ExprNode::Lit { .. }
        | ExprNode::Const { .. } => true,
        ExprNode::Send { recv: None, args, block: None, .. } => args.is_empty(),
        // A LITERAL hash of pure reads. It is the options tail every
        // form wrapper takes (`profile_form_with @user, class: "…"`),
        // and it is built fresh at the call site — so substituting it
        // wherever the parameter is named re-allocates rather than
        // re-observes, which is the property this predicate is about.
        ExprNode::Hash { entries, .. } => {
            entries.iter().all(|(k, v)| is_pure_read(k) && is_pure_read(v))
        }
        _ => false,
    }
}

fn substitute_vars(e: &Expr, binding: &crate::det::HashMap<&Symbol, &Expr>) -> Expr {
    if let ExprNode::Var { name, .. } = &*e.node {
        if let Some(replacement) = binding.get(name) {
            return (*replacement).clone();
        }
    }
    let mut out = e.clone();
    out.node.for_each_child_mut(&mut |c| *c = substitute_vars(c, binding));
    out
}

/// Emit the IR for `io << <argument>` given the argument expression
/// from a `_buf = _buf + ARG` step. Splits into text-chunk vs.
/// output-interpolation handling — the latter goes through the
/// helper / partial / auto-escape classifiers.
fn emit_io_append(arg: &Expr, ctx: &ViewCtx) -> Vec<Expr> {
    // Text chunk → io << "literal".
    if let ExprNode::Lit { value: Literal::Str { .. } } = &*arg.node {
        return vec![accumulator_append_call(arg.clone(), ctx)];
    }
    // The compiler wraps `<%= expr %>` as `(expr).to_s`; strip that
    // wrapper. If the source wrote `<%= x.to_s %>` explicitly, we lose
    // the trailing `.to_s` — the round-trip is stable on the second
    // pass either way (matches the existing reconstruct_erb policy).
    let inner = unwrap_to_s(arg);

    // `<%= yield %>` and `<%= yield :slot %>` — appears in layouts
    // (and other capture-style templates that delegate body
    // rendering). Bare `yield` resolves to the layout's `body`
    // parameter (the rendered inner-view string); `yield :slot` is
    // a slot lookup against the content_for store.
    if let ExprNode::Yield { args: ya } = &*inner.node {
        return vec![accumulator_append_call(emit_yield(ya, ctx), ctx)];
    }

    // `<%= expr if cond %>` / `<%= expr unless cond %>` — modifier
    // conditional with only ONE live branch. Rails renders the expr only
    // when the guard passes, nothing otherwise. Emit a GUARDED append so
    // that branch goes through the same render/helper/escape classifiers
    // and a nil/false cond yields no output — instead of
    // `html_escape(<If>)`, which both wrongly escapes html_safe render
    // output and crashes on `html_escape(nil)`. Full if/else and ternaries
    // (`a ? b : c`, both branches live) fall through to the default
    // escape path.
    //
    // BOTH modifiers matter, and reading only the `if` shape is what left
    // campfire's `accounts/edit.html.erb:110` — `<%= render
    // "accounts/users/next_page_container", page: @page.next_param unless
    // @page.last? %>` — emitting a bare `render` that no view module
    // defines, which is where the spinel build stopped. `unless`
    // normalizes to `if cond then nil else <expr>`: the same node with
    // the live branch on the other side, not a different node.
    if let ExprNode::If { cond, then_branch, else_branch } = &*inner.node {
        let is_empty = |e: &Expr| {
            matches!(&*e.node, ExprNode::Lit { value: Literal::Nil })
                || matches!(&*e.node, ExprNode::Seq { exprs } if exprs.is_empty())
        };
        // An `if` with a dead ELSE keeps its live branch in `then`; an
        // `unless` with a dead THEN keeps it in `else`. Either way the
        // guard's own shape is preserved — negating the condition to
        // normalize the two would change what a nil cond means.
        let live = if is_empty(else_branch) {
            Some(false)
        } else if is_empty(then_branch) {
            Some(true)
        } else {
            None
        };
        if let Some(live_is_else) = live {
            let branch = if live_is_else { else_branch } else { then_branch };
            let stmts = seq(emit_io_append(branch, ctx));
            let dead = Expr::new(inner.span, ExprNode::Lit { value: Literal::Nil });
            let (then_branch, else_branch) =
                if live_is_else { (dead, stmts) } else { (stmts, dead) };
            let guarded = Expr::new(
                inner.span,
                ExprNode::If {
                    // Same predicate rewrite as statement-level conds
                    // (see the value-position If arm in
                    // rewrite_helpers_in_expr).
                    cond: rewrite_predicates(
                        &rewrite_helpers_in_expr(cond, ctx),
                        &ctx.nullable_locals,
                        &ctx.reference_reads,
                        &ctx.nilable_scalar_reads,
                    ),
                    then_branch,
                    else_branch,
                },
            );
            return vec![guarded];
        }
    }

    // form_with capture: `<%= form_with(opts) do |form| ...inner... %>`
    // — inline-expanded at lower time. Emits the opening `<form ...>`
    // tag, runtime CSRF + _method override helpers, a typed
    // FormBuilder constructor (no `ViewHelpers.form_with(HashMap)`
    // call), the walked body directly against the outer accumulator,
    // and the closing `</form>`. See `emit_form_with_inline` for the
    // shape rationale (Wedge 1b-i of the form_with macro-inline
    // retirement; tracking memo project_form_with_inlining.md).
    if let ExprNode::Send {
        recv: None,
        method,
        args: sa,
        block: Some(block),
        ..
    } = &*inner.node
    {
        if method.as_str() == "form_with" {
            return emit_form_with_inline(sa, block, ctx);
        }
        // `form_for :filter, url: ..., method: :get do |form|`: a form for
        // no record, which is form_with's `scope:` form (Rails moves
        // form_for's top-level `data:` into its html options, as
        // form_with takes it).
        if method.as_str() == "form_for" && matches!(sa.first().map(|a| &*a.node), Some(ExprNode::Lit { value: Literal::Sym { .. } })) {
            if let Some(args) = form_for_scope_args(sa, ctx) {
                return emit_form_with_inline(&args, block, ctx);
            }
            crate::emit::diagnostics::push(crate::diagnostic::Diagnostic::unsupported(
                inner.span,
                None,
                Symbol::from("form_for scope form"),
                "form_for :scope is lowered only with url: and no @scope record to fill its fields",
            ));
            return vec![accumulator_append_call(lit_str(String::new()), ctx)];
        }
        // `form_for(record, as: :user, url: …)`: form_with's `model:` form, with the class and id form_for gives the tag.
        if method.as_str() == "form_for" {
            match form_for_record_args(sa) {
                Ok((args, html)) => return super::form_with::emit_form_for_inline(&args, block, ctx, html),
                Err(why) => {
                    crate::emit::diagnostics::push(crate::diagnostic::Diagnostic::unsupported(inner.span, None, Symbol::from("form_for"), why));
                    return vec![accumulator_append_call(lit_str(String::new()), ctx)];
                }
            }
        }
        // `<%= form_tag(action, opts) do ...inner... %>` — the
        // builder-less bare form, same inline expansion minus the
        // FormBuilder binding (lobsters' link_post).
        if method.as_str() == "form_tag" {
            return emit_form_tag_inline(sa, block, ctx);
        }
        // `<%= label_tag name do %>…<% end %>` — block-content label
        // (filters page); the open tag inlines, the body splices.
        if method.as_str() == "label_tag" {
            if let Some(out) = emit_label_tag(sa, Some(block), ctx) {
                return out;
            }
        }
        // `<%= button_tag(opts) do %>…<% end %>` — block-content
        // button (campfire's message editor). Must be caught HERE:
        // the blockless dispatch below matches `block: None`, and the
        // generic block-helper arm captures the body to `_cap` while
        // leaving the `button_tag` call itself bare.
        if method.as_str() == "button_tag" {
            if let Some(out) = emit_button_tag_block(sa, block, ctx) {
                return out;
            }
        }
    }

    // Bare `<%= submit_tag label, opts %>` / `<%= button_tag content,
    // opts %>` — builder-less controls, inline-expanded like the
    // `form.*` builder methods (the opts hashes are literal at every
    // call site; the runtime alternative is the CRuby overlay's
    // untyped opts-walk).
    if let ExprNode::Send { recv: None, method, args: sa, block: None, .. } = &*inner.node {
        // `<%= turbo_exempts_page_from_preview %>` (campfire's
        // rooms/show) — the OUTPUT spelling of a Drive directive.
        // Rails' `provide` returns nil once it is handed content, so
        // the `<%= %>` renders "": the deposit is the whole effect and
        // nothing joins the accumulator.
        if let Some(out) = emit_turbo_drive_directive(method.as_str(), sa, inner.span) {
            return out;
        }
        // `<%= form_with url: …, method: :delete, id: … %>` with no
        // block — Rails' empty `<form>`, written to be targeted by a
        // detached `button_tag form: …`. The block-form inliner above
        // matches `block: Some(block)` and never sees it.
        if method.as_str() == "form_with" {
            if let Some(out) = emit_form_with_blockless(sa, ctx) {
                return out;
            }
        }
        if method.as_str() == "submit_tag" {
            return emit_submit_tag(sa, ctx);
        }
        if method.as_str() == "button_tag" {
            return emit_button_tag(sa, ctx);
        }
        if method.as_str() == "check_box_tag" {
            return emit_check_box_tag(sa, ctx);
        }
        // Blockless `<%= label_tag name, content[, opts] %>` — the
        // settings page's labeled fields. No-content non-literal
        // shapes fall through to the runtime helper.
        if method.as_str() == "label_tag" {
            if let Some(out) = emit_label_tag(sa, None, ctx) {
                return out;
            }
        }
    }

    // FormBuilder method dispatch: `<%= form.text_field :title, opts
    // %>` where `form` is the active form_with block param. After
    // Wedge 1b-ii these inline-expand to direct HTML accumulation —
    // no runtime FormBuilder dispatch survives in lowered output.
    // `textarea` alias normalizes to `text_area` via
    // `classify_form_builder_method`; class-array opts simplify to
    // base + first-key composition.
    if let ExprNode::Send {
        recv: Some(r),
        method,
        args: sa,
        block: None,
        ..
    } = &*inner.node
    {
        // Var inside a form_with lambda; bare Send in a bound PARTIAL
        // (the form local dropped out of the partial's params).
        if let Some(name) = super::form_with::form_param_ref_name(r) {
            if let Some(binding) = ctx
                .form_records
                .iter()
                .find(|b| b.form_param == name)
            {
                if let Some(fb) = classify_form_builder_method(method.as_str()) {
                    // Not for a recordless form: Trix's input id is the record's dom_id, which it has none of.
                    if !(binding.recordless && matches!(fb, FormBuilderMethod::RichTextArea)) {
                        return emit_form_builder_inline(binding, fb, sa, ctx);
                    }
                }
            }
        }
    }

    // The same dispatch for the BLOCK form — `<%= form.button(opts) do
    // %> … <% end %>`, where the label is markup rather than a string
    // argument (campfire's sign-in page and message composer).
    if let ExprNode::Send {
        recv: Some(r),
        method,
        args: sa,
        block: Some(block),
        ..
    } = &*inner.node
    {
        if let Some(name) = super::form_with::form_param_ref_name(r) {
            if let Some(binding) = ctx.form_records.iter().find(|b| b.form_param == name) {
                if let Some(fb) = classify_form_builder_method(method.as_str()) {
                    if let Some(out) =
                        emit_form_builder_block_inline(binding, fb, sa, block, ctx)
                    {
                        return out;
                    }
                }
            }
        }
    }

    // `<%= render(@posts) || "No posts" %>` — Rails answers nil for a
    // collection with no records (`CollectionRenderer`'s
    // `EmptyCollection`), so the fallback shows exactly when the
    // collection is empty. A collection render lowers to an `each` that
    // appends as it goes and has no value, so nothing below could read
    // the `||`, and the bare `render` reached the emit. Render the
    // collection with a flag set per record, then append the fallback
    // when none rendered: the collection is evaluated once, as Rails
    // does, where asking it `empty?` afterwards ran a relation's query
    // again. The flag is distinct per site and always a Boolean.
    if let ExprNode::BoolOp { op: BoolOpKind::Or, left, right, .. } = &*inner.node
        && let ExprNode::Send { recv, method, args: sa, block, .. } = &*left.node
    {
        let is_local = |n: &str| ctx.is_local(n);
        let is_options_ivar = |n: &str| ctx.is_options_ivar(n);
        let flag = Symbol::from(format!("__rendered_{}", inner.span.start));
        if let Some(rp) = classify_render_partial(
            recv.as_ref(),
            method.as_str(),
            sa,
            block.as_ref(),
            &is_local,
            &is_options_ivar,
        ) && let Some(render) = emit_flagged_collection_render(&rp, &flag, &ctx.for_render(sa))
        {
            let unset = Expr::new(
                inner.span,
                ExprNode::Assign {
                    target: LValue::Var { id: VarId(0), name: flag.clone() },
                    value: Expr::new(inner.span, ExprNode::Lit { value: Literal::Bool { value: false } }),
                },
            );
            let fallback = send(Some(right.clone()), "to_s", Vec::new(), None, false);
            let guarded = Expr::new(
                inner.span,
                ExprNode::If {
                    cond: send(None, "!", vec![var_ref(flag.clone())], None, false),
                    then_branch: seq(emit_io_append(&fallback, ctx)),
                    else_branch: nil_lit(),
                },
            );
            return vec![unset, render, guarded];
        }
    }

    // Render-partial classifier: `render @articles` / `render
    // @article.comments` / `render "x", k: v` → spinel-shape iteration
    // or named-partial dispatch. Wins over the helper classifier
    // because `render` is reserved.
    if let ExprNode::Send { recv, method, args: sa, block, .. } = &*inner.node {
        let is_local = |n: &str| ctx.is_local(n);
        let is_options_ivar = |n: &str| ctx.is_options_ivar(n);
        if let Some(rp) = classify_render_partial(
            recv.as_ref(),
            method.as_str(),
            sa,
            block.as_ref(),
            &is_local,
            &is_options_ivar,
        ) {
            let render_ctx = ctx.for_render(sa);
            // `render layout: "x" do … end` lowers to SEVERAL statements
            // (a capture local, then the layout call), so it can't ride
            // `emit_render_partial`'s single-expression return. It also
            // has to be caught here rather than in the generic
            // block-form helper arm below, which would keep the bare
            // `render` and emit a call to a method no view module has.
            if let crate::lower::view::RenderPartial::LayoutBlock { layout, locals, block } = &rp {
                if let Some(stmts) =
                    super::partial::emit_layout_block(layout, *locals, block, &render_ctx)
                {
                    return stmts;
                }
            }
            if let Some(stmt) = emit_render_partial(&rp, &render_ctx) {
                return vec![stmt];
            }
        }
    }

    // `turbo_stream.<action>(...)` in a `.turbo_stream.erb` template.
    // Needs the RECEIVER (turbo_stream is a builder object, not a bare
    // helper), so it can't ride the classifier below.
    // Matched WITHOUT the `block: None` constraint so the block form
    // (`turbo_stream.append target do … end`) reaches the residue arm
    // below instead of falling through unmentioned.
    if let ExprNode::Send { recv, method, args: sa, block, .. } = &*inner.node {
        let classified = block.is_none().then(|| {
            crate::lower::view::classify_turbo_stream_call(recv.as_ref(), method.as_str(), sa)
        }).flatten();
        if let Some(ts) = &classified {
            if let Some(call) = super::helpers::emit_turbo_stream_fragment(ts, ctx) {
                return vec![accumulator_append_call(call, ctx)];
            }
        }
        // The two spellings `classify_turbo_stream_call` declines, both
        // of which campfire's `rooms/refreshes/show.turbo_stream.erb`
        // writes in six lines:
        //
        //   turbo_stream.append dom_id(@room, :messages) do … end
        //   turbo_stream.replace dom_id(message), partial: "…", locals: {…}
        //
        // Both differ from the positional form only in where the
        // `<template>` CONTENT comes from, so both reuse the same target
        // builder and the same `Broadcasts.turbo_stream_fragment`
        // composer — the markup keeps one owner.
        if super::helpers::is_turbo_stream_builder(recv.as_ref())
            && matches!(
                method.as_str(),
                "append" | "prepend" | "replace" | "update" | "before" | "after"
            )
        {
            // BLOCK form. The content is the block's template body, so
            // walk it into its own accumulator and hand the composer the
            // captured String. Emitted as three STATEMENTS rather than
            // one nested expression: the walker arms already return a
            // statement list, and a `Seq` in argument position renders as
            // newline-joined statements on the ruby family.
            if let (1, Some(ExprNode::Lambda { params, extra_params, body, .. })) =
                (sa.len(), block.as_ref().map(|b| &*b.node))
            {
                let cap = "_ts_cap";
                let cap_ctx = ViewCtx {
                    accumulator: cap.to_string(),
                    ..ctx.with_locals(
                        params.iter().chain(extra_params.iter().map(|p| &p.name)).map(|p| p.as_str().to_string()),
                    )
                };
                let mut out = vec![assign_accumulator_string_new(cap)];
                out.extend(walk_body(body, &cap_ctx));
                out.push(accumulator_append_call(
                    super::helpers::turbo_stream_fragment_call(
                        method.as_str(),
                        super::helpers::turbo_stream_target(&sa[0], ctx),
                        accumulator_result_ref(cap),
                    ),
                    ctx,
                ));
                return out;
            }
            // OPTION form: `partial:` (+ optional `locals:` / `collection:`
            // / `as:`). Same resolution a `render partial:` call site
            // gets, through the same `named_partial_call` — a second
            // partial resolver here would be a second set of rules to
            // keep in step. Looked up in html: turbo-rails renders it
            // `formats: [:html]` (`TagBuilder#render_template`).
            if let Some(ts) = block
                .is_none()
                .then(|| super::helpers::turbo_stream_partial(recv.as_ref(), method.as_str(), sa))
                .flatten()
            {
                let locals = ts.locals.map(|entries| {
                    entries
                        .iter()
                        .map(|(k, v)| (k.clone(), rewrite_helpers_in_expr(v, ctx)))
                        .collect::<Vec<_>>()
                });
                // `partial:` alone renders once. `collection:` is
                // Rails' "once per element" — campfire's
                // `accounts/users/index.turbo_stream.erb` writes
                // `turbo_stream.replace :next_page_container,
                // partial: "…/user", collection: @page.records,
                // as: :user`. Treating that as a single render
                // would keep only the first element, so the
                // collection walks into a capture accumulator and
                // the captured String is the fragment.
                if let Some(coll) = ts.collection {
                    if let Some(out) = turbo_stream_collection_fragment(
                        method.as_str(),
                        ts.target,
                        ts.partial,
                        rewrite_helpers_in_expr(coll, ctx),
                        ts.as_name,
                        locals.as_deref(),
                        ctx,
                    ) {
                        return out;
                    }
                } else if let Some(html) = super::partial::named_partial_call(
                    ts.partial,
                    None,
                    locals.as_deref(),
                    &ctx.in_formats(&["html"]),
                ) {
                    return vec![accumulator_append_call(
                        super::helpers::turbo_stream_fragment_call(
                            method.as_str(),
                            super::helpers::turbo_stream_target(ts.target, ctx),
                            html,
                        ),
                        ctx,
                    )];
                }
            }
        }
        // Recognized the builder but not this spelling. Left in
        // source shape, where `turbo_stream` resolves to nothing, so say
        // so rather than emitting a call that silently won't run.
        if super::helpers::is_turbo_stream_builder(recv.as_ref()) {
            crate::emit::diagnostics::push(crate::lower::residue_diagnostic(
                "turbo_stream_builder",
                "turbo_stream.<action>",
                inner.span,
                "only the positional `target[, record]` spelling is lowered",
                format!(
                    "`turbo_stream.{}` left in source shape — the option form \
                     (partial:/collection:/locals:) and the block form need the \
                     partial machinery a `render` call site gets, so this \
                     template will not render",
                    method.as_str()
                ),
            ));
        }
    }

    // turbo-rails' `turbo_frame_tag`, in both spellings — with a block
    // (the frame wraps template content) and without (a lazily-loaded
    // frame that is only a `src`). Placed ahead of the classifier
    // because the block form never reaches it: the classifier matches
    // `block: None`, so a framed template body fell through to the
    // generic block-form helper below, which captured the body and left
    // the `turbo_frame_tag` call itself unresolved.
    if let ExprNode::Send { recv: None, method, args: sa, block, .. } = &*inner.node {
        if method.as_str() == "turbo_frame_tag" && !ctx.is_local("turbo_frame_tag") {
            if let Some(stmts) = emit_turbo_frame_tag(sa, block.as_ref(), ctx) {
                return stmts;
            }
        }
    }

    // View-helper classifier: `link_to`, `dom_id`, `pluralize`,
    // `truncate`, `turbo_stream_from`, `content_for(:slot)`, …. The
    // classifier matches bare Sends (no recv, no block) only.
    if let ExprNode::Send { recv: None, method, args: sa, block: None, parenthesized } = &*inner.node {
        if let Some(kind) = classify_template_helper(method.as_str(), sa, *parenthesized, ctx) {
            if let Some(call) = emit_view_helper_call(&kind, ctx) {
                if let ViewHelperKind::LinkToIf { url, negate, .. } = &kind
                    && let Some(stmts) = link_to_if_statement(&call, url, *negate, ctx)
                {
                    return stmts;
                }
                return vec![accumulator_append_call(call, ctx)];
            }
        }
    }

    // ActionView dynamic tag builder: `<%= tag.<element>(opts) do
    // ...inner... %>` (lobsters' stories/_form `tag.details`). The
    // receiver is the bare `tag` builder and the method name is the HTML
    // element. Inline-expand to open/walk/close rather than let it fall
    // to the generic fallback, which rebuilds `tag.<element> do ... end`
    // verbatim — an unresolved `sp_raise_nomethod` under spinel AOT.
    if let ExprNode::Send {
        recv: Some(r),
        method,
        args: sa,
        block: Some(block),
        ..
    } = &*inner.node
    {
        let is_bare_tag = matches!(
            &*r.node,
            ExprNode::Send { recv: None, method: m, args, block: None, .. }
                if m.as_str() == "tag" && args.is_empty()
        );
        if is_bare_tag && !ctx.is_local("tag") {
            return emit_tag_builder_inline(method.as_str(), sa, block, ctx);
        }
    }

    // The same builder WITHOUT a block: `<%= tag.meta name: "…", content:
    // "…" %>` / `<%= tag.img src: … %>` (campfire's application layout
    // writes five of these, so every page depends on it). Without this
    // arm the call falls to the auto-escape default below, which is
    // wrong twice over — `tag` is unbound at run time, and even bound,
    // Rails' builder returns a SafeBuffer that must not be escaped.
    if let ExprNode::Send {
        recv: Some(r),
        method,
        args: sa,
        block: None,
        ..
    } = &*inner.node
    {
        let is_bare_tag = matches!(
            &*r.node,
            ExprNode::Send { recv: None, method: m, args, block: None, .. }
                if m.as_str() == "tag" && args.is_empty()
        );
        if is_bare_tag && !ctx.is_local("tag") {
            return emit_tag_builder_void_or_content(method.as_str(), sa, ctx);
        }
    }

    // `<%= composer_form_tag(room) do |form| %> … <% end %>` — a helper
    // that is nothing but a `form_with` with the block forwarded
    // through. Splice the wrapped call in with the caller's block
    // attached, then walk THAT: the form-builder macro-inline needs the
    // `form_with` and the `form.rich_text_area` calls visible at once,
    // and they are on opposite sides of the helper boundary until this
    // runs. See `form_wrapper_helpers` for why the other block-
    // forwarding wrappers don't need it.
    if let ExprNode::Send {
        recv: None,
        method,
        args: sa,
        block: Some(block),
        ..
    } = &*inner.node
    {
        if let Some(w) = ctx.form_wrappers.get(method.as_str()) {
            if !ctx.is_local(method.as_str()) {
                if let Some(spliced) = splice_form_wrapper(w, sa, block) {
                    // The helper's body is read in the VIEW's context,
                    // which is what Rails does — a view helper runs
                    // against the view, so its `@user` is the
                    // controller's ivar. Here the view is a module
                    // function whose ivars became parameters before the
                    // walker ran, so the spliced body has to make the
                    // same move. campfire's `profile_form_with(model,
                    // …)` ignores its own parameter and names `@user`;
                    // unrewritten it stayed an ivar on a module
                    // function — nil — and every field died on
                    // `[]` for nil.
                    let spliced = super::rewrite_ivars_to_locals(&spliced, &ctx.ivar_locals);
                    return emit_io_append(&spliced, ctx);
                }
            }
        }
    }

    // `<%= link_to_if cond, name, url do |name| %> … <% end %>`, and the
    // same for `link_to_unless`: when no link is made, Rails calls the
    // block with the name and shows what it renders. Ahead of the generic
    // arm below, which would rebuild the call with its block and leave
    // the runtime's blockless helper to answer it, dropping the block.
    if let ExprNode::Send {
        recv: None,
        method,
        args: sa,
        block: Some(block),
        ..
    } = &*inner.node
    {
        if matches!(method.as_str(), "link_to_if" | "link_to_unless") && !ctx.is_local(method.as_str()) {
            // Refused (`analyze::diagnostics`): a raise where it runs, as a
            // refused gem constant lowers (`gem_refusal`), not the blockless
            // helper the generic arm below would leave, which drops the block.
            if let ExprNode::Lambda { params, .. } = &*block.node
                && let Some(p) = params.first()
                && refuses_param(p.as_str(), &ctx.assigned_locals, &ctx.nested_assigned_locals)
            {
                return vec![send(
                    None,
                    "raise",
                    vec![lit_str(format!(
                        "{method}'s block parameter `{p}` shadows a template local assigned in an earlier nested block; not modeled"
                    ))],
                    None,
                    true,
                )];
            }
            if let Some(stmts) = emit_link_to_if_block(method.as_str(), sa, block, ctx) {
                return stmts;
            }
        }
    }

    // `<%= button_to url, opts do %> …markup… <% end %>` and the same
    // for `link_to` — the block spelling of a helper whose positional
    // form already inlines. Ahead of the generic arm below, which would
    // rebuild the call verbatim and leave a bare `button_to` / `link_to`
    // the emitted module does not define: the whole helper is inlined at
    // lower time, so nothing answers it at run time.
    if let ExprNode::Send {
        recv: None,
        method,
        args: sa,
        block: Some(block),
        ..
    } = &*inner.node
    {
        if matches!(method.as_str(), "button_to" | "link_to") && !ctx.is_local(method.as_str()) {
            if let ExprNode::Lambda { params, extra_params, body, .. } = &*block.node {
                if block_body_is_template(body) && extra_params.is_empty() {
                    if let Some(stmts) =
                        emit_inline_helper_block(method.as_str(), sa, body, params, ctx)
                    {
                        return stmts;
                    }
                }
            }
        }
    }

    // Generic block-form output helper: `<%= form_tag(...) do %> INNER
    // <% end %>` (form_tag / content_tag / link_to-with-block — anything
    // not form_with, form_builder, or render). The block body is template
    // buffer ops; walk it into a fresh capture accumulator the block
    // *returns*, so the inner `_buf = _buf + …` lines become real appends
    // instead of surviving raw (an undefined `_buf`, and a paren-less
    // helper arg whose comma Ruby reads as a multi-assign target). The
    // wrapping call's parens bind the `do`-block to the helper, not `<<`.
    if let ExprNode::Send {
        recv,
        method,
        args: sa,
        block: Some(block),
        parenthesized,
    } = &*inner.node
    {
        if let ExprNode::Lambda { rest_param, extra_params, params, block_param, body, block_style } = &*block.node {
            if block_body_is_template(body) {
                // NESTED CAPTURES NEED DISTINCT NAMES. A block-with-block
                // helper (campfire's rooms/show wraps `messages_tag` inside
                // `message_area_tag`) walks the inner block while the outer
                // one is still accumulating, and a Ruby block does NOT
                // introduce a new scope for an assignment to a name the
                // enclosing block already bound: the inner `_cap =
                // String.new` REBINDS the outer local. What follows is
                // silent — `outer << inner_helper { … }` has already
                // evaluated its receiver, so the wrapped result appends to
                // a buffer nothing returns, and the outer block hands back
                // the INNER string. campfire's room page rendered its
                // composer and its stream source and dropped the message
                // list and everything before it: 24KB where Rails sends
                // 456KB, with the app's own controller test still green.
                //
                // capture_inline.rs called this shot — "the shared `_cap`
                // name would shadow if one appears" — on the assumption
                // that no app in the corpus nested one. campfire does.
                let cap_owned = next_capture_name(&ctx.accumulator);
                let cap = cap_owned.as_str();
                // `content_tag(name[, opts]) do … end`: Rails renders the
                // block's markup as the tag's content and the second
                // argument as its options. The runtime's `content_tag` takes
                // no block, so the capture is a statement and its String the
                // (already escaped) content of `content_tag_html`.
                if recv.is_none()
                    && method.as_str() == "content_tag"
                    && params.is_empty()
                    && extra_params.is_empty()
                    && rest_param.is_none()
                    && block_param.is_none()
                    && (1..=2).contains(&sa.len())
                {
                    // The capture's statements share the view's scope with
                    // every other flat capture (a `cache` body's included,
                    // whose `__cache_io_N` restarts `next_capture_name`), so
                    // the name is per site, as `emit_cached_fragment`'s are.
                    let cap_owned = format!("__ctag_io_{}", inner.span.start);
                    let cap = cap_owned.as_str();
                    // Rails evaluates the name and options, in order,
                    // before the block runs: each is read into a local
                    // first, so the block neither reassigns what they read
                    // nor runs ahead of a call in them.
                    let mut stmts = Vec::new();
                    let mut bound = 0;
                    let args: Vec<Expr> = sa
                        .iter()
                        .map(|a| {
                            // A literal `class:` Array is space-joined here, as
                            // form_with's options are: strict targets render
                            // an Array value with its `to_s`.
                            let a = match &*a.node {
                                ExprNode::Hash { entries, kwargs } => Expr::new(a.span, ExprNode::Hash {
                                    entries: entries.iter().map(|(k, v)| (k.clone(), super::form_with::simplify_opts_value(k, v))).collect(),
                                    kwargs: *kwargs,
                                }),
                                _ => a.clone(),
                            };
                            bind_before_block(rewrite_helpers_in_expr(&a, ctx), inner.span.start, &mut bound, &mut stmts)
                        })
                        .collect();
                    let cap_ctx = ViewCtx { accumulator: cap.to_string(), ..ctx.with_locals(std::iter::empty()) };
                    stmts.push(assign_accumulator_string_new(cap));
                    stmts.extend(walk_body(body, &cap_ctx));
                    let mut args = args.into_iter();
                    let mut call_args = vec![args.next().expect("a content_tag name"), accumulator_result_ref(cap)];
                    call_args.extend(args);
                    stmts.push(accumulator_append_call(view_helpers_call("content_tag_html", call_args), ctx));
                    return stmts;
                }
                // Every parameter the block binds is a local in its body, the
                // optional and keyword ones too: a read of one is not a
                // helper call.
                let cap_ctx = ViewCtx {
                    accumulator: cap.to_string(),
                    ..ctx.with_locals(
                        params.iter().chain(extra_params.iter().map(|p| &p.name)).map(|p| p.as_str().to_string()),
                    )
                };
                // A helper a gem mixes into the view (`simple_form_for`): Rails' method, on a view context, whose block's
                // capture is the template's markup, a safe buffer.
                let on_view_context = recv.is_none() && ctx.view_context_helpers.contains(method.as_str());
                let mut cap_stmts = vec![assign_accumulator_string_new(cap)];
                cap_stmts.extend(walk_body(body, &cap_ctx));
                cap_stmts.push(if on_view_context {
                    Expr::new(Span::synthetic(), ExprNode::Send { recv: Some(accumulator_result_ref(cap)), method: Symbol::from("html_safe"), args: Vec::new(), block: None, parenthesized: false })
                } else {
                    accumulator_result_ref(cap)
                });
                let new_block = Expr::new(
                    block.span,
                    ExprNode::Lambda { rest_param: rest_param.clone(), extra_params: extra_params.clone(),
                        params: params.clone(),
                        block_param: block_param.clone(),
                        body: seq(cap_stmts),
                        block_style: *block_style,
                    },
                );
                let view_context = Expr::new(Span::synthetic(), ExprNode::Send {
                    recv: Some(Expr::new(Span::synthetic(), ExprNode::Const { path: vec![Symbol::from("ActionView"), Symbol::from("Base")] })),
                    method: Symbol::from("new"),
                    args: Vec::new(),
                    block: None,
                    parenthesized: false,
                });
                let rebuilt = Expr::new(
                    inner.span,
                    ExprNode::Send {
                        recv: if on_view_context { Some(view_context) } else { recv.as_ref().map(|r| rewrite_helpers_in_expr(r, ctx)) },
                        method: method.clone(),
                        args: sa.iter().map(|a| rewrite_helpers_in_expr(a, ctx)).collect(),
                        block: Some(new_block),
                        parenthesized: *parenthesized,
                    },
                );
                let escaped = view_helpers_call("html_escape", vec![rebuilt]);
                return vec![accumulator_append_call(escaped, ctx)];
            }
        }
    }

    // Default: bare interpolation `<%= expr %>` of a non-helper —
    // auto-escape. `<%= article.title %>` becomes
    // `io << ViewHelpers.html_escape(article.title)`. This matches
    // Rails's default behavior on `<%= %>` outside of helper output.
    //
    // `<%= h(x) %>` — ERB's escape alias asks for exactly what the
    // default already does; unwrap it so the auto-escape applies once
    // rather than twice (nested `h` is handled — and double-escapes,
    // as in Rails — inside rewrite_helpers_in_expr).
    let mut inner = inner;
    while let ExprNode::Send { recv: None, method, args, block: None, .. } = &*inner.node {
        if method.as_str() == "h" && args.len() == 1 {
            inner = &args[0];
        } else {
            break;
        }
    }
    //
    // Before wrapping, recurse through the expression and rewrite
    // any nested helper Sends to their ViewHelpers.* form so shapes
    // like `<%= content_for(:title) || "Real Blog" %>` (a BoolOp
    // whose left side is a bare helper call) come out as
    // `html_escape(ViewHelpers.content_for_get(:title) || "Real
    // Blog")` rather than carrying the raw `content_for` Send.
    let rewritten = rewrite_helpers_in_expr(inner, ctx);
    // A marked value skips the wrap. `<%= x.html_safe %>` says so right
    // here; a call to a method whose body ends in `.html_safe` says it
    // one level down, and `lower::html_safe` recorded that on the App so
    // this side can see it. Escaping either one ships literal
    // `&lt;span&gt;` markup where the author asked for markup —
    // lobsters' `hat.to_html_label` renders a whole element.
    if let Some(safe) = html_safe_value(&rewritten, ctx) {
        return vec![accumulator_append_call(coerce_to_s(safe), ctx)];
    }
    // `truncate(x) + "!"`: Rails' SafeBuffer `+` stays html-safe, escaping
    // only what it appends, so the whole is written as it is.
    if let Some(safe) = safe_buffer_value(&rewritten, ctx) {
        return vec![accumulator_append_call(coerce_to_s(safe), ctx)];
    }
    // Not left to each emitter: only the Ruby one unwrapped the escape around `raw`, and TypeScript shipped `html_escape(raw(x))`
    if let ExprNode::Send { recv, method, args, block: None, .. } = &*rewritten.node
        && method.as_str() == "raw"
        && args.len() == 1
        && recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::Const { path } if path.last().is_some_and(|s| s.as_str() == "ViewHelpers")))
    {
        let mut call = view_helpers_call("raw", vec![args[0].clone()]);
        call.ty = Some(crate::ty::Ty::Str);
        return vec![accumulator_append_call(coerce_to_s(call), ctx)];
    }
    let escaped = view_helpers_call("html_escape", vec![coerce_to_s(rewritten)]);
    vec![accumulator_append_call(escaped, ctx)]
}

/// `turbo_stream.replace target, partial: "…", collection: xs, as: :x`.
///
/// Rails renders the named partial once per element and concatenates
/// the markup into the fragment. The same `named_partial_call` a
/// `render partial:, collection:` site uses supplies each element's
/// HTML; the capture accumulator is the block-form's `_ts_cap`, so a
/// nested block-form helper would still collide — no corpus view nests
/// one inside a collection fragment.
fn turbo_stream_collection_fragment(
    action: &str,
    target: &Expr,
    partial: &str,
    collection: Expr,
    as_name: Option<&str>,
    locals: Option<&[(Expr, Expr)]>,
    ctx: &ViewCtx,
) -> Option<Vec<Expr>> {
    let base = match partial.rsplit_once('/') {
        Some((_, name)) => name,
        None => partial,
    };
    let var = Symbol::from(crate::naming::safe_local(
        as_name.unwrap_or_else(|| base.trim_start_matches('_')),
    ));
    let mut prelude = Vec::new();
    let bound_locals: Option<Vec<(Expr, Expr)>> = locals.map(|entries| {
        entries
            .iter()
            .enumerate()
            .map(|(i, (k, v))| {
                let name = Symbol::from(format!("_ts_local_{i}"));
                prelude.push(Expr::new(
                    Span::synthetic(),
                    ExprNode::Assign {
                        target: LValue::Var {
                            id: VarId(0),
                            name: name.clone(),
                        },
                        value: v.clone(),
                    },
                ));
                (k.clone(), var_ref(name))
            })
            .collect()
    });
    let html = super::partial::named_partial_call_with_record(
        partial,
        None,
        bound_locals.as_deref(),
        Some(var_ref(var.clone())),
        &ctx.in_formats(&["html"]),
    )?;
    let cap = "_ts_cap";
    let cap_ctx = ViewCtx {
        accumulator: cap.to_string(),
        ..ctx.clone()
    };
    // Rails treats a nil collection as empty, not as `nil.each`.
    let each_recv = Expr::new(
        Span::synthetic(),
        ExprNode::BoolOp {
            op: BoolOpKind::Or,
            surface: BoolOpSurface::Symbol,
            left: collection,
            right: Expr::new(
                Span::synthetic(),
                ExprNode::Array {
                    elements: vec![],
                    style: Default::default(),
                },
            ),
        },
    );
    let each = send(
        Some(each_recv),
        "each",
        Vec::new(),
        Some(Expr::new(
            Span::synthetic(),
            ExprNode::Lambda { extra_params: Vec::new(),
                rest_param: None,
                params: vec![var],
                block_param: None,
                body: accumulator_append_call(html, &cap_ctx),
                block_style: BlockStyle::Brace,
            },
        )),
        false,
    );
    let mut out = prelude;
    out.push(assign_accumulator_string_new(cap));
    out.push(each);
    out.push(accumulator_append_call(
        super::helpers::turbo_stream_fragment_call(
            action,
            super::helpers::turbo_stream_target(target, ctx),
            accumulator_result_ref(cap),
        ),
        ctx,
    ));
    Some(out)
}

/// `turbo_frame_tag <ids…>[, opts][ do … end]` → the `<turbo-frame …>`
/// element, the walked block body, and the closing tag — the same
/// open/walk/close splice `emit_tag_builder_inline` does, against the
/// SAME outer accumulator, because an ERB block body is template buffer
/// ops that have to be walked rather than captured.
///
/// The id positionals go through `rewrite_helpers_in_expr` first: that
/// is what turns campfire's `turbo_frame_tag dom_id(room, :involvement)`
/// into the `ViewHelpers.dom_id(…)` call an emitted view can answer.
/// Options are left as written, the way every other tag inlined here
/// leaves them (route helpers in their values are rewritten later, by
/// the routes pass, over the whole IR).
///
/// `None` when the argument shape declines (see
/// `turbo_frames::frame_open_parts`), which leaves the call to the
/// generic paths below rather than inventing an id.
fn emit_turbo_frame_tag(args: &[Expr], block: Option<&Expr>, ctx: &ViewCtx) -> Option<Vec<Expr>> {
    let args: Vec<Expr> = args
        .iter()
        .map(|a| match &*a.node {
            ExprNode::Hash { .. } => a.clone(),
            _ => rewrite_helpers_in_expr(a, ctx),
        })
        .collect();
    let is_record = |e: &Expr| super::turbo_frames::names_a_record(e, &ctx.model_singulars);
    let parts = super::turbo_frames::frame_open_parts(&args, &is_record)?;
    let mut out = vec![accumulator_append_call(
        super::attr_parts::string_interp(parts),
        ctx,
    )];
    if let Some(ExprNode::Lambda { params, extra_params, body, .. }) = block.map(|b| &*b.node) {
        let inner_ctx = ctx.with_locals(
            params.iter().chain(extra_params.iter().map(|p| &p.name)).map(|p| p.as_str().to_string()),
        );
        out.extend(walk_body(body, &inner_ctx));
    }
    out.push(accumulator_append_call(
        super::lit_str(super::turbo_frames::FRAME_CLOSE.to_string()),
        ctx,
    ));
    Some(out)
}

/// The value behind an html-safe interpolation, or `None` when the
/// expression has to be escaped. `<e>.html_safe` yields `<e>` (the mark
/// is answered here rather than left for a runtime that has no
/// safe-string type); a call to a recorded producer yields itself, when
/// its receiver is one of the classes that have the marked definition.
pub(super) fn html_safe_value(e: &Expr, ctx: &ViewCtx) -> Option<Expr> {
    let ExprNode::Send { recv, method, args, block: None, .. } = &*e.node else {
        return None;
    };
    if method.as_str() == "html_safe" && args.is_empty() {
        return recv.clone();
    }
    let Some(r) = recv else { return None };
    if !ctx.html_safe_methods.names(method.as_str()) {
        return None;
    }
    let (classes, singleton) = receiver_classes(r)?;
    classes
        .iter()
        .all(|c| ctx.html_safe_methods.resolves_marked(c, method.as_str(), singleton))
        .then(|| e.clone())
}

/// The classes a receiver can be, when they are all known, and whether
/// the call is on the class itself: a constant (`Formatting.label`) is
/// that class, singleton; a typed receiver is its class, or the non-nil
/// classes of a union (`Hat | nil`). `None` for anything else, an
/// untyped receiver among them.
fn receiver_classes(r: &Expr) -> Option<(Vec<String>, bool)> {
    if let ExprNode::Const { path } = &*r.node {
        return Some((vec![path.iter().map(|p| p.as_str()).collect::<Vec<_>>().join("::")], true));
    }
    fn of(ty: &crate::ty::Ty, out: &mut Vec<String>) -> bool {
        match ty {
            crate::ty::Ty::Class { id, .. } => {
                out.push(id.0.as_str().to_string());
                true
            }
            crate::ty::Ty::Nil => true,
            crate::ty::Ty::Union { variants } => variants.iter().all(|v| of(v, out)),
            _ => false,
        }
    }
    let mut out = Vec::new();
    (of(r.ty.as_ref()?, &mut out) && !out.is_empty()).then_some((out, false))
}

/// The value of a lowered expression that Rails holds as an html-safe
/// buffer, ready to write unescaped, or `None` when it has to be escaped
/// as a whole. A buffer is an escape the view helpers applied
/// (`ActionView::ViewHelpers.html_escape(…)`, which is how `truncate`,
/// `word_wrap` and `h` lower), an explicit `x.html_safe`, a call to an
/// app method marked html-safe on every class its receiver can be
/// ([`html_safe_value`]), or a buffer `+` anything: SafeBuffer's
/// `+` stays safe and escapes the operand unless that is a buffer too. A
/// String on the left (`"!" + truncate(x)`) is a plain String in Rails,
/// so it isn't one.
pub(super) fn safe_buffer_value(e: &Expr, ctx: &ViewCtx) -> Option<Expr> {
    let ExprNode::Send { recv: Some(r), method, args, block: None, parenthesized } = &*e.node else {
        return None;
    };
    if let Some(marked) = html_safe_value(e, ctx) {
        return Some(marked);
    }
    match (method.as_str(), args.len()) {
        ("html_escape", 1) if matches!(&*r.node, ExprNode::Const { path } if path.iter().map(|p| p.as_str()).eq(["ActionView", "ViewHelpers"])) => {
            Some(e.clone())
        }
        // `simple_format`'s `sanitize: false` form, the one it lowers to,
        // returns Rails' html-safe buffer.
        ("simple_format", 2 | 3) if matches!(&*r.node, ExprNode::Const { path } if path.iter().map(|p| p.as_str()).eq(["ActionView", "ViewHelpers"])) => {
            Some(e.clone())
        }
        ("html_safe", 0) => Some((*r).clone()),
        ("+", 1) => {
            let left = safe_buffer_value(r, ctx)?;
            let right = safe_buffer_value(&args[0], ctx)
                .unwrap_or_else(|| view_helpers_call("html_escape", vec![coerce_to_s(args[0].clone())]));
            let mut sum = Expr::new(
                e.span,
                ExprNode::Send {
                    recv: Some(left),
                    method: method.clone(),
                    args: vec![right],
                    block: None,
                    parenthesized: *parenthesized,
                },
            );
            sum.ty = Some(crate::ty::Ty::Str);
            Some(sum)
        }
        _ => None,
    }
}

/// Re-add the `.to_s` coercion that `unwrap_to_s` stripped, so the
/// auto-escape `html_escape(...)` wrap always feeds a String. The ERB
/// compiler wraps every `<%= expr %>` as `(expr).to_s`; we strip that
/// up front so the render / yield / helper / modifier-if classifiers can
/// pattern-match the bare inner expr, but the bare-interpolation default
/// then has to put it back. `html_escape` is deliberately monomorphic
/// `(String) -> String` (it calls `.gsub`; see ViewHelpers.html_escape),
/// so a bare `<%= article.id %>` / `<%= comment.score %>` — an Integer —
/// would otherwise crash. Rails likewise coerces with `to_s` before
/// escaping, and `nil.to_s == ""` gives the empty-render Rails produces
/// for a nil interpolation.
///
/// String literals are returned untouched so `view_helpers_call` can
/// still constant-fold `html_escape("literal")`; coercing one would be a
/// no-op (`String#to_s` is identity) that only defeats the fold.
fn coerce_to_s(expr: Expr) -> Expr {
    if matches!(&*expr.node, ExprNode::Lit { value: Literal::Str { .. } }) {
        return expr;
    }
    // Already a `.to_s` send — the source wrote `<%= x.to_s %>` and
    // `unwrap_to_s` peeled only the compiler's outer wrap, leaving the
    // explicit one. `String#to_s` is identity, so don't double it.
    if let ExprNode::Send { method, args, .. } = &*expr.node {
        if method.as_str() == "to_s" && args.is_empty() {
            return expr;
        }
    }
    let span = expr.span;
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(expr),
            method: Symbol::from("to_s"),
            args: Vec::new(),
            block: None,
            parenthesized: false,
        },
    )
}

/// `classify_view_helper` for a bare Send in a template, except that a
/// zero-arg Send without parens naming one of the view's locals is a
/// read of it: a partial handed `csrf_meta_tags:` reads the caller's
/// value, as Ruby takes the bare name for the lvar, not the helper.
/// `csrf_meta_tags()` is still the call.
pub(super) fn classify_template_helper<'a>(
    method: &str,
    args: &'a [Expr],
    parenthesized: bool,
    ctx: &ViewCtx,
) -> Option<ViewHelperKind<'a>> {
    if args.is_empty() && !parenthesized && ctx.binds_local(method) {
        return None;
    }
    classify_view_helper(method, args)
}

/// Recursively walk `expr` and rewrite any bare view-helper Send
/// (`Send { recv: None, method, args, block: None }`) into its
/// `ViewHelpers.*` form via `classify_view_helper` +
/// `emit_view_helper_call`. Threads through `BoolOp` and `Send`
/// children so helpers nested inside expressions get reached. Other
/// shapes pass through; leaf nodes are unchanged.
///
/// The auto-escape fallback uses this so `<%= helper(...) || default
/// %>` and similar combinations end up with the helper rewritten
/// before the html_escape wrap, matching the convention every
/// emitter would otherwise have to repeat.
pub(super) fn rewrite_helpers_in_expr(e: &Expr, ctx: &ViewCtx) -> Expr {
    // `link_to "Back", :back` by any route a link takes: `:back` is Rails' `url_for(:back)`, the referrer.
    if let ExprNode::Send { recv: None, method, args, block, parenthesized } = &*e.node
        && method.as_str() == "link_to"
        && args.get(1).is_some_and(|a| matches!(&*a.node, ExprNode::Lit { value: crate::expr::Literal::Sym { value } } if value.as_str() == "back"))
    {
        let mut args = args.clone();
        args[1] = super::view_helpers_call("back_url", Vec::new());
        let swapped = Expr { node: Box::new(ExprNode::Send { recv: None, method: method.clone(), args, block: block.clone(), parenthesized: *parenthesized }), ..e.clone() };
        return rewrite_helpers_in_expr(&swapped, ctx);
    }
    // `x.presence` in a view's output (`<%= r[:title].presence || t(...) %>`):
    // ActiveSupport's Object#presence, which only the CRuby overlay can
    // reopen onto every value. `lower::blank` grounds it in app bodies but
    // does not walk views (their conditions have their own predicate
    // rewrite), so an output site stayed a dynamic call that the spinel
    // binary answered with NoMethodError. It becomes the runtime predicate
    // `lower::blank` uses for a receiver it cannot type:
    // `ActiveSupport.presence(x)`.
    if let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*e.node {
        if method.as_str() == "presence" && args.is_empty() {
            let mut call = Expr::new(
                e.span,
                ExprNode::Send {
                    recv: Some(Expr::new(
                        e.span,
                        ExprNode::Const { path: vec![crate::ident::Symbol::from("ActiveSupport")] },
                    )),
                    method: crate::ident::Symbol::from("presence"),
                    args: vec![rewrite_helpers_in_expr(r, ctx)],
                    block: None,
                    parenthesized: true,
                },
            );
            call.ty = e.ty.clone();
            return call;
        }
    }
    let new_node = match &*e.node {
        ExprNode::Send {
            recv: None,
            method,
            args,
            block: None,
            parenthesized,
        } => {
            if let Some(kind) = classify_template_helper(method.as_str(), args, *parenthesized, ctx) {
                // Rails' `pluralize` returns a String that isn't html-safe,
                // so inside a larger expression (`pluralize(n, w) + "!"`,
                // `"#{pluralize(n, w)}"`, a `link_to` label) it is plain
                // text, escaped once where the whole is written. Only a
                // whole `<%= pluralize(…) %>` takes the arm's escape.
                if let ViewHelperKind::Pluralize { count, word } = &kind {
                    let mut call = crate::lower::view::pluralize_helper_call((*count).clone(), (*word).clone());
                    call.inherit_span(e.span);
                    return call;
                }
                if let Some(mut call) = emit_view_helper_call(&kind, ctx) {
                    call.inherit_span(e.span);
                    return call;
                }
            }
            // ERB can interpolate a route `_url` directly, not only as
            // the URL argument to link_to/button_to. Ground it at the
            // shared seam here so Ruby-family views use the request
            // origin while strict targets keep the path-only runtime
            // implementation. Leave unknown route helpers untouched.
            if let Some(stem) = method.as_str().strip_suffix("_url") {
                let path_helper = format!("{stem}_path");
                // An app helper of the `_url` name is what the bare call
                // reaches first (Rails mixes app helpers in above the url
                // helpers), so it stays a call to that method.
                if ctx.route_helper_names.contains(&path_helper) && !ctx.app_helper_names.contains(method.as_str()) {
                    let args = route_args(method.as_str(), args, ctx);
                    let mut call = super::absolute_url_interp(stem, args);
                    call.inherit_span(e.span);
                    return call;
                }
            }
            // ERB's `h` alias, in a nested/statement position: an explicit
            // escape call. (Nested `h` inside an escaped interpolation
            // double-escapes — as it does in Rails, where interpolating a
            // safe string into a plain one drops safety and the outer
            // `<%= %>` escapes again.) The top-level `<%= h(x) %>` shape
            // never reaches here — the append path unwraps it so the
            // default auto-escape applies exactly once.
            if method.as_str() == "h" && args.len() == 1 {
                let mut call = view_helpers_call(
                    "html_escape",
                    vec![coerce_to_s(rewrite_helpers_in_expr(&args[0], ctx))],
                );
                call.inherit_span(e.span);
                return call;
            }
            // `u(x)` / `url_encode(x)`: ERB::Util.url_encode, which a
            // view reaches through ERB::Util (Rails includes it in every
            // view). Percent-encoding with `%20` for a space, not
            // `CGI.escape`'s `+` that the runtime's `url_encode` is.
            if matches!(method.as_str(), "u" | "url_encode") && args.len() == 1 {
                let mut call = view_helpers_call(
                    "erb_url_encode",
                    vec![coerce_to_s(rewrite_helpers_in_expr(&args[0], ctx))],
                );
                call.inherit_span(e.span);
                return call;
            }
            let args = if ctx.route_helper_names.contains(method.as_str()) {
                route_args(method.as_str(), args, ctx)
            } else {
                args.iter().map(|a| rewrite_helpers_in_expr(a, ctx)).collect()
            };
            ExprNode::Send {
                recv: None,
                method: method.clone(),
                args,
                block: None,
                parenthesized: *parenthesized,
            }
        }
        ExprNode::Send { recv, method, args, block, parenthesized } => ExprNode::Send {
            recv: recv.as_ref().map(|r| rewrite_helpers_in_expr(r, ctx)),
            method: method.clone(),
            args: args.iter().map(|a| rewrite_helpers_in_expr(a, ctx)).collect(),
            block: block.as_ref().map(|b| rewrite_helpers_in_expr(b, ctx)),
            parenthesized: *parenthesized,
        },
        ExprNode::BoolOp { op, surface, left, right } => ExprNode::BoolOp {
            op: *op,
            surface: *surface,
            left: rewrite_helpers_in_expr(left, ctx),
            right: rewrite_helpers_in_expr(right, ctx),
        },
        // Value-position compounds that can carry helper calls in their
        // legs — `<%= cond ? ": " + h(x) : "" %>` (lobsters rss title),
        // `<%= "#{h(x)}!" %>`, `<%= [link_to(...), …].join %>`. Without
        // these arms the nested helper Send survives raw and the view
        // module has no method to answer it.
        ExprNode::If { cond, then_branch, else_branch } => ExprNode::If {
            // Value-position conds get the same predicate rewrite a
            // statement-level `<% if … %>` does — `<%= (if
            // title.present? … end) %>` (the lobsters layout <title>)
            // otherwise ships a verbatim `present?` no non-CRuby
            // runtime answers.
            cond: rewrite_predicates(
                &rewrite_helpers_in_expr(cond, ctx),
                &ctx.nullable_locals,
                &ctx.reference_reads,
                &ctx.nilable_scalar_reads,
            ),
            then_branch: rewrite_helpers_in_expr(then_branch, ctx),
            else_branch: rewrite_helpers_in_expr(else_branch, ctx),
        },
        ExprNode::StringInterp { parts } => ExprNode::StringInterp {
            parts: parts
                .iter()
                .map(|p| match p {
                    InterpPart::Text { value } => InterpPart::Text { value: value.clone() },
                    InterpPart::Expr { expr } => {
                        InterpPart::Expr { expr: rewrite_helpers_in_expr(expr, ctx) }
                    }
                })
                .collect(),
        },
        ExprNode::Array { elements, style } => ExprNode::Array {
            elements: elements.iter().map(|el| rewrite_helpers_in_expr(el, ctx)).collect(),
            style: *style,
        },
        // A Hash literal's VALUES can carry a helper call the same way an
        // Array element can — the HAML compiler's shortcut-class merge
        // emits `render_attrs({ class: haml_class("g", k), … })`, where
        // `render_attrs`'s own classify+emit (`RenderAttrs` in
        // `helpers.rs`) clones its `attrs` Hash whole rather than
        // threading each entry back through this walk, so a nested
        // helper reaches emit here or not at all. Keys are threaded too,
        // for the same reason Array elements all are, though a literal
        // Hash key never carries one in practice.
        ExprNode::Hash { entries, kwargs } => ExprNode::Hash {
            entries: entries
                .iter()
                .map(|(k, v)| (rewrite_helpers_in_expr(k, ctx), rewrite_helpers_in_expr(v, ctx)))
                .collect(),
            kwargs: *kwargs,
        },
        // Statement compounds: a form-builder map lambda hoisted into a
        // select-options loop is a `Seq` of local Assigns building the
        // option text (`html = "<strong>#{h(t.tag)}</strong>"`;
        // `html << …`) — the helper calls live under the Assign values.
        ExprNode::Seq { exprs } => ExprNode::Seq {
            exprs: exprs.iter().map(|s| rewrite_helpers_in_expr(s, ctx)).collect(),
        },
        ExprNode::Assign { target, value } => ExprNode::Assign {
            target: target.clone(),
            value: rewrite_helpers_in_expr(value, ctx),
        },
        ExprNode::OpAssign { target, op, value } => ExprNode::OpAssign {
            target: target.clone(),
            op: op.clone(),
            value: rewrite_helpers_in_expr(value, ctx),
        },
        other => other.clone(),
    };
    let mut out = Expr::new(e.span, new_node);
    // `f.object.class::X` stays a path (`expr::CONST_PATH`).
    out.decisions |= e.decisions & crate::expr::CONST_PATH;
    // A grounded `iso8601` (`to_rfc3339`) stays one: the Ruby emit prints `iso8601` by this flag.
    out.decisions |= e.decisions & crate::expr::GROUNDED_TIME_FORMAT;
    // Not a fresh, untyped node: a rebuilt `[]` the i18n lowering typed `Array[String]` would read as `Array[untyped]`
    out.ty = e.ty.clone();
    out
}

/// A generated route helper's arguments inside an output expression
/// (`<%= "" + article_path(article) %>`): an argument typed as one of the
/// app's models passes its `to_param` (`.id`, or the model's own
/// `to_param` where it overrides it), as Rails fills the segment from
/// it. The strict targets' helpers take that scalar, not the record. A
/// scalar argument, an untyped one, and every argument of a helper whose
/// body the app wrote pass as written: an app helper module's method of
/// the name (`app_helper_names`, which a bare call reaches first) or a
/// `direct` route's block (`direct_route_helpers`).
fn route_args(helper: &str, args: &[Expr], ctx: &ViewCtx) -> Vec<Expr> {
    let shadowed = ctx.app_helper_names.contains(helper) || ctx.direct_route_helpers.contains(helper);
    args.iter()
        .map(|a| {
            let arg = rewrite_helpers_in_expr(a, ctx);
            match record_segment_method(a, ctx) {
                Some(param) if !shadowed => send(Some(arg), param, Vec::new(), None, false),
                _ => arg,
            }
        })
        .collect()
}

/// The method that fills a route segment from `a` when `a` is typed as a
/// record of one of the app's models: `id`, or `to_param` for a model
/// that overrides it. `None` for any other value.
pub(super) fn record_segment_method(a: &Expr, ctx: &ViewCtx) -> Option<&'static str> {
    let Some(crate::ty::Ty::Class { id, .. }) = a.ty.as_ref().map(crate::ty::Ty::peel_nilable) else {
        return None;
    };
    // The app's key for the class (`model_param_keys`, which drops an
    // isolated engine's namespace), the one `model_singulars` and
    // `slug_models` hold.
    let key = ctx
        .model_param_keys
        .get(id.0.as_str())
        .cloned()
        .unwrap_or_else(|| crate::naming::model_param_key(id.0.as_str()));
    ctx.model_singulars
        .contains(&key)
        .then(|| if ctx.slug_models.contains(&key) { "to_param" } else { "id" })
}

/// Does a block body hold compiled-template buffer ops (`_buf = _buf + …`)?
/// Distinguishes a capture block (`<%= form_tag … do %> INNER <% end %>`)
/// from a plain value block (`<%= items.map { |x| … } %>`), so only the
/// former is rewritten into a returned capture accumulator.
/// The accumulator name for a capture nested inside `outer`.
///
/// `io` (a method body) or any non-capture accumulator yields `_cap`;
/// `_cap` yields `_cap2`, `_cap2` yields `_cap3`. Depth is read off the
/// enclosing name rather than counted in the context because the walk
/// reaches nested blocks through several recursion paths and a counter
/// threaded through all of them is a second source of truth.
fn next_capture_name(outer: &str) -> String {
    let Some(rest) = outer.strip_prefix("_cap") else {
        return "_cap".to_string();
    };
    match rest.parse::<u32>() {
        Ok(n) => format!("_cap{}", n + 1),
        // bare "_cap" is depth 1
        Err(_) if rest.is_empty() => "_cap2".to_string(),
        Err(_) => "_cap".to_string(),
    }
}

/// `e`, evaluated ahead of a `content_tag` block into
/// `__ctag_<site>_<n>` locals pushed on `stmts`. A literal stays inline,
/// and so does a Hash or Array literal, whose parts are bound in order:
/// the options keep the shape the runtime's typed parameter reads.
fn bind_before_block(e: Expr, site: u32, bound: &mut usize, stmts: &mut Vec<Expr>) -> Expr {
    match &*e.node {
        ExprNode::Lit { .. } => e,
        ExprNode::Hash { entries, kwargs } => {
            let kwargs = *kwargs;
            let entries = entries
                .iter()
                .map(|(k, v)| {
                    let k = bind_before_block(k.clone(), site, bound, stmts);
                    (k, bind_before_block(v.clone(), site, bound, stmts))
                })
                .collect();
            Expr::new(e.span, ExprNode::Hash { entries, kwargs })
        }
        ExprNode::Array { elements, style } => {
            let style = *style;
            let elements = elements.iter().map(|x| bind_before_block(x.clone(), site, bound, stmts)).collect();
            Expr::new(e.span, ExprNode::Array { elements, style })
        }
        _ => {
            *bound += 1;
            let at = Symbol::from(format!("__ctag_{site}_{bound}"));
            stmts.push(Expr::new(Span::synthetic(), ExprNode::Assign {
                target: LValue::Var { id: VarId(0), name: at.clone() },
                value: e,
            }));
            var_ref(at)
        }
    }
}

/// `_ = value`: Ruby's discard, which evaluates `value` and keeps nothing.
/// Go rejects an unused local, and an unused value that isn't a call; Go,
/// Rust and Kotlin each write the discard their own way.
fn discard(value: Expr) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Assign { target: LValue::Var { id: VarId(0), name: Symbol::from("_") }, value },
    )
}

/// The discard of a `link_to_if` / `link_to_unless` url when no link is
/// made: Rails evaluates it before it picks a branch. A literal, a local,
/// an ivar or a constant does nothing and is left out; a route helper is
/// kept, since Rails raises for a key it lacks.
fn url_discard(url: &Expr, ctx: &ViewCtx) -> Option<Expr> {
    if matches!(&*url.node, ExprNode::Lit { .. } | ExprNode::Var { .. } | ExprNode::Ivar { .. } | ExprNode::Const { .. } | ExprNode::SelfRef) {
        return None;
    }
    // Only the bare spelling reads a local (Prism's shape for a partial
    // local): `url()` or `url { … }` calls the method of that name even
    // where a local has it, and is kept with its effects.
    if let ExprNode::Send { recv: None, method, args, block: None, parenthesized: false, .. } = &*url.node
        && args.is_empty()
        && ctx.binds_local(method.as_str())
    {
        return None;
    }
    Some(discard(rewrite_helpers_in_expr(url, ctx)))
}

/// A blockless `<%= link_to_if/unless … %>` as statements: the link, or
/// the name followed by the url's discard, so the url is evaluated, after
/// the name, whether or not a link is made (#1862). Rails evaluates both
/// before it writes either; a raise between them aborts the render, so
/// what is written is the same. `None` when the url has nothing to
/// evaluate, and the value form stays.
fn link_to_if_statement(call: &Expr, url: &Expr, negate: bool, ctx: &ViewCtx) -> Option<Vec<Expr>> {
    let ExprNode::If { cond, then_branch, else_branch } = &*call.node else {
        return None;
    };
    let url = url_discard(url, ctx)?;
    let append = |e: &Expr| accumulator_append_call(e.clone(), ctx);
    let (then_branch, else_branch) = if negate {
        (seq(vec![append(then_branch), url]), append(else_branch))
    } else {
        (append(then_branch), seq(vec![append(else_branch), url]))
    };
    Some(vec![Expr::new(Span::synthetic(), ExprNode::If { cond: cond.clone(), then_branch, else_branch })])
}

/// `link_to_if`'s and `link_to_unless`'s block form: the link the
/// blockless form writes when the condition says to link, and otherwise
/// the block's body, walked against the view's accumulator with its
/// parameter bound to the name (a block without one still evaluates the
/// name, as Rails does). `None` for a block this doesn't take
/// (one that isn't template markup, or has more than one parameter).
fn emit_link_to_if_block(method: &str, args: &[Expr], block: &Expr, ctx: &ViewCtx) -> Option<Vec<Expr>> {
    let ExprNode::Lambda { params, rest_param, extra_params, body, .. } = &*block.node else {
        return None;
    };
    if !block_body_is_template(body) || params.len() > 1 || rest_param.is_some() || !extra_params.is_empty() {
        return None;
    }
    let Some(ViewHelperKind::LinkToIf { cond, text, url, opts, negate }) = classify_view_helper(method, args) else {
        return None;
    };
    // The condition and the link, as the blockless `link_to_if` lowers
    // them.
    let blockless = emit_view_helper_call(&ViewHelperKind::LinkToIf { cond, text, url, opts, negate: false }, ctx)?;
    let ExprNode::If { cond, then_branch: linked, .. } = &*blockless.node else {
        return None;
    };
    // Rails evaluates the name whether or not the block takes it, as the
    // blockless form's text does, and then the url, before the block
    // runs. A discard (`_ = …`) evaluates the name where the block may not
    // read it. Without a parameter, a literal, a local or an ivar does
    // nothing and is left out.
    let mut captured = Vec::new();
    let mut url_read = url_discard(url, ctx);
    let mut restore = None;
    let assign = |name: Symbol, value: Expr| {
        Expr::new(Span::synthetic(), ExprNode::Assign { target: LValue::Var { id: VarId(0), name }, value })
    };
    let inner_ctx = match params.first() {
        // Refused (`analyze::diagnostics`) where it shadows a local an
        // earlier nested body assigned: not lowered.
        Some(p) if refuses_param(p.as_str(), &ctx.assigned_locals, &ctx.nested_assigned_locals) => return None,
        Some(p) => {
            let name = rewrite_helpers_in_expr(text, ctx);
            let fresh = |base: &str| {
                let mut temp = base.to_string();
                while ctx.is_local(&temp)
                    || ctx.assigned_locals.contains(&temp)
                    || ctx.nested_assigned_locals.contains(&temp)
                    || [text, url, body].into_iter().any(|e| reads_name(e, &temp))
                {
                    temp.push('_');
                }
                Symbol::from(temp.as_str())
            };
            // A local the parameter shadows keeps its value after the
            // block, as a Ruby block parameter leaves it, assignments to
            // the parameter inside included: saved before the binding and
            // restored after the body (#1874).
            if ctx.is_local(p.as_str()) || ctx.assigned_locals.iter().any(|n| n == p.as_str()) {
                let saved = fresh("_link_saved");
                captured.push(assign(saved.clone(), var_ref(p.clone())));
                restore = Some(assign(p.clone(), var_ref(saved)));
            }
            // A url reading a template local the parameter shadows
            // (`<% x = "o" %>… Tracker.at(x) do |x|`) reads the outer
            // one, as in Rails: the name goes to a temp no one else reads,
            // then the url, and the parameter is bound last (#1874).
            if url_read.as_ref().is_some_and(|u| reads_name(u, p.as_str())) {
                let temp = fresh("_link_name");
                captured.push(assign(temp.clone(), name));
                captured.extend(url_read.take());
                captured.push(assign(p.clone(), var_ref(temp)));
            } else {
                captured.push(assign(p.clone(), name));
            }
            captured.push(discard(var_ref(p.clone())));
            ctx.with_locals([p.as_str().to_string()])
        }
        None => {
            if !matches!(&*text.node, ExprNode::Lit { .. } | ExprNode::Var { .. } | ExprNode::Ivar { .. }) {
                captured.push(discard(rewrite_helpers_in_expr(text, ctx)));
            }
            ctx.with_locals(std::iter::empty())
        }
    };
    captured.extend(url_read);
    captured.extend(walk_body(body, &inner_ctx));
    captured.extend(restore);
    let shown = if captured.len() == 1 { captured.pop().unwrap() } else { seq(captured) };
    let link = accumulator_append_call(linked.clone(), ctx);
    let (then_branch, else_branch) = if negate { (shown, link) } else { (link, shown) };
    Some(vec![Expr::new(Span::synthetic(), ExprNode::If { cond: cond.clone(), then_branch, else_branch })])
}

/// Whether `e` reads or binds the local `name`: a `Var`, an assignment
/// to it, a bare argument-less `Send` (Prism's shape for a local it hasn't
/// seen assigned), or a block parameter.
fn reads_name(e: &Expr, name: &str) -> bool {
    let here = match &*e.node {
        ExprNode::Var { name: n, .. } | ExprNode::Assign { target: LValue::Var { name: n, .. }, .. } => n.as_str() == name,
        ExprNode::Send { recv: None, method, args, block: None, .. } => args.is_empty() && method.as_str() == name,
        ExprNode::Lambda { params, .. } => params.iter().any(|p| p.as_str() == name),
        _ => false,
    };
    let mut found = here;
    e.node.for_each_child(&mut |c| found = found || reads_name(c, name));
    found
}

/// Whether a helper's block is template markup: it writes the ERB buffer
/// (`_buf = …`) somewhere, at its top level or inside the control flow it
/// starts with (`<% if … %>…<% end %>`, an `each`, a `case`), which
/// `walk_body` walks as it walks the rest.
fn block_body_is_template(body: &Expr) -> bool {
    if matches!(&*body.node,
        ExprNode::Assign { target: LValue::Var { name, .. }, .. } if name.as_str() == "_buf")
    {
        return true;
    }
    let mut found = false;
    body.node.for_each_child(&mut |c| found = found || block_body_is_template(c));
    found
}

fn unwrap_to_s(expr: &Expr) -> &Expr {
    if let ExprNode::Send { recv: Some(inner), method, args, .. } = &*expr.node {
        if method.as_str() == "to_s" && args.is_empty() {
            return inner;
        }
    }
    expr
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::BlockStyle;
    use crate::ident::VarId;
    use crate::span::Span;

    fn str_lit(s: &str) -> Expr {
        Expr::new(Span::default(), ExprNode::Lit { value: Literal::Str { value: s.into() } })
    }
    fn var(name: &str) -> Expr {
        Expr::new(Span::default(), ExprNode::Var { id: VarId(0), name: Symbol::from(name) })
    }
    /// `recv.to_s` — a no-arg to_s send.
    fn send_to_s(recv: Expr) -> Expr {
        Expr::new(
            Span::default(),
            ExprNode::Send {
                recv: Some(recv),
                method: Symbol::from("to_s"),
                args: Vec::new(),
                block: None,
                parenthesized: false,
            },
        )
    }
    /// `_buf = _buf + arg` — the compiled-ERB append shape.
    fn buf_append(arg: Expr) -> Expr {
        let plus = Expr::new(
            Span::default(),
            ExprNode::Send {
                recv: Some(var("_buf")),
                method: Symbol::from("+"),
                args: vec![arg],
                block: None,
                parenthesized: false,
            },
        );
        Expr::new(
            Span::default(),
            ExprNode::Assign {
                target: LValue::Var { id: VarId(0), name: Symbol::from("_buf") },
                value: plus,
            },
        )
    }
    fn test_ctx() -> ViewCtx {
        ViewCtx {
            locals: Vec::new(),
            ivar_params: Vec::new(),
            assigned_locals: Vec::new(),
            nested_assigned_locals: Vec::new(),
            arg_name: String::new(),
            resource_dir: String::new(),
            accumulator: "io".to_string(),
            form_records: Vec::new(),
            nullable_locals: Default::default(),
            reference_reads: Default::default(),
            reference_targets: Default::default(),
            nested_record_classes: Default::default(),
            nilable_scalar_reads: Default::default(),
            html_safe_methods: Default::default(),
            model_singulars: Default::default(),
            slug_models: Default::default(),
            bool_readers: Default::default(),
            time_columns: Default::default(),
            store_readers: Default::default(),
            route_helper_names: Default::default(),
            app_helper_names: Default::default(),
            view_context_helpers: Default::default(),
            direct_route_helpers: Default::default(),
            i18n: Default::default(),
            route_helper_arity: Default::default(),
            form_wrappers: Default::default(),
            form_with_entry: false,
            model_param_keys: Default::default(),
            stylesheets: Vec::new(),
            lexxy: false,
            lexxy_editor_adapter: false,
            partial_ivars: Default::default(),
            ivar_records: Default::default(),
            formats: vec!["html".to_string()],
            view_formats: Default::default(),
            multipart_partials: Default::default(),
            dyn_pools: Default::default(),
            partial_stems: Default::default(),
            partial_extras: Default::default(),
            strict_locals: Default::default(),
            yielding_partials: Default::default(),
            record_carries_yield: false,
            view_name: "messages/_message".to_string(),
            ivar_models: Default::default(),
            nilable_ivar_models: Default::default(),
            record_classes: Default::default(),
            str_ivars: Default::default(),
            ivar_locals: Default::default(),
            own_flash: Default::default(),
            held_ivars: Vec::new(),
        }
    }

    /// `test_ctx` plus what makes a `<% cache %>` key groundable: the
    /// partial's record local, and the app's model singulars.
    fn cache_ctx() -> ViewCtx {
        ViewCtx {
            locals: vec!["message".to_string(), "notice".to_string()],
            model_singulars: std::rc::Rc::new(
                ["message", "room"].iter().map(|s| s.to_string()).collect(),
            ),
            ..test_ctx()
        }
    }

    /// `<% cache <args> do %> inner <% end %>` — campfire's shape, in
    /// statement position (the `<% %>` form, not `<%= %>`). The body is
    /// a compiled-ERB `_buf` append, which is what the walker is handed
    /// in a real template.
    fn cache_call(args: Vec<Expr>) -> Expr {
        let inner = buf_append(Expr::new(
            Span::default(),
            ExprNode::Lit { value: Literal::Str { value: "inner".to_string() } },
        ));
        Expr::new(
            Span::default(),
            ExprNode::Send {
                recv: None,
                method: Symbol::from("cache"),
                args,
                block: Some(Expr::new(
                    Span::default(),
                    ExprNode::Lambda { extra_params: Vec::new(),
                        rest_param: None,
                        params: Vec::new(),
                        block_param: None,
                        body: inner,
                        block_style: crate::expr::BlockStyle::Do,
                    },
                )),
                parenthesized: false,
            },
        )
    }

    fn cache_emit(args: Vec<Expr>) -> String {
        walk_body(&cache_call(args), &cache_ctx())
            .iter()
            .map(crate::emit::ruby::emit_expr)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn cache_block_reads_renders_and_writes() {
        // campfire's `<% cache [ message, "presentation-v2" ] do %>`.
        let key = Expr::new(
            Span::default(),
            ExprNode::Array {
                elements: vec![var("message"), str_lit("presentation-v2")],
                style: Default::default(),
            },
        );
        let emitted = cache_emit(vec![key]);

        assert!(emitted.contains("ActionView::ViewHelpers.fragment_read("), "the read leads:\n{emitted}");
        assert!(emitted.contains("ActionView::ViewHelpers.fragment_write("), "the miss arm writes:\n{emitted}");
        assert!(
            emitted.contains("message.cache_key_with_version"),
            "the record contributes its versioned key:\n{emitted}"
        );
        assert!(
            emitted.contains("views/") && emitted.contains("messages/_message/"),
            "the site is in the key, so two templates caching one record cannot collide:\n{emitted}"
        );
        assert!(
            emitted.contains("presentation-v2"),
            "a literal folds into the key at COMPILE time — this is the app's own \
             version bump and it still works:\n{emitted}"
        );
        assert!(
            emitted.contains("cache_scope"),
            "the broadcast namespace keeps a request render and a broadcast render \
             (which omits the CSRF input) from being served for each other:\n{emitted}"
        );
        assert!(emitted.contains("inner"), "the body still renders on a miss:\n{emitted}");
        assert!(!emitted.contains("_buf"), "raw _buf must not survive:\n{emitted}");
    }

    #[test]
    fn a_record_key_reaches_the_controller_as_the_record() {
        // campfire's `<% cache message do %>`: Rails hands the controller
        // `[template, record]`, so the app's `combined_fragment_cache_key`
        // sees the Message, and a `cached: true` collection of the same
        // partial files the fragment under the same key.
        let emitted = cache_emit(vec![var("message")]);

        assert!(
            emitted.contains("ActionView::ViewHelpers.fragment_read_record(\"#{ActionView::ViewHelpers.cache_scope}messages/_message\", message)"),
            "the read hands over the template and the record:\n{emitted}"
        );
        assert!(
            emitted.contains("ActionView::ViewHelpers.fragment_write_record(\"#{ActionView::ViewHelpers.cache_scope}messages/_message\", message, "),
            "the miss arm writes under the same key:\n{emitted}"
        );
        assert!(
            !emitted.contains("cache_key_with_version"),
            "the record is not folded to text before the controller sees it:\n{emitted}"
        );
    }

    #[test]
    fn the_body_renders_into_its_own_accumulator_not_the_page() {
        let key = Expr::new(
            Span::default(),
            ExprNode::Array { elements: vec![var("message")], style: Default::default() },
        );
        let emitted = cache_emit(vec![key]);
        assert!(
            emitted.contains("__cache_io_"),
            "the miss arm needs its own builder — what it accumulates is what gets \
             STORED, and appending straight to `io` would store nothing:\n{emitted}"
        );
        // Two appends, not one reassigned local: `read_str` answers
        // `String?` and the render answers `String`, and a single local
        // holding both is a type that changes mid-method.
        assert_eq!(
            emitted.matches("io << ").count(),
            2,
            "one append per arm:\n{emitted}"
        );
    }

    #[test]
    fn an_ungroundable_key_renders_transparently() {
        // `cache [ma, ma.item, @user]` (lobsters' mod activity table) —
        // `ma` is not a model singular and not a local here. Declining
        // is the SAFE answer: a key that misses costs a render, a key
        // that collides serves the wrong bytes and no gate would see it.
        let key = Expr::new(
            Span::default(),
            ExprNode::Array { elements: vec![var("ma")], style: Default::default() },
        );
        let emitted = cache_emit(vec![key]);
        assert!(!emitted.contains("fragment_read"), "no cache:\n{emitted}");
        assert!(
            emitted.contains("inner"),
            "but the body still renders — transparent, never DROPPED:\n{emitted}"
        );
    }

    #[test]
    fn a_helper_that_shares_a_model_name_is_not_a_record() {
        // `model_singulars` alone would claim this; the conjunction with
        // `locals` is what keeps a helper call out of a cache key.
        let key = Expr::new(
            Span::default(),
            ExprNode::Array { elements: vec![var("room")], style: Default::default() },
        );
        let emitted = cache_emit(vec![key]);
        assert!(
            !emitted.contains("fragment_read"),
            "`room` is a model singular but not one of THIS view's locals:\n{emitted}"
        );
    }

    fn nil_lit() -> Expr {
        Expr::new(Span::default(), ExprNode::Lit { value: Literal::Nil })
    }

    /// `render "accounts/users/next_page_container", page: p`
    fn render_partial_call() -> Expr {
        let key = Expr::new(
            Span::default(),
            ExprNode::Lit { value: Literal::Sym { value: Symbol::from("page") } },
        );
        let kwargs = Expr::new(
            Span::default(),
            ExprNode::Hash { entries: vec![(key, var("p"))], kwargs: true },
        );
        Expr::new(
            Span::default(),
            ExprNode::Send {
                recv: None,
                method: Symbol::from("render"),
                args: vec![str_lit("accounts/users/next_page_container"), kwargs],
                block: None,
                parenthesized: false,
            },
        )
    }

    fn modifier(then_branch: Expr, else_branch: Expr) -> Expr {
        send_to_s(Expr::new(
            Span::default(),
            ExprNode::If { cond: var("done"), then_branch, else_branch },
        ))
    }

    fn renders_a_view_module(e: &Expr) -> bool {
        let mut found = false;
        fn walk(e: &Expr, found: &mut bool) {
            if let ExprNode::Send { recv: Some(r), .. } = &*e.node {
                if let ExprNode::Const { path } = &*r.node {
                    if path.first().is_some_and(|s| s.as_str() == "Views") {
                        *found = true;
                    }
                }
            }
            e.node.for_each_child(&mut |c| walk(c, found));
        }
        walk(e, &mut found);
        found
    }

    /// `<%= render "…" if cond %>` — the shape that already worked.
    #[test]
    fn a_modifier_if_render_reaches_the_partial_classifier() {
        let out = emit_io_append(&modifier(render_partial_call(), nil_lit()), &test_ctx());
        assert_eq!(out.len(), 1);
        assert!(
            matches!(&*out[0].node, ExprNode::If { .. }),
            "a one-branch modifier emits a GUARDED append, not html_escape(<If>)"
        );
        assert!(renders_a_view_module(&out[0]), "the render must reach a Views:: module");
    }

    /// `<%= render "…" unless cond %>` — the MIRROR, and the shape that
    /// stopped campfire's spinel build. `unless` normalizes to `if cond
    /// then nil else <expr>`: the same node with the live branch on the
    /// other side, so a rule that reads only "no else" walks past it and
    /// leaves a bare `render` no view module defines.
    #[test]
    fn a_modifier_unless_render_reaches_the_partial_classifier_too() {
        let out = emit_io_append(&modifier(nil_lit(), render_partial_call()), &test_ctx());
        assert_eq!(out.len(), 1);
        assert!(
            matches!(&*out[0].node, ExprNode::If { .. }),
            "the `unless` mirror must be guarded exactly as the `if` form is"
        );
        assert!(renders_a_view_module(&out[0]), "the render must reach a Views:: module");
    }

    /// A real two-branch conditional is NOT a modifier and must keep
    /// falling through to the default escape path.
    #[test]
    fn a_two_branch_conditional_is_not_treated_as_a_modifier() {
        let out = emit_io_append(&modifier(str_lit("a"), str_lit("b")), &test_ctx());
        assert_eq!(out.len(), 1);
        assert!(
            !matches!(&*out[0].node, ExprNode::If { .. }),
            "both branches live — this is a ternary, not a guard"
        );
    }

    fn block_helper_call(method: &str) -> Expr {
        block_helper_call_with(method, vec![var("x")])
    }

    /// `<%= <helper> account_join_code_path, class: "btn" do %> inner
    /// <% end %>` — campfire's shape. A path-helper call in the URL
    /// position, because a bare local (what `block_helper_call` passes)
    /// is not a URL the inliner can resolve and would decline.
    fn inline_helper_block_call(helper: &str) -> Expr {
        let path = Expr::new(
            Span::default(),
            ExprNode::Send {
                recv: None,
                method: Symbol::from("account_join_code_path"),
                args: Vec::new(),
                block: None,
                parenthesized: false,
            },
        );
        let opts = Expr::new(
            Span::default(),
            ExprNode::Hash {
                entries: vec![(lit_sym(Symbol::from("class")), str_lit("btn"))],
                kwargs: true,
            },
        );
        block_helper_call_with(helper, vec![path, opts])
    }

    /// The emitted Ruby for one of those, as a single string.
    fn inline_helper_block_emit(helper: &str) -> String {
        walk_body(&buf_append(inline_helper_block_call(helper)), &test_ctx())
            .iter()
            .map(crate::emit::ruby::emit_expr)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Compiled `<%= <method>(<args>) do %> inner <% end %>` is
    ///   _buf = _buf + (<method>(<args>) do _buf = _buf + "inner" end).to_s
    fn block_helper_call_with(method: &str, args: Vec<Expr>) -> Expr {
        let inner = Expr::new(
            Span::default(),
            ExprNode::Lambda { extra_params: Vec::new(), rest_param: None,
                params: Vec::new(),
                block_param: None,
                body: buf_append(str_lit("inner")),
                block_style: BlockStyle::Do,
            },
        );
        let call = Expr::new(
            Span::default(),
            ExprNode::Send {
                recv: None,
                method: Symbol::from(method),
                args,
                block: Some(inner),
                parenthesized: false,
            },
        );
        Expr::new(
            Span::default(),
            ExprNode::Send {
                recv: Some(call),
                method: Symbol::from("to_s"),
                args: Vec::new(),
                block: None,
                parenthesized: false,
            },
        )
    }

    /// `recv.<method>` — a no-arg send with an explicit receiver.
    fn send0(recv: Expr, method: &str) -> Expr {
        Expr::new(
            Span::default(),
            ExprNode::Send {
                recv: Some(recv),
                method: Symbol::from(method),
                args: Vec::new(),
                block: None,
                parenthesized: false,
            },
        )
    }

    #[test]
    fn interpolating_a_marked_value_skips_the_escape() {
        // `<%= x.html_safe %>` says "do not escape this" at the call
        // site. Wrapping it ships `&lt;b&gt;` to the page; and the mark
        // itself has to go, since no target's String answers it.
        let stmts = walk_body(&buf_append(send_to_s(send0(var("x"), "html_safe"))), &test_ctx());
        let emitted = stmts.iter().map(crate::emit::ruby::emit_expr).collect::<Vec<_>>().join("\n");
        assert!(!emitted.contains("html_escape"), "marked value must not be escaped:\n{emitted}");
        assert!(!emitted.contains("html_safe"), "the mark itself must not survive:\n{emitted}");
        assert!(emitted.contains("io << x.to_s"), "value still appended:\n{emitted}");
    }

    #[test]
    fn interpolating_a_recorded_producer_skips_the_escape() {
        // One level down: `Hat#to_html_label` ends in `.html_safe`, so
        // `lower::html_safe` recorded it and this side must not escape
        // the element it returns on a `Hat` receiver, nilable or not.
        let ctx = producer_ctx();
        for ty in [class_ty("Hat"), crate::ty::Ty::Union { variants: vec![class_ty("Hat"), crate::ty::Ty::Nil].into() }] {
            let stmts = walk_body(&buf_append(send_to_s(send0(typed_var("hat", ty), "to_html_label"))), &ctx);
            let emitted = stmts.iter().map(crate::emit::ruby::emit_expr).collect::<Vec<_>>().join("\n");
            assert!(!emitted.contains("html_escape"), "recorded producer must not be escaped:\n{emitted}");
            assert!(emitted.contains("hat.to_html_label"), "call preserved:\n{emitted}");
        }
    }

    #[test]
    fn another_classs_method_of_a_marked_name_is_escaped() {
        // `Badge#to_html_label` isn't marked: only `Hat`'s is. Neither is
        // a `Hat | Badge` receiver, nor one whose class isn't known.
        let ctx = producer_ctx();
        let either = crate::ty::Ty::Union { variants: vec![class_ty("Hat"), class_ty("Badge")].into() };
        for recv in [typed_var("badge", class_ty("Badge")), typed_var("item", either), var("hat")] {
            let stmts = walk_body(&buf_append(send_to_s(send0(recv, "to_html_label"))), &ctx);
            let emitted = stmts.iter().map(crate::emit::ruby::emit_expr).collect::<Vec<_>>().join("\n");
            assert!(emitted.contains("html_escape"), "unmarked definition must be escaped:\n{emitted}");
        }
    }

    fn producer_ctx() -> ViewCtx {
        let mut ctx = test_ctx();
        let class = |defines: &[&str]| super::super::MarkClass {
            defines: defines.iter().map(|m| (m.to_string(), false)).collect(),
            ..Default::default()
        };
        ctx.html_safe_methods = std::rc::Rc::new(super::super::HtmlSafeMarks {
            classes: [("Hat".to_string(), class(&["to_html_label"])), ("Badge".to_string(), class(&["to_html_label"]))]
                .into_iter()
                .collect(),
            marked: [("Hat".to_string(), "to_html_label".to_string(), false)].into_iter().collect(),
        });
        ctx
    }
    fn class_ty(name: &str) -> crate::ty::Ty {
        crate::ty::Ty::Class { id: crate::ident::ClassId(Symbol::from(name)), args: Vec::new().into() }
    }
    fn typed_var(name: &str, ty: crate::ty::Ty) -> Expr {
        let mut e = var(name);
        e.ty = Some(ty);
        e
    }

    #[test]
    fn an_ordinary_read_is_still_escaped() {
        // The default has to survive the exception: a plain attribute
        // read is user text and Rails escapes it.
        let stmts = walk_body(&buf_append(send_to_s(send0(var("user"), "about"))), &test_ctx());
        let emitted = stmts.iter().map(crate::emit::ruby::emit_expr).collect::<Vec<_>>().join("\n");
        assert!(emitted.contains("html_escape"), "ordinary read stays escaped:\n{emitted}");
    }

    #[test]
    fn form_block_body_lowers_to_capture_accumulator() {
        // A generic block helper's inner `_buf` ops must be walked into
        // a returned capture accumulator, not left raw (the bug found
        // against lobsters). `form_tag` used to be the example here but
        // now inline-expands (test below); any unclassified block
        // helper still rides the capture machinery.
        let stmts = walk_body(&buf_append(block_helper_call("custom_wrapper")), &test_ctx());
        let emitted = stmts
            .iter()
            .map(crate::emit::ruby::emit_expr)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(emitted.contains("_cap"), "expected capture accumulator:\n{emitted}");
        assert!(emitted.contains("custom_wrapper"), "helper call preserved:\n{emitted}");
        assert!(!emitted.contains("_buf"), "raw _buf must not survive:\n{emitted}");
    }

    #[test]
    fn form_tag_block_inline_expands_to_form_statements() {
        // `<%= form_tag(x) do %> inner <% end %>` no longer survives as
        // a runtime call: the open tag (action through html_escape),
        // the CSRF hidden input, the walked body, and the close tag
        // splice directly into the outer accumulator.
        let stmts = walk_body(&buf_append(block_helper_call("form_tag")), &test_ctx());
        let emitted = stmts
            .iter()
            .map(crate::emit::ruby::emit_expr)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(emitted.contains("<form action="), "expected inline open tag:\n{emitted}");
        assert!(
            emitted.contains("csrf_token_hidden_input"),
            "CSRF input must follow the open tag:\n{emitted}"
        );
        assert!(emitted.contains("inner"), "block body walked inline:\n{emitted}");
        assert!(emitted.contains("</form>"), "close tag emitted:\n{emitted}");
        assert!(!emitted.contains("form_tag"), "no runtime form_tag call:\n{emitted}");
        assert!(!emitted.contains("_buf"), "raw _buf must not survive:\n{emitted}");
    }

    #[test]
    fn button_to_block_form_inlines_like_its_positional_twin() {
        // `<%= button_to url do %> markup <% end %>` — campfire's
        // join-code regenerate button. Without its own arm this fell to
        // the generic block helper below, which rebuilds the call
        // verbatim and leaves a bare `button_to` that nothing defines:
        // the helper is inlined at lower time, so there is no runtime
        // method to land on.
        let emitted = inline_helper_block_emit("button_to");

        assert!(emitted.contains("<form action="), "the wrapping form opens inline:\n{emitted}");
        assert!(
            emitted.contains("<button type=\\\"submit\\\""),
            "the button opens before the block body:\n{emitted}"
        );
        assert!(emitted.contains("inner"), "block body walked inline:\n{emitted}");
        assert!(
            emitted.contains("</button>"),
            "and the button closes after it:\n{emitted}"
        );
        assert!(
            emitted.contains("csrf_token_hidden_input"),
            "CSRF input rides the same wrapper the positional form uses:\n{emitted}"
        );
        assert!(
            !emitted.contains("button_to("),
            "no runtime button_to call survives:\n{emitted}"
        );
        assert!(!emitted.contains("_cap"), "no capture accumulator needed:\n{emitted}");
    }

    #[test]
    fn link_to_block_form_inlines_to_an_anchor() {
        // The same defect one helper over, and the one campfire writes
        // most — five templates, `messages/_actions` among them, which
        // renders per message on the room page.
        let emitted = inline_helper_block_emit("link_to");

        // Rails' attribute order: the options, then `href` last.
        assert!(emitted.contains("<a class="), "the anchor opens inline:\n{emitted}");
        assert!(emitted.contains(" href="), "with the href after the options:\n{emitted}");
        assert!(emitted.contains("inner"), "block body walked inline:\n{emitted}");
        assert!(emitted.contains("</a>"), "and closes after it:\n{emitted}");
        assert!(
            !emitted.contains("link_to("),
            "no runtime link_to call survives:\n{emitted}"
        );
        assert!(!emitted.contains("_cap"), "no capture accumulator needed:\n{emitted}");
    }

    #[test]
    fn an_inline_helper_block_body_is_not_escaped() {
        // The positional form escapes its label — it is a String. A
        // BLOCK yields markup (`image_tag`, a literal `<span>`), and
        // Rails renders it as the element's HTML. Escaping it would show
        // the tags as visible text, so the body must append raw.
        for helper in ["button_to", "link_to"] {
            let emitted = inline_helper_block_emit(helper);
            let body_line = emitted
                .lines()
                .find(|l| l.contains("inner"))
                .unwrap_or_else(|| panic!("{helper}: no block body line in\n{emitted}"));
            assert!(
                !body_line.contains("html_escape"),
                "{helper}: block body appends raw:\n{body_line}"
            );
        }
    }

    #[test]
    fn a_form_wrapper_helper_is_spliced_to_the_call_it_wraps() {
        // `composer_form_tag(room) do |form| … end` where the helper is
        // `def composer_form_tag(room, &) = form_with(url: …, &)`. The
        // splice has to happen before the walk reaches the block, or the
        // form-builder macro-inline never sees the `form_with` and the
        // `form.x` calls together — which is the NoMethodError campfire's
        // room page died on.
        let wrapper = super::super::FormWrapperHelper {
            params: vec![(Symbol::from("room"), None)],
            keyword_rest: false,
            call: Expr::new(
                Span::default(),
                ExprNode::Send {
                    recv: None,
                    method: Symbol::from("form_with"),
                    args: vec![Expr::new(
                        Span::default(),
                        ExprNode::Hash {
                            entries: vec![(lit_sym(Symbol::from("url")), var("room"))],
                            kwargs: true,
                        },
                    )],
                    block: None,
                    parenthesized: true,
                },
            ),
        };
        let mut ctx = test_ctx();
        ctx.form_wrappers = std::rc::Rc::new(
            [("composer_form_tag".to_string(), wrapper)].into_iter().collect(),
        );
        // The call site passes a template local, which inside an
        // ERB-ingested body parses as a zero-arg Send rather than a Var.
        let local_read = Expr::new(
            Span::default(),
            ExprNode::Send {
                recv: None,
                method: Symbol::from("the_room"),
                args: Vec::new(),
                block: None,
                parenthesized: false,
            },
        );
        let call = block_helper_call_with("composer_form_tag", vec![local_read]);
        let emitted = walk_body(&buf_append(call), &ctx)
            .iter()
            .map(crate::emit::ruby::emit_expr)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            !emitted.contains("composer_form_tag"),
            "the wrapper call is replaced, not kept:\n{emitted}"
        );
        assert!(
            emitted.contains("<form action="),
            "and the form_with it wraps expands inline:\n{emitted}"
        );
        assert!(
            emitted.contains("the_room"),
            "with the call site's argument substituted for the parameter:\n{emitted}"
        );
        assert!(emitted.contains("inner"), "block body walked inline:\n{emitted}");
    }

    #[test]
    fn a_form_wrapper_declines_an_argument_it_cannot_move() {
        // Substituting a parameter means moving its argument into the
        // wrapped call. A read is free to move; a call is not — it could
        // run a different number of times, or in a different order. Such
        // a site keeps its shape and stays a loud failure.
        let wrapper = super::super::FormWrapperHelper {
            params: vec![(Symbol::from("room"), None)],
            keyword_rest: false,
            call: Expr::new(
                Span::default(),
                ExprNode::Send {
                    recv: None,
                    method: Symbol::from("form_with"),
                    args: Vec::new(),
                    block: None,
                    parenthesized: true,
                },
            ),
        };
        let mut ctx = test_ctx();
        ctx.form_wrappers = std::rc::Rc::new(
            [("composer_form_tag".to_string(), wrapper)].into_iter().collect(),
        );
        // `find_room(1)` — an argument-taking call, not a bare read.
        let impure = Expr::new(
            Span::default(),
            ExprNode::Send {
                recv: None,
                method: Symbol::from("find_room"),
                args: vec![str_lit("1")],
                block: None,
                parenthesized: true,
            },
        );
        let call = block_helper_call_with("composer_form_tag", vec![impure]);
        let emitted = walk_body(&buf_append(call), &ctx)
            .iter()
            .map(crate::emit::ruby::emit_expr)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            emitted.contains("composer_form_tag"),
            "the site keeps its source shape:\n{emitted}"
        );
    }

    #[test]
    fn auto_escape_recoerces_with_to_s() {
        // Compiled `<%= comment.score %>` is `_buf = _buf + (comment.score).to_s`.
        // `unwrap_to_s` strips the `.to_s` so the classifiers see the bare
        // `comment.score`; the auto-escape default must re-add it before the
        // `html_escape` wrap, or the monomorphic `(String) -> String` helper
        // crashes on an Integer score.
        let score = Expr::new(
            Span::default(),
            ExprNode::Send {
                recv: Some(var("comment")),
                method: Symbol::from("score"),
                args: Vec::new(),
                block: None,
                parenthesized: false,
            },
        );
        let to_s = Expr::new(
            Span::default(),
            ExprNode::Send {
                recv: Some(score),
                method: Symbol::from("to_s"),
                args: Vec::new(),
                block: None,
                parenthesized: false,
            },
        );
        let stmts = walk_body(&buf_append(to_s), &test_ctx());
        let emitted = stmts
            .iter()
            .map(crate::emit::ruby::emit_expr)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(emitted.contains("html_escape"), "auto-escape wrap present:\n{emitted}");
        assert!(
            emitted.contains("comment.score.to_s") || emitted.contains("(comment.score).to_s"),
            "score must be coerced with .to_s before html_escape:\n{emitted}"
        );
    }

    #[test]
    fn auto_escape_explicit_to_s_is_not_doubled() {
        // `<%= x.to_s %>` compiles to `_buf = _buf + (x.to_s).to_s`;
        // `unwrap_to_s` strips one, and the auto-escape coercion must not
        // re-add a second — `html_escape(x.to_s)`, not `x.to_s.to_s`.
        let inner = send_to_s(var("x"));
        let stmts = walk_body(&buf_append(send_to_s(inner)), &test_ctx());
        let emitted = stmts
            .iter()
            .map(crate::emit::ruby::emit_expr)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(emitted.contains("html_escape(x.to_s)"), "expected single .to_s:\n{emitted}");
        assert!(!emitted.contains("to_s.to_s"), "must not double .to_s:\n{emitted}");
    }

    #[test]
    fn auto_escape_string_literal_stays_foldable() {
        // A bare `<%= "hi" %>` must NOT pick up `.to_s` — `view_helpers_call`
        // constant-folds `html_escape("literal")`, and coercing a String
        // literal is a no-op that only defeats the fold.
        let lit = Expr::new(
            Span::default(),
            ExprNode::Send {
                recv: Some(str_lit("hi")),
                method: Symbol::from("to_s"),
                args: Vec::new(),
                block: None,
                parenthesized: false,
            },
        );
        let stmts = walk_body(&buf_append(lit), &test_ctx());
        let emitted = stmts
            .iter()
            .map(crate::emit::ruby::emit_expr)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(!emitted.contains("to_s"), "string literal must not be coerced:\n{emitted}");
    }

    const ZERO_ARG_HELPERS: [&str; 4] =
        ["csrf_meta_tags", "csrf_meta_tag", "csp_meta_tag", "javascript_importmap_tags"];

    fn bare(method: &str) -> Expr {
        bare_call(method, false)
    }

    fn bare_call(method: &str, parenthesized: bool) -> Expr {
        Expr::new(
            Span::default(),
            ExprNode::Send {
                recv: None,
                method: Symbol::from(method),
                args: Vec::new(),
                block: None,
                parenthesized,
            },
        )
    }

    /// `<name> || "dflt"`.
    fn or_default(left: Expr) -> Expr {
        Expr::new(
            Span::default(),
            ExprNode::BoolOp {
                op: BoolOpKind::Or,
                surface: BoolOpSurface::Symbol,
                left,
                right: str_lit("dflt"),
            },
        )
    }

    fn emit_output(arg: Expr, ctx: &ViewCtx) -> String {
        walk_body(&buf_append(send_to_s(arg)), ctx)
            .iter()
            .map(crate::emit::ruby::emit_expr)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn calls_a_helper(emitted: &str) -> bool {
        ["csrf_meta_tags", "csp_meta_tag", "javascript_importmap_tags"]
            .iter()
            .any(|h| emitted.contains(&format!("ViewHelpers.{h}")))
    }

    #[test]
    fn a_local_named_like_a_zero_arg_helper_is_read_not_called() {
        for name in ZERO_ARG_HELPERS {
            let ctx = ViewCtx { locals: vec![name.to_string()], ..test_ctx() };
            for arg in [bare(name), or_default(bare(name))] {
                let emitted = emit_output(arg, &ctx);
                assert!(!calls_a_helper(&emitted), "`{name}` is the partial's local:\n{emitted}");
                assert!(emitted.contains(&format!("{name}.to_s")) || emitted.contains(&format!("{name} ||")), "the local is still read:\n{emitted}");
            }
        }
    }

    #[test]
    fn a_zero_arg_helper_without_a_local_is_still_the_helper() {
        for name in ZERO_ARG_HELPERS {
            for arg in [bare(name), or_default(bare(name))] {
                let emitted = emit_output(arg, &test_ctx());
                assert!(calls_a_helper(&emitted), "`{name}` is the helper:\n{emitted}");
            }
        }
    }

    #[test]
    fn empty_parens_call_the_helper_past_a_local() {
        for name in ZERO_ARG_HELPERS {
            let ctx = ViewCtx { locals: vec![name.to_string()], ..test_ctx() };
            for arg in [bare_call(name, true), or_default(bare_call(name, true))] {
                let emitted = emit_output(arg, &ctx);
                assert!(calls_a_helper(&emitted), "`{name}()` is the helper:\n{emitted}");
            }
        }
    }

    #[test]
    fn a_closure_ivar_named_like_a_helper_does_not_shadow_it() {
        for name in ZERO_ARG_HELPERS {
            let ctx = ViewCtx {
                locals: vec![name.to_string()],
                ivar_params: vec![name.to_string()],
                ..test_ctx()
            };
            for arg in [bare(name), or_default(bare(name))] {
                let emitted = emit_output(arg, &ctx);
                assert!(calls_a_helper(&emitted), "`@{name}` leaves `{name}` the helper:\n{emitted}");
            }
            let emitted = emit_output(bare(name), &ctx.with_locals([name.to_string()]));
            assert!(!calls_a_helper(&emitted), "a block param `{name}` is a local again:\n{emitted}");
        }
    }

    /// `<x>_path(rec)` nested in an output expression, `rec` typed as an
    /// isolated engine's model.
    fn nested_route_call(direct: bool) -> Expr {
        let ctx = ViewCtx {
            locals: vec!["rec".to_string()],
            route_helper_names: std::rc::Rc::new(["article_path".to_string()].into_iter().collect()),
            direct_route_helpers: std::rc::Rc::new(
                if direct { vec!["article_path".to_string()] } else { Vec::new() }.into_iter().collect(),
            ),
            model_singulars: std::rc::Rc::new(["article".to_string()].into_iter().collect()),
            model_param_keys: std::rc::Rc::new(
                [("Blorgh::Article".to_string(), "article".to_string())].into_iter().collect(),
            ),
            ..test_ctx()
        };
        let mut rec = var("rec");
        rec.ty = Some(crate::ty::Ty::Class {
            id: crate::ident::ClassId(Symbol::from("Blorgh::Article")),
            args: Default::default(),
        });
        let call = Expr::new(
            Span::default(),
            ExprNode::Send {
                recv: None,
                method: Symbol::from("article_path"),
                args: vec![rec],
                block: None,
                parenthesized: true,
            },
        );
        rewrite_helpers_in_expr(&call, &ctx)
    }

    fn first_arg(call: &Expr) -> &Expr {
        let ExprNode::Send { args, .. } = &*call.node else { panic!("not a call: {call:?}") };
        &args[0]
    }

    #[test]
    fn an_isolated_engine_record_projects_under_the_apps_key() {
        // `Blorgh::Article`'s key is `article` (`model_param_keys`), not
        // `blorgh_article` (`naming::model_param_key`).
        let out = nested_route_call(false);
        assert!(
            matches!(&*first_arg(&out).node, ExprNode::Send { recv: Some(r), method, .. }
                if method.as_str() == "id" && matches!(&*r.node, ExprNode::Var { name, .. } if name.as_str() == "rec")),
            "{out:?}"
        );
    }

    #[test]
    fn a_direct_route_keeps_its_record() {
        let out = nested_route_call(true);
        assert!(matches!(&*first_arg(&out).node, ExprNode::Var { name, .. } if name.as_str() == "rec"), "{out:?}");
    }
}


// ── `<% cache %>` fragment caching ───────────────────────────────

/// The key a `<% cache %>` site reads and writes under, as interpolation
/// parts — or None when any element of the key is something this pass
/// cannot ground, in which case the site renders transparently.
///
/// DECLINING IS THE SAFE ANSWER and the reason this is conservative. A
/// key that misses costs a render; a key that collides or fails to move
/// when the record does serves the WRONG BYTES, and no gate in this repo
/// would see it — the compare harness renders each lane once from cold.
/// So only two element shapes are claimed: a literal, which folds into
/// the constant text at compile time, and a record, which contributes
/// `cache_key_with_version`. Anything else and the site keeps the
/// behaviour it had before there was a store.
///
/// The view's own name leads, because two templates can cache on the
/// same record — campfire caches `[message, "presentation-v2"]` from
/// `messages/_message`, and a turbo-stream refresh renders the same
/// partial — and without the site in the key one would serve the
/// other's markup.
///
/// NO TEMPLATE DIGEST, unlike Rails. Rails hashes the template source
/// (and its render tree) into the key because its store is Redis and
/// outlives the deploy that changed the template. This store lives in
/// the process; a template change means a recompile, a new binary and
/// an empty store, so the digest would be a constant that changes
/// exactly when the thing it guards is already gone. An app's own
/// manual version string still works and is still honoured — campfire's
/// `"presentation-v2"` is a literal and folds into the text.
fn cache_key_parts(args: &[Expr], ctx: &ViewCtx) -> Option<Vec<InterpPart>> {
    // `cache(key)` or `cache(key, expires_in: …)`. More than that is a
    // shape this has not seen.
    if args.is_empty() || args.len() > 2 {
        return None;
    }
    let elements: Vec<&Expr> = match &*args[0].node {
        ExprNode::Array { elements, .. } => elements.iter().collect(),
        _ => vec![&args[0]],
    };

    // `views/` + the broadcast namespace + this site + the key parts.
    // `cache_scope` is a runtime read, not a constant: the same partial
    // renders both for a request and for a broadcast, and those two
    // outputs differ (the broadcast omits the CSRF input). See its note
    // in view_helpers.rb.
    let mut parts: Vec<InterpPart> = vec![
        InterpPart::Text { value: "views/".to_string() },
        InterpPart::Expr { expr: view_helpers_call("cache_scope", Vec::new()) },
        InterpPart::Text { value: format!("{}/", ctx.view_name) },
    ];
    for (i, el) in elements.iter().enumerate() {
        if i > 0 {
            push_key_text(&mut parts, "/");
        }
        match key_element(el, ctx)? {
            KeyPart::Text(t) => push_key_text(&mut parts, &t),
            KeyPart::Expr(e) => parts.push(InterpPart::Expr { expr: e }),
        }
    }
    Some(parts)
}

enum KeyPart {
    Text(String),
    Expr(Expr),
}

/// How a `<% cache %>` site names its fragment.
enum FragmentKey {
    /// `cache_key_parts`' interpolated text.
    Text(Vec<InterpPart>),
    /// `cache record`: the template's path and the record, which the
    /// controller combines, as Rails' CacheHelper hands it
    /// `cache_fragment_name(record, digest_path:)`.
    Record { template: Vec<InterpPart>, record: Expr },
}

/// `cache record` — one record, nothing else in the key — keeps the
/// record in the key, as Rails does, instead of folding it to text.
///
/// TWO THINGS DEPEND ON IT. An app's `combined_fragment_cache_key`
/// override sees what Rails would hand it: campfire's `CachedResponses`
/// shares a message's fragment across viewers only when the key holds a
/// `Message`, and keys a String per viewer and per database commit. And
/// the key is the one a `cached: true` collection of the same partial
/// files the fragment under (`collection_fragment_key` with the same
/// template path), so a collection miss that renders the partial writes
/// one entry, not a second that nothing ever reads.
///
/// The template is `ctx.view_name`, behind the broadcast scope as in
/// `cache_key_parts`. Other key shapes keep `cache_key_parts`.
fn record_cache_key(args: &[Expr], ctx: &ViewCtx) -> Option<FragmentKey> {
    if args.is_empty() || args.len() > 2 {
        return None;
    }
    let record = match &*args[0].node {
        ExprNode::Array { elements, .. } if elements.len() == 1 => &elements[0],
        ExprNode::Array { .. } => return None,
        _ => &args[0],
    };
    if !is_record(record, ctx) {
        return None;
    }
    Some(FragmentKey::Record {
        template: vec![
            InterpPart::Expr { expr: view_helpers_call("cache_scope", Vec::new()) },
            InterpPart::Text { value: ctx.view_name.clone() },
        ],
        record: record.clone(),
    })
}

/// Append literal text, merging into the previous chunk so the emitted
/// string carries one `Text` run rather than a chain of adjacent ones.
fn push_key_text(parts: &mut Vec<InterpPart>, t: &str) {
    if let Some(InterpPart::Text { value }) = parts.last_mut() {
        value.push_str(t);
        return;
    }
    parts.push(InterpPart::Text { value: t.to_string() });
}

/// One key element: a literal folds to compile-time text, a record
/// contributes `record.cache_key_with_version`, everything else declines.
fn key_element(el: &Expr, ctx: &ViewCtx) -> Option<KeyPart> {
    if let ExprNode::Lit { value } = &*el.node {
        return match value {
            Literal::Str { value } => Some(KeyPart::Text(value.clone())),
            Literal::Sym { value } => Some(KeyPart::Text(value.as_str().to_string())),
            Literal::Int { value } => Some(KeyPart::Text(value.to_string())),
            Literal::Bool { value } => Some(KeyPart::Text(value.to_string())),
            _ => None,
        };
    }
    if !is_record(el, ctx) {
        return None;
    }
    Some(KeyPart::Expr(send(
        Some(el.clone()),
        "cache_key_with_version",
        Vec::new(),
        None,
        false,
    )))
}

/// Does this expression evaluate to one of the app's model records?
///
/// NOT BY TYPE, because at this point there is none to read. A view
/// body reaches the walker as compiled ERB whose locals are still bare
/// `Send { recv: None }` barewords with `ty: None` — models type before
/// views lower. So this asks the two questions the rest of this file
/// asks about a name: is it one of this view's LOCALS (rather than a
/// helper call that happens to share the name), and is that name a
/// model's snake singular? Both, or it is not a record.
///
/// The conjunction is the conservative half. `model_singulars` alone
/// would claim a helper named `message`; `locals` alone would claim
/// `notice`. Together they claim exactly the Rails convention these
/// partials are written to — `messages/_message.html.erb` receives a
/// `message` — which is every cache site in the corpus.
///
/// `ivar_models` covers the other spelling: a view's `@story` is
/// rewritten to a local by the time it gets here, and that map holds
/// only names whose type IS a known model. An ivar that may be nil
/// (`@comment = Comment.first`) is not a record here, under either
/// spelling: its key would call `cache_key_with_version` on nil.
///
/// The type check stays first for the day these carry one.
fn is_record(el: &Expr, ctx: &ViewCtx) -> bool {
    if let Some(crate::ty::Ty::Class { id, .. }) = &el.ty {
        return ctx.model_singulars.contains(&crate::naming::snake_case(id.0.as_str()));
    }
    let name = match &*el.node {
        ExprNode::Var { name, .. } => name.as_str(),
        ExprNode::Send { recv: None, method, args, block: None, .. } if args.is_empty() => {
            method.as_str()
        }
        _ => return false,
    };
    if ctx.nilable_ivar_models.contains(name) {
        return false;
    }
    if ctx.ivar_models.contains_key(name) {
        return true;
    }
    ctx.locals.iter().any(|l| l == name) && ctx.model_singulars.contains(name)
}

/// `expires_in:` off the options hash, in whole seconds; 0 (never
/// expires) when absent, which is Rails' behaviour for an entry with no
/// TTL and the only spelling the corpus writes. `.to_i` reads seconds
/// off an Integer literal and off an `ActiveSupport::Duration` alike —
/// the same forwarding `lower::rails_cache` does for the controller-side
/// `Rails.cache.fetch`.
fn cache_ttl(args: &[Expr]) -> Expr {
    let zero = || Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Int { value: 0 } });
    let Some(opts) = args.get(1) else { return zero() };
    let ExprNode::Hash { entries, .. } = &*opts.node else { return zero() };
    entries
        .iter()
        .find(|(k, _)| {
            matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } }
                if value.as_str() == "expires_in")
        })
        .map(|(_, v)| send(Some(v.clone()), "to_i", Vec::new(), None, false))
        .unwrap_or_else(zero)
}

/// The read / render / write triple. See the walker arm's note for the
/// shape and why it is spelled without a block.
fn emit_cached_fragment(
    fragment: &FragmentKey,
    ttl: Expr,
    body: &Expr,
    ctx: &ViewCtx,
    span: Span,
) -> Vec<Expr> {
    // Distinct per site within a view function: two `<% cache %>` blocks
    // in one template would otherwise share a local, and a nested one
    // would shadow its parent's hit mid-render.
    let uniq = span.start;
    let hit = Symbol::from(format!("__cache_hit_{uniq}"));
    let cap = format!("__cache_io_{uniq}");

    let interp = |parts: &[InterpPart]| Expr::new(span, ExprNode::StringInterp { parts: parts.to_vec() });
    // The read's and the write's arguments, before the value and TTL.
    let key = || match fragment {
        FragmentKey::Text(parts) => vec![interp(parts)],
        FragmentKey::Record { template, record } => vec![interp(template), record.clone()],
    };
    let (read_method, write_method) = match fragment {
        FragmentKey::Text(_) => ("fragment_read", "fragment_write"),
        FragmentKey::Record { .. } => ("fragment_read_record", "fragment_write_record"),
    };
    // `ActionView::ViewHelpers.fragment_read/fragment_write`: the
    // runtime's store on every target, reopened on the ruby family and
    // spinel to ask the controller as Rails' CacheHelper does.
    let store = || {
        Expr::new(
            span,
            ExprNode::Const { path: vec![Symbol::from("ActionView"), Symbol::from("ViewHelpers")] },
        )
    };
    let hit_ref = || Expr::new(span, ExprNode::Var { id: VarId(0), name: hit.clone() });

    let read = Expr::new(
        span,
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: hit.clone() },
            value: send(Some(store()), read_method, key(), None, true),
        },
    );

    // Miss: render into the site's own accumulator, then append what
    // `write_str` answers — it returns the value it stored, so the
    // fragment reaches the page and the store in one statement.
    let mut miss = vec![assign_accumulator_string_new(&cap)];
    miss.extend(walk_body(body, &ViewCtx { accumulator: cap.clone(), ..ctx.clone() }));
    miss.push(accumulator_append_call(
        send(
            Some(store()),
            write_method,
            key().into_iter().chain([accumulator_result_ref(&cap), ttl]).collect(),
            None,
            true,
        ),
        ctx,
    ));

    vec![
        read,
        Expr::new(
            span,
            ExprNode::If {
                cond: send(Some(hit_ref()), "nil?", Vec::new(), None, false),
                then_branch: seq(miss),
                else_branch: seq(vec![accumulator_append_call(hit_ref(), ctx)]),
            },
        ),
    ]
}

/// `form_for :scope, k: v, ...` as `form_with(scope: :scope, k: v, ...)`.
/// None for a record form (`form_for @user`), which names and fills its
/// fields from the record.
/// `form_for(record, opts)` as form_with's arguments: `as:` is its `scope:`, the verb is the record's (Rails'
/// `apply_form_for_options!`), and `html:` (with a top-level `data:`, which Rails moves into it) renders on the tag.
fn form_for_record_args(args: &[Expr]) -> Result<(Vec<Expr>, super::form_with::FormForHtml), String> {
    let (record, rest) = args.split_first().ok_or("form_for is lowered only with a record")?;
    let opts: Vec<(Expr, Expr)> = match rest {
        [] => Vec::new(),
        [opts] => match &*opts.node {
            ExprNode::Hash { entries, .. } => entries.clone(),
            _ => return Err("form_for is lowered only with its options written as a Hash".into()),
        },
        _ => return Err("form_for is lowered only with a record and an options Hash".into()),
    };
    let sym = |k: &str| Expr::new(record.span, ExprNode::Lit { value: Literal::Sym { value: Symbol::from(k) } });
    let mut entries = vec![(sym("model"), record.clone())];
    let mut html = Vec::new();
    let mut named = false;
    let mut method = false;
    for (k, v) in opts {
        let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else {
            return Err("form_for is lowered only with Symbol option keys".into());
        };
        match key.as_str() {
            "as" => {
                if super::form_with::str_or_sym_literal(&v).is_none() {
                    return Err("form_for is lowered only with `as:` a literal name: the field names are written from it".into());
                }
                named = true;
                entries.push((sym("scope"), v));
            }
            "url" | "namespace" => entries.push((k, v)),
            "method" => {
                method = true;
                entries.push((k, v));
            }
            "html" => match &*v.node {
                ExprNode::Hash { entries: inner, .. } => {
                    for (hk, hv) in inner {
                        if matches!(&*hk.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "method") {
                            method = true;
                            entries.push((hk.clone(), hv.clone()));
                        } else {
                            html.push((hk.clone(), hv.clone()));
                        }
                    }
                }
                _ => return Err("form_for is lowered only with `html:` written as a Hash".into()),
            },
            "data" => html.push((k, v)),
            other => return Err(format!("form_for's `{other}:` option is not lowered")),
        }
    }
    if !method {
        let persisted = Expr::new(record.span, ExprNode::Send { recv: Some(record.clone()), method: Symbol::from("persisted?"), args: Vec::new(), block: None, parenthesized: false });
        entries.push((sym("method"), Expr::new(record.span, ExprNode::If { cond: persisted, then_branch: sym("patch"), else_branch: sym("post") })));
    }
    Ok((vec![Expr::new(record.span, ExprNode::Hash { entries, kwargs: true })], super::form_with::FormForHtml { html, named }))
}

fn form_for_scope_args(args: &[Expr], ctx: &ViewCtx) -> Option<Vec<Expr>> {
    let (first, rest) = args.split_first()?;
    let ExprNode::Lit { value: Literal::Sym { value: scope } } = &*first.node else { return None };
    // Not as recordless when the view has a `@<scope>` record: Rails fills the fields from it.
    if ctx.record_classes.contains_key(scope.as_str()) || ctx.ivar_models.contains_key(scope.as_str()) {
        return None;
    }
    let mut entries = vec![(
        Expr::new(first.span, ExprNode::Lit { value: Literal::Sym { value: Symbol::from("scope") } }),
        first.clone(),
    )];
    match rest {
        [] => {}
        [opts] => {
            let ExprNode::Hash { entries: more, .. } = &*opts.node else { return None };
            entries.extend(more.iter().cloned());
        }
        _ => return None,
    }
    // Not without `url:`: Rails posts to the current URL, which the form_with expansion has no spelling for.
    let has_url = entries.iter().any(|(k, _)| matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "url"));
    has_url.then(|| vec![Expr::new(first.span, ExprNode::Hash { entries, kwargs: true })])
}
