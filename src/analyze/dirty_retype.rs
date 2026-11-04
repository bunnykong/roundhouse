//! Call-graph dirty frontier for narrowed production retype rounds.

use std::collections::BTreeSet;
use crate::det::{HashMap, HashSet};

use crate::analyze::body::ClassInfo;
use crate::App;
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

/// Snapshot of inference the fixpoint refines: per-class instance/class
/// method return types plus `inferred_params`. Used for convergence
/// checks (`inference_matches`).
#[derive(Clone)]
pub(super) struct InferenceSig {
    pub instance: HashMap<ClassId, HashMap<Symbol, Ty>>,
    pub class_methods: HashMap<ClassId, HashMap<Symbol, Ty>>,
    pub params: HashMap<super::ParamKey, Vec<Ty>>,
}

/// Snapshot for dirty-frontier construction: the convergence sig plus
/// block-value methods. Kept separate so `block_value` cannot drift
/// into / out of [`super::Analyzer::inference_matches`].
#[derive(Clone)]
pub(super) struct DirtyHints {
    pub sig: InferenceSig,
    pub block_value: HashMap<ClassId, HashSet<Symbol>>,
}

/// The method names whose answer differs between a class's current
/// return table and the one the last typing pass was given. Both
/// directions: a name `fold_host_surfaces` took back off a module is
/// movement its callers have to see, same as a refinement. `None` for
/// `before` is a class the previous snapshot had never registered —
/// everything on it is new.
pub(super) fn moved_method_names(
    now: &HashMap<Symbol, Ty>,
    before: Option<&HashMap<Symbol, Ty>>,
) -> BTreeSet<Symbol> {
    let Some(before) = before else {
        return now.keys().cloned().collect();
    };
    let mut out: BTreeSet<Symbol> = BTreeSet::new();
    for (name, ty) in now {
        if before.get(name) != Some(ty) {
            out.insert(name.clone());
        }
    }
    for name in before.keys() {
        if !now.contains_key(name) {
            out.insert(name.clone());
        }
    }
    out
}

