//! Bound-entrance and cross-harvest checks, without a new widening policy.
use std::collections::{BTreeSet, HashMap};

use super::{MAX_DEPTH, MAX_NODES, body, bound, measure, writer_law_tests};
use crate::analyze::{IvarWalk, ivar_set::IvarNameEnv, writer_laws as laws};
use crate::expr::{Expr, ExprNode, InterpPart, LValue, Literal};
use crate::ident::Symbol;
use crate::ty::Ty;

const ORDERS: [[usize; 3]; 6] = [
    [0, 1, 2],
    [0, 2, 1],
    [1, 0, 2],
    [1, 2, 0],
    [2, 0, 1],
    [2, 1, 0],
];

pub(super) fn check_sampled_membership(input: &Ty, output: &Ty) -> usize {
    let witnesses = values(input);
    for value in &witnesses {
        assert!(accepts(input, value), "invalid finite witness");
        assert!(accepts(output, value), "container entrance lost a sampled value: {value:?}");
    }
    witnesses.len()
}

fn plain(t: Ty) -> Expr {
    laws::expr(ExprNode::Assign {
        target: LValue::Ivar {
            name: Symbol::from("x"),
        },
        value: laws::stamped(t),
    })
}

fn name_literal(symbol: bool) -> Expr {
    laws::expr(ExprNode::Lit {
        value: if symbol {
            Literal::Sym {
                value: Symbol::from("@x"),
            }
        } else {
            Literal::Str { value: "@x".into() }
        },
    })
}

fn reflective(t: Ty, name: Expr, explicit_self: bool) -> Expr {
    laws::expr(ExprNode::Send {
        recv: explicit_self.then(|| laws::expr(ExprNode::SelfRef)),
        method: Symbol::from("instance_variable_set"),
        args: vec![name, laws::stamped(t)].into(),
        block: None,
        parenthesized: false,
    })
}

fn sequence(exprs: Vec<Expr>) -> Expr {
    laws::expr(ExprNode::Seq { exprs })
}

// Independent finite Ruby value shapes. No production join, bound, fits, cut,
// Ty equality, renderer or serializer is used to decide membership. Object
// shapes check nominal class identity; Fn contracts and generic interiors
// are outside this finite witness checker.
#[derive(Clone, Debug)]
enum Value {
    Nil,
    Bool,
    Int,
    Float,
    Str,
    Sym,
    Date,
    Time,
    Object(String),
    Array(Vec<Value>),
    Hash(Vec<(Value, Value)>),
}

fn values(t: &Ty) -> Vec<Value> {
    match t {
        Ty::Nil => vec![Value::Nil],
        Ty::Bool => vec![Value::Bool],
        Ty::Int => vec![Value::Int],
        Ty::Float => vec![Value::Float],
        Ty::Str => vec![Value::Str],
        Ty::Sym => vec![Value::Sym],
        Ty::Date => vec![Value::Date],
        Ty::Time => vec![Value::Time],
        Ty::Class { id, .. } => vec![Value::Object(id.0.as_str().into())],
        Ty::Untyped { .. } | Ty::Var { .. } => vec![
            Value::Nil,
            Value::Int,
            Value::Str,
            Value::Array(vec![Value::Bool]),
            Value::Hash(vec![(Value::Str, Value::Int)]),
        ],
        Ty::Array { elem } => std::iter::once(Value::Array(vec![]))
            .chain(values(elem).into_iter().map(|v| Value::Array(vec![v])))
            .collect(),
        Ty::Hash { key, value } => {
            let mut out = vec![Value::Hash(vec![])];
            for k in values(key) {
                for v in values(value) {
                    out.push(Value::Hash(vec![(k.clone(), v)]));
                }
            }
            out
        }
        Ty::Tuple { elems } => {
            let choices: Vec<_> = elems.iter().map(values).collect();
            if choices.iter().any(Vec::is_empty) {
                return vec![];
            }
            let first: Vec<_> = choices.iter().map(|v| v[0].clone()).collect();
            let mut out = vec![Value::Array(first.clone())];
            for (i, choices) in choices.iter().enumerate() {
                for choice in choices.iter().skip(1) {
                    let mut value = first.clone();
                    value[i] = choice.clone();
                    out.push(Value::Array(value));
                }
            }
            out
        }
        // Named record keys are represented separately from arbitrary String.
        // Record/Fn/Relation witnesses need the runtime exporter's contracts.
        Ty::Record { .. } | Ty::Fn { .. } | Ty::Relation { .. } | Ty::SelfInstance | Ty::Rec { .. } | Ty::Bottom => {
            vec![]
        }
        Ty::Union { variants } => variants.iter().flat_map(values).collect(),
    }
}

