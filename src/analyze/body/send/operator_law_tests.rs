use super::*;
use crate::analyze::body::operator_laws::{self as law, Demand, Suite};

// Keep arity, block presence, literal nodes, and the class registry fixed for
// each projection. Only the stamped Ty is varied; absence is never confused
// with a present argument whose type is Pending.
pub(crate) fn register(s: &mut Suite) {
    let kinds: std::collections::BTreeMap<_, _> = crate::catalog::AR_CATALOG
        .iter()
        .filter_map(|entry| entry.return_kind.map(|kind| (format!("{kind:?}"), kind)))
        .collect();
    for (name, kind) in kinds {
        s.transfer(
            &format!("relation_return/{name}/elem"),
            "body/send.rs:2927",
            Demand::Member,
            |t| relation_return_on_array_repr(kind, t),
        );
    }
    for (name, site, f) in [
        (
            "non_nil_elem",
            "body/send.rs:2963",
            non_nil_elem as fn(&Ty) -> Ty,
        ),
        ("flatten_elem", "body/send.rs:3886", flatten_elem),
    ] {
        s.transfer(name, site, Demand::Whole, f);
    }
    // A registry entry is either a full signature or a bare return, selected
    // by fixed declaration metadata. Do not order those two encodings as if
    // a growing function value changed into its own return value.
    s.transfer(
        "unwrap_fn_ret/Fn-return",
        "body/send.rs:3793",
        Demand::Member,
        |t| unwrap_fn_ret(&law::function(t.clone())),
    );
    s.check_where(
        "unwrap_fn_ret/bare",
        "body/send.rs:3793",
        Demand::Whole,
        |t| !matches!(t, Ty::Fn { .. }),
        |t| Some(unwrap_fn_ret(t)),
    );
    s.transfer(
        "jbuilder_value/present",
        "body/send.rs:3502",
        Demand::Whole,
        |t| jbuilder_value(Some(&law::stamped(t))),
    );

    // Native element/key/value helpers know their receiver's outer shape.
    // C2 applies to whole receivers, below, not to Array[P]'s child P.
    for m in array_names() {
        let method = Symbol::from(m);
        for block in [None, Some(Ty::Int), Some(law::arr(Ty::Str))] {
            s.transfer(
                &format!(
                    "array_method/{m}/elem/block={}",
                    block
                        .as_ref()
                        .map(law::show)
                        .unwrap_or_else(|| "absent".into())
                ),
                "body/send.rs:2978",
                Demand::Member,
                |t| array_method(&method, t, block.as_ref()),
            );
        }
        for elem in [Ty::Int, law::class("Model", vec![])] {
            s.transfer(
                &format!("array_method/{m}/block/elem={}", law::show(&elem)),
                "body/send.rs:2978",
                Demand::Member,
                |t| array_method(&method, &elem, Some(t)),
            );
        }
    }
    for m in hash_names() {
        let method = Symbol::from(m);
        s.transfer(
            &format!("hash_method/{m}/key"),
            "body/send.rs:3269",
            Demand::Member,
            |t| hash_method(&method, t, &Ty::Int, None, &[]),
        );
        s.transfer(
            &format!("hash_method/{m}/value"),
            "body/send.rs:3269",
            Demand::Member,
            |t| hash_method(&method, &Ty::Sym, t, None, &[]),
        );
        s.transfer(
            &format!("hash_method/{m}/block"),
            "body/send.rs:3269",
            Demand::Member,
            |t| hash_method(&method, &Ty::Sym, &Ty::Int, Some(t), &[]),
        );
    }
    s.alternative(
        "hash_method/fetch/default",
        "body/send.rs:3269",
        Ty::Int,
        |t| {
            hash_method(
                &Symbol::from("fetch"),
                &Ty::Sym,
                &Ty::Int,
                None,
                &[law::key(), law::stamped(t)],
            )
        },
    );
    s.alternative(
        "hash_method/deep_merge/arg",
        "body/send.rs:3269",
        law::hash(Ty::Sym, Ty::Int),
        |t| {
            hash_method(
                &Symbol::from("deep_merge"),
                &Ty::Sym,
                &Ty::Int,
                None,
                &[law::stamped(t)],
            )
        },
    );

    for m in [
        "first", "last", "min", "max", "begin", "end", "size", "include?", "to_a", "step", "each",
    ] {
        s.check(
            &format!("range_method/{m}/elem"),
            "body/send.rs:2585",
            Demand::Member,
            |t| range_method(&Symbol::from(m), Some(t)),
        );
    }
    for m in ["[]", "length", "keys", "empty?"] {
        s.transfer(
            &format!("record_method/{m}/field"),
            "body/send.rs:3229",
            Demand::Member,
            |t| {
                let mut row = crate::ty::Row::closed();
                row.fields.insert(Symbol::from("k"), t.clone());
                record_method(&Symbol::from(m), &row, &[law::key()])
            },
        );
    }
    s.check(
        "indexed_pluck/elem",
        "body/send.rs:2882",
        Demand::Member,
        |t| indexed_pluck(&Symbol::from("pluck"), t, &[law::key()]),
    );
    for m in ["[]", "slice"] {
        s.check(
            &format!("sub_array_slice/{m}/arg"),
            "body/send.rs:2849",
            Demand::Whole,
            |t| sub_array_slice(&Symbol::from(m), &[law::stamped(t)], &Ty::Int),
        );
    }
    for m in ["new", "civil", "parse", "strptime", "iso8601", "today"] {
        s.check(
            &format!("date_constructor/{m}/arg0"),
            "body/send.rs:2690",
            Demand::Whole,
            |t| date_constructor(&Symbol::from(m), &[law::stamped(t)]),
        );
    }
    for m in [
        "+",
        "-",
        ">>",
        "<<",
        "prev_day",
        "next_month",
        "in_time_zone",
    ] {
        s.check(
            &format!("date_method/{m}/arg0"),
            "body/send.rs:2720",
            Demand::Whole,
            |t| date_method(&Symbol::from(m), &[law::stamped(t)]),
        );
    }
    for m in ["+", "-", "*", "/", "<", "<=>", "floor", "ceil"] {
        s.check(
            &format!("bigdecimal_method/{m}/arg0"),
            "body/send.rs:3739",
            Demand::Whole,
            |t| bigdecimal_method(&Symbol::from(m), &[law::stamped(t)]),
        );
    }
    for m in [
        "dup",
        "clone",
        "to_param",
        "instance_eval",
        "instance_exec",
        "class_eval",
        "module_eval",
    ] {
        let module = matches!(m, "class_eval" | "module_eval");
        if !module {
            s.check(
                &format!("object_protocol/{m}/recv"),
                "body/send.rs:3922",
                Demand::Whole,
                |t| object_protocol_method(Some(t), &Symbol::from(m), Some(&Ty::Int), false),
            );
        }
        s.check(
            &format!("object_protocol/{m}/block"),
            "body/send.rs:3922",
            Demand::Whole,
            |t| {
                object_protocol_method(
                    Some(&law::class("Box", vec![])),
                    &Symbol::from(m),
                    Some(t),
                    module,
                )
            },
        );
    }
    for m in [
        "[]",
        "map",
        "flat_map",
        "filter_map",
        "fetch",
        "each",
        "dup",
    ] {
        s.check(
            &format!("param_value_method/{m}/block"),
            "body/send.rs:3986",
            Demand::Member,
            |t| param_value_method(&Symbol::from(m), Some(t)),
        );
    }

    // Full pure dispatch with a frozen empty registry exposes first-match
    // behavior and supplies C2 checks at the actual whole receiver boundary.
    let classes = std::collections::HashMap::new();
    let typer = BodyTyper::new(&classes);
    for m in [
        "[]",
        "first",
        "last",
        "compact_blank",
        "flat_map",
        "map",
        "sum",
        "to_h",
        "to_a",
        "to_s",
        "flatten",
        "where",
        "model",
        "fetch",
        "deep_merge",
        "+",
        "concat",
        "call",
        "nil?",
        "instance_eval",
        "to_param",
    ] {
        let method = Symbol::from(m);
        s.transfer(
            &format!("dispatch/{m}/receiver"),
            "body/send.rs:1023",
            Demand::Whole,
            |t| typer.dispatch(Some(t), &method, None, &[]),
        );
    }
    for m in ["+", "-", "*", "/"] {
        s.transfer(
            &format!("dispatch/Int/{m}/arg"),
            "body/send.rs:1023",
            Demand::Whole,
            |t| typer.dispatch(Some(&Ty::Int), &Symbol::from(m), None, &[law::stamped(t)]),
        );
    }
    for m in ["+", "|", "concat"] {
        s.transfer(
            &format!("dispatch/Array/{m}/elem"),
            "body/send.rs:1023",
            Demand::Member,
            |t| {
                typer.dispatch(
                    Some(&law::arr(t.clone())),
                    &Symbol::from(m),
                    None,
                    &[law::stamped(&law::arr(Ty::Str))],
                )
            },
        );
    }
    for m in ["first", "last", "take", "sample", "[]", "slice", "fetch"] {
        s.transfer(
            &format!("dispatch/Array/{m}/arg"),
            "body/send.rs:1023",
            Demand::Whole,
            |t| {
                typer.dispatch(
                    Some(&law::arr(Ty::Int)),
                    &Symbol::from(m),
                    None,
                    &[law::stamped(t)],
                )
            },
        );
    }
    for m in ["fetch", "deep_merge"] {
        s.alternative(
            &format!("dispatch/Hash/{m}/arg"),
            "body/send.rs:1023",
            if m == "fetch" {
                law::raw(vec![Ty::Int, Ty::Nil])
            } else {
                law::hash(Ty::Sym, Ty::Int)
            },
            |t| {
                typer.dispatch(
                    Some(&law::hash(Ty::Sym, Ty::Int)),
                    &Symbol::from(m),
                    None,
                    &[law::stamped(t)],
                )
            },
        );
    }
    s.alternative(
        "dispatch/Hash/fetch/default",
        "body/send.rs:1023",
        Ty::Int,
        |t| {
            typer.dispatch(
                Some(&law::hash(Ty::Sym, Ty::Int)),
                &Symbol::from("fetch"),
                None,
                &[law::key(), law::stamped(t)],
            )
        },
    );
    s.alternative(
        "dispatch/Array/fetch/default",
        "body/send.rs:1023",
        Ty::Int,
        |t| {
            typer.dispatch(
                Some(&law::arr(Ty::Int)),
                &Symbol::from("fetch"),
                None,
                &[law::key(), law::stamped(t)],
            )
        },
    );
    for m in ["map", "flat_map", "filter_map", "sum", "fetch"] {
        s.transfer(
            &format!("dispatch/Array/{m}/block"),
            "body/send.rs:1023",
            Demand::Member,
            |t| {
                typer.dispatch(
                    Some(&law::arr(Ty::Int)),
                    &Symbol::from(m),
                    Some(t),
                    &[law::key()],
                )
            },
        );
    }
    // Fn.call consumes a known function's return slot, not an unshaped receiver.
    s.transfer(
        "dispatch/Fn/call/ret",
        "body/send.rs:1023",
        Demand::Member,
        |t| {
            typer.dispatch(
                Some(&law::function(t.clone())),
                &Symbol::from("call"),
                None,
                &[],
            )
        },
    );
    // Exact yielded-parameter projection, no AST evaluation or app analysis.
    for m in ["each", "each_with_index", "map", "then"] {
        for i in 0..2 {
            s.check(
                &format!("block_params_for/{m}/{i}"),
                "body/send.rs:487",
                Demand::Whole,
                |t| {
                    typer
                        .block_params_for(Some(t), &Symbol::from(m), false)
                        .and_then(|ts| ts.get(i).cloned())
                },
            );
        }
    }
}

