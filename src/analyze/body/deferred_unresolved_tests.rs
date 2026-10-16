//! deferral-protocol's native probes exercise the actual producer paths in separate flag processes.
use super::*;
use crate::analyze::{Analyzer, current_returns, strict_producers as strict};
use crate::dialect::{MethodDef, MethodReceiver};
use crate::span::Span;
use crate::ty::Provenance;
use std::collections::BTreeMap;
use std::sync::Arc;

fn class(t: Option<&Ty>) -> &'static str {
    let Some(t) = t else { return "V" };
    if matches!(t, Ty::Var { .. }) { return "V"; }
    let reasons = crate::analyze::producer_evidence::origins(t);
    if reasons.iter().any(|r|r.ends_with(":Unresolved")) { return "U"; }
    if !reasons.is_empty() && reasons.iter().all(|r|r.ends_with(":Pending")) { return "P"; }
    "other"
}
fn nil() -> Expr { Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }) }
fn sym(s: &str) -> Symbol { Symbol::from(s) }
fn nominal(s: &str) -> Ty { Ty::Class { id: ClassId(sym(s)), args: vec![].into() } }
fn method() -> MethodDef {
    MethodDef { name:sym("m"),receiver:MethodReceiver::Instance,visibility:Default::default(),
        params:vec![serde_json::from_value(serde_json::json!({"name":"x"})).unwrap()],unsupported_formals:None,
        has_anonymous_block:false,block_param:None,name_span:Span::synthetic(),body:nil(),signature:None,
        effects:Default::default(),enclosing_class:None,kind:Default::default(),is_async:false,mutates_self:false }
}
fn producers() -> BTreeMap<&'static str, &'static str> {
    let var=Ty::Var { var:TyVar(0) };
    let classes=HashMap::from([(ClassId(sym("Missing")), ClassInfo {
        class_methods: HashMap::from([(sym("method_missing"),var.clone())]), ..Default::default() })]);
    let typer=BodyTyper::new(&classes);
    let mut sites=BTreeMap::new();
    let mut register=HashMap::new();
    Analyzer::register_method_return(&mut register,&sym("m"),Some(&var));
    sites.insert("register",class(register.get(&sym("m"))));
    let mut absent=HashMap::new();
    Analyzer::register_method_return(&mut absent,&sym("m"),None);
    assert_eq!(sites["register"],class(absent.get(&sym("m"))));
    let union=Ty::Union { variants:vec![var.clone(),Ty::Nil].into() };
    sites.insert("union_var",class(Some(&typer.dispatch_on(Some(&union),&sym("not_a_method"),None,&[],true))));
    sites.insert("conv_var",class(Some(&typer.dispatch_on(Some(&var),&sym("to_h"),None,&[],true))));
    let conv_a=typer.dispatch_on(Some(&var),&sym("to_a"),None,&[],true);
    assert_eq!(sites["conv_var"],class(Some(&conv_a)));
    sites.insert("use_zone",class(Some(&typer.dispatch_on(Some(&nominal("Time")),&sym("use_zone"),Some(&var),&[],false))));
    sites.insert("timeout",class(Some(&typer.dispatch_on(Some(&nominal("Timeout")),&sym("timeout"),Some(&var),&[],false))));
    sites.insert("method_missing",class(Some(&typer.dispatch_on(Some(&nominal("Missing")),&sym("not_a_method"),None,&[],false))));
    let mut arg=nil();arg.ty=Some(var.clone());
    sites.insert("jbuilder",class(Some(&super::send::jbuilder_value(Some(&arg)))));
    sites.insert("block_closure",class(super::send::object_protocol_method(None,&sym("class_eval"),Some(&var),true).as_ref()));
    let app=crate::App::default();let analyzer=Analyzer::new(&app);
    let ctx=analyzer.seed_method_params(&Ctx::default(),&ClassId(sym("Probe")),&method(),false);
    sites.insert("param_seed",class(ctx.local_bindings.get(&sym("x"))));
    let row=Row { fields:vec![(sym("x"),var.clone())].into_iter().collect(),rest:None };
    let action=analyzer.seed_action_params(&Ctx::default(),&ClassId(sym("Probe")),None,&sym("m"),&row,&[(sym("kw"),None)],None,None);
    assert_eq!(sites["param_seed"],class(action.local_bindings.get(&sym("x"))));
    assert_eq!(sites["param_seed"],class(action.local_bindings.get(&sym("kw"))));
    let mut ivar=Expr::new(Span::synthetic(),ExprNode::Ivar { name:sym("@x") });
    typer.analyze_expr(&mut ivar,&mut Ctx::default());
    sites.insert("ivar_read",class(ivar.ty.as_ref()));
    sites.insert("rs4",class(Some(&current_returns::boundary_normalize(var))));
    // The unmodeled/missing cases explicitly excluded by the card keep today's answers.
    assert_eq!(class(Some(&typer.dispatch_on(Some(&nominal("Timeout")),&sym("timeout"),None,&[],false))),"U");
    assert_eq!(super::send::jbuilder_value(None),Ty::Nil);
    assert_eq!(class(Some(&typer.dispatch_on(Some(&Ty::Date),&sym("to_h"),None,&[],true))),"V");
    sites
}

