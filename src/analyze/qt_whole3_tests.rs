use super::*;
use std::sync::Arc;

fn var() -> Ty { Ty::Var { var: crate::ident::TyVar(0) } }

#[test]
#[ignore = "RH_UNK_AT_SOURCE=1"]
fn unknown_at_source_canonicalizes_before_consumers_and_keeps_pending() {
    assert!(canon_var::source_on());
    let classes = HashMap::new();
    let mut expr = Expr::new(crate::span::Span::synthetic(), ExprNode::Var {
        id: crate::ident::VarId(1), name: Symbol::from("input"),
    });
    let ctx = body::Ctx { local_bindings: HashMap::from([(Symbol::from("input"),
        Ty::Hash {key:Arc::new(Ty::Sym),value:Arc::new(Ty::Union {variants:vec![var(),Ty::unresolved()].into()})})]),
        ..Default::default() };
    let result = body::BodyTyper::new(&classes).analyze_expr(&mut expr, &ctx);
    let Ty::Hash {value,..} = result else { panic!("hash") };
    assert_eq!(*value, Ty::unresolved());
    assert!(matches!(&*value, Ty::Untyped {why} if why.effective() == crate::ty::Provenance::Unresolved));
    assert!(strict_producers::is_p(&canon_var::produced(Ty::pending_untyped())));
    let mut empty = Expr::new(crate::span::Span::synthetic(),ExprNode::Array {
        elements:Vec::new(),style:Default::default()});
    assert_eq!(body::BodyTyper::new(&classes).analyze_expr(&mut empty,&ctx),
        Ty::Array {elem:Arc::new(Ty::Bottom)}, "an empty literal has no element values");
}

#[test]
#[ignore = "RH_CUR_REFRET=1 RH_FOLD=1 RH_FOLD_JOIN=1"]
fn current_reference_return_does_not_resurrect_superseded_unknown() {
    fold::reset(); handoff::reset();
    let class = ClassId(Symbol::from("CurrentRefret"));
    let method = Symbol::from("call");
    fold::add_rec_methods(vec![(class.clone(),method.clone())]);
    let mut classes = HashMap::from([(class.clone(),body::ClassInfo::default())]);
    classes.get_mut(&class).unwrap().instance_methods.insert(method.clone(),
        parameter_conservation::join(Ty::Str,Ty::unresolved()));
    handoff::join_rets(&mut classes);
    classes.get_mut(&class).unwrap().instance_methods.insert(method.clone(),Ty::Str);
    handoff::join_ret_one(&mut classes,&class,&method);
    handoff::join_rets(&mut classes);
    assert_eq!(classes[&class].instance_methods[&method],Ty::Str);
}

#[test]
#[ignore = "RH_UNK_AT_SOURCE=1 RH_ROW_CURRENT=1"]
fn omitted_keywords_are_absence_and_opaque_keywords_keep_real_unknown() {
    let shape = ParamShape { slots:vec![
        (Symbol::from("url"),ParamKind::Keyword {required:false}),
        (Symbol::from("title"),ParamKind::Keyword {required:false}),
        (Symbol::from("rest"),ParamKind::KeywordRest)],keywords_by_kind:true };
    let kw = |opaque| SiteKeywords {group:true,keys:vec![(Symbol::from("title"),Ty::Str),
        (Symbol::from("extra"),Ty::Bool)],splat:None,anonymous_forward:false,opaque_arity:false,
        opaque_keywords:opaque,positional_splats:vec![],class_body_site:false,sources:Vec::new(),span:crate::span::Span::synthetic()};
    let placed = Analyzer::place_keyword_args(Some(&shape),vec![Ty::unresolved()],kw(false));
    assert_eq!(placed[0],Ty::Bottom, "an omitted url is no incoming value");
    assert_eq!(parameter_conservation::join(Ty::Str,placed[0].clone()),Ty::Str);
    assert_eq!(placed[1],Ty::Str);
    assert_eq!(placed[2],Ty::Hash {key:Arc::new(Ty::Sym),value:Arc::new(Ty::Bool)});
    let opaque = Analyzer::place_keyword_args(Some(&shape),vec![Ty::unresolved()],kw(true));
    assert!(matches!(&opaque[0],Ty::Untyped {why} if why.effective()==crate::ty::Provenance::Unresolved));
}

