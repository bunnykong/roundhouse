//! Test-only return pilot, explicitly porting W09/W10's operations and minimal
//! witnesses from bf2f052f. Boxed main Ty -> shared/tagged next Ty is intentional;
//! this is not a port of all 40 F25 representations or all 55 writer checks.
use super::*;
use crate::{effect::EffectSet, ty::Provenance};
use std::sync::Arc;

#[derive(Clone, Default, Debug)]
struct Evidence {
    known: BTreeMap<String, Ty>,
    // Stable producer + component path + reason, never a pointer or visit number.
    origins: BTreeSet<String>,
    permanent: BTreeSet<String>,
    evaluated: bool,
}
fn shape_key(t: &Ty) -> String {
    serde_json::to_string(t).unwrap()
}
impl Evidence {
    fn input(writer: &str, t: Ty) -> Self {
        let mut origins = origins(&t)
            .into_iter()
            .map(|r| format!("{writer}|{r}"))
            .collect::<BTreeSet<_>>();
        let pending = matches!(t, Ty::Untyped { why } if why == Provenance::Pending);
        let permanent = origins
            .iter()
            .filter(|o| !o.ends_with(":Pending"))
            .cloned()
            .collect();
        if matches!(t, Ty::Bottom) {
            origins.insert(format!("{writer}|$:ProvenNonReturn"));
        } else if !matches!(t, Ty::Untyped { .. } | Ty::Var { .. }) {
            origins.insert(format!("{writer}|$:Computed:{}", shape_key(&t)));
        }
        let mut out = Self {
            origins,
            permanent,
            evaluated: !pending,
            ..Self::default()
        };
        if !matches!(t, Ty::Bottom | Ty::Untyped { .. }) {
            assert!(
                !matches!(t, Ty::Var { .. }),
                "bare Var needs an explicit producer classification"
            );
            out.known.insert(shape_key(&t), t);
        }
        out
    }
    fn external(writer: &str) -> Self {
        let mut e = Self::input(writer, Ty::unresolved());
        e.origins
            .insert(format!("{writer}|$:MissingExternalDispatch"));
        e.permanent
            .insert(format!("{writer}|$:MissingExternalDispatch"));
        e
    }
    fn join(&self, other: &Self) -> Self {
        let mut out = self.clone();
        for (key, value) in &other.known {
            out.known
                .entry(key.clone())
                .and_modify(|old| *old = merge_equal(old.clone(), value))
                .or_insert_with(|| value.clone());
        }
        out.origins.extend(other.origins.iter().cloned());
        out.permanent.extend(other.permanent.iter().cloned());
        out.evaluated |= other.evaluated;
        out
    }
    fn signature(&self) -> Value {
        json!({"known":self.known.keys().collect::<Vec<_>>(),"origins":self.origins,
            "permanent":self.permanent,"evaluated":self.evaluated})
    }
    fn value_signature(&self) -> Value {
        json!({"known":self.known.keys().collect::<Vec<_>>(),"permanent":self.permanent,"evaluated":self.evaluated})
    }
    fn below(&self, other: &Self) -> bool {
        (!self.evaluated || other.evaluated)
            && self.known.keys().all(|k| other.known.contains_key(k))
            && self.permanent.is_subset(&other.permanent)
    }
    fn value(&self) -> Option<Ty> {
        if !self.evaluated {
            return None;
        }
        let mut values: Vec<_> = self.known.values().cloned().collect();
        let mut why: Option<Provenance> = None;
        let mut uncertain = false;
        for reason in &self.permanent {
            // Unknowns inside a container remain local to that container.
            if reason.contains("|$:") {
                uncertain = true;
                let next = if reason.ends_with(":Gradual") {
                    Provenance::Gradual
                } else {
                    Provenance::Unresolved
                };
                why = Some(why.map_or(next, |old| old.union(next)));
            }
        }
        if uncertain {
            values.push(Ty::untyped(why.unwrap()));
        }
        Some(
            values
                .into_iter()
                .reduce(crate::analyze::body::union_of)
                .unwrap_or(Ty::Bottom),
        )
    }
}
#[derive(Clone)]
struct Accumulator {
    writers: BTreeMap<String, Evidence>,
}
impl Accumulator {
    fn declared(ids: &[&str]) -> Self {
        Self {
            writers: ids
                .iter()
                .map(|id| ((*id).into(), Evidence::default()))
                .collect(),
        }
    }
    fn replay(&mut self, id: &str, evidence: Evidence) {
        let existing = self.writers.get_mut(id).expect("undeclared writer");
        *existing = existing.join(&evidence);
    }
    fn replace(&mut self, id: &str, evidence: Evidence) -> Result<(), &'static str> {
        let existing = self.writers.get_mut(id).expect("undeclared writer");
        if !existing.below(&evidence) {
            return Err("replacement needs producer-specific removal justification");
        }
        *existing = evidence;
        Ok(())
    }
    fn joined(&self) -> Evidence {
        self.writers
            .values()
            .fold(Evidence::default(), |a, b| a.join(b))
    }
    fn export(&self) -> Option<Ty> {
        // An OR bit in joined() cannot certify completion of all producers.
        if self.writers.values().any(|e| !e.evaluated) {
            return None;
        }
        self.joined().value()
    }
}

