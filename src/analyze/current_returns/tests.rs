use super::*;
use crate::{
    ident::TyVar,
    ty::{Param, ParamKind, Provenance, Row},
};
use std::sync::Arc;
fn slot(c: &str) -> Slot {
    (ClassId(Symbol::from(c)), Symbol::from("answer"), false)
}
fn add(s: &mut State, id: &str, dest: &Slot) {
    s.own.entry(dest.clone()).or_default().insert(id.into());
    s.writer_slots.insert(id.into(), dest.clone());
}
fn origins(t: &Ty) -> BTreeSet<String> {
    super::super::producer_evidence::origins(t)
        .into_iter()
        .map(|x| x.replace("/arm", ""))
        .collect()
}
fn equal(a: &Ty, b: &Ty) {
    assert!(same(a, b), "left={a:?}\nright={b:?}");
}
fn arr(t: Ty) -> Ty {
    Ty::Array { elem: Arc::new(t) }
}
fn hash(t: Ty) -> Ty {
    Ty::Hash {
        key: Arc::new(Ty::Str),
        value: Arc::new(t),
    }
}
fn samples() -> Vec<Ty> {
    let mut out = vec![
        Ty::Bottom,
        Ty::Int,
        Ty::Str,
        Ty::Nil,
        Ty::pending_untyped(),
        Ty::gradual(),
        Ty::unresolved(),
        Ty::Var { var: TyVar(0) },
        Ty::Var { var: TyVar(7) },
    ];
    for wrap in [arr, hash] {
        for t in [
            Ty::Bottom,
            Ty::Int,
            Ty::pending_untyped(),
            Ty::gradual(),
            Ty::unresolved(),
            Ty::Var { var: TyVar(0) },
        ] {
            out.push(wrap(t));
        }
    }
    out.push(Ty::Union {
        variants: vec![Ty::Int, Ty::unresolved()].into(),
    });
    out.push(Ty::Record {
        row: Row {
            fields: vec![(Symbol::from("x"), Ty::gradual())]
                .into_iter()
                .collect(),
            rest: None,
        },
    });
    out.push(Ty::Tuple {
        elems: vec![Ty::Int, Ty::unresolved()].into(),
    });
    out
}
#[test]
fn current_return_replace_same_writer_replaces() {
    for next in [Ty::gradual(), Ty::unresolved(), Ty::Var { var: TyVar(0) }] {
        let mut s = State::default();
        let dst = slot("Own");
        add(&mut s, "writer", &dst);
        s.replace("writer", Ty::Str);
        s.replace("writer", next.clone());
        equal(&s.aggregate(&dst), &next);
        assert_eq!(s.versions["writer"], 2);
        assert!(!format!("{:?}", s.aggregate(&dst)).contains("Str"));
    }
    println!(
        "research condition case 1: same-writer replacement G/U/ambiguous Var, old informative history retired"
    );
}
#[test]
fn current_return_replace_second_writer_survives() {
    for evidence in [
        Ty::gradual(),
        Ty::unresolved(),
        Ty::Var { var: TyVar(0) },
        hash(Ty::gradual()),
        arr(Ty::unresolved()),
    ] {
        let mut s = State::default();
        let dst = slot("Own");
        add(&mut s, "module", &dst);
        add(&mut s, "receiver-context", &dst);
        s.replace("module", evidence.clone());
        s.replace("receiver-context", Ty::Int);
        assert!(origins(&evidence).is_subset(&origins(&s.aggregate(&dst))));
        s.replace("receiver-context", Ty::Str);
        assert!(origins(&evidence).is_subset(&origins(&s.aggregate(&dst))));
        equal(&s.values["module"], &evidence);
    }
    println!("research condition case 2: distinct module/receiver live G/U/Var paths conserved");
}
fn permutations(xs: &[usize]) -> Vec<Vec<usize>> {
    if xs.is_empty() {
        return vec![vec![]];
    }
    let mut out = Vec::new();
    for (i, x) in xs.iter().enumerate() {
        let rest = xs
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, x)| *x)
            .collect::<Vec<_>>();
        for mut p in permutations(&rest) {
            p.insert(0, *x);
            out.push(p);
        }
    }
    out
}
#[test]
fn current_return_replace_permutations_agree() {
    let ts = samples();
    let dst = slot("Own");
    let mut cases = 0;
    for n in [2, 3] {
        for a in &ts {
            for b in &ts {
                for c in [Ty::gradual(), Ty::Int, Ty::Var { var: TyVar(0) }] {
                    let values = [a.clone(), b.clone(), c];
                    let expected = values[..n]
                        .iter()
                        .cloned()
                        .fold(Ty::pending_untyped(), join);
                    for order in permutations(&(0..n).collect::<Vec<_>>()) {
                        let mut s = State::default();
                        for i in 0..n {
                            add(&mut s, &i.to_string(), &dst);
                        }
                        for i in order {
                            s.replace(&i.to_string(), values[i].clone());
                        }
                        equal(&s.aggregate(&dst), &expected);
                        cases += 1;
                    }
                }
            }
        }
    }
    println!("research condition case 3: {cases} two/three-writer publication permutations");
}
#[test]
fn current_return_replace_idempotent_associative_pending_identity() {
    let ts = samples();
    let mut triples = 0;
    for a in &ts {
        equal(&join(a.clone(), a.clone()), a);
        equal(
            &join(Ty::pending_untyped(), a.clone()),
            &super::super::parameter_conservation::normalize(a.clone()),
        );
        equal(
            &join(a.clone(), Ty::pending_untyped()),
            &super::super::parameter_conservation::normalize(a.clone()),
        );
        let mut s = State::default();
        let dst = slot("Own");
        add(&mut s, "a", &dst);
        s.replace("a", a.clone());
        let version = s.versions["a"];
        s.replace("a", a.clone());
        assert_eq!(version, s.versions["a"]);
        for b in &ts {
            equal(&join(a.clone(), b.clone()), &join(b.clone(), a.clone()));
            for c in &ts {
                equal(
                    &join(join(a.clone(), b.clone()), c.clone()),
                    &join(a.clone(), join(b.clone(), c.clone())),
                );
                triples += 1;
            }
        }
    }
    println!(
        "research condition case 4: {triples} associative triples; identity, commutativity, repeated publication"
    );
}
#[test]
fn current_return_replace_copies_keep_identity_and_signatures() {
    let mut s = State::default();
    let source = slot("Module");
    let receiver = slot("Receiver");
    let child = slot("Child");
    s.declare(
        &source.0,
        &source.0,
        &receiver.0,
        &source.1,
        false,
        Span::synthetic(),
        &[],
    );
    let id = s.own[&source].iter().next().unwrap().clone();
    s.replace(&id, Ty::gradual());
    s.route(receiver.clone(), source.clone(), "include", &Ty::gradual());
    s.route(child.clone(), receiver.clone(), "inherit", &Ty::gradual());
    assert_eq!(s.members(&child), BTreeSet::from([id.clone()]));
    assert_eq!(s.declarations[&id]["owner"], "Module");
    assert_eq!(s.declarations[&id]["context"], "Receiver");
    s.replace(&id, Ty::Str);
    equal(&s.aggregate(&child), &Ty::Str);
    let signature = Ty::Fn {
        params: vec![Param {
            name: Symbol::from("x"),
            ty: Arc::new(Ty::Int),
            kind: ParamKind::Required,
        }]
        .into(),
        block: None,
        ret: Arc::new(Ty::Int),
        effects: Default::default(),
    };
    equal(
        &authoritative(Some(&signature), s.aggregate(&child)),
        &signature,
    );
    equal(
        &authoritative_copy(None, &signature, s.aggregate(&child)),
        &signature,
    );
    s.declare(
        &receiver.0,
        &source.0,
        &receiver.0,
        &source.1,
        false,
        Span::synthetic(),
        &[],
    );
    assert_eq!(s.declarations.len(), 2);
    println!(
        "research condition case 5: transitive/inherited writer identity and typing contexts; authored Fn authority"
    );
}
#[test]
fn current_return_current_return_replacing_one_writer_never_retires_another() {
    let mut s = State::default();
    let dst = slot("Own");
    add(&mut s, "a", &dst);
    add(&mut s, "b", &dst);
    add(&mut s, "c", &dst);
    s.replace("a", Ty::gradual());
    s.replace("b", Ty::unresolved());
    s.replace("c", Ty::Int);
    s.replace("c", Ty::Str);
    assert_eq!(s.values.len(), 3);
    assert!(origins(&s.aggregate(&dst)).contains("$:Gradual"));
    assert!(origins(&s.aggregate(&dst)).contains("$:Unresolved"));
    let mirror = slot("Mirror");
    s.route(mirror.clone(), dst.clone(), "copy", &Ty::Str);
    s.remove_routes(&mirror, "copy");
    assert_eq!(s.values.len(), 3);
    assert!(s.members(&mirror).is_empty());
    assert_eq!(
        Provenance::Pending.union(Provenance::Gradual),
        Provenance::Gradual
    );
}
#[test]
fn current_return_current_return_native_audit_fixture() {
    let Some(path) = std::env::var_os("RH_ROW_NATIVE_AUDIT") else {
        return;
    };
    let mut s = State::default();
    let a = slot("Module");
    let b = slot("Receiver");
    let c = slot("Child");
    for dest in [&a, &b] {
        s.declare(
            &dest.0,
            &dest.0,
            &dest.0,
            &dest.1,
            false,
            Span::synthetic(),
            &[],
        );
    }
    s.events = Some(std::io::BufWriter::new(
        std::fs::File::create(path).unwrap(),
    ));
    s.event(json!({"event":"inventory","schema":1,"candidate":true,"declarations":s.declarations.values().collect::<Vec<_>>() }));
    let aid = s.own[&a].iter().next().unwrap().clone();
    let bid = s.own[&b].iter().next().unwrap().clone();
    s.replace(&aid, hash(Ty::gradual()));
    s.replace(&bid, Ty::Int);
    s.replace(&bid, Ty::unresolved());
    s.route(b.clone(), a.clone(), "concern:Module", &Ty::Str);
    s.route(c.clone(), b.clone(), "inherited", &Ty::Str);
    s.replace(&bid, Ty::Var { var: TyVar(0) });
    let mut audit_values = samples();
    audit_values.push(Ty::Class {
        id: ClassId(Symbol::from("Box")),
        args: vec![Ty::gradual()].into(),
    });
    audit_values.push(Ty::Fn {
        params: vec![].into(),
        block: None,
        ret: Arc::new(Ty::unresolved()),
        effects: Default::default(),
    });
    audit_values.push(Ty::Union {
        variants: vec![Ty::gradual(), Ty::pending_untyped()].into(),
    });
    audit_values.push(Ty::Union {
        variants: vec![
            Ty::Class {
                id: ClassId(Symbol::from("A")),
                args: vec![Ty::gradual()].into(),
            },
            Ty::Class {
                id: ClassId(Symbol::from("B")),
                args: vec![Ty::unresolved()].into(),
            },
        ]
        .into(),
    });
    for (i, ty) in audit_values.into_iter().enumerate() {
        s.external(&slot(&format!("NativeCase{i}")), ty, "native-fixture");
    }
    let mut classes = HashMap::<ClassId, ClassInfo>::new();
    let all_slots = s
        .own
        .keys()
        .chain(s.routes.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for dest in &all_slots {
        classes
            .entry(dest.0.clone())
            .or_default()
            .instance_methods
            .insert(dest.1.clone(), s.aggregate(dest));
    }
    let snapshot = s.snapshot(&classes, true);
    s.event(snapshot);
    s.events.as_mut().unwrap().flush().unwrap();
    println!("research condition native fixture emitted through the production ledger event/snapshot code");
}

#[test]
fn current_return_replace_var_normalizes_to_unresolved() {
    equal(
        &boundary_normalize(Ty::Var { var: TyVar(0) }),
        &Ty::unresolved(),
    );
    let placeholders = Ty::Union {
        variants: vec![Ty::Var { var: TyVar(0) }, Ty::gradual()].into(),
    };
    let result = boundary_normalize(placeholders);
    assert!(origins(&result).contains("$:Gradual"));
    assert!(origins(&result).contains("$:Unresolved"));
    let informative = Ty::Union {
        variants: vec![Ty::Int, Ty::Var { var: TyVar(0) }].into(),
    };
    equal(&boundary_normalize(informative.clone()), &informative);
    equal(
        &boundary_normalize(arr(Ty::Var { var: TyVar(0) })),
        &arr(Ty::Var { var: TyVar(0) }),
    );
}
#[test]
fn current_return_replace_no_route_removal() {
    assert!(
        on(),
        "run the strict-replace laws with RH_CURRENT_RETURN_REPLACE=1"
    );
    let mut s = State::default();
    let src = slot("A");
    let dst = slot("B");
    add(&mut s, "a", &src);
    s.replace("a", Ty::Str);
    s.route(dst.clone(), src, "host-lend:A", &Ty::Str);
    let before = s.routes.clone();
    LEDGER.with(|c| *c.borrow_mut() = s);
    remove_copy((&dst.0, &dst.1, false), "host-lend:");
    LEDGER.with(|c| assert_eq!(c.borrow().routes, before));
}
#[test]
fn current_return_replace_predeclared_slot_reads_pending() {
    assert!(on());
    let tree = std::collections::HashMap::from([
        (
            std::path::PathBuf::from("db/schema.rb"),
            b"ActiveRecord::Schema.define do\nend\n".to_vec(),
        ),
        (
            std::path::PathBuf::from("lib/early.rb"),
            b"class Early; def answer; 1; end; end".to_vec(),
        ),
    ]);
    let app = crate::ingest::ingest_app_from_tree(tree).unwrap();
    let mut classes = HashMap::new();
    reset(&app, &mut classes);
    assert!(pending_only(
        &classes[&ClassId(Symbol::from("Early"))].instance_methods[&Symbol::from("answer")]
    ));
}
#[test]
fn current_return_replace_host_lend_joins_includers() {
    let tree=std::collections::HashMap::from([
        (std::path::PathBuf::from("db/schema.rb"),b"ActiveRecord::Schema.define do\nend\n".to_vec()),
        (std::path::PathBuf::from("lib/loans.rb"),b"module M; def answer; sql_array; end; end; class A; include M; def sql_array; 1; end; end; class B; include M; def sql_array; \"x\"; end; end".to_vec())]);
    let app = crate::ingest::ingest_app_from_tree(tree).unwrap();
    let m = ClassId(Symbol::from("M"));
    let name = Symbol::from("sql_array");
    let mut classes = HashMap::new();
    let mut s = State::default();
    for (id, value) in [("A", Ty::Int), ("B", Ty::Str)] {
        let cid = ClassId(Symbol::from(id));
        let src = (cid.clone(), name.clone(), false);
        classes
            .entry(cid)
            .or_insert_with(ClassInfo::default)
            .includes
            .push(m.clone());
        add(&mut s, id, &src);
        s.replace(id, value);
    }
    s.declare_fixed_routes(&app, &classes);
    let dest = (m, name, false);
    assert_eq!(
        s.members(&dest),
        BTreeSet::from(["A".to_string(), "B".to_string()])
    );
    equal(&s.aggregate(&dest), &join(Ty::Int, Ty::Str));
    s.replace("B", Ty::gradual());
    equal(&s.aggregate(&dest), &join(Ty::Int, Ty::gradual()));
}
