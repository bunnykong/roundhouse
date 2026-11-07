//! `validates_with SomeValidator, max: 5, if: -> { … }, on: :create`
//! (#1348). Rails 8.1.4 builds one `SomeValidator.new(options)` per
//! declaration and calls its `validate(record)` while validating, gated
//! by `if:` / `unless:` / `on:`; the validator adds the record's errors.
//!
//! On the ruby family (`BuildTarget::lowers_validates_with`, whose
//! runtime defines `ActiveModel::Validator`) each declaration becomes
//! model source, ingested like the app's own:
//!
//! ```ruby
//! def __validates_with_article_1
//!   SomeValidator.new(max: 5).validate(self)
//! end
//! validate :__validates_with_article_1, if: -> { … }, on: :create
//! ```
//!
//! so the conditions take `validate`'s existing path. The validator is
//! built on each validation rather than once per declaration, which only
//! a validator keeping state between calls would notice, and its
//! `options` hold the declaration's own keys without the conditions.
//! Several validator classes in one call, or a condition `validate`
//! does not take (`on: :custom`), keep the declaration the reported gap,
//! as do strict targets.

use crate::app::App;
use crate::dialect::ModelBodyItem;
use crate::expr::ExprNode;
use crate::ingest::prism_api::PrismLocation;

/// Expand every reproducible `validates_with` in a model body, in place,
/// and mark the runtime as defining `ActiveModel::Validator`.
pub(crate) fn expand(app: &mut App) -> Vec<crate::diagnostic::Diagnostic> {
    app.ports_active_model_validator = true;
    let mut diagnostics = Vec::new();
    let sources = app.sources.clone();
    let guards = app.validates_with_guards.clone();
    // One count for the app, not per model: a parent model's method must
    // not share a name with its subclass's, which would override it, and
    // owners can underscore alike (`Foo_Bar`, `Foo::Bar`). The owner in
    // the name is only for reading.
    let mut counter = 0usize;
    for model in &mut app.models {
        let owner = crate::naming::underscore_words(model.name.0.as_str());
        let mut body = Vec::with_capacity(model.body.len());
        for item in std::mem::take(&mut model.body) {
            let expanded = match &item {
                ModelBodyItem::Unknown { expr, .. } => match &*expr.node {
                    ExprNode::Send { recv: None, method, .. } if method.as_str() == "validates_with" => {
                        let text = source_text(&sources, expr.span);
                        // An STI subclass's, moved here: run for it only.
                        let guard = guards.iter().find(|g| (g.file, g.start) == (expr.span.file.0, expr.span.start)).map(|g| &g.class).map(|class| format!("if: -> {{ is_a?({class}) }}"));
                        text.and_then(|t| {
                            counter += 1;
                            let name = format!("__validates_with_{owner}_{counter}");
                            generated_source(t, &name, guard.as_deref())
                                .and_then(|src| ingest_source_with_constants(&src, &model.name, &constants_at(&sources, expr.span)))
                        })
                    }
                    _ => None,
                },
                _ => None,
            };
            match expanded {
                Some((items, warnings)) => {
                    diagnostics.extend(warnings.into_iter().map(|mut diagnostic| {
                        diagnostic.span = item.span();
                        diagnostic
                    }));
                    body.extend(items);
                }
                None => body.push(item),
            }
        }
        model.body = body;
    }
    diagnostics
}

fn source_text(sources: &[crate::span::SourceFile], span: crate::span::Span) -> Option<&str> {
    let file = sources.get((span.file.0 as usize).checked_sub(1)?)?;
    file.text.get(span.start as usize..span.end as usize)
}