#[test]
#[ignore = "RH_REPLAY_CLOSED_READS=1 RH_FOLD=1"]
fn completion_reference_reads_do_not_recreate_pending_and_keep_computed_bottom() {
    fold::reset(); completion_rerun::reset();
    assert!(strict_producers::is_p(&completion_rerun::unavailable_read()));
    completion_rerun::begin_completion();
    assert_eq!(completion_rerun::unavailable_read(),Ty::unresolved());
    let class = ClassId(Symbol::from("ClosedReference")); let method=Symbol::from("call");
    fold::add_rec_methods(vec![(class.clone(),method.clone())]);
    let reference=fold::ret_ref(&class,&method,false).unwrap();
    assert_eq!(fold::head(&reference,(1,1,2),&HashMap::new()),Some(Ty::unresolved()));
    assert_eq!(body::multiassign_target_ty(&Some(reference.clone()),0),Some(Ty::unresolved()),
        "the registry-free destructuring harvest cannot recreate Pending after completion");
    let mut info=body::ClassInfo::default();info.instance_methods.insert(method,Ty::Bottom);
    assert_eq!(fold::head(&reference,(1,1,2),&HashMap::from([(class,info)])),Some(Ty::Bottom));
}

#[test]
#[ignore = "RH_HWIA_ROUTES=1 RH_ROUTE_TOP=1 RH_WRITERS=1"]
fn hwia_structural_builtin_is_a_frozen_unknown_receiver_destination() {
    let app = App::new();
    let analyzer = Analyzer::new(&app);
    let id = ClassId(Symbol::from(indifferent::CLASS));
    assert!(!analyzer.classes.get(&id).is_some_and(|c| c.instance_methods.contains_key(&Symbol::from("[]"))),
        "HWIA's body builtin has no catalog method");
    assert!(route_static::hwia_on());
    let routes = route_static::Index::build(&app,&analyzer).resolve_top(&Symbol::from("[]"),1,
        &SiteKeywords {group:false, keys:Vec::new(),splat:None,anonymous_forward:false,
            opaque_arity:false,positional_splats:vec![],class_body_site:false,opaque_keywords:false,sources:Vec::new(),span:crate::span::Span::synthetic()});
    assert!(routes.iter().any(|(key,row)| key == &(id.clone(),Symbol::from("[]"),MethodReceiver::Instance)
        && row == &vec![Ty::unresolved()]), "{routes:?}");
}

