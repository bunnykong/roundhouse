// Not left to the controller lowering alone: it runs after analysis, which then saw no method for a delegated name.
use crate::app::App;
use crate::dialect::{Action, ControllerBodyItem, LibraryClass, MethodDef, MethodReceiver, MethodVisibility, RenderTarget};
use crate::expr::{Expr, ExprNode};

pub(super) fn expand(app: &mut App) {
    let App { controllers, sources, .. } = app;
    for controller in controllers.iter_mut() {
        let delegates: Vec<Expr> = controller
            .body
            .iter()
            .filter_map(|item| match item {
                ControllerBodyItem::Unknown { expr, .. }
                    if matches!(&*expr.node, ExprNode::Send { recv: None, method, .. } if method.as_str() == "delegate") =>
                {
                    Some(expr.clone())
                }
                _ => None,
            })
            .collect();
        if delegates.is_empty() {
            continue;
        }
        let methods: Vec<MethodDef> = controller
            .body
            .iter()
            .filter_map(|item| match item {
                ControllerBodyItem::Action { action, .. } => Some(method_of(&controller.name, action)),
                _ => None,
            })
            .collect();
        let mut lc = LibraryClass {
            name: controller.name.clone(),
            is_module: false,
            parent: controller.parent.clone(),
            parent_span: controller.parent_span,
            includes: Vec::new(),
            include_spans: Vec::new(),
            methods,
            nullable_columns: Vec::new(),
            origin: None,
            constants: Vec::new(),
            conditional_definitions: Vec::new(),
            unknown_calls: delegates.clone(),
            class_ivar_initializers: Vec::new(),
        };
        let forwarders = super::delegate::expand_delegates(
            &lc.name,
            &lc.methods,
            &mut lc.unknown_calls,
            &[],
            &crate::det::HashSet::default(),
            None,
            &crate::det::HashSet::default(),
            sources,
            &[],
        );
        if forwarders.is_empty() {
            continue;
        }
        // Analysis needs these methods, while lowering needs the original
        // declarations to resolve definitions and alias snapshots in order.
        // Keep both; lowering recognizes the analysis copies by their span.
        for m in forwarders {
            let item = ControllerBodyItem::Action { action: action_of(m.clone()), leading_comments: Vec::new(), leading_blank_line: true };
            if m.visibility == MethodVisibility::Private {
                if !controller.body.iter().any(|i| matches!(i, ControllerBodyItem::PrivateMarker { .. })) {
                    controller.body.push(ControllerBodyItem::PrivateMarker { leading_comments: Vec::new(), leading_blank_line: true });
                }
                controller.body.push(item);
            } else {
                // Ahead of `private`: Rails defines the forwarder public.
                let at = controller
                    .body
                    .iter()
                    .position(|i| matches!(i, ControllerBodyItem::PrivateMarker { .. }))
                    .unwrap_or(controller.body.len());
                controller.body.insert(at, item);
            }
        }
    }
}

fn method_of(owner: &crate::ident::ClassId, action: &Action) -> MethodDef {
    let mut m = super::library_class::synth_attr_reader(owner, &action.name, MethodReceiver::Instance);
    m.kind = crate::dialect::AccessorKind::Method;
    m.params = action.formal_params();
    m.body = action.body.clone();
    m
}

fn action_of(m: MethodDef) -> Action {
    let mut params = crate::ty::Row::default();
    let mut fields = indexmap::IndexMap::new();
    for p in m.params.iter().filter(|p| !p.keyword && !p.rest) {
        fields.insert(p.name.clone(), crate::ty::Ty::Var { var: crate::ident::TyVar(0) });
    }
    params.fields = fields.into();
    Action {
        name: m.name,
        params,
        opt_params: Vec::new(),
        kw_params: Vec::new(),
        kwrest_param: None,
        block_param: m.block_param.as_ref().map(|b| b.name.clone()),
        rest_param: None,
        anonymous_formal: None,
        name_span: m.name_span,
        body: m.body,
        renders: RenderTarget::Inferred,
        effects: crate::effect::EffectSet::pure(),
    }
}