#[test]
#[ignore]
fn deferred_sites() {
    strict::deferred_reset();
    let s1=producers();strict::set_stratum(2);let stratum2=producers();strict::set_stratum(1);
    let sites:BTreeMap<_,_>=s1.iter().map(|(k,v)|(*k,serde_json::json!({"s1":v,"stratum2":stratum2[k]}))).collect();
    let mut app=crate::App::default();let mut analyzer=Analyzer::new(&app);let id=ClassId(sym("Probe"));
    analyzer.classes.insert(id.clone(),ClassInfo::default());
    for name in ["seen_none","seen_pending","skipped"] {
        analyzer.classes.get_mut(&id).unwrap().instance_methods.insert(sym(name),Ty::pending_untyped());
        analyzer.strict_stub_originals.push(((id.clone(),sym(name),MethodReceiver::Instance),Ty::unresolved()));
    }
    let mut none=method();none.name=sym("seen_none");none.body=Expr::new(Span::synthetic(),ExprNode::Var {id:crate::ident::VarId(0),name:sym("x")});
    let mut pending=none.clone();pending.name=sym("seen_pending");pending.body.ty=Some(Ty::pending_untyped());
        let mut library: crate::dialect::LibraryClass=serde_json::from_value(serde_json::json!({"name":id,"methods":[]})).unwrap();
    library.methods=vec![none,pending];app.library_classes.push(library);
    analyzer.harvest_returns_to_registry(&app,false);
    let snapshot=|analyzer:&Analyzer| ["seen_none","seen_pending","skipped"].into_iter()
        .map(|n|(n,class(analyzer.classes[&id].instance_methods.get(&sym(n))))).collect::<BTreeMap<_,_>>();
    let first=snapshot(&analyzer);analyzer.restore_strict_stubs();let boundary=snapshot(&analyzer);
    let restore=serde_json::json!({"after_first_harvest":first,"after_boundary":boundary});
    println!("DEFERRED-SITES {}",serde_json::json!({"sites":sites,"restore":restore}));
    for (site,answer) in &s1 {
        let today=if matches!(*site,"param_seed"|"ivar_read") {"V"} else {"U"};
        let deferred=strict::deferred_defer()=="all" || (strict::deferred_defer()=="stub" && *site=="register");
        assert_eq!(*answer,if deferred {"P"} else {today},"{site} s1");assert_eq!(stratum2[site],today,"{site} stratum2");
    }
    for answer in first.values() { assert_eq!(*answer,if strict::deferred_defer()=="0" {"U"} else {"P"}); }
    assert!(boundary.values().all(|v|*v=="U"));
}

fn arms(t: &Ty) -> usize { if let Ty::Union { variants }=t {variants.len()} else {1} }
fn arr(t: Ty) -> Ty { Ty::Array { elem:Arc::new(t) } }
#[test]
#[ignore]
fn deferred_ndedup() {
    let mut flat=vec![Ty::Str,Ty::Nil,Ty::Str,Ty::Nil];Ty::canonicalize_variants(&mut flat);
    let once=flat.clone();Ty::canonicalize_variants(&mut flat);
    let raw=Ty::Union { variants:vec![Ty::Str,Ty::Nil,Ty::Str,Ty::Nil].into() };
    let hash=Ty::Hash { key:Arc::new(Ty::Str),value:Arc::new(raw) };
    let witness=join_ivar_slot(hash.clone(),hash);
    let count=if let Ty::Hash {value,..}=&witness {arms(value)} else {panic!("{witness:?}")};
    let mut mixed=vec![arr(Ty::gradual()),arr(Ty::unresolved()),Ty::Nil];Ty::canonicalize_variants(&mut mixed);
    let both=Ty::untyped(Provenance::Gradual.union(Provenance::Unresolved));
    let mut expected=vec![arr(both),Ty::Nil];Ty::canonicalize_variants(&mut expected);
    let mut pending=vec![arr(Ty::gradual()),arr(Ty::pending_untyped())];Ty::canonicalize_variants(&mut pending);
    let pending_ok=if strict::deferred_ndedup() { matches!(&pending[0],Ty::Array {elem} if matches!(&**elem,Ty::Untyped {why} if why.reporting_bits()==3)) } else {true};
    let preserve=if strict::deferred_ndedup() {crate::wire_ty::storage_types(&mixed,&expected) && pending_ok} else {true};
    let idempotent=crate::wire_ty::storage_types(&flat,&once);
    println!("DEFERRED-NDEDUP {}",serde_json::json!({"flat_arms":flat.len(),"hash_value_arms":count,
        "idempotent":idempotent,"mixed_provenance_preserved":preserve}));
    let want=if strict::deferred_ndedup() {2} else {4};
    assert_eq!(flat.len(),want);assert_eq!(count,want,"{witness:?}");assert!(idempotent);assert!(preserve);
}