fn array_names() -> Vec<&'static str> {
    let mut out = vec![
        "length",
        "size",
        "count",
        "first",
        "last",
        "second",
        "third",
        "fourth",
        "fifth",
        "[]",
        "map",
        "collect",
        "map!",
        "collect!",
        "filter_map",
        "index_with",
        "flat_map",
        "collect_concat",
        "partition",
        "each_slice",
        "each_cons",
        "flatten",
        "each",
        "reverse_each",
        "select",
        "filter",
        "reject",
        "sort",
        "sort_by",
        "reverse",
        "compact",
        "uniq",
        "drop",
        "take",
        "drop_while",
        "take_while",
        "without",
        "excluding",
        "including",
        "compact_blank",
        "compact_blank!",
        "delete",
        "delete_at",
        "pop",
        "shift",
        "sample",
        "index",
        "find_index",
        "dup",
        "clone",
        "deep_dup",
        "clear",
        "+",
        "-",
        "&",
        "|",
        "<<",
        "concat",
        "push",
        "unshift",
        "prepend",
        "append",
        "*",
        "any?",
        "all?",
        "none?",
        "one?",
        "empty?",
        "include?",
        "many?",
        "sum",
        "exclude?",
        "intersect?",
        "to_set",
        "lazy",
        "to_json",
        "as_json",
        "find",
        "detect",
        "max",
        "min",
        "max_by",
        "min_by",
        "each_with_index",
        "keep_if",
        "delete_if",
        "select!",
        "reject!",
        "sort!",
        "uniq!",
        "compact!",
        "reverse!",
        "sort_by!",
        "insert",
        "with_index",
        "group_by",
        "index_by",
        "tally",
        "inject",
        "reduce",
        "each_with_object",
        "with_object",
        "to_sentence",
        "to_h",
        "to_a",
        "join",
        "pack",
        "[]=",
        "store",
        "where",
        "model",
        "arel",
        "missing_method",
    ];
    // New relation-catalog entries automatically join the law surface.
    out.extend(
        crate::catalog::AR_CATALOG
            .iter()
            .filter(|entry| entry.receiver == crate::catalog::ReceiverContext::Relation)
            .map(|entry| entry.name),
    );
    out.sort_unstable();
    out.dedup();
    out
}
fn hash_names() -> Vec<&'static str> {
    vec![
        "[]",
        "[]=",
        "store",
        "delete",
        "clear",
        "to_a",
        "dup",
        "clone",
        "deep_dup",
        "value?",
        "has_value?",
        "member?",
        "each",
        "each_pair",
        "length",
        "size",
        "count",
        "values",
        "empty?",
        "any?",
        "none?",
        "all?",
        "one?",
        "key?",
        "has_key?",
        "include?",
        "keys",
        "key",
        "fetch",
        "merge",
        "deep_merge",
        "to_h",
        "map",
        "collect",
        "transform_values",
        "transform_values!",
        "symbolize_keys",
        "deep_symbolize_keys",
        "symbolize_keys!",
        "deep_symbolize_keys!",
        "with_indifferent_access",
        "stringify_keys",
        "deep_stringify_keys",
        "stringify_keys!",
        "deep_stringify_keys!",
        "transform_keys",
        "transform_keys!",
        "except",
        "except!",
        "slice",
        "slice!",
        "without",
        "select",
        "filter",
        "reject",
        "compact",
        "compact!",
        "select!",
        "filter!",
        "reject!",
        "keep_if",
        "delete_if",
        "merge!",
        "update",
        "with_defaults",
        "with_defaults!",
        "reverse_merge",
        "reverse_merge!",
        "compact_blank",
        "compact_blank!",
        "to_unsafe_h",
        "permit!",
        "values_at",
        "fetch_values",
        "to_query",
        "to_param",
        "sort",
        "sort_by",
        "min_by",
        "max_by",
        "find",
        "detect",
        "invert",
        "flat_map",
        "reduce",
        "inject",
        "each_with_object",
        "sum",
        "dig",
        "each_value",
        "each_key",
        "each_with_index",
        "expect",
        "require",
        "permit",
        "to_xml",
        "first",
        "assoc",
        "rassoc",
        "missing_method",
    ]
}