/// The classes a retype round actually has to walk: `None` for
/// "walk everything", `Some(set)` for the change frontier the
/// harvest just produced, closed over everything that reads it.
pub(super) fn dirty_classes_for_retype(
    app: &App,
    classes: &HashMap<ClassId, ClassInfo>,
    inferred_params: &HashMap<super::ParamKey, Vec<Ty>>,
    callers_by_target: &HashMap<(ClassId, Symbol), HashSet<ClassId>>,
    hints: &DirtyHints,
) -> Option<HashSet<ClassId>> {
    let prev = &hints.sig;
    let prev_block_value = &hints.block_value;
    let mut dirty: HashSet<ClassId> = HashSet::default();
    let mut moved_by_class: HashMap<ClassId, BTreeSet<Symbol>> = HashMap::default();
    for (id, cls) in classes {
        let mut names = moved_method_names(&cls.instance_methods, prev.instance.get(id));
        names.extend(moved_method_names(&cls.class_methods, prev.class_methods.get(id)));
        match prev_block_value.get(id) {
            Some(before) if before == &cls.block_value_methods => {}
            Some(before) => {
                names.extend(cls.block_value_methods.symmetric_difference(before).cloned())
            }
            None => names.extend(cls.block_value_methods.iter().cloned()),
        }
        if names.is_empty() {
            continue;
        }
        dirty.insert(id.clone());
        moved_by_class.entry(id.clone()).or_default().extend(names);
    }
    for (key, tys) in inferred_params {
        if prev.params.get(key) != Some(tys) {
            dirty.insert(key.0.clone());
        }
    }
    for key in prev.params.keys() {
        if !inferred_params.contains_key(key) {
            dirty.insert(key.0.clone());
        }
    }
    let budget =
        (app.models.len() + app.library_classes.len() + app.controllers.len()) / 2;
    if dirty.is_empty() || dirty.len() > budget {
        return None;
    }

    let mut children: HashMap<ClassId, Vec<ClassId>> = HashMap::default();
    for (id, cls) in classes {
        if let Some(parent) = &cls.parent {
            children
                .entry(parent.clone())
                .or_default()
                .push(id.clone());
        }
        for module in &cls.includes {
            children.entry(module.clone()).or_default().push(id.clone());
        }
    }

    let mut queue: Vec<ClassId> = moved_by_class.keys().cloned().collect();
    let mut qi = 0;
    while qi < queue.len() {
        let id = queue[qi].clone();
        qi += 1;
        let names = moved_by_class.get(&id).cloned().unwrap_or_default();
        for child in children.get(&id).cloned().unwrap_or_default() {
            // A `super` records no call site, so a child's own method of a
            // moved name, which may read the moved one through it, is no
            // caller of it: the child is dirty itself. The spread below
            // reaches a subclass anyway, but not a model or plain class
            // that includes a module.
            if classes.get(&child).is_some_and(|c| {
                names.iter().any(|n| c.source_instance_methods.contains(n) || c.source_class_methods.contains(n))
            }) {
                dirty.insert(child.clone());
            }
            let entry = moved_by_class.entry(child.clone()).or_default();
            let before = entry.len();
            entry.extend(names.iter().cloned());
            if entry.len() != before {
                queue.push(child);
            }
        }
    }
    // `foo.map` is typed from `each` (`enumerable_view`), with no `map` entry of its own to move.
    for names in moved_by_class.values_mut() {
        if names.contains(&Symbol::from("each")) {
            names.extend(crate::analyze::body::ENUMERABLE_METHODS.iter().map(|m| Symbol::from(*m)));
        }
    }
    for (id, names) in &moved_by_class {
        for name in names {
            if let Some(callers) = callers_by_target.get(&(id.clone(), name.clone())) {
                dirty.extend(callers.iter().cloned());
            }
        }
    }

    // Descendants and included modules share one worklist so a module
    // pulled in for concern-ivar seeding also dirties its other
    // includers (children). Separate passes left those siblings clean.
    //
    // Only its controller (and module) includers, though: the ivar
    // environment `concern_ivar_env_of` gives a module comes from the
    // controllers that include it, and a controller re-types the
    // concern's spliced actions against it (Phase B′). A model or plain
    // class that includes the module reads nothing of that environment;
    // what it reads of the module is the module's method table, and a
    // change there reaches it through the moved names above in the
    // round it happens. Dirtying every includer of a shared mixin pushed
    // the frontier over its budget whenever one of them moved, and every
    // round then re-typed the whole app.
    let controllers: HashSet<&ClassId> = app.controllers.iter().map(|c| &c.name).collect();
    let is_module = |id: &ClassId| classes.get(id).is_some_and(|c| c.is_module);
    let mut queue: Vec<ClassId> = dirty.iter().cloned().collect();
    let mut qi = 0;
    while qi < queue.len() {
        let id = queue[qi].clone();
        qi += 1;
        let module = is_module(&id);
        for child in children.get(&id).cloned().unwrap_or_default() {
            if module && !controllers.contains(&child) && !is_module(&child) {
                continue;
            }
            if dirty.insert(child.clone()) {
                queue.push(child);
            }
        }
        let mut cursor = Some(id);
        for _ in 0..32 {
            let Some(cid) = cursor else { break };
            let Some(cls) = classes.get(&cid) else { break };
            for module in &cls.includes {
                if dirty.insert(module.clone()) {
                    queue.push(module.clone());
                }
            }
            cursor = cls.parent.clone();
        }
    }

    // Seed sinks: `CurrentAttributes` is written FROM dirty callers via
    // class-level forwarders (`Current.session = x`), not as a caller of
    // a moved method. Keep those classes on every narrowed frontier so
    // write-site refinements reseed `@attr` / readers / forwarders.
    for id in &app.current_attribute_classes {
        dirty.insert(id.clone());
    }

    if std::env::var("RH_DEBUG_DIRTY").is_ok() {
        let mut names: Vec<&str> = dirty.iter().map(|c| c.0.as_str()).collect();
        names.sort();
        eprintln!("DBG dirty {}/{budget}: {names:?}", dirty.len());
    }
    (dirty.len() <= budget).then_some(dirty)
}
