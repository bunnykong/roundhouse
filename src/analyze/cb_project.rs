//! P5 controller bindings: a projection of completed, expanded body values.
//! No refinement history or cleanliness gate participates in this table.

use super::*;

pub(super) fn on() -> bool {
    static ON: std::sync::LazyLock<bool> = std::sync::LazyLock::new(||
        std::env::var("RH_CB_PROJECT").as_deref() == Ok("1"));
    *ON
}

pub(super) fn project(app: &App) -> HashMap<(ClassId, Symbol), HashMap<Symbol, Ty>> {
    let models = ivar_set::models_by_conventional_ivar(app.models.iter().map(|m| &m.name));
    let controllers: HashMap<_, _> = app.controllers.iter().map(|c| (&c.name, c)).collect();
    let modules: HashMap<_, _> = app.library_classes.iter().filter(|c| c.is_module)
        .map(|c| (&c.name, c)).collect();
    let mut projected = HashMap::new();
    for controller in &app.controllers {
        let mut chain = vec![controller];
        let mut seen = BTreeSet::from([controller.name.clone()]);
        while let Some(parent) = chain.last().and_then(|c| c.parent.as_ref()) {
            // Match Phase B's lexical parent lookup for nested controllers.
            let child = &chain.last().unwrap().name;
            let mut resolved = parent.clone();
            if !controllers.contains_key(parent) && !parent.0.as_str().contains("::") {
                let mut scope: Vec<_> = child.0.as_str().split("::").collect();
                scope.pop();
                while !scope.is_empty() {
                    let candidate = ClassId(Symbol::from(format!("{}::{}", scope.join("::"), parent.0.as_str())));
                    if controllers.contains_key(&candidate) { resolved = candidate; break; }
                    scope.pop();
                }
            }
            let parent = &resolved;
            if !seen.insert(parent.clone()) { break; }
            let Some(parent) = controllers.get(parent) else { break };
            chain.push(parent);
        }
        chain.reverse();
        // Phase A's writer universe also contains mixed-in module methods.
        // In particular, ingest represents a
        // module_function by its class-side copy, although Ruby retains a
        // private instance copy. It can write an includer's ivar without
        // ever becoming a controller Action. Read the FINAL module bodies,
        // not Phase A's provisional clones or the retained CB/cache rows.
        let own_bodies: Vec<HashMap<Symbol, &Expr>> = chain.iter().map(|owner| {
            let mut bodies: HashMap<_, _> = owner.actions()
                .map(|a| (a.name.clone(), &a.body)).collect();
            let mut mixed_in = controller_includes(owner);
            let mut seen: BTreeSet<_> = mixed_in.iter().cloned().collect();
            let mut i = 0;
            while i < mixed_in.len() {
                if let Some(module) = modules.get(&mixed_in[i]) {
                    for method in &module.methods {
                        bodies.entry(method.name.clone()).or_insert(&method.body);
                    }
                    for nested in &module.includes {
                        if seen.insert(nested.clone()) { mixed_in.push(nested.clone()); }
                    }
                }
                i += 1;
            }
            bodies
        }).collect();
        let mut bodies: HashMap<Symbol, &Expr> = HashMap::new();
        for own in &own_bodies {
            bodies.extend(own.iter().map(|(name, body)| (name.clone(), *body)));
        }
        let lookup = |name: &Symbol| bodies.get(name).copied();
        let env = ivar_set::IvarNameEnv {
            self_class: Some(&controller.name),
            owned: None,
            lookup: Some(&lookup),
            models_by_ivar: Some(&models),
        };
        // Reproduce the existing nearest-owner/super layering, reading
        // final bodies rather than Phase A/B caches or earlier CB rows.
        let mut chained: HashMap<Symbol, HashMap<Symbol, Ty>> = HashMap::new();
        for own in &own_bodies {
            for (name, body) in own {
                let mut bindings = HashMap::new();
                extract_ivar_assignments_in(body, &mut bindings, &env);
                if expr_calls_super(body) {
                    let mut inherited = chained.remove(name).unwrap_or_default();
                    inherited.extend(bindings);
                    bindings = inherited;
                }
                chained.insert(name.clone(), bindings);
            }
        }
        // CB already includes receiverless helper writes reached by
        // Before/Around targets. Keep that traversal and its nilability.
        // Only the target/kind structure is read; cached assigns are not.
        let filters: Vec<&Filter> = match app.controller_resolutions.get(&controller.name) {
            Some(resolution) => resolution.filter_chain.iter().map(|f| &f.filter).collect(),
            None => chain.iter().flat_map(|c| c.filters()).collect(),
        };
        for filter in filters {
            if !matches!(filter.kind, FilterKind::Before | FilterKind::Around) { continue; }
            let Some(body) = bodies.get(&filter.target) else { continue };
            let mut visited = BTreeSet::from([filter.target.clone()]);
            let transitive = collect_transitive_filter_ivars(
                body, &bodies, MAX_FILTER_CALL_DEPTH, &mut visited, true);
            union_ivar_maps(chained.entry(filter.target.clone()).or_default(), transitive);
        }
        // An inherited method remains attributed to its owner, as before.
        for action in controller.actions() {
            let bindings = chained.remove(&action.name).unwrap_or_default();
            if !bindings.is_empty() {
                projected.insert((controller.name.clone(), action.name.clone()), bindings);
            }
        }
    }
    projected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_bindings_keep_unknown_and_replace_old_contents() {
        crate::ingest::survey::activate();
        let mut app = crate::ingest::app::ingest_app_from_tree(HashMap::from([
            (std::path::PathBuf::from("app/controllers/projection_controller.rb"),
             b"class ProjectionController < ApplicationController\n  def show\n    @article = missing\n    @empty = nil\n  end\nend\n".to_vec()),
        ])).unwrap();
        assert_eq!(app.controllers.len(), 1);
        let controller = &mut app.controllers[0];
        for action in controller.actions_mut() {
            fn stamp(e: &mut Expr) {
                if let ExprNode::Assign { value, .. } = &mut *e.node {
                    value.ty = Some(if matches!(&*value.node, ExprNode::Lit { .. }) {
                        Ty::Nil
                    } else { Ty::unresolved() });
                }
                e.node.for_each_child_mut(&mut stamp);
            }
            stamp(&mut action.body);
        }
        let key = (controller.name.clone(), Symbol::from("show"));
        let result = project(&app);
        assert_eq!(result[&key][&Symbol::from("article")], Ty::unresolved());
        assert_eq!(result[&key][&Symbol::from("empty")], Ty::Nil);
        assert_eq!(result[&key].len(), 2);
        app.controllers.clear();
        assert!(project(&app).is_empty(), "projection carries no earlier bindings");
    }

    #[test]
    fn projection_keeps_callback_helper_and_super_writes() {
        crate::ingest::survey::activate();
        let mut app = crate::ingest::app::ingest_app_from_tree(HashMap::from([
            (std::path::PathBuf::from("app/controllers/parent_controller.rb"), br#"
class ParentController < ApplicationController
  def show; @parent = nil; end
  def set_auth; @authenticated_by = nil; end
end
"#.to_vec()),
            (std::path::PathBuf::from("app/controllers/projection_controller.rb"), br#"
class ProjectionController < ParentController
  before_action :authenticate
  def authenticate; set_auth; end
  def show; super; @child = nil; end
end
"#.to_vec()),
        ])).unwrap();
        assert_eq!(app.controllers.len(), 2);
        fn stamp(e: &mut Expr) {
            if let ExprNode::Assign { value, .. } = &mut *e.node { value.ty = Some(Ty::Nil); }
            e.node.for_each_child_mut(&mut stamp);
        }
        for c in &mut app.controllers { for a in c.actions_mut() { stamp(&mut a.body); } }
        let result = project(&app);
        let owner = ClassId(Symbol::from("ProjectionController"));
        assert_eq!(result[&(owner.clone(), Symbol::from("authenticate"))]
            [&Symbol::from("authenticated_by")], Ty::Nil);
        let show = &result[&(owner, Symbol::from("show"))];
        assert_eq!(show[&Symbol::from("parent")], Ty::Nil);
        assert_eq!(show[&Symbol::from("child")], Ty::Nil);
    }

    #[test]
    fn module_function_instance_writes_use_final_claims_through_nested_includes() {
        crate::ingest::survey::activate();
        let mut app = crate::ingest::app::ingest_app_from_tree(HashMap::from([
            (std::path::PathBuf::from("app/controllers/concerns/projection_config.rb"), br#"
module ProjectionConfig
  module_function
  def complete?; missing_configs.empty?; end
  def missing_configs; @missing_configs ||= lookup; end
end
"#.to_vec()),
            (std::path::PathBuf::from("app/controllers/concerns/projection_helpers.rb"), br#"
module ProjectionHelpers
  extend ActiveSupport::Concern
  include ProjectionConfig
  def verify_setup_completed; complete?; end
end
"#.to_vec()),
            (std::path::PathBuf::from("app/controllers/projection_controller.rb"), br#"
class ProjectionController < ApplicationController
  include ProjectionHelpers
  before_action :verify_setup_completed
end
"#.to_vec()),
        ])).unwrap();
        let controller = app.controllers.iter().find(|c| c.name.0.as_str() == "ProjectionController").unwrap();
        assert!(!controller.actions().any(|a| a.name.as_str() == "missing_configs"),
            "the Ruby instance copy is absent from controller actions");
        let module = app.library_classes.iter().find(|c| c.name.0.as_str() == "ProjectionConfig").unwrap();
        assert!(module.methods.iter().any(|m| m.name.as_str() == "missing_configs"
            && m.receiver == MethodReceiver::Class));
        fn stamp(expr: &mut Expr, ty: &Ty) {
            match &mut *expr.node {
                ExprNode::Assign { value, .. } | ExprNode::OpAssign { value, .. } => {
                    value.ty = Some(ty.clone());
                }
                _ => {}
            }
            expr.node.for_each_child_mut(&mut |e| stamp(e, ty));
        }
        let key = (controller.name.clone(), Symbol::from("verify_setup_completed"));
        let binding = Symbol::from("missing_configs");
        for final_ty in [Ty::Str, Ty::Int] {
            for module in &mut app.library_classes {
                for method in &mut module.methods { stamp(&mut method.body, &final_ty); }
            }
            let expected = final_ty;
            let result = project(&app);
            assert_eq!(result[&key][&binding], expected,
                "the final writer replaces provisional or earlier claims");
            app.library_classes.reverse();
            assert_eq!(project(&app), result, "module enumeration is not a schedule");
        }
    }
}
