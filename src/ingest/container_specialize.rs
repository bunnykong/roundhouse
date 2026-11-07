// Not one class for every use of a collection class: its contents would type as the union of what all of them hold.
use crate::app::App;
use crate::dialect::{LibraryClass, MethodDef, MethodReceiver};
use crate::expr::{Expr, ExprNode};
use crate::ident::{ClassId, Symbol};

pub(super) fn specialize(app: &mut App) {
    let models: Vec<ClassId> = app.models.iter().map(|m| m.name.clone()).collect();
    let candidates: Vec<ClassId> = app
        .library_classes
        .iter()
        .filter(|lc| !lc.is_module && !models.contains(&lc.name) && generic(lc))
        .map(|lc| lc.name.clone())
        .collect();
    // Not a walk of the whole app per candidate: one walk counts every
    // candidate's `new` sites, and the methods copied for a candidate add
    // theirs as they are made, so each count is what a fresh walk at that
    // candidate's turn would find. A name that is a candidate twice (a
    // reopened class) is counted afresh: its first turn rewrote its sites.
    let mut occurrences: crate::det::HashMap<&str, usize> = crate::det::HashMap::default();
    for c in &candidates {
        *occurrences.entry(c.0.as_str()).or_default() += 1;
    }
    let keys: crate::det::HashMap<String, usize> = candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| occurrences[c.0.as_str()] == 1 && !c.0.as_str().split("::").any(str::is_empty))
        .map(|(i, c)| (c.0.as_str().to_string(), i))
        .collect();
    let mut counts = vec![0usize; candidates.len()];
    visit_all(app, &mut |e| count_all_sites(e, &keys, &mut counts));
    for (ci, class) in candidates.iter().enumerate() {
        let class = class.clone();
        let sites = if keys.contains_key(class.0.as_str()) {
            counts[ci]
        } else {
            let mut fresh = 0usize;
            visit_all(app, &mut |e| count_sites(e, &class, &mut fresh));
            fresh
        };
        let subclasses: Vec<usize> = app
            .library_classes
            .iter()
            .enumerate()
            .filter(|(_, lc)| lc.parent.as_ref() == Some(&class))
            .map(|(i, _)| i)
            .collect();
        if sites + subclasses.len() < 2 {
            continue;
        }
        let Some(original) = app.library_classes.iter().find(|lc| lc.name == class).cloned() else { continue };
        let instance: Vec<MethodDef> =
            original.methods.iter().filter(|m| m.receiver == MethodReceiver::Instance).cloned().collect();
        for i in subclasses {
            let sub = &mut app.library_classes[i];
            for m in &instance {
                if !sub.methods.iter().any(|own| own.receiver == MethodReceiver::Instance && own.name == m.name) {
                    let mut copy = retarget(m, &sub.name);
                    count_method_sites(&mut copy, &keys, &mut counts);
                    sub.methods.push(copy);
                }
            }
        }
        let mut next = 0usize;
        let mut clones: Vec<ClassId> = Vec::new();
        visit_all(app, &mut |e| rewrite_sites(e, &class, &mut next, &mut clones));
        for name in clones {
            let mut methods: Vec<MethodDef> = instance.iter().map(|m| retarget(m, &name)).collect();
            for m in &mut methods {
                count_method_sites(m, &keys, &mut counts);
            }
            app.library_classes.push(LibraryClass {
                name: name.clone(),
                is_module: false,
                parent: Some(class.clone()),
                parent_span: original.parent_span,
                includes: Vec::new(),
                include_spans: Vec::new(),
                methods,
                nullable_columns: Vec::new(),
                origin: original.origin.clone(),
                constants: Vec::new(),
                conditional_definitions: Vec::new(),
                unknown_calls: Vec::new(),
                class_ivar_initializers: Vec::new(),
            });
        }
    }
}

// Not a class whose methods call `super` or read `self.class`: a copy's would reach the original or name the copy.
fn generic(lc: &LibraryClass) -> bool {
    let instance = || lc.methods.iter().filter(|m| m.receiver == MethodReceiver::Instance);
    let stores = instance().any(|m| {
        let params: Vec<&Symbol> = m.params.iter().filter(|p| !p.keyword && !p.rest).map(|p| &p.name).collect();
        !params.is_empty() && stores_param(&m.body, &params)
    });
    stores && instance().all(|m| !reads_own_class(&m.body))
}

fn stores_param(e: &Expr, params: &[&Symbol]) -> bool {
    let is_param = |a: &Expr| matches!(&*a.node, ExprNode::Var { name, .. } if params.contains(&name));
    if let ExprNode::Send { recv: Some(r), method, args, .. } = &*e.node
        && matches!(&*r.node, ExprNode::Ivar { .. })
    {
        let stored = match method.as_str() {
            "[]=" | "store" => args.len() == 2 && is_param(&args[1]),
            "<<" | "push" | "add" | "unshift" | "append" | "prepend" => args.iter().any(is_param),
            _ => false,
        };
        if stored {
            return true;
        }
    }
    let mut found = false;
    e.node.for_each_child(&mut |c| found = found || stores_param(c, params));
    found
}

fn reads_own_class(e: &Expr) -> bool {
    match &*e.node {
        ExprNode::Super { .. } => return true,
        ExprNode::Send { recv, method, args, .. }
            if method.as_str() == "class" && args.is_empty() && recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef)) =>
        {
            return true;
        }
        _ => {}
    }
    let mut found = false;
    e.node.for_each_child(&mut |c| found = found || reads_own_class(c));
    found
}