#[derive(Clone, Debug)]
enum Concrete {
    Nil,
    Int,
    Str,
    Array(Vec<Concrete>),
    Hash(Vec<(Concrete, Concrete)>),
    Tuple(Vec<Concrete>),
}
// Independent finite interpretation; it does not use the candidate join/order.
fn contains(t: &Ty, v: &Concrete) -> bool {
    match (t, v) {
        (Ty::Untyped { .. }, _) => true,
        (Ty::Nil, Concrete::Nil) | (Ty::Int, Concrete::Int) | (Ty::Str, Concrete::Str) => true,
        (Ty::Array { elem }, Concrete::Array(items)) => items.iter().all(|v| contains(elem, v)),
        (Ty::Hash { key, value }, Concrete::Hash(items)) => items
            .iter()
            .all(|(k, v)| contains(key, k) && contains(value, v)),
        (Ty::Tuple { elems }, Concrete::Tuple(items)) => {
            elems.len() == items.len() && elems.iter().zip(items).all(|(t, v)| contains(t, v))
        }
        (Ty::Union { variants }, _) => variants.iter().any(|t| contains(t, v)),
        _ => false,
    }
}
fn array(t: Ty) -> Ty {
    Ty::Array { elem: Arc::new(t) }
}
fn hash(k: Ty, v: Ty) -> Ty {
    Ty::Hash {
        key: Arc::new(k),
        value: Arc::new(v),
    }
}
fn samples() -> Vec<Ty> {
    vec![
        Ty::pending_untyped(),
        Ty::Bottom,
        Ty::unresolved(),
        Ty::gradual(),
        Ty::Nil,
        Ty::Int,
        Ty::Str,
        array(Ty::pending_untyped()),
        array(Ty::Bottom),
        array(Ty::Int),
        array(Ty::Str),
        array(Ty::gradual()),
        array(Ty::unresolved()),
        array(array(Ty::Int)),
        hash(Ty::Str, Ty::pending_untyped()),
        hash(Ty::Str, Ty::Int),
        hash(Ty::Str, Ty::Str),
        hash(Ty::Str, Ty::gradual()),
        Ty::Tuple {
            elems: vec![Ty::pending_untyped(), Ty::Str].into(),
        },
        Ty::Tuple {
            elems: vec![Ty::Int, Ty::Str].into(),
        },
        Ty::Tuple {
            elems: vec![Ty::unresolved(), array(Ty::Str)].into(),
        },
    ]
}
fn concrete() -> Vec<Concrete> {
    use Concrete::*;
    vec![
        Nil,
        Int,
        Str,
        Array(vec![]),
        Array(vec![Int]),
        Array(vec![Str]),
        Array(vec![Nil]),
        Array(vec![Int, Str]),
        Array(vec![Array(vec![Int])]),
        Hash(vec![]),
        Hash(vec![(Str, Int)]),
        Hash(vec![(Str, Str)]),
        Hash(vec![(Str, Nil)]),
        Tuple(vec![Int, Str]),
        Tuple(vec![Nil, Array(vec![Str])]),
    ]
}

