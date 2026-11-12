//! `obj.respond_to?(:name)` answered at compile time, where `obj`'s class
//! is known and its method surface is complete.
//!
//! A strict target has no runtime method table to ask, so the question
//! can only be answered from the class registry, walked in Ruby's lookup
//! order (the class, its modules last-included first, then the parent
//! chain). The nearest definition answers: `true` when it is public,
//! `false` when it is private or protected, as `respond_to?` does by
//! default. A miss is `false` only when the walk saw the whole surface:
//! every class on it is one the app declares (so analysis lists its
//! source methods, signed or not, with their visibility), none is open,
//! none has an include analysis could not resolve, none answers
//! dynamically (`method_missing` / `respond_to_missing?`), and the name
//! is not a column attribute. A class that defines `respond_to?` itself
//! answers for itself, so nothing is folded. Nor is a receiver that may
//! hold a class object rather than an instance (`klass = Snapshot`),
//! whose `respond_to?` asks the class side, or an assertion the test
//! class defines itself. Anything short of that leaves the call as
//! written, for the target to answer or refuse.
//!
//! Run after analysis ([`apply`], a post-analyze lowering) against the
//! analyzer's own registry, which holds each app class's includes, its
//! source methods with their visibility, and whether the app declares
//! it. The registries the emitters rebuild for test lowering hold
//! signatures only, so they cannot decide a miss. Reached through
//! `assert_respond_to` / `refute_respond_to`, the slice of Minitest a
//! model test uses to say a model does NOT have an attribute
//! (`refute_respond_to(manifest, :shop_id)`).

use crate::analyze::ClassInfo;
use crate::det::HashMap;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

/// Answer every `assert_respond_to` / `refute_respond_to` /
/// `assert_not_respond_to` and `respond_to?(:sym)` in the test tree that
/// the analyzer's registry decides, for a target with no runtime method
/// table. An assertion becomes the `assert` / `refute` of its answer and
/// a bare `respond_to?` the Bool. What is not decided stays as written:
/// the assertion inliner turns it into a `respond_to?` guard.
///
/// The Ruby family keeps the call: its runtime answers `respond_to?`
/// itself. Spinel is folded too, since a decided answer is the answer.
pub fn apply(
    app: &mut crate::app::App,
    registry: &HashMap<ClassId, ClassInfo>,
    target: crate::build_target::BuildTarget,
) {
    use crate::build_target::BuildTarget;
    if matches!(target, BuildTarget::Blog | BuildTarget::Ruby | BuildTarget::Jruby | BuildTarget::Roda | BuildTarget::Futamura) {
        return;
    }
    let surface = Surface {
        registry,
        model_parents: app.models.iter().map(|m| (m.name.clone(), m.parent.clone())).collect(),
    };
    // A test class (or a test class it inherits from) that defines an
    // assertion owns it: the call is its method, not Minitest's.
    let helpers: HashMap<ClassId, (Option<ClassId>, Vec<Symbol>)> = app
        .test_modules
        .iter()
        .map(|tm| (tm.name.clone(), (tm.parent.clone(), tm.helpers.iter().map(|h| h.name.clone()).collect())))
        .collect();
    for tm in &mut app.test_modules {
        let mut overridden = crate::det::HashSet::default();
        let mut owner = Some(tm.name.clone());
        let mut seen = std::collections::BTreeSet::new();
        while let Some(cid) = owner.filter(|c| seen.insert(c.clone())) {
            let Some((parent, names)) = helpers.get(&cid) else { break };
            overridden.extend(names.iter().filter(|n| ASSERTIONS.contains(&n.as_str())).cloned());
            owner = parent.clone();
        }
        let mut maybe_class = crate::det::HashSet::default();
        loop {
            let before = maybe_class.len();
            let bodies = tm.setup.iter().chain(tm.tests.iter().map(|t| &t.body)).chain(tm.helpers.iter().map(|h| &h.body));
            for body in bodies {
                class_bindings(body, &mut maybe_class);
            }
            if maybe_class.len() == before {
                break;
            }
        }
        let fold_ctx = Fold { surface: &surface, overridden: &overridden, maybe_class: &maybe_class };
        if let Some(setup) = &mut tm.setup {
            fold_ctx.fold(setup);
        }
        for t in &mut tm.tests {
            fold_ctx.fold(&mut t.body);
        }
        for h in &mut tm.helpers {
            fold_ctx.fold(&mut h.body);
        }
    }
}