fn retarget(m: &MethodDef, owner: &ClassId) -> MethodDef {
    let mut m = m.clone();
    m.enclosing_class = Some(owner.0.clone());
    m
}

fn new_of<'e>(e: &'e mut Expr, class: &ClassId) -> Option<&'e mut Expr> {
    let ExprNode::Send { recv: Some(r), method, .. } = &mut *e.node else { return None };
    if method.as_str() != "new" {
        return None;
    }
    let ExprNode::Const { path } = &*r.node else { return None };
    let written: Vec<&str> = path.iter().map(|p| p.as_str()).filter(|p| !p.is_empty()).collect();
    let full: Vec<&str> = class.0.as_str().split("::").collect();
    (written == full).then_some(r)
}

// The parts of a method `visit_all` walks: its parameter defaults and body.
fn count_method_sites(m: &mut MethodDef, keys: &crate::det::HashMap<String, usize>, counts: &mut [usize]) {
    for p in &mut m.params {
        if let Some(default) = &mut p.default {
            count_all_sites(default, keys, counts);
        }
    }
    count_all_sites(&mut m.body, keys, counts);
}

fn count_all_sites(e: &mut Expr, keys: &crate::det::HashMap<String, usize>, counts: &mut [usize]) {
    if let ExprNode::Send { recv: Some(r), method, .. } = &*e.node
        && method.as_str() == "new"
        && let ExprNode::Const { path } = &*r.node
    {
        let written: Vec<&str> = path.iter().map(|p| p.as_str()).filter(|p| !p.is_empty()).collect();
        if let Some(&i) = keys.get(&written.join("::")) {
            counts[i] += 1;
        }
    }
    e.node.for_each_child_mut(&mut |c| count_all_sites(c, keys, counts));
}

fn count_sites(e: &mut Expr, class: &ClassId, sites: &mut usize) {
    if new_of(e, class).is_some() {
        *sites += 1;
    }
    e.node.for_each_child_mut(&mut |c| count_sites(c, class, sites));
}

fn rewrite_sites(e: &mut Expr, class: &ClassId, next: &mut usize, clones: &mut Vec<ClassId>) {
    if let Some(r) = new_of(e, class) {
        *next += 1;
        let name = ClassId(Symbol::from(format!("{}__{}", class.0.as_str(), next)));
        if let ExprNode::Const { path } = &mut *r.node {
            *path = name.0.as_str().split("::").map(Symbol::from).collect();
        }
        r.ty = None;
        // Not resolved from the source at its span: that text names the original class.
        r.decisions |= crate::expr::GENERATED_CONST_REF;
        clones.push(name);
    }
    e.node.for_each_child_mut(&mut |c| rewrite_sites(c, class, next, clones));
}

fn visit_all(app: &mut App, f: &mut impl FnMut(&mut Expr)) {
    crate::lower::for_each_hook_body(app, f);
    for view in &mut app.views {
        f(&mut view.body);
    }
    crate::lower::for_each_test_body(app, f);
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    fn library_class_names(files: &[(&str, &str)]) -> Vec<String> {
        let tree = files.iter().map(|(path, src)| (PathBuf::from(path), src.as_bytes().to_vec())).collect();
        let app = crate::ingest::ingest_app_from_tree(tree).expect("ingest");
        app.library_classes.iter().map(|lc| lc.name.0.as_str().to_string()).collect()
    }

    const BAG: &str = "class Bag\n  def initialize\n    @items = []\n  end\n\n  def add(item)\n    @items << item\n    self\n  end\nend\n";

    fn crate_with(method: &str) -> String {
        format!("class Crate\n  def initialize\n    @items = []\n  end\n\n  def add(item)\n    @items << item\n    self\n  end\n\n{method}end\n")
    }

    // `Bag.new` is written once, in Crate. Shelf inherits Crate's methods
    // as copies and `Crate.new` gets a copy of its own, so by Bag's turn
    // it has three sites, each its own copy of Bag.
    fn bags_after_crate_copies(method: &str) -> Vec<String> {
        library_class_names(&[
            ("app/lib/a_crate.rb", &crate_with(method)),
            ("app/lib/b_bag.rb", BAG),
            ("app/lib/c_shelf.rb", "class Shelf < Crate\nend\n"),
            ("app/lib/d_store.rb", "class Store\n  def self.run\n    Crate.new\n  end\nend\n"),
        ])
        .into_iter()
        .filter(|n| n.starts_with("Bag__"))
        .collect()
    }

    #[test]
    fn sites_in_bodies_copied_for_an_earlier_class_count() {
        let bags = bags_after_crate_copies("  def spare\n    Bag.new\n  end\n");
        assert_eq!(bags, ["Bag__1", "Bag__2", "Bag__3"]);
    }

    #[test]
    fn sites_in_parameter_defaults_copied_for_an_earlier_class_count() {
        let bags = bags_after_crate_copies("  def pick(bag = Bag.new)\n    bag\n  end\n");
        assert_eq!(bags, ["Bag__1", "Bag__2", "Bag__3"]);
    }

    // One written site and no subclass: nothing to tell apart.
    #[test]
    fn a_class_built_once_is_not_copied() {
        let names = library_class_names(&[
            ("app/lib/b_bag.rb", BAG),
            ("app/lib/d_store.rb", "class Store\n  def self.run\n    Bag.new\n  end\nend\n"),
        ]);
        assert!(!names.iter().any(|n| n.starts_with("Bag__")), "{names:?}");
    }
}