/// The model source for one `validates_with` call's text, or None when
/// it isn't one validator class with keyword options. `guard` is one more
/// condition (`if: -> { is_a?(Sub) }`).
fn generated_source(call_text: &str, name: &str, guard: Option<&str>) -> Option<String> {
    let result = ruby_prism::parse(call_text.as_bytes());
    let root = result.node();
    let call = root.as_program_node()?.statements().body().iter().next()?.as_call_node()?;
    let args: Vec<_> = call.arguments()?.arguments().iter().collect();
    let slice = |loc: ruby_prism::Location<'_>| -> Option<String> {
        call_text.get(loc.start_offset()..loc.end_offset()).map(str::to_string)
    };
    let (class, rest) = args.split_first()?;
    if class.as_constant_read_node().is_none() && class.as_constant_path_node().is_none() {
        return None;
    }
    let class = slice(class.location())?;
    let mut own = Vec::new();
    let mut conditions = Vec::new();
    match rest {
        [] => {}
        [options] => {
            let hash = options.as_keyword_hash_node()?;
            for el in hash.elements().iter() {
                let assoc = el.as_assoc_node()?;
                let key = assoc.key().as_symbol_node()?;
                let key_name = String::from_utf8_lossy(key.unescaped()).into_owned();
                let text = slice(assoc.location())?;
                // Scheduling also remains visible in the validator's options.
                if key_name == "prepend" {
                    own.push(text.clone());
                }
                if matches!(key_name.as_str(), "if" | "unless" | "on" | "prepend") {
                    conditions.push(text);
                } else {
                    own.push(text);
                }
            }
        }
        _ => return None,
    }
    let built = if own.is_empty() { format!("{class}.new") } else { format!("{class}.new({})", own.join(", ")) };
    conditions.extend(guard.map(str::to_string));
    let validate = std::iter::once(format!(":{name}")).chain(conditions).collect::<Vec<_>>().join(", ");
    Some(format!(
        "class GeneratedValidatesWith\n  def {name}\n    {built}.validate(self)\n  end\n\n  validate {validate}\nend\n"
    ))
}

/// The body items of `src`'s one class, ingested as model source under
/// its own isolated scope (as `delegate` forwarders are). None when it
/// doesn't ingest cleanly, or `validate` doesn't take its conditions.
pub(super) fn ingest_source(src: &str, owner: &crate::ident::ClassId) -> Option<Vec<ModelBodyItem>> {
    ingest_source_with_constants(src, owner, &Default::default())
        .and_then(|(items, warnings)| warnings.is_empty().then_some(items))
}

pub(super) fn ingest_source_with_constants(src: &str, owner: &crate::ident::ClassId, constants: &super::model::ClassConsts) -> Option<(Vec<ModelBodyItem>, Vec<crate::diagnostic::Diagnostic>)> {
    let (items, diags) = crate::ingest::prism::scope(|| -> Option<Vec<ModelBodyItem>> {
        let result = crate::ingest::prism::parse(src.as_bytes(), "<validates_with>");
        let root = result.node();
        let class = root.as_program_node()?.statements().body().iter().next()?.as_class_node()?;
        let body = class.body()?;
        let mut out = Vec::new();
        for stmt in super::util::flatten_statements(body) {
            out.extend(
                super::model::ingest_model_body_items(&stmt, owner, "<validates_with>", Vec::new(), None, constants)
                    .ok()?,
            );
        }
        Some(out)
    });
    // An unknown scheduling value keeps the callback in ordinary order.
    // Other ingest diagnostics still decline the generated declaration.
    if diags.iter().any(|diagnostic| diagnostic.severity != crate::diagnostic::Severity::Warning
        || !matches!(&diagnostic.kind, crate::diagnostic::DiagnosticKind::IgnoredValidatorOption { validator, option, .. }
            if validator.as_str() == "validate" && option.as_str() == "prepend")) {
        return None;
    }
    let items = items?;
    // `validate` declined its conditions: it stayed an Unknown.
    if items.iter().any(|i| matches!(i, ModelBodyItem::Unknown { .. })) {
        return None;
    }
    Some((items, diags))
}

/// Generated callbacks read scheduling constants in the original declaration's
/// body. The constructor source stays unchanged, retaining options[:prepend].
pub(super) fn constants_at(sources: &[crate::span::SourceFile], span: crate::span::Span) -> super::model::ClassConsts {
    let Some(source) = (span.file.0 as usize).checked_sub(1).and_then(|i| sources.get(i)) else { return Default::default() };
    let parsed = super::prism::parse_silent(source.text.as_bytes());
    let root = parsed.node();
    let classes = super::util::find_all_classes_with_scope(&root).into_iter().map(|(_, c)| (c.location(), c.body()));
    let modules = super::util::find_all_module_declarations_with_scope(&root).into_iter().map(|(_, m)| (m.location(), m.body()));
    classes.chain(modules)
        .filter(|(loc, _)| loc.start_offset() <= span.start as usize && (span.start as usize) < loc.end_offset())
        .max_by_key(|(loc, _)| loc.start_offset())
        .and_then(|(_, body)| body)
        .map(|body| super::model::ClassConsts::collect(&super::util::flatten_statements(body), &source.path))
        .unwrap_or_default()
}