#[test]
fn writer_law_return_evidence_diamond() {
    let ts = samples();
    let values = concrete();
    let mut pairs = 0;
    let mut triples = 0;
    let mut permutations = 0;
    let mut memberships = 0;
    for a in &ts {
        let a = Evidence::input("Fixture|instance|a|fixture.rb:1-2", a.clone());
        assert_eq!(a.join(&a).signature(), a.signature());
        assert_eq!(
            Evidence::input("pending", Ty::pending_untyped())
                .join(&a)
                .value_signature(),
            a.value_signature()
        );
        for b in &ts {
            let b = Evidence::input("Fixture|instance|b|fixture.rb:3-4", b.clone());
            let ab = a.join(&b);
            pairs += 1;
            assert_eq!(ab.signature(), b.join(&a).signature());
            assert_eq!(ab.join(&a).signature(), ab.signature());
            assert!(a.below(&ab) && b.below(&ab));
            if let Some(t) = ab.value() {
                for v in &values {
                    let input_accepts = a.value().is_some_and(|t| contains(&t, v))
                        || b.value().is_some_and(|t| contains(&t, v));
                    if input_accepts {
                        assert!(contains(&t, v), "lost {v:?} from {t:?}");
                    }
                    memberships += 1;
                }
            }
            for c in &ts {
                let c = Evidence::input("Fixture|instance|c|fixture.rb:5-6", c.clone());
                let expected = ab.join(&c);
                triples += 1;
                assert_eq!(expected.signature(), a.join(&b.join(&c)).signature());
                for order in [
                    [0, 1, 2],
                    [0, 2, 1],
                    [1, 0, 2],
                    [1, 2, 0],
                    [2, 0, 1],
                    [2, 1, 0],
                ] {
                    let events = [("a", &a), ("b", &b), ("c", &c)];
                    let (x, y, z) = (events[order[0]].1, events[order[1]].1, events[order[2]].1);
                    permutations += 1;
                    assert_eq!(x.join(y).join(z).signature(), expected.signature());
                    let mut ledger = Accumulator::declared(&["a", "b", "c"]);
                    for index in order {
                        let (id, e) = events[index];
                        ledger.replay(id, e.clone());
                        ledger.replay(id, e.clone());
                    }
                    assert_eq!(ledger.joined().signature(), expected.signature());
                    let inputs = a
                        .origins
                        .union(&b.origins)
                        .cloned()
                        .collect::<BTreeSet<_>>()
                        .union(&c.origins)
                        .cloned()
                        .collect::<BTreeSet<_>>();
                    assert_eq!(expected.origins, inputs);
                }
            }
        }
    }
    let int_string = Evidence::input("integer", Ty::Int)
        .join(&Evidence::input("string", Ty::Str))
        .value()
        .unwrap();
    assert!(
        matches!(&int_string,Ty::Union {variants} if variants.len()==2&&variants.contains(&Ty::Int)&&variants.contains(&Ty::Str))
    );
    assert!(
        !contains(&int_string, &Concrete::Nil),
        "all-unknown must not pass"
    );
    println!(
        "RETURN-EVIDENCE {}",
        json!({"samples":ts.len(),"pairs":pairs,"triples":triples,
        "permutations":permutations,"finite_membership_checks":memberships,"violations":0})
    );
}

