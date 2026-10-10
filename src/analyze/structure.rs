//! Observed equation structure, separate from inferred values.
//!
//! This records the existing analyzer's cumulative slot allocation and writer
//! routing. It does not precompute the structure or claim it is fixed before typing.
//! All keys are canonical and exclude slot ids, types and scheduling metadata.
//! Start/end digests and component counts are emitted only with FIXPOINT_STATS.
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{Hash, Hasher};

use super::{Analyzer, fold};
use crate::App;
use crate::expr::{Expr, ExprNode, LValue};
use crate::ident::{ClassId, Symbol};

#[derive(Clone, Default)]
pub(super) struct Structure {
    slots: BTreeSet<String>,
    writers: BTreeMap<String, BTreeSet<String>>,
    refs: BTreeSet<String>,
    routing: BTreeSet<String>,
}
#[derive(Default)]
struct Observed {
    structure: Structure,
    defined: BTreeSet<(ClassId, Symbol)>,
    shapes: HashMap<(ClassId, Symbol), super::ParamShape>,
}
thread_local! { static OBS: RefCell<Observed> = RefCell::new(Observed::default()); }

pub(super) fn reset(
    inputs: impl FnOnce() -> (
        BTreeSet<(ClassId, Symbol)>,
        HashMap<(ClassId, Symbol), super::ParamShape>,
    ),
) {
    if super::fixpoint_check::stats_on() {
        let (defined, shapes) = inputs();
        OBS.with(|o| {
            *o.borrow_mut() = Observed {
                defined,
                shapes,
                ..Observed::default()
            }
        });
    }
}
pub(super) fn owner(f: impl FnOnce(&BTreeSet<(ClassId, Symbol)>) -> ClassId) -> ClassId {
    OBS.with(|o| f(&o.borrow().defined))
}
pub(super) fn placed_arity(
    f: impl FnOnce(&HashMap<(ClassId, Symbol), super::ParamShape>) -> usize,
) -> usize {
    OBS.with(|o| f(&o.borrow().shapes))
}
pub(super) fn write(slot: String, writer: String) {
    if super::fixpoint_check::stats_on() {
        OBS.with(|o| o.borrow_mut().structure.write(slot, writer));
    }
}
pub(super) fn fold_write(key: &fold::SlotKey, writer: &str) {
    if super::fixpoint_check::stats_on() {
        write(format!("fold:{key:?}"), writer.to_string());
    }
}
pub(super) fn param_site(
    class: &ClassId,
    method: &Symbol,
    n: usize,
    span: &crate::span::Span,
    context: Option<&ClassId>,
) {
    if !super::fixpoint_check::stats_on() {
        return;
    }
    for i in 0..n {
        write(
            format!("param:{}#{}:{i}", class.0, method),
            format!("call:{:?}:context:{context:?}", fold::site_of(span)),
        );
    }
}

impl Structure {
    fn write(&mut self, slot: String, writer: String) {
        self.slots.insert(slot.clone());
        self.writers.entry(slot).or_default().insert(writer);
    }
    fn add_slot(&mut self, slot: String) {
        self.slots.insert(slot);
    }
    pub(super) fn summary(&self) -> serde_json::Value {
        fn hash(value: impl Hash) -> String {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            value.hash(&mut h);
            format!("{:016x}", h.finish())
        }
        serde_json::json!({
            "digest": hash((&self.slots, &self.writers, &self.refs, &self.routing)),
            "universe": {"digest":hash(&self.slots), "n":self.slots.len()},
            "writers": {"digest":hash(&self.writers), "slots":self.writers.len(),
                        "n":self.writers.values().map(BTreeSet::len).sum::<usize>()},
            "reference_mode": {"digest":hash(&self.refs), "n":self.refs.len()},
            "routing": {"digest":hash(&self.routing), "n":self.routing.len()},
        })
    }
}

fn syntax(e: &Expr, context: &str, owner: &str, s: &mut Structure) {
    let site = fold::site_of(&e.span);
    // Empty literal/parameter-default stamps are carried state too. Each
    // expression has a syntax identity, shared-node addresses are irrelevant.
    s.add_slot(format!("ir:{context}:{site:?}"));
    let mut assignment = |target: &LValue| match target {
        LValue::Ivar { name } => {
            s.write(format!("ivar:{owner}:{name}"), format!("assign:{site:?}"))
        }
        LValue::Const { path } => s.write(
            format!("constant:{owner}:{path:?}"),
            format!("assign:{context}:{site:?}"),
        ),
        _ => {}
    };
    match &*e.node {
        ExprNode::Assign { target, .. } | ExprNode::OpAssign { target, .. } => assignment(target),
        ExprNode::MultiAssign { targets, .. } => targets.iter().for_each(assignment),
        ExprNode::Lambda { params, .. } => {
            s.write(
                format!("closure-result:{context}:{site:?}"),
                format!("closure-body:{site:?}"),
            );
            for (i, _) in params.iter().enumerate() {
                s.write(
                    format!("closure-param:{context}:{site:?}:{i}"),
                    format!("block-yield:{site:?}"),
                );
            }
        }
        _ => {}
    }
    e.node.for_each_child(&mut |c| syntax(c, context, owner, s));
}

