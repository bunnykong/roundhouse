use roundhouse::ident::Symbol;
use roundhouse::ty::{Row, Ty};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

fn dag(depth: usize) -> Ty {
    if depth == 0 { return Ty::Int; }
    let child=Arc::new(dag(depth-1));
    Ty::Tuple { elems: vec![Ty::Array {elem:child.clone()},
        Ty::Hash {key:Arc::new(Ty::Str),value:child}].into() }
}
fn hash(t: &Ty)->u64 {
    let mut h=std::collections::hash_map::DefaultHasher::new();
    t.hash(&mut h);h.finish()
}
#[test]
fn independent_dags_equal_their_unfolded_json_and_keep_hash_contract() {
    let a=dag(8);let b=dag(8);
    assert_eq!(a,b);
    assert_eq!(serde_json::to_value(&a).unwrap(),serde_json::to_value(&b).unwrap());
    assert_eq!(hash(&a),hash(&b));
    let Ty::Tuple {mut elems}=b else {unreachable!()};
    elems[0]=Ty::Bool;
    assert_ne!(a,Ty::Tuple {elems});
    assert_eq!(a,dag(8));
}
#[test]
fn row_order_stays_equal_without_reordering_serialized_fields() {
    let a=Ty::Record {row:Row {fields:vec![(Symbol::from("a"),dag(6)),
        (Symbol::from("b"),Ty::Str)].into_iter().collect(),rest:None}};
    let b=Ty::Record {row:Row {fields:vec![(Symbol::from("b"),Ty::Str),
        (Symbol::from("a"),dag(6))].into_iter().collect(),rest:None}};
    assert_eq!(a,b);assert_eq!(hash(&a),hash(&b));
    assert_ne!(serde_json::to_string(&a).unwrap(),serde_json::to_string(&b).unwrap());
}
#[test]
fn substitution_reuses_unchanged_vectors_and_mutation_detaches_them() {
    let a=dag(20);let b=a.subst_self(&Ty::Str);
    let (Ty::Tuple {elems:x},Ty::Tuple {elems:mut y})=(a,b) else {unreachable!()};
    assert!(x.ptr_eq(&y));y[0]=Ty::Bool;
    assert!(!x.ptr_eq(&y));assert!(matches!(x[0],Ty::Array {..}));
    let t=Ty::Array {elem:Arc::new(Ty::SelfInstance)};
    assert_eq!(t.subst_self(&Ty::Str),Ty::Array {elem:Arc::new(Ty::Str)});
}

#[test]
fn persistent_comparisons_invalidate_unique_and_copied_mutations() {
    let mut a: roundhouse::shared::Shared<Vec<Ty>> = vec![dag(7),Ty::Str].into();
    let b: roundhouse::shared::Shared<Vec<Ty>> = vec![dag(7),Ty::Str].into();
    assert_eq!(a,b);
    assert_eq!(a,b);
    a[1]=Ty::Bool;
    assert_ne!(a,b);
    a[1]=Ty::Str;
    assert_eq!(a,b);
    let mut c=a.clone();
    c[1]=Ty::Int;
    assert_ne!(a,c);
    assert_eq!(a,b);
    let wire=serde_json::to_string(&a).unwrap();
    let mut decoded: roundhouse::shared::Shared<Vec<Ty>>=serde_json::from_str(&wire).unwrap();
    assert_eq!(decoded,a);
    decoded[1]=Ty::Nil;
    assert_ne!(decoded,a);
}

#[test]
fn nested_record_order_survives_interning_parent_payloads() {
    let a=Ty::Record {row:Row {fields:vec![(Symbol::from("a"),Ty::Int),(Symbol::from("b"),Ty::Str)].into_iter().collect(),rest:None}};
    let b=Ty::Record {row:Row {fields:vec![(Symbol::from("b"),Ty::Str),(Symbol::from("a"),Ty::Int)].into_iter().collect(),rest:None}};
    assert_eq!(a,b);
    let raw_b=serde_json::to_string(&b).unwrap();
    let keep=Ty::Tuple {elems:vec![a.clone()].into()};
    let nested=Ty::Tuple {elems:vec![b.clone()].into()};
    assert!(serde_json::to_string(&nested).unwrap().contains(&raw_b),"nested tuple changed record order");
    let keep_outer=Ty::Record {row:Row {fields:vec![(Symbol::from("r"),a)].into_iter().collect(),rest:None}};
    let outer=Ty::Record {row:Row {fields:vec![(Symbol::from("r"),b)].into_iter().collect(),rest:None}};
    assert!(serde_json::to_string(&outer).unwrap().contains(&raw_b),"nested record changed record order");
    std::hint::black_box((keep,keep_outer));
}

#[test]
fn independently_built_dags_share_their_canonical_vector_payloads() {
    let a=dag(22);let b=dag(22);
    let (Ty::Tuple {elems:x},Ty::Tuple {elems:y})=(&a,&b) else {unreachable!()};
    assert!(x.ptr_eq(y));assert_eq!(hash(&a),hash(&b));
}