fn accepts(t: &Ty, value: &Value) -> bool {
    match (t, value) {
        (Ty::Untyped { .. } | Ty::Var { .. }, _) => true,
        (Ty::Nil, Value::Nil)
        | (Ty::Bool, Value::Bool)
        | (Ty::Int, Value::Int)
        | (Ty::Float, Value::Float)
        | (Ty::Str, Value::Str)
        | (Ty::Sym, Value::Sym)
        | (Ty::Date, Value::Date)
        | (Ty::Time, Value::Time) => true,
        (Ty::Class { id, .. }, Value::Object(name)) => id.0.as_str() == name,
        (Ty::Array { elem }, Value::Array(elems)) => elems.iter().all(|v| accepts(elem, v)),
        (Ty::Tuple { elems }, Value::Array(items)) => {
            elems.len() == items.len() && elems.iter().zip(items).all(|(t, v)| accepts(t, v))
        }
        (Ty::Hash { key, value }, Value::Hash(items)) => items
            .iter()
            .all(|(k, v)| accepts(key, k) && accepts(value, v)),
        (Ty::Union { variants }, value) => variants.iter().any(|t| accepts(t, value)),
        _ => false,
    }
}

fn harvest_partition(
    writers: &[Ty; 3],
    order: [usize; 3],
    splits: usize,
    reflect: bool,
    out: &mut HashMap<Symbol, Ty>,
) {
    let mut batch = Vec::new();
    for (position, i) in order.into_iter().enumerate() {
        batch.push(if reflect {
            reflective(writers[i].clone(), name_literal(true), false)
        } else {
            plain(writers[i].clone())
        });
        if position == 2 || splits & (1 << position) != 0 {
            crate::analyze::extract_ivar_assignments(&sequence(std::mem::take(&mut batch)), out);
        }
    }
}

#[test]
#[ignore = "known: bounding separate body harvests/rounds can disagree with one complete batch"]
fn writer_law_bound_partitions() {
    let name = Symbol::from("x");
    let mut report = laws::Report::default();
    let (mut schedules, mut membership, mut differences, mut replay_differences) = (0, 0, 0, 0);
    let mut saved_splits = BTreeSet::new();
    for writers in writer_law_tests::writer_triples() {
        let expected = bound(
            writers
                .iter()
                .cloned()
                .fold(laws::pending(), body::join_ivar_slot),
        );
        let possible: Vec<_> = writers.iter().flat_map(values).collect();
        for order in ORDERS {
            let inputs: Vec<_> = order.iter().map(|&i| &writers[i]).collect();
            for splits in 0..4 {
                let mut plain_out = HashMap::new();
                let mut reflect_out = HashMap::new();
                harvest_partition(&writers, order, splits, false, &mut plain_out);
                harvest_partition(&writers, order, splits, true, &mut reflect_out);
                let first = plain_out[&name].clone();
                for round in 1..=2 {
                    let plain = plain_out[&name].clone();
                    let reflected = reflect_out[&name].clone();
                    assert_eq!(
                        format!("{plain:?}"),
                        format!("{reflected:?}"),
                        "native storage shape"
                    );
                    report.equal(
                        "plain-reflective-storage",
                        &inputs,
                        plain.clone(),
                        reflected.clone(),
                    );
                    report.equal(
                        "partition-vs-complete/plain",
                        &inputs,
                        expected.clone(),
                        plain.clone(),
                    );
                    report.equal(
                        "partition-vs-complete/reflective",
                        &inputs,
                        expected.clone(),
                        reflected.clone(),
                    );
                    if round == 2 {
                        report.equal(
                            "duplicate-round-replay",
                            &inputs,
                            first.clone(),
                            plain.clone(),
                        );
                        replay_differences += usize::from(first != plain);
                    }
                    differences += usize::from(expected != plain);
                    if expected != plain && saved_splits.insert((splits, round)) {
                        println!(
                            "REFLECTIVE-ENTRANCE-PARTITION {}",
                            serde_json::json!({"splits": splits,
                            "round": round, "order": order, "inputs": writers,
                            "complete": expected, "plain": plain, "reflective": reflected})
                        );
                    }
                    for value in &possible {
                        let retained = accepts(&plain, value) && accepts(&reflected, value);
                        report.equal(
                            "finite-value-inclusion",
                            &inputs,
                            Ty::Bool,
                            if retained { Ty::Bool } else { Ty::Bottom },
                        );
                        membership += 1;
                    }
                    for stored in [&plain, &reflected] {
                        let (depth, nodes) = measure(stored);
                        assert!(depth <= MAX_DEPTH && nodes <= MAX_NODES);
                    }
                    schedules += 1;
                    if round == 1 {
                        harvest_partition(&writers, order, splits, false, &mut plain_out);
                        harvest_partition(&writers, order, splits, true, &mut reflect_out);
                    }
                }
            }
        }
    }
    assert!(schedules > 0 && membership > 0);
    println!(
        "REFLECTIVE-ENTRANCE-METRIC {}",
        serde_json::json!({"partition_round_schedules": schedules,
        "partition_differences": differences, "duplicate_round_differences": replay_differences,
        "finite_membership_checks": membership})
    );
    report.finish("extract_ivar_assignments/body-harvest-partitions");
}

