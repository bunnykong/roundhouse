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
    for class in candidates {
        let mut sites = 0usize;
        visit_all(app, &mut |e| count_sites(e, &class, &mut sites));
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
                    sub.methods.push(retarget(m, &sub.name));
                }
            }
        }
        let mut next = 0usize;
        let mut clones: Vec<ClassId> = Vec::new();
        visit_all(app, &mut |e| rewrite_sites(e, &class, &mut next, &mut clones));
        for name in clones {
            app.library_classes.push(LibraryClass {
                name: name.clone(),
                is_module: false,
                parent: Some(class.clone()),
                parent_span: original.parent_span,
                includes: Vec::new(),
                include_spans: Vec::new(),
                methods: instance.iter().map(|m| retarget(m, &name)).collect(),
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