#[test]
fn strict_catalog_pending_boundaries() {
    if !strict_catalog_on() { return; }
    let classes = std::collections::HashMap::new();
    let typer = BodyTyper::new(&classes);
    for pending in [law::p(), law::raw(vec![law::p(), law::p()])] {
        for method in ["to_h", "to_a", "to_ary", "to_s", "class", "to_param"] {
            let result = typer.dispatch(Some(&pending), &Symbol::from(method), None, &[]);
            assert!(law::only_pending(&result), "receiver {method}: {result:?}");
        }
        for receiver in [Ty::Int, Ty::Float] {
            for method in ["+", "-", "*", "/"] {
                let result = typer.dispatch(Some(&receiver), &Symbol::from(method), None, &[law::stamped(&pending)]);
                assert!(law::only_pending(&result), "numeric {method}: {result:?}");
            }
        }
        for receiver in [law::arr(Ty::Int), Ty::Relation { of: ClassId(Symbol::from("Model")) }] {
            for method in ["first", "last", "take", "sample", "[]", "slice"] {
                let result = typer.dispatch(Some(&receiver), &Symbol::from(method), None, &[law::stamped(&pending)]);
                assert!(law::only_pending(&result), "collection {method}: {result:?}");
            }
        }
        for method in ["new", "civil", "parse", "strptime", "iso8601", "today"] {
            assert!(date_constructor(&Symbol::from(method), &[law::stamped(&pending)]).is_some_and(|t| law::only_pending(&t)));
        }
        assert!(date_method(&Symbol::from("in_time_zone"), &[law::stamped(&pending)]).is_some_and(|t| law::only_pending(&t)));
        let result = hash_method(&Symbol::from("deep_merge"), &Ty::Sym, &Ty::Int, None, &[law::stamped(&pending)]);
        assert!(law::only_pending(&result));
        assert!(object_protocol_method(Some(&pending), &Symbol::from("to_param"), None, false).is_some_and(|t| law::only_pending(&t)));
    }
    // A known outer shape, an omitted optional argument, and value top are
    // not waits. The catalog must still provide its ordinary answers.
    assert_eq!(typer.dispatch(Some(&law::arr(law::p())), &Symbol::from("length"), None, &[]), Ty::Int);
    assert_eq!(date_constructor(&Symbol::from("today"), &[]), Some(Ty::Date));
    assert_eq!(date_constructor(&Symbol::from("new"), &[law::stamped(&law::u())]), Some(Ty::Date));
    assert_eq!(typer.dispatch(Some(&Ty::Int), &Symbol::from("+"), None, &[law::stamped(&bigdecimal())]), bigdecimal());
}

#[test]
fn strict_catalog_flag_off_legacy_boundary() {
    if strict_catalog_on() { return; }
    let classes = std::collections::HashMap::new();
    let typer = BodyTyper::new(&classes);
    assert_eq!(typer.dispatch(Some(&Ty::Int), &Symbol::from("+"), None, &[law::stamped(&law::p())]), Ty::Int);
    assert_eq!(date_constructor(&Symbol::from("today"), &[law::stamped(&law::p())]), Some(Ty::Date));
}