const ASSERTIONS: [&str; 3] = ["assert_respond_to", "refute_respond_to", "assert_not_respond_to"];

/// The registry, plus each app model's written parent: the analyzer keeps
/// a model's parent only when it is another app model, so the root's
/// `< ActiveRecord::Base` (or a gem's base class) comes from here.
pub(crate) struct Surface<'a> {
    pub registry: &'a HashMap<ClassId, ClassInfo>,
    pub model_parents: HashMap<ClassId, Option<ClassId>>,
}

/// A local or ivar the test module may bind to a class object rather
/// than an instance. Analysis types `klass = Snapshot` as `Snapshot`, the
/// same as an instance, and keeps the difference only while it walks the
/// body, so a name bound to a constant, to `x.class` /
/// `x.singleton_class`, to another such name, or as a block parameter
/// (which may iterate classes) is not folded: its `respond_to?` asks the
/// class side.
fn class_bindings(e: &Expr, out: &mut crate::det::HashSet<Symbol>) {
    use crate::expr::LValue;
    let name_of = |target: &LValue| match target {
        LValue::Var { name, .. } => Some(name.clone()),
        LValue::Ivar { name } => Some(Symbol::from(format!("@{}", name.as_str()))),
        _ => None,
    };
    match &*e.node {
        ExprNode::Assign { target, value } | ExprNode::OpAssign { target, value, .. } => {
            if may_be_class(value, out) {
                out.extend(name_of(target));
            }
        }
        ExprNode::MultiAssign { targets, .. } => out.extend(targets.iter().filter_map(name_of)),
        ExprNode::Lambda { params, rest_param, .. } => {
            out.extend(params.iter().cloned());
            out.extend(rest_param.iter().cloned());
        }
        _ => {}
    }
    e.node.for_each_child(&mut |child| class_bindings(child, out));
}

fn may_be_class(value: &Expr, known: &crate::det::HashSet<Symbol>) -> bool {
    match &*value.node {
        ExprNode::Const { .. } => true,
        ExprNode::Var { name, .. } => known.contains(name),
        ExprNode::Ivar { name } => known.contains(&Symbol::from(format!("@{}", name.as_str()))),
        ExprNode::Send { method, .. } => {
            matches!(method.as_str(), "class" | "singleton_class" | "constantize" | "safe_constantize" | "const_get")
        }
        _ => false,
    }
}

struct Fold<'a> {
    surface: &'a Surface<'a>,
    overridden: &'a crate::det::HashSet<Symbol>,
    maybe_class: &'a crate::det::HashSet<Symbol>,
}

