//! Paired, public-input error-gate census. Observes final IR and catalog
//! answers without typing bodies or changing diagnostics. The pure comparator
//! in tools/errgate.py assigns the cross-run verdicts.
use super::{Analyzer, body::BodyTyper};
use crate::diagnostic::{Diagnostic, DiagnosticKind, Severity};
use crate::{
    App,
    expr::{Expr, ExprNode},
    ty::Ty,
};
use std::collections::BTreeSet;
use std::sync::LazyLock;

static ON: LazyLock<bool> = LazyLock::new(|| {
    std::env::var("RH_ERRGATE").is_ok_and(|v| v == "1")
        && std::env::var("RH_PUBLIC_INPUT").is_ok_and(|v| v == "1")
});
pub(crate) fn on() -> bool {
    *ON
}

fn site(app: &App, span: &crate::span::Span) -> String {
    let path = span
        .file
        .0
        .checked_sub(1)
        .and_then(|i| app.sources.get(i as usize))
        .map(|s| s.path.as_str())
        .unwrap_or("<synthetic>");
    format!("{path}:{}:{}", span.start, span.end)
}

/// Top-level value arms only: gradual and pending inside a constructor
/// are not the receiver bit or Var marker that absorbs union dispatch.
fn arms<'a>(t: &'a Ty, out: &mut Vec<&'a Ty>, bit: &mut bool, var: &mut bool) {
    match t {
        Ty::Union { variants } => {
            for t in variants.iter() {
                arms(t, out, bit, var);
            }
        }
        Ty::Untyped { .. } => *bit = true,
        Ty::Var { .. } => *var = true,
        Ty::Nil | Ty::Bottom => {}
        _ => out.push(t),
    }
}
fn label(t: &Ty) -> String {
    // render_ty hides allocation ids; union and record order are canonical.
    crate::ide::render_ty(t)
}
fn recv(t: Option<&Ty>, answers: impl Fn(&Ty) -> bool) -> serde_json::Value {
    let (mut values, mut bit, mut var) = (Vec::new(), false, false);
    if let Some(t) = t {
        arms(t, &mut values, &mut bit, &mut var);
    } else {
        var = true;
    }
    let heads: BTreeSet<String> = values.iter().map(|t| label(t)).collect();
    let answering: BTreeSet<String> = values
        .iter()
        .filter(|t| answers(t))
        .map(|t| label(t))
        .collect();
    serde_json::json!({"arms":heads,"answering":answering,"bit":u8::from(bit),"var":u8::from(var)})
}
fn verdict(e: &Expr, r: &serde_json::Value, failed: bool) -> &'static str {
    if failed {
        "failed"
    } else if r["bit"] == 1 && matches!(e.ty.as_ref(), Some(Ty::Untyped { .. })) {
        "gradual"
    } else if r["var"] == 1 && r["answering"].as_array().is_none_or(Vec::is_empty) {
        if matches!(e.ty.as_ref(), Some(Ty::Untyped { .. })) {
            "gradual"
        } else if e
            .ty
            .as_ref()
            .is_none_or(|t| matches!(t, Ty::Var { .. } | Ty::Bottom))
        {
            "pending"
        } else {
            "ok"
        }
    } else {
        "ok"
    }
}