#[test]
fn writer_law_return_evidence_completion_and_growth() {
    let mut a = Accumulator::declared(&["raises", "unfinished"]);
    a.replay("raises", Evidence::input("raises", Ty::Bottom));
    a.replay(
        "unfinished",
        Evidence::input("unfinished", Ty::pending_untyped()),
    );
    assert!(a.joined().evaluated);
    assert!(a.export().is_none(), "slot-level OR exports early never");
    a.replay("unfinished", Evidence::input("unfinished", Ty::Int));
    assert_eq!(a.export(), Some(Ty::Int));
    a.replay(
        "unfinished",
        Evidence::input("unfinished", Ty::pending_untyped()),
    );
    assert_eq!(
        a.export(),
        Some(Ty::Int),
        "old pending replay reopens completion"
    );
    let mut never = Accumulator::declared(&["raises"]);
    never.replay("raises", Evidence::input("raises", Ty::Bottom));
    assert_eq!(never.export(), Some(Ty::Bottom));
    let mut growth = Accumulator::declared(&["w"]);
    growth.replace("w", Evidence::input("w", Ty::Int)).unwrap();
    let both = growth.joined().join(&Evidence::input("w", Ty::Str));
    growth.replace("w", both).unwrap();
    assert!(growth.replace("w", Evidence::input("w", Ty::Int)).is_err());
    let unresolved = growth.joined().join(&Evidence::external("w"));
    growth.replace("w", unresolved).unwrap();
    assert!(
        growth.replace("w", Evidence::input("w", Ty::Int)).is_err(),
        "no unjustified uncertainty removal"
    );
    println!("RETURN-EVIDENCE completion=per-writer early-never=0 monotone-growth=pass");
}

#[test]
fn writer_law_return_evidence_priority_and_contract() {
    // Fixed metadata chooses the own method even before its value is informative.
    fn selected(a: &Accumulator, contract: Option<&Ty>) -> Option<Ty> {
        contract.cloned().or_else(|| a.writers["own"].value())
    }
    let contract = Ty::Fn {
        params: vec![].into(),
        block: None,
        ret: Arc::new(Ty::Str),
        effects: EffectSet::default(),
    };
    for reverse in [false, true] {
        let mut a = Accumulator::declared(&["own", "included"]);
        let own = Evidence::input("own", Ty::pending_untyped());
        let included = Evidence::input("included", Ty::Int);
        let events = if reverse {
            vec![("included", included), ("own", own)]
        } else {
            vec![("own", own), ("included", included)]
        };
        for (id, e) in events {
            a.replay(id, e);
        }
        assert!(
            selected(&a, None).is_none(),
            "informative-first changes method precedence"
        );
        assert_eq!(selected(&a, Some(&contract)), Some(contract.clone()));
        assert!(
            a.joined()
                .known
                .values()
                .all(|t| !matches!(t, Ty::Fn { .. })),
            "contract became ordinary contributor"
        );
        a.replay("own", Evidence::input("own", Ty::Str));
        assert_eq!(selected(&a, None), Some(Ty::Str));
        assert_eq!(selected(&a, Some(&contract)), Some(contract.clone()));
    }
    println!("RETURN-EVIDENCE priority=metadata contract=separate arrival-orders=2");
}

#[test]
fn writer_law_return_evidence_missing_dispatch_and_pending_seed() {
    let ghost = Ty::Class {
        id: ClassId(Symbol::from("EvidenceExternal")),
        args: vec![].into(),
    };
    let ordinary = crate::analyze::body::evidence_control_dispatch(
        &ghost,
        &Symbol::from("producer_reason_unmodeled"),
    );
    assert!(
        matches!(ordinary, Ty::Var { .. }),
        "unchanged dispatch control"
    );
    // Classify from the producer's explicit status, never from the wire-equal Var.
    let miss = Evidence::external("EvidenceExternal|instance|unmodeled|fixture.rb:9-10");
    let seed = Evidence::input("verified-seed", Ty::pending_untyped());
    let nil = Evidence::input("nil", Ty::Nil);
    let unresolved = nil.join(&miss).value().unwrap();
    assert!(contains(&unresolved, &Concrete::Int) && contains(&unresolved, &Concrete::Str));
    assert_ne!(unresolved, Ty::Nil);
    assert_eq!(nil.join(&seed).value(), Some(Ty::Nil));
    println!(
        "RETURN-EVIDENCE external=unresolved verified-seed=no-value ordinary-dispatch=unchanged"
    );
}