// S4b observer: continue every original schedule after independent law failures.
// No production operation, writer triple, order, split, round or reference changed.
#[test]
#[ignore = "explicit exhaustive S4b reproduction"]
fn writer_partition_partition_census() {
    let name = Symbol::from("x");
    let mut counts = std::collections::BTreeMap::<String,usize>::new();
    let mut schedules = 0usize;
    let mut membership = 0usize;
    for (case,writers) in writer_law_tests::writer_triples().into_iter().enumerate() {
        let expected = bound(writers.iter().cloned().fold(laws::pending(), body::join_ivar_slot));
        let possible: Vec<_> = writers.iter().flat_map(values).collect();
        let depth = [0,1,2,10,MAX_DEPTH-1,MAX_DEPTH,MAX_DEPTH+1][case/98];
        let n = [1,2,254,255,256,257,300][(case%98)/14];
        let m = [1,2,254,255,256,257,300][(case%14)/2];
        let hash = case%2 == 1;
        for order in ORDERS {
            for splits in 0..4 {
                let mut plain_out=HashMap::new();
                let mut reflect_out=HashMap::new();
                harvest_partition(&writers,order,splits,false,&mut plain_out);
                harvest_partition(&writers,order,splits,true,&mut reflect_out);
                let first_plain=plain_out[&name].clone();
                let first_reflective=reflect_out[&name].clone();
                for round in 1..=2 {
                    let plain=plain_out[&name].clone();
                    let reflected=reflect_out[&name].clone();
                    let (pd,pn)=measure(&plain);
                    let (rd,rn)=measure(&reflected);
                    let mut failures=Vec::new();
                    for (law,fail) in [
                        ("partition_plain",expected!=plain),
                        ("partition_reflective",expected!=reflected),
                        ("replay_plain",round==2 && first_plain!=plain),
                        ("replay_reflective",round==2 && first_reflective!=reflected),
                        ("plain_reflective_native",format!("{plain:?}")!=format!("{reflected:?}")),
                        ("plain_bound",pd>MAX_DEPTH || pn>MAX_NODES),
                        ("reflective_bound",rd>MAX_DEPTH || rn>MAX_NODES),
                    ] {
                        counts.entry(law.to_string()).or_default();
                        if fail {
                            *counts.get_mut(law).unwrap()+=1;
                            failures.push(law);
                        }
                    }
                    for value in &possible {
                        assert!(accepts(&writers[0],value) || accepts(&writers[1],value) || accepts(&writers[2],value),"invalid finite witness");
                        for (law,fail) in [("membership_plain",!accepts(&plain,value)),("membership_reflective",!accepts(&reflected,value))] {
                            counts.entry(law.to_string()).or_default();
                            if fail { *counts.get_mut(law).unwrap()+=1; }
                        }
                        membership+=1;
                    }
                    if !failures.is_empty() {
                        println!("WRITER-PARTITION-CELL {}",serde_json::json!({
                            "case":case,"depth":depth,"n":n,"m":m,"hash":hash,
                            "order":order,"splits":splits,"round":round,"failures":failures,
                            "expected":laws::show(&expected),"plain":laws::show(&plain),
                            "reflective":laws::show(&reflected),
                            "plain_measure":[pd,pn],"reflective_measure":[rd,rn],
                        }));
                    }
                    schedules+=1;
                    if round==1 {
                        harvest_partition(&writers,order,splits,false,&mut plain_out);
                        harvest_partition(&writers,order,splits,true,&mut reflect_out);
                    }
                }
            }
        }
    }
    assert_eq!(schedules,32928);
    assert_eq!(membership,418656);
    println!("WRITER-PARTITION-SUMMARY {}",serde_json::json!({"counts":counts,"schedules":schedules,"membership_checks":membership}));
}