impl Analyzer {
    pub(super) fn report_errgate(&self, app: &App) {
        if !on() {
            return;
        }
        let typer = BodyTyper::new(&self.classes)
            .with_typed_constants(&self.typed_constants)
            .with_data_factories(&self.data_factories)
            .with_inquirers(&self.inquirers);
        let mut body = 0usize;
        crate::lower::for_each_emit_body_ref(app, &mut |root| {
            let mut ordinal = 0usize;
            visit(root, app, &typer, body, &mut ordinal);
            body += 1;
        });
        eprintln!(
            "rh-errgate-end: {}",
            serde_json::json!({"schema":1,"bodies":body,"scope":"final-source-IR", "build":option_env!("ROUNDHOUSE_COMMIT"), "input_digest":input_digest(app)})
        );
    }
}
fn visit(e: &Expr, app: &App, typer: &BodyTyper<'_>, body: usize, ordinal: &mut usize) {
    let occurrence = format!("{body}:{}", *ordinal);
    *ordinal += 1;
    if let ExprNode::Send {
        recv: Some(r),
        method,
        args,
        block,
        ..
    } = &*e.node
    {
        let block_ret = block.as_ref().and_then(|b| match &*b.node {
            ExprNode::Lambda { body, .. } => body.ty.as_ref(),
            _ => b.ty.as_ref(),
        });
        let failed = matches!(
            &e.diagnostic,
            Some(DiagnosticKind::SendDispatchFailed { .. })
        ) || (r.ty.as_ref().is_some_and(|t| !matches!(t, Ty::Var { .. }))
            && super::diagnostics::send_result_is_unknown(e));
        let mut row = recv(r.ty.as_ref(), |arm| {
            typer.errgate_answer(arm, method, block_ret, args)
        });
        row["recv_slot"] = serde_json::json!(origin_slot(&r.span));
        row["schema"] = 1.into();
        row["kind"] = "send_dispatch_failed".into();
        row["site"] = site(app, &e.span).into();
        row["recv_site"] = site(app, &r.span).into();
        row["op"] = method.as_str().into();
        row["occurrence"] = occurrence.clone().into();
        row["verdict"] = verdict(e, &row, failed).into();
        eprintln!("rh-errgate: {row}");
        if args.len() == 1
            && matches!(
                method.as_str(),
                "+" | "-" | "*" | "/" | "**" | "%" | "<" | "<=" | ">" | ">="
            )
        {
            let rhs = &args[0];
            let left = recv(r.ty.as_ref(), |_| false);
            let right = recv(rhs.ty.as_ref(), |_| false);
            let (mut ls, mut rs, mut b, mut v) = (Vec::new(), Vec::new(), false, false);
            if let Some(t) = &r.ty {
                arms(t, &mut ls, &mut b, &mut v);
            }
            if let Some(t) = &rhs.ty {
                arms(t, &mut rs, &mut b, &mut v);
            }
            let mut pairs = BTreeSet::new();
            let mut answering = BTreeSet::new();
            for l in ls {
                for rr in &rs {
                    let pair = (label(l), label(rr));
                    pairs.insert(pair.clone());
                    if typer.errgate_compatible(method, r, rhs, l, rr) {
                        answering.insert(pair);
                    }
                }
            }
            let failed = matches!(
                &e.diagnostic,
                Some(DiagnosticKind::IncompatibleBinop { .. })
            );
            let verdict = if failed {
                "failed"
            } else if b {
                "gradual"
            } else if v {
                "pending"
            } else {
                "ok"
            };
            eprintln!(
                "rh-errgate: {}",
                serde_json::json!({"schema":1,"kind":"incompatible_binop",
                "site":site(app,&e.span),"recv_site":site(app,&r.span),"op":method.as_str(),
                "occurrence":occurrence,"lhs":left,"rhs":right,"arms":pairs,"answering":answering,
                "bit":u8::from(b),"var":u8::from(v),"verdict":verdict})
            );
        }
    }
    e.node
        .for_each_child(&mut |c| visit(c, app, typer, body, ordinal));
}

