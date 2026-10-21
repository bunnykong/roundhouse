use super::*;
use crate::analyze::body::operator_laws::{self as law, Demand, Suite};

pub(crate) fn register(s: &mut Suite) {
    s.transfer(
        "remove_nil",
        "body/narrowing.rs:482",
        Demand::Whole,
        remove_nil,
    );
    let targets = [
        Ty::Int,
        Ty::Nil,
        law::arr(law::u()),
        law::hash(law::u(), law::u()),
        law::class("Box", vec![]),
        law::class("Box", vec![Ty::Int]),
    ];
    for target in targets {
        let suffix = law::show(&target);
        s.transfer(
            &format!("intersect_with/{suffix}"),
            "body/narrowing.rs:526",
            Demand::Whole,
            |t| intersect_with(t, &target),
        );
        s.check_where(
            &format!("intersect_variant/{suffix}"),
            "body/narrowing.rs:550",
            Demand::Whole,
            |t| !matches!(t, Ty::Union { .. }),
            |t| intersect_variant(t, &target),
        );
        s.transfer(
            &format!("remove_variant/{suffix}"),
            "body/narrowing.rs:567",
            Demand::Whole,
            |t| remove_variant(t, &target),
        );
    }
    // Research-only head matching: positive and negative contributions, plus
    // their actual intersect/remove caller path under the scoped head flag.
    for target in [
        Ty::Int,
        Ty::Nil,
        law::arr(law::u()),
        law::hash(law::u(), law::u()),
        law::class("Box", vec![]),
    ] {
        let suffix = law::show(&target);
        s.transfer(
            &format!("head_split/this/{suffix}"),
            "body/narrowing.rs:649",
            Demand::Whole,
            |t| head_split(t, &target).0,
        );
        s.transfer(
            &format!("head_split/rest/{suffix}"),
            "body/narrowing.rs:649",
            Demand::Whole,
            |t| head_split(t, &target).1,
        );
        s.transfer(
            &format!("intersect_with/head/{suffix}"),
            "body/narrowing.rs:526",
            Demand::Whole,
            |t| with_head_match(|| intersect_with(t, &target)),
        );
        s.transfer(
            &format!("remove_variant/head/{suffix}"),
            "body/narrowing.rs:567",
            Demand::Whole,
            |t| with_head_match(|| remove_variant(t, &target)),
        );
    }
    // The narrower/filter is fixed syntax in ordinary guards. Also audit its
    // type argument independently for future dynamic use (not a C2 operand).
    for current in [
        Ty::Int,
        law::u(),
        law::arr(Ty::Int),
        law::hash(Ty::Sym, Ty::Str),
    ] {
        let suffix = law::show(&current);
        s.transfer(
            &format!("intersect_with/filter/{suffix}"),
            "body/narrowing.rs:526",
            Demand::Member,
            |t| intersect_with(&current, t),
        );
        s.check(
            &format!("intersect_variant/filter/{suffix}"),
            "body/narrowing.rs:550",
            Demand::Member,
            |t| intersect_variant(&current, t),
        );
        s.transfer(
            &format!("remove_variant/filter/{suffix}"),
            "body/narrowing.rs:567",
            Demand::Member,
            |t| remove_variant(&current, t),
        );
    }
    // Exercise the actual caller chain, including reader/member rebinding.
    for kind in 0..4 {
        for pred in 0..9 {
            for side in [true, false] {
                s.transfer(
                    &format!("apply_narrowing/key{kind}/pred{pred}/{side}"),
                    "body/narrowing.rs:392",
                    Demand::Whole,
                    |t| {
                        let name = Symbol::from("x");
                        let attr = Symbol::from("field");
                        let key = match kind {
                            0 => VarKey::Local(name.clone()),
                            1 => VarKey::Ivar(name.clone()),
                            2 => VarKey::Reader(name.clone(), t.clone()),
                            _ => VarKey::Member(name.clone(), attr.clone(), t.clone()),
                        };
                        let p = predicate(pred, key);
                        let mut ctx = Ctx::default();
                        if kind == 0 {
                            ctx.local_bindings.insert(name.clone(), t.clone());
                        }
                        if kind == 1 {
                            ctx.ivar_bindings.insert(name.clone(), t.clone());
                        }
                        let out = apply_narrowing(&ctx, &p, side);
                        let binding = if kind == 1 {
                            &out.ivar_bindings
                        } else {
                            &out.local_bindings
                        };
                        let name = if kind == 3 {
                            member_key(&name, &attr)
                        } else {
                            name
                        };
                        binding.get(&name).cloned().unwrap_or_else(|| t.clone())
                    },
                );
            }
        }
    }
}

fn predicate(n: usize, key: VarKey) -> NarrowPred {
    match n {
        0 => NarrowPred::IsNil(key),
        1 => NarrowPred::IsNotNil(key),
        2 => NarrowPred::IsA(key, law::hash(law::u(), law::u())),
        3 => NarrowPred::IsNotA(key, law::hash(law::u(), law::u())),
        4 => NarrowPred::IsTruthy(key),
        5 => NarrowPred::IsFalsy(key),
        6 => NarrowPred::IsPresent(key),
        7 => NarrowPred::IsBlank(key),
        _ => NarrowPred::Compound {
            if_true: vec![
                NarrowPred::IsNotNil(key.clone()),
                NarrowPred::IsA(key, law::hash(law::u(), law::u())),
            ],
            if_false: vec![],
        },
    }
}

#[test]
#[ignore = "known C2: Pending guard manufactures a concrete Hash"]
fn operator_law_pending_guard() {
    let target = law::hash(law::u(), law::u());
    let output = intersect_with(&law::p(), &target);
    assert!(
        law::only_pending(&output),
        "intersect_with(P,H[U,U]) = {}",
        law::show(&output)
    );
}

#[test]
#[ignore = "known O012: empty intersection uses the requested target"]
fn operator_law_census_o012() {
    let target = law::arr(law::u());
    let (a, b) = (Ty::Int, law::raw(vec![Ty::Int, law::arr(Ty::Str)]));
    assert!(law::leq(&a, &b));
    assert!(law::leq(
        &intersect_with(&a, &target),
        &intersect_with(&b, &target)
    ));
}

#[test]
#[ignore = "known O013: all-nil result retains Nil"]
fn operator_law_census_o013() {
    let (a, b) = (Ty::Nil, law::raw(vec![Ty::Nil, Ty::Int]));
    assert!(law::leq(&a, &b));
    assert!(law::leq(&remove_nil(&a), &remove_nil(&b)));
}

#[test]
#[ignore = "known O014: exhausted subtraction retains the current type"]
fn operator_law_census_o014() {
    let (a, b) = (Ty::Int, law::raw(vec![Ty::Int, Ty::Str]));
    assert!(law::leq(&a, &b));
    assert!(law::leq(
        &remove_variant(&a, &Ty::Int),
        &remove_variant(&b, &Ty::Int)
    ));
}