impl Fold<'_> {
    fn fold(&self, e: &mut Expr) {
        e.node.for_each_child_mut(&mut |child| self.fold(child));
        if let Some((assertion, answer)) = self.assertion(e) {
            let span = e.span;
            let value = Expr { ty: Some(Ty::Bool), ..Expr::new(span, ExprNode::Lit { value: Literal::Bool { value: answer } }) };
            *e.node = ExprNode::Send {
                recv: None,
                method: Symbol::from(assertion),
                args: vec![value],
                block: None,
                parenthesized: true,
            };
        } else if let Some(answer) = self.question(e) {
            *e.node = ExprNode::Lit { value: Literal::Bool { value: answer } };
            e.ty = Some(Ty::Bool);
        }
    }

    /// `assert_respond_to obj, :name` → (`"assert"`, answer); the refute
    /// forms → (`"refute"`, answer). The message argument, if any, is
    /// dropped, as the inliner drops every assertion's message.
    fn assertion(&self, e: &Expr) -> Option<(&'static str, bool)> {
        let ExprNode::Send { recv: None, method, args, block: None, .. } = &*e.node else { return None };
        let kind = match method.as_str() {
            "assert_respond_to" => "assert",
            "refute_respond_to" | "assert_not_respond_to" => "refute",
            _ => return None,
        };
        if self.overridden.contains(method) {
            return None;
        }
        let [obj, name, rest @ ..] = args.as_slice() else { return None };
        if rest.len() > 1 {
            return None;
        }
        Some((kind, self.ask(obj, name)?))
    }

    /// `obj.respond_to?(:name)`, one argument.
    fn question(&self, e: &Expr) -> Option<bool> {
        let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*e.node else { return None };
        if method.as_str() != "respond_to?" {
            return None;
        }
        let [name] = args.as_slice() else { return None };
        self.ask(recv, name)
    }

    fn ask(&self, obj: &Expr, name: &Expr) -> Option<bool> {
        let ExprNode::Lit { value: Literal::Sym { value: name } } = &*name.node else { return None };
        // Only a receiver whose evaluation has no effect (folding drops
        // it), and one that holds an instance.
        let binding = match &*obj.node {
            ExprNode::Var { name, .. } => Some(name.clone()),
            ExprNode::Ivar { name } => Some(Symbol::from(format!("@{}", name.as_str()))),
            ExprNode::SelfRef => None,
            _ => return None,
        };
        if binding.is_some_and(|b| self.maybe_class.contains(&b)) {
            return None;
        }
        let Some(Ty::Class { id, .. }) = obj.ty.as_ref() else { return None };
        responds(self.surface, id, name)
    }
}

/// `Some(true)` when the nearest definition of `name` on the class's
/// ancestor chain is public, `Some(false)` when it is private/protected
/// or when the walk saw the class's whole surface without finding it,
/// `None` otherwise.
///
/// The surface is trusted only for classes the app declares (models,
/// library classes and concerns, whose source methods analysis lists
/// signed or not, with their visibility). A test class, a class declared
/// inside a test file, and a framework stub have no such listing (their
/// registry entries hold signatures only, without ancestry), so a walk
/// that reaches one answers nothing. Nor does one that reaches a class
/// defining `respond_to?` itself: its answer is its own. A record's
/// framework surface is the AR catalog plus the runtime's own
/// `ActiveRecord::Base` (`to_key`, …).
pub(crate) fn responds(surface: &Surface, class: &ClassId, name: &Symbol) -> Option<bool> {
    let registry = surface.registry;
    let mut walk = Walk::default();
    walk.visit(surface, class);
    let own_answer = Symbol::from("respond_to?");
    if walk.order.iter().flatten().any(|cid| defines(&registry[cid], &own_answer)) {
        return None;
    }
    for step in &walk.order {
        // A class the walk cannot see into comes before any later hit,
        // and may define the name itself.
        let Some(cid) = step else { return None };
        let cls = &registry[cid];
        if cls.source_instance_methods.contains(name) {
            // Public in one reopening and private in another (which wins
            // is load order, not modeled here), or a source method whose
            // visibility analysis did not record: neither way.
            let private = cls.nonpublic_instance_methods.contains(name);
            let public = cls.public_instance_methods.contains(name);
            return if private == public { None } else { Some(public) };
        }
        // A signed member (a column, an association, a declared
        // accessor), which is public.
        if cls.instance_methods.contains_key(name) {
            return Some(true);
        }
        if column_method(cls, name) {
            return Some(true);
        }
    }
    if !walk.complete {
        return None;
    }
    // Every catalogued AR instance method, typed or not: dispatch seeds
    // only the typed ones onto the model, but the rest are Rails' too.
    // And the runtime's own Base, whose methods a record has however
    // dispatch types them.
    if walk.record
        && (crate::catalog::AR_CATALOG
            .iter()
            .any(|e| e.receiver == crate::catalog::ReceiverContext::Instance && e.name == name.as_str())
            || record_base_methods().contains(name))
    {
        return Some(true);
    }
    // Object's own public surface (`to_s`, `nil?`, …) is every class's;
    // the registry models it in dispatch rather than as a class.
    if crate::lower::core_method_names::OBJECT_METHODS.contains(&name.as_str()) {
        return Some(true);
    }
    // ActiveSupport's Object extensions (`blank?`, `try`, `to_param`)
    // are every Rails object's too, but the only list of them is nil's,
    // which also has nil's own (`to_a`, `&`): not answered either way.
    if crate::ingest::delegate::declarations::NIL_PUBLIC_METHODS.contains(&name.as_str()) {
        return None;
    }
    Some(false)
}

