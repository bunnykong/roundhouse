//! Quick-test HK: one replaceable body result per source declaration/context.
//! Copies keep their existing routing; they are not registered as new writers.
use super::{ParamKey, parameter_conservation as pc};
use crate::{App, dialect::MethodReceiver, ident::{ClassId, Symbol}, span::Span, ty::Ty};
use std::{cell::RefCell, collections::{BTreeMap, HashMap}, sync::LazyLock};

static ON: LazyLock<bool> = LazyLock::new(|| std::env::var("RH_CUR_HK").as_deref() == Ok("1"));
pub(super) fn on() -> bool { *ON }
type Declaration = (u32, u32, u32);
type Writer = (ParamKey, Declaration);
#[derive(Default)]
struct State {
    current: Option<Writer>,
    cells: HashMap<ParamKey, BTreeMap<Declaration, Ty>>,
    unscoped: u64,
}
thread_local! { static ST: RefCell<State> = RefCell::new(State::default()); }

fn identity(c: &ClassId, m: &Symbol, side: bool, span: Span) -> Writer {
    ((c.clone(), m.clone(), if side { MethodReceiver::Class } else { MethodReceiver::Instance }),
     (span.file.0, span.start, span.end))
}
pub(super) fn reset(app: &App) {
    if !on() { return; }
    let mut state = State::default();
    let mut declare = |c: &ClassId, m: &Symbol, side: bool, span: Span| {
        let (slot, declaration) = identity(c, m, side, span);
        state.cells.entry(slot).or_default().insert(declaration, Ty::pending_untyped());
    };
    for model in &app.models { for m in model.methods() {
        declare(&model.name, &m.name, m.receiver == MethodReceiver::Class, m.body.span);
    }}
    for class in &app.library_classes { for m in &class.methods {
        declare(&class.name, &m.name, m.receiver == MethodReceiver::Class, m.body.span);
    }}
    for c in &app.controllers {
        for m in c.class_methods() { declare(&c.name, &m.name, true, m.body.span); }
        for a in c.actions() { declare(&c.name, &a.name, false, a.body.span); }
    }
    for module in &app.test_modules { for m in &module.helpers {
        declare(&module.name, &m.name, m.receiver == MethodReceiver::Class, m.body.span);
    }}
    ST.with(|s| *s.borrow_mut() = state);
}
pub(super) struct Guard(Option<Option<Writer>>);
impl Drop for Guard {
    fn drop(&mut self) { if let Some(previous) = self.0.take() {
        ST.with(|s| s.borrow_mut().current = previous);
    }}
}
pub(super) fn writer(c: &ClassId, m: &Symbol, side: bool, span: Span) -> Guard {
    if !on() { return Guard(None); }
    let current = identity(c, m, side, span);
    Guard(Some(ST.with(|s| s.borrow_mut().current.replace(current))))
}
pub(super) fn publish(mut new: Ty) -> Ty {
    new = super::canon_var::produced(new);
    ST.with(|s| {
        let mut s = s.borrow_mut();
        let Some((slot, declaration)) = s.current.clone() else {
            s.unscoped += 1;
            return super::fixpoint_bound::bound(new);
        };
        let cells = s.cells.entry(slot).or_default();
        if let Some(old) = cells.get(&declaration) {
            if let Some(untied) = super::harvest_return::untie_recursive_return(old, &new) {
                super::fixpoint_check::note_untie_cut();
                super::det::note_cap("harvest_untie_cut");
                new = untied;
            }
        }
        cells.insert(declaration, new);
        let joined = cells.values().cloned().fold(Ty::pending_untyped(), |a, b| {
            if super::strict_producers::is_p(&a) { b }
            else if super::strict_producers::is_p(&b) { a }
            else { pc::join(a, b) }
        });
        super::fixpoint_bound::bound(pc::normalize(joined))
    })
}
pub(super) fn completion_values(f: &mut dyn FnMut(&mut Ty)) {
    if on() { ST.with(|s| {
        for t in s.borrow_mut().cells.values_mut().flat_map(|cells| cells.values_mut()) { f(t); }
    }); }
}
pub(super) fn report() {
    if on() { ST.with(|s| {
        let s = s.borrow();
        eprintln!("rh-current-harvest: {}", serde_json::json!({
            "slots":s.cells.len(),"writers":s.cells.values().map(|c|c.len()).sum::<usize>(),
            "unscoped":s.unscoped}));
    }); }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scope(start: u32) -> Guard {
        writer(&ClassId(Symbol::from("Probe")), &Symbol::from("value"), false,
            Span { file: crate::span::FileId(1), start, end: start + 1 })
    }
    fn publish_from(start: u32, value: Ty) -> Ty { let _guard = scope(start); publish(value) }
    #[test]
    #[ignore = "RH_CUR_HK=1"]
    fn qt_history_hk_current_writers_preserve_unknown_and_nil_in_both_orders() {
        assert!(on());
        for reverse in [false, true] {
            ST.with(|s| *s.borrow_mut() = State::default());
            let (a, b) = if reverse { (2, 1) } else { (1, 2) };
            publish_from(a, Ty::Int);
            publish_from(b, Ty::Nil);
            let joined = publish_from(a, Ty::unresolved());
            let expected = pc::join(Ty::unresolved(), Ty::Nil);
            assert_eq!(joined, expected, "superseded Int must go; distinct Nil and current unknown must survive");
            assert_eq!(publish_from(b, Ty::Bool), pc::join(Ty::unresolved(), Ty::Bool));
        }
    }
    #[test]
    #[ignore = "RH_CUR_HK=1"]
    fn qt_history_hk_untie_528_keeps_independent_writer() {
        assert!(on());
        ST.with(|s| *s.borrow_mut() = State::default());
        let array = |t| Ty::Array { elem: std::sync::Arc::new(t) };
        let prior = pc::join(Ty::Str, array(Ty::unresolved()));
        publish_from(1, prior.clone());
        publish_from(2, Ty::Nil);
        let joined = publish_from(1, pc::join(Ty::Str, array(prior.clone())));
        assert_eq!(joined, pc::join(prior, Ty::Nil));
    }
    #[test]
    #[ignore = "RH_CUR_HK=1"]
    fn qt_history_hk_registers_latest_ambiguous_var_without_retaining_old_core() {
        assert!(on());
        ST.with(|s| *s.borrow_mut() = State::default());
        let mut table = HashMap::new();
        let name = Symbol::from("value");
        let _guard = scope(1);
        super::super::Analyzer::register_method_return(&mut table, &name, Some(&Ty::Int));
        let var = Ty::Var {var:crate::ident::TyVar(0)};
        super::super::Analyzer::register_method_return(&mut table, &name, Some(&var));
        assert_eq!(table[&name], if super::super::canon_var::source_on() { Ty::unresolved() } else { var },
            "a current ambiguous result must replace the same writer's old Int");
        super::super::Analyzer::register_method_return(&mut table, &name, None);
        assert!(super::super::strict_producers::is_p(&table[&name]),
            "a current missing result must record its wait instead of retaining history");
    }
}