impl Analyzer {
    pub(super) fn structure_snapshot(&self, app: &App) -> Structure {
        let mut s = OBS.with(|o| o.borrow().structure.clone());
        // Registry slots exist independently of whether anyone reads them by
        // reference; distinguish class and instance method slots.
        for (id, ci) in &self.classes {
            for (side, table) in [(false, &ci.instance_methods), (true, &ci.class_methods)] {
                for method in table.keys() {
                    s.add_slot(format!("ret:{}:{}:{}", id.0, side, method));
                }
            }
            for name in ci.constants.keys() {
                s.add_slot(format!("constant:{}::{name}", id.0));
            }
            for name in ci.attributes.fields.keys() {
                s.add_slot(format!("attribute:{}@{name}", id.0));
            }
        }
        for ((class, method), row) in &self.inferred_params {
            for i in 0..row.len() {
                s.add_slot(format!("param:{}#{}:{i}", class.0, method));
            }
        }
        for (decl, _) in &self.typed_constants {
            s.add_slot(format!("typed-constant:{decl:?}"));
        }
        for ((class, action), row) in &self.refined_action_bindings {
            for name in row.keys() {
                s.write(
                    format!("controller:{}#{action}@{name}", class.0),
                    format!("action-context:{}#{action}", class.0),
                );
            }
        }
        let mut method = |id: &ClassId, m: &crate::dialect::MethodDef| {
            let context = format!("{}:{:?}:{}", id.0, m.receiver, m.name);
            s.write(
                format!(
                    "ret:{}:{}:{}",
                    id.0,
                    m.receiver == crate::dialect::MethodReceiver::Class,
                    m.name
                ),
                format!("body:{:?}:{context}", fold::site_of(&m.body.span)),
            );
            for (i, p) in m.params.iter().enumerate() {
                s.add_slot(format!("param:{}#{}:{i}", id.0, m.name));
                if let Some(default) = &p.default {
                    s.write(
                        format!("param:{}#{}:{i}", id.0, m.name),
                        format!("default:{:?}:{context}", fold::site_of(&default.span)),
                    );
                    syntax(default, &context, id.0.as_str(), &mut s);
                }
            }
            syntax(&m.body, &context, id.0.as_str(), &mut s);
        };
        for model in &app.models {
            for m in model.methods() {
                method(&model.name, m);
            }
        }
        for class in app
            .library_classes
            .iter()
            .chain(app.rails_application.iter())
        {
            for m in &class.methods {
                method(&class.name, m);
            }
        }
        for module in &app.test_modules {
            for m in &module.helpers {
                method(&module.name, m);
            }
        }
        for c in &app.controllers {
            for a in c.actions() {
                let context = format!("{}#{}", c.name.0, a.name);
                s.write(
                    format!("ret:{}:false:{}", c.name.0, a.name),
                    format!("body:{:?}:{context}", fold::site_of(&a.body.span)),
                );
                syntax(&a.body, &context, c.name.0.as_str(), &mut s);
            }
        }
        for v in &app.views {
            syntax(
                &v.body,
                &format!("view:{}", v.name),
                v.name.as_str(),
                &mut s,
            );
        }
        for module in &app.test_modules {
            if let Some(body) = &module.setup {
                syntax(
                    body,
                    &format!("test-setup:{}", module.name.0),
                    module.name.0.as_str(),
                    &mut s,
                );
            }
            for test in &module.tests {
                syntax(
                    &test.body,
                    &format!(
                        "test:{}:{:?}",
                        module.name.0,
                        fold::site_of(&test.body.span)
                    ),
                    module.name.0.as_str(),
                    &mut s,
                );
            }
        }
        for key in self.state_fp(app).context_slot_keys() {
            s.write(key.clone(), format!("context-channel:{key}"));
        }
        // The real allocated fold universe, references, aliases and may-flow
        // graph are observed before expansion clears active reference mode.
        let (keys, refs, routing) = fold::structure_parts();
        for key in keys {
            s.add_slot(format!("fold:{key:?}"));
        }
        s.refs.extend(refs);
        s.routing.extend(routing);
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn structure_ignores_insertion_order_and_counts_writer_identity() {
        let mut a = Structure::default();
        let mut b = Structure::default();
        for (slot, writer) in [("x", "a"), ("x", "b"), ("y", "a")] {
            a.write(slot.into(), writer.into());
        }
        for (slot, writer) in [("y", "a"), ("x", "b"), ("x", "a")] {
            b.write(slot.into(), writer.into());
        }
        assert_eq!(a.summary(), b.summary());
        b.write("x".into(), "c".into());
        assert_ne!(a.summary()["digest"], b.summary()["digest"]);
        b = a.clone();
        b.refs.insert("x".into());
        assert_ne!(a.summary()["digest"], b.summary()["digest"]);
    }
}