// Focused explicit W09/W10 ports: the exact same real operations, adapted to next.
// Keep them ignored known violations; --include-ignored executes these controls.
fn control_samples() -> Vec<Ty> {
    vec![
        Ty::Var {
            var: crate::ident::TyVar(0),
        },
        Ty::Bottom,
        Ty::Bool,
        Ty::Int,
        Ty::Str,
        Ty::Nil,
        Ty::unresolved(),
    ]
}
fn control(name: &str, op: impl Fn(Ty, Ty) -> Ty) {
    let ts = control_samples();
    let mut bad = 0;
    for a in &ts {
        for b in &ts {
            if op(a.clone(), b.clone()) != op(b.clone(), a.clone()) {
                bad += 1;
            }
            for c in &ts {
                if op(op(a.clone(), b.clone()), c.clone())
                    != op(a.clone(), op(b.clone(), c.clone()))
                {
                    bad += 1;
                }
            }
        }
    }
    let forward = op(Ty::Bool, Ty::Bottom);
    let reverse = op(Ty::Bottom, Ty::Bool);
    println!(
        "PORTED-F25 {name} samples={} pairs=49 triples=343 violations={bad} Bool-Bottom={forward:?} Bottom-Bool={reverse:?}",
        ts.len()
    );
    assert_eq!(bad, 0, "known F25 control remains non-lawful");
}
#[test]
#[ignore = "ported W09 control: distinct return cores replace"]
fn writer_law_return_control_decision() {
    control(
        "W09",
        crate::analyze::harvest_return::evidence_control_decision,
    );
}
#[test]
#[ignore = "ported W10 control: insertion inherits replacement"]
fn writer_law_return_control_table() {
    control("W10", |a, b| {
        let mut table = std::collections::HashMap::new();
        let name = Symbol::from("m");
        crate::analyze::harvest_return::insert_inferred_return(&mut table, &name, a);
        crate::analyze::harvest_return::insert_inferred_return(&mut table, &name, b);
        table.remove(&name).unwrap()
    });
}

#[test]
fn writer_law_return_evidence_storage_and_origin_conservation() {
    // The per-writer channel works even when a legacy value policy strips a tag.
    let inputs = [
        Evidence::input("a", array(Ty::unresolved())),
        Evidence::input("b", array(Ty::gradual())),
    ];
    let e = inputs[0].join(&inputs[1]);
    assert!(e.origins.iter().any(|o| o.ends_with(":Unresolved")));
    assert!(e.origins.iter().any(|o| o.ends_with(":Gradual")));
    let both = merge_equal(array(Ty::unresolved()), &array(Ty::gradual()));
    let tags = origins(&both);
    assert_eq!(tags.len(), 2);
    let mut stored = crate::shared::Shared::from(vec![both.clone()]);
    assert_eq!(origins(&stored[0]), tags);
    let clone = stored.clone();
    stored[0] = Ty::Int;
    assert_eq!(origins(&clone[0]), tags, "COW/interner erased evidence");
    let roundtrip: Ty = serde_json::from_value(serde_json::to_value(&both).unwrap()).unwrap();
    assert_ne!(
        origins(&roundtrip),
        tags,
        "serde must not masquerade as a lossless audit codec"
    );
    println!("RETURN-EVIDENCE origins=conserved shared-storage=pass serde=known-lossy");
}