#[test]
#[ignore = "whole-rule research flags + RH_CLASS_BODY_SITES=1"]
fn class_body_sites_reach_concern_declaration_rows() {
    assert!(class_body_sites_on());
    crate::ingest::survey::activate();
    let tree = HashMap::from([
        ("app/controllers/concerns/authentication.rb", "module Authentication\n extend ActiveSupport::Concern\n class_methods do\n def allow_unauthenticated_access(**options)\n skip_before_action :require_authentication, **options\n end\n end\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\n include Authentication\n allow_unauthenticated_access only: [:new]\nend\n"),
        ("app/controllers/sessions_controller.rb", "class SessionsController < ApplicationController\n allow_unauthenticated_access only: [:new, :create]\nend\n"),
        ("app/models/concerns/rate_limitable.rb", "module RateLimitable\n extend ActiveSupport::Concern\n class_methods do\n def rate_limit(options = {})\n options[:by]\n end\n end\nend\n"),
        ("app/models/follow_request.rb", "class FollowRequest < ActiveRecord::Base\n include RateLimitable\n rate_limit by: :account, family: :follows\nend\n"),
    ]).into_iter().map(|(p, s)| (std::path::PathBuf::from(p), s.as_bytes().to_vec())).collect();
    let mut app = crate::ingest::app::ingest_app_from_tree(tree).unwrap();
    assert!(!app.expanded_class_body_calls.is_empty(), "the fixture must cover a consumed macro call");
    let mut analyzer = Analyzer::new(&app);
    analyzer.analyze(&mut app);
    let row = |owner, method| {
        let declaration = app.library_classes.iter().find(|c| c.name.0.as_str() == owner).unwrap();
        let declaration = declaration.methods.iter().find(|m| m.name.as_str() == method).unwrap();
        analyzer.inferred_params.get(&(
            ClassId(Symbol::from(owner)), Symbol::from(method), declaration.receiver)).cloned()
    };
    let auth = row("Authentication", "allow_unauthenticated_access").expect("declaration row");
    assert_eq!(auth[0], Ty::Hash { key: Arc::new(Ty::Sym),
        value: Arc::new(Ty::Array { elem: Arc::new(Ty::Sym) }) });
    let rate = row("RateLimitable", "rate_limit").expect("declaration row");
    assert_eq!(rate[0], Ty::Hash { key: Arc::new(Ty::Sym), value: Arc::new(Ty::Sym) });
    let declaration = app.library_classes.iter().find(|c| c.name.0.as_str() == "RateLimitable").unwrap();
    let method = declaration.methods.iter().find(|m| m.name.as_str() == "rate_limit").unwrap();
    let answer = method.body.ty.as_ref().expect("typed declaration body");
    assert!(!matches!(answer, Ty::Bottom | Ty::Nil), "the declaration must read its class-body caller: {answer:?}");
}

#[test]
#[ignore = "whole-rule research flags + RH_CLASS_BODY_SITES=1 RH_ROUTE_PER_CLASS=1"]
fn per_class_routes_include_rows_owned_only_by_the_class_body_walk() {
    assert!(class_body_sites_on() && route_static::per_class_on());
    crate::ingest::survey::activate();
    let mut app = crate::ingest::app::ingest_app_from_tree(HashMap::from([
        (std::path::PathBuf::from("app/models/native_model.rb"),
            b"class NativeModel < ActiveRecord::Base\n dynamic_writer 'seed'\n def use(receiver)\n receiver.dynamic_writer(:incoming)\n end\nend\n".to_vec()),
    ])).unwrap();
    let mut analyzer = Analyzer::new(&app);
    analyzer.analyze(&mut app);
    let target = (ClassId(Symbol::from("NativeModel")), Symbol::from("dynamic_writer"), MethodReceiver::Instance);
    let row = analyzer.inferred_params.get(&target).expect("ordinary class-body row");
    assert!(matches!(&row[0], Ty::Untyped { .. }) || matches!(&row[0], Ty::Union { variants }
        if variants.iter().any(|ty| matches!(ty, Ty::Untyped { .. }))),
        "top routing must reach a row even when no captured method root declared its native destination: {row:?}");
}

#[test]
#[ignore = "whole-rule research flags + RH_CLASS_BODY_SITES=1"]
fn class_body_rest_binding_keeps_splat_and_trailing_hash() {
    assert!(class_body_sites_on());
    crate::ingest::survey::activate();
    let mut app = crate::ingest::app::ingest_app_from_tree(HashMap::from([
        (std::path::PathBuf::from("app/models/native_model.rb"),
            b"class NativeModel < ActiveRecord::Base\n def self.capture(*values)\n values\n end\n capture(*[:a], nested: [[:b]])\nend\n".to_vec()),
    ])).unwrap();
    let mut analyzer = Analyzer::new(&app);
    analyzer.analyze(&mut app);
    let target = (ClassId(Symbol::from("NativeModel")), Symbol::from("capture"), MethodReceiver::Class);
    let row = analyzer.inferred_params.get(&target).expect("class-body rest row");
    let nested = Ty::Hash { key: Arc::new(Ty::Sym),
        value: Arc::new(Ty::Array { elem: Arc::new(Ty::Array { elem: Arc::new(Ty::Sym) }) }) };
    assert_eq!(row, &vec![Ty::Array { elem: Arc::new(parameter_conservation::join(Ty::Sym, nested)) }]);
}