/// The public instance methods the runtime defines on
/// `ActiveRecord::Base` itself (base.rb and connection.rb's reopening),
/// owner-qualified: those files also define `Connection`, `Relation` and
/// others, whose methods a record does not have. `Base` includes no
/// module there, so this is its whole runtime surface.
fn record_base_methods() -> &'static crate::det::HashSet<Symbol> {
    static NAMES: std::sync::OnceLock<crate::det::HashSet<Symbol>> = std::sync::OnceLock::new();
    NAMES.get_or_init(|| {
        let mut names = crate::det::HashSet::default();
        for (file, source) in [
            ("runtime/ruby/active_record/base.rb", include_str!("../../runtime/ruby/active_record/base.rb")),
            ("runtime/ruby/active_record/connection.rb", include_str!("../../runtime/ruby/active_record/connection.rb")),
        ] {
            let classes = crate::ingest::ingest_library_classes(source.as_bytes(), file).expect("framework Base ingests");
            for class in classes.iter().filter(|c| c.name.0.as_str() == "ActiveRecord::Base") {
                names.extend(
                    class
                        .methods
                        .iter()
                        .filter(|m| {
                            m.receiver == crate::dialect::MethodReceiver::Instance
                                && m.visibility == crate::dialect::MethodVisibility::Public
                        })
                        .map(|m| m.name.clone()),
                );
            }
        }
        names
    })
}

fn defines(cls: &ClassInfo, name: &Symbol) -> bool {
    cls.instance_methods.contains_key(name) || cls.source_instance_methods.contains(name)
}

/// A column's generated reader, writer or predicate: `checksum`,
/// `checksum=`, `checksum?`, and nothing else.
fn column_method(cls: &ClassInfo, name: &Symbol) -> bool {
    let name = name.as_str();
    let column = name.strip_suffix('=').or_else(|| name.strip_suffix('?')).unwrap_or(name);
    cls.attributes.fields.contains_key(&Symbol::from(column))
}

/// An ancestor chain in Ruby's lookup order (the class, its modules
/// last-included first, then its parent's chain): each trusted class,
/// `None` where an untrusted one stands; and whether the chain held
/// only trusted classes and the AR root.
#[derive(Default)]
struct Walk {
    order: Vec<Option<ClassId>>,
    seen: std::collections::BTreeSet<ClassId>,
    complete: bool,
    record: bool,
    started: bool,
}