/// Called after the CLI's attribution passes, so the diagnostic multiset
/// exactly matches the errors the check command reports (including copies).
pub(crate) fn diagnostics(diags: &[Diagnostic], app: &App) {
    if !on() {
        return;
    }
    let mut n = 0usize;
    for d in diags.iter().filter(|d| d.severity == Severity::Error) {
        let op = match &d.kind {
            DiagnosticKind::SendDispatchFailed { method, .. } => method.as_str(),
            DiagnosticKind::IncompatibleBinop { op, .. } => op.as_str(),
            DiagnosticKind::IvarUnresolved { name } => name.as_str(),
            DiagnosticKind::Unsupported { construct, .. } => construct.as_str(),
            _ => "",
        };
        eprintln!(
            "rh-errgate-diag: {}",
            serde_json::json!({"schema":1,"kind":d.code(),
            "site":site(app,&d.span),"op":op,"detail":d.kind})
        );
        n += 1;
    }
    eprintln!(
        "rh-errgate-diag-end: {}",
        serde_json::json!({"schema":1,"errors":n})
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bit_and_var_are_top_level_and_nil_is_not_a_value_arm() {
        let t = Ty::Union {
            variants: vec![
                Ty::Int,
                Ty::Nil,
                Ty::unresolved(),
                Ty::Var {
                    var: crate::ident::TyVar(9),
                },
            ]
            .into(),
        };
        let r = recv(Some(&t), |t| matches!(t, Ty::Int));
        assert_eq!(r["arms"], serde_json::json!(["Integer"]));
        assert_eq!(r["answering"], r["arms"]);
        assert_eq!(r["bit"], 1);
        assert_eq!(r["var"], 1);
        let nested = Ty::Array {
            elem: std::sync::Arc::new(Ty::unresolved()),
        };
        assert_eq!(recv(Some(&nested), |_| true)["bit"], 0);
    }
}

thread_local! {
    static WRITER: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    static INVALID_ORIGINS: std::cell::RefCell<BTreeSet<(u32,u32,u32)>> = std::cell::RefCell::new(BTreeSet::new());
    static ORIGINS: std::cell::RefCell<std::collections::BTreeMap<(u32,u32,u32), BTreeSet<String>>> = std::cell::RefCell::new(std::collections::BTreeMap::new());
}
pub(super) struct WriterGuard(Option<Option<String>>);
impl Drop for WriterGuard {
    fn drop(&mut self) {
        if let Some(prior) = self.0.take() {
            WRITER.with(|w| *w.borrow_mut() = prior);
        }
    }
}
pub(super) fn ret_slot(
    class: &crate::ident::ClassId,
    method: &crate::ident::Symbol,
    side: bool,
) -> String {
    format!("ret:{}:{}:{}", class.0, side, method)
}
pub(super) fn writer(
    class: &crate::ident::ClassId,
    method: &crate::ident::Symbol,
    side: bool,
) -> WriterGuard {
    if !on() {
        return WriterGuard(None);
    }
    WriterGuard(Some(
        WRITER.with(|w| w.replace(Some(ret_slot(class, method, side)))),
    ))
}
pub(super) fn reset() {
    if on() {
        ORIGINS.with(|o| o.borrow_mut().clear());
        INVALID_ORIGINS.with(|o| o.borrow_mut().clear());
        WRITER.with(|w| *w.borrow_mut() = None);
    }
}
pub(super) fn origin(span: &crate::span::Span, slots: BTreeSet<String>) {
    if on() {
        ORIGINS.with(|o| {
            let key = super::fold::site_of(span);
            if slots.is_empty() {
                INVALID_ORIGINS.with(|i| {
                    i.borrow_mut().insert(key);
                });
            }
            o.borrow_mut().entry(key).or_default().extend(slots);
        });
    }
}
fn origin_slot(span: &crate::span::Span) -> Option<String> {
    if INVALID_ORIGINS.with(|i| i.borrow().contains(&super::fold::site_of(span))) {
        return None;
    }
    ORIGINS.with(|o| {
        o.borrow()
            .get(&super::fold::site_of(span))
            .filter(|slots| slots.len() == 1)
            .and_then(|slots| slots.first().cloned())
    })
}
pub(super) fn note_drop(old: Option<&Ty>, new: &Ty) {
    if !on() {
        return;
    }
    WRITER.with(|w| {
        if let Some(slot) = w.borrow().as_ref() {
            note_slot_drop(slot, old, new);
        }
    });
}
pub(super) fn note_slot_drop(slot: &str, old: Option<&Ty>, new: &Ty) {
    let Some(old) = old else { return };
    if !on() || super::det::leq(old, new) {
        return;
    }
    let old = recv(Some(old), |_| false);
    let new = recv(Some(new), |_| false);
    let a: BTreeSet<_> = old["arms"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    let b: BTreeSet<_> = new["arms"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    let dropped: Vec<_> = a.difference(&b).copied().collect();
    if !dropped.is_empty() {
        eprintln!(
            "rh-descent-drop: {}",
            serde_json::json!({"slot":slot,"arms":dropped,"order":"D"})
        );
    }
}

fn input_digest(app: &App) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for source in &app.sources {
        source.path.hash(&mut h);
        source.text.hash(&mut h);
    }
    format!("{:016x}", h.finish())
}