// A short native, batch-by-batch trace of rows selected from the exhaustive census.
#[test]
#[ignore = "requires WRITER_PARTITION_TRACE_SPEC"]
fn writer_partition_partition_trace() {
    let path=std::env::var("WRITER_PARTITION_TRACE_SPEC").expect("WRITER_PARTITION_TRACE_SPEC receipt path");
    let specs:serde_json::Value=serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let triples=writer_law_tests::writer_triples();
    let name=Symbol::from("x");
    for spec in specs.as_array().unwrap() {
        let case=spec["case"].as_u64().unwrap() as usize;
        let order:[usize;3]=std::array::from_fn(|i|spec["order"][i].as_u64().unwrap() as usize);
        let splits=spec["splits"].as_u64().unwrap() as usize;
        let writers=&triples[case];
        let expected=bound(writers.iter().cloned().fold(laws::pending(),body::join_ivar_slot));
        for reflect in [false,true] {
            let mut out=HashMap::new();
            let mut first=None;
            for round in 1..=2 {
                let mut batch=Vec::new();
                for (position,i) in order.into_iter().enumerate() {
                    batch.push(if reflect {reflective(writers[i].clone(),name_literal(true),false)} else {plain(writers[i].clone())});
                    if position==2 || splits & (1<<position)!=0 {
                        let before=out.get(&name).map(laws::show);
                        let expr=sequence(std::mem::take(&mut batch));
                        let mut raw=out.clone();
                        let mut walk=IvarWalk::default();
                        crate::analyze::walk_ivar_assignments(&expr,&mut raw,&IvarNameEnv::NONE,&mut walk);
                        let raw_ty=raw[&name].clone();
                        crate::analyze::extract_ivar_assignments(&expr,&mut out);
                        let stored=&out[&name];
                        println!("WRITER-PARTITION-TRACE {}",serde_json::json!({
                            "class":spec["class"],"case":case,"order":order,"splits":splits,"round":round,
                            "reflective":reflect,"batch_end":position,"before":before,
                            "raw":laws::show(&raw_ty),"raw_measure":measure(&raw_ty),
                            "bound_raw":laws::show(&bound(raw_ty)),"stored":laws::show(stored),
                            "stored_measure":measure(stored),"expected":laws::show(&expected),
                        }));
                    }
                }
                let stored=out[&name].clone();
                println!("WRITER-PARTITION-TRACE-ROUND {}",serde_json::json!({
                    "class":spec["class"],"reflective":reflect,"round":round,
                    "partition_failure":stored!=expected,
                    "replay_failure":round==2 && first.as_ref()!=Some(&stored),
                    "stored":laws::show(&stored),"expected":laws::show(&expected),
                }));
                if round==1 {first=Some(stored);}
            }
        }
    }
}
