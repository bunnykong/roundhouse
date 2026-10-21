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
        opaque_keywords:opaque,sources:Vec::new(),span:crate::span::Span::synthetic()};
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
            opaque_arity:false,opaque_keywords:false,sources:Vec::new(),span:crate::span::Span::synthetic()});
    assert!(routes.iter().any(|(key,row)| key == &(id.clone(),Symbol::from("[]"),MethodReceiver::Instance)
        && row == &vec![Ty::unresolved()]), "{routes:?}");
}