impl Walk {
    fn visit(&mut self, surface: &Surface, cid: &ClassId) {
        let registry = surface.registry;
        if !self.started {
            self.started = true;
            self.complete = true;
        }
        if !self.seen.insert(cid.clone()) {
            return;
        }
        // A model's framework root. Whatever the registry holds under
        // that name is a stub of it; its surface is the AR catalog's and
        // the runtime Base's (see `responds`).
        if cid.0.as_str() == "ActiveRecord::Base" {
            self.record = true;
            return;
        }
        let Some(cls) = registry.get(cid) else {
            self.complete = false;
            self.order.push(None);
            return;
        };
        if !cls.app_declared {
            self.complete = false;
            self.order.push(None);
            return;
        }
        let dynamic = ["method_missing", "respond_to_missing?"].map(Symbol::from);
        if cls.open || cls.unresolved_include || dynamic.iter().any(|m| defines(cls, m)) {
            self.complete = false;
        }
        self.order.push(Some(cid.clone()));
        for module in cls.includes.iter().rev() {
            self.visit(surface, module);
        }
        // A model root's written parent, which the registry drops.
        let parent = cls.parent.clone().or_else(|| surface.model_parents.get(cid).cloned().flatten());
        if let Some(parent) = parent {
            self.visit(surface, &parent);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(name: &str) -> ClassId {
        ClassId(Symbol::from(name))
    }

    /// `Snapshot < ApplicationRecord < ActiveRecord::Base`, with a
    /// `checksum` column and `Snapshot#digest` public, `#salt` private and
    /// `#rehash` defined in source without a signature.
    fn registry() -> HashMap<ClassId, ClassInfo> {
        let mut registry: HashMap<ClassId, ClassInfo> = HashMap::default();
        let mut base = ClassInfo::default();
        base.app_declared = true;
        base.parent = Some(id("ActiveRecord::Base"));
        registry.insert(id("ApplicationRecord"), base);
        let mut model = ClassInfo::default();
        model.app_declared = true;
        model.parent = Some(id("ApplicationRecord"));
        let mut fields = indexmap::IndexMap::new();
        fields.insert(Symbol::from("checksum"), Ty::Str);
        model.attributes = crate::ty::Row { fields: fields.into(), rest: None };
        model.instance_methods.insert(Symbol::from("digest"), Ty::Str);
        model.instance_methods.insert(Symbol::from("salt"), Ty::Str);
        model.source_instance_methods.extend(["digest", "salt", "rehash"].map(Symbol::from));
        model.nonpublic_instance_methods.insert(Symbol::from("salt"));
        model.public_instance_methods.extend(["digest", "rehash"].map(Symbol::from));
        registry.insert(id("Snapshot"), model);
        registry
    }

    fn ask(registry: &HashMap<ClassId, ClassInfo>, class: &str, name: &str) -> Option<bool> {
        let surface = Surface { registry, model_parents: HashMap::default() };
        responds(&surface, &id(class), &Symbol::from(name))
    }

    #[test]
    fn the_nearest_definition_answers_by_its_visibility() {
        let registry = registry();
        assert_eq!(ask(&registry, "Snapshot", "digest"), Some(true));
        assert_eq!(ask(&registry, "Snapshot", "salt"), Some(false));
        assert_eq!(ask(&registry, "Snapshot", "rehash"), Some(true), "a source method without a signature");
        assert_eq!(ask(&registry, "Snapshot", "checksum"), Some(true));
        assert_eq!(ask(&registry, "Snapshot", "checksum="), Some(true));
        assert_eq!(ask(&registry, "Snapshot", "save"), Some(true), "the AR catalog");
        assert_eq!(ask(&registry, "Snapshot", "to_key"), Some(true), "the runtime's own Base");
        assert_eq!(ask(&registry, "Snapshot", "exec_delete"), Some(false), "a runtime Connection method, not Base's");
        assert_eq!(ask(&registry, "Snapshot", "shop_id"), Some(false));
        assert_eq!(ask(&registry, "Snapshot", "checksum=="), Some(false), "only the generated suffixes");
        assert_eq!(ask(&registry, "Snapshot", "checksum?="), Some(false));
    }

    #[test]
    fn a_model_roots_written_parent_supplies_the_framework_surface() {
        // As analysis registers them: ApplicationRecord's parent is not
        // another app model, so the registry holds none.
        let mut registry = registry();
        registry.get_mut(&id("ApplicationRecord")).unwrap().parent = None;
        let with = |parent: &str| Surface {
            registry: &registry,
            model_parents: [
                (id("ApplicationRecord"), Some(id(parent))),
                (id("Snapshot"), Some(id("ApplicationRecord"))),
            ]
            .into_iter()
            .collect(),
        };
        let on = |surface: &Surface, name: &str| responds(surface, &id("Snapshot"), &Symbol::from(name));
        let record = with("ActiveRecord::Base");
        assert_eq!(on(&record, "save"), Some(true));
        assert_eq!(on(&record, "to_key"), Some(true));
        assert_eq!(on(&record, "shop_id"), Some(false));
        let gem = with("Gem::Record");
        assert_eq!(on(&gem, "digest"), Some(true));
        assert_eq!(on(&gem, "to_key"), None, "a base class the registry cannot see");
        assert_eq!(on(&gem, "shop_id"), None);
    }

    #[test]
    fn a_method_public_in_one_reopening_and_private_in_another_is_not_folded() {
        let mut registry = registry();
        let model = registry.get_mut(&id("Snapshot")).unwrap();
        model.public_instance_methods.insert(Symbol::from("salt"));
        assert_eq!(ask(&registry, "Snapshot", "salt"), None);
        // A source method whose visibility analysis did not record.
        registry.get_mut(&id("Snapshot")).unwrap().source_instance_methods.insert(Symbol::from("mystery"));
        assert_eq!(ask(&registry, "Snapshot", "mystery"), None);
    }

    #[test]
    fn a_class_answering_respond_to_itself_is_not_folded() {
        let mut registry = registry();
        registry.get_mut(&id("ApplicationRecord")).unwrap().source_instance_methods.insert(Symbol::from("respond_to?"));
        assert_eq!(ask(&registry, "Snapshot", "digest"), None);
        assert_eq!(ask(&registry, "Snapshot", "shop_id"), None);
    }

    #[test]
    fn a_class_the_app_does_not_declare_is_not_folded() {
        // A test class's registry entry: signatures only, no ancestry.
        let mut registry = registry();
        let mut test_class = ClassInfo::default();
        test_class.instance_methods.insert(Symbol::from("helper"), Ty::Nil);
        registry.insert(id("SnapshotTest"), test_class);
        assert_eq!(ask(&registry, "SnapshotTest", "helper"), None);
        assert_eq!(ask(&registry, "SnapshotTest", "shared_helper"), None);
    }

    #[test]
    fn an_unseen_module_before_a_hit_is_not_folded() {
        // The model includes a module the registry does not hold; it
        // comes before ApplicationRecord's `touch_up` in lookup order.
        let mut registry = registry();
        registry.get_mut(&id("ApplicationRecord")).unwrap().source_instance_methods.insert(Symbol::from("touch_up"));
        registry.get_mut(&id("Snapshot")).unwrap().includes.push(id("Gem::Unseen"));
        assert_eq!(ask(&registry, "Snapshot", "digest"), Some(true), "the class's own method still answers");
        assert_eq!(ask(&registry, "Snapshot", "touch_up"), None);
        assert_eq!(ask(&registry, "Snapshot", "shop_id"), None);
    }

    #[test]
    fn a_dynamic_or_open_class_answers_only_hits() {
        let mut registry = registry();
        registry.get_mut(&id("Snapshot")).unwrap().source_instance_methods.insert(Symbol::from("method_missing"));
        assert_eq!(ask(&registry, "Snapshot", "digest"), Some(true));
        assert_eq!(ask(&registry, "Snapshot", "shop_id"), None);
        let mut registry = self::registry();
        registry.get_mut(&id("ApplicationRecord")).unwrap().open = true;
        assert_eq!(ask(&registry, "Snapshot", "shop_id"), None);
    }

    #[test]
    fn a_name_that_may_hold_a_class_is_not_an_instance() {
        let source = "def t\n  klass = Snapshot\n  other = klass\n  @kind = klass.class\n  record = Snapshot.new\n  [1].each { |item| item }\nend\n";
        let methods = crate::runtime_src::parse_methods(source).unwrap();
        let mut names = crate::det::HashSet::default();
        for _ in 0..2 {
            class_bindings(&methods[0].body, &mut names);
        }
        for name in ["klass", "other", "@kind", "item"] {
            assert!(names.contains(&Symbol::from(name)), "{name}: {names:?}");
        }
        assert!(!names.contains(&Symbol::from("record")), "{names:?}");
    }
}
