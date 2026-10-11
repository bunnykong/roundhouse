use super::*;
use std::hash::{Hash, Hasher};

pub(super) fn hash(value: &Value) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_vec(value).unwrap().hash(&mut h);
    h.finish()
}

fn strip(value: &mut Value, syntax: bool) {
    match value {
        Value::Object(map) => {
            map.remove("span");
            map.remove("name_span");
            if syntax && map.contains_key("node") {
                for key in [
                    "ty",
                    "effects",
                    "diagnostic",
                    "decisions",
                    "leading_blank_line",
                ] {
                    map.remove(key);
                }
            }
            // Binding identities are local to ingest; names carry syntax.
            if syntax && map.get("id").is_some_and(Value::is_number) {
                map.remove("id");
            }
            for v in map.values_mut() {
                strip(v, syntax);
            }
        }
        Value::Array(items) => {
            for v in items {
                strip(v, syntax);
            }
        }
        _ => {}
    }
}

pub(super) fn body_hash(mut value: Value) -> u64 {
    strip(&mut value, true);
    hash(&value)
}

pub(super) fn unit(app: &App, u: &Unit) -> u64 {
    let _profile = phase(Phase::Fingerprint);
    let value = match u.family {
        Family::Lib => serde_json::to_value(&app.library_classes[u.ci].methods[u.mi]),
        Family::ModelMethod => serde_json::to_value(app.models[u.ci].methods().nth(u.mi).unwrap()),
        Family::ModelScope => serde_json::to_value(app.models[u.ci].scopes().nth(u.mi).unwrap()),
        Family::CtrlAction => {
            serde_json::to_value(app.controllers[u.ci].actions().nth(u.mi).unwrap())
        }
        Family::CtrlClassMethod => serde_json::to_value(
            app.controllers[u.ci]
                .body
                .iter()
                .filter_map(|b| match b {
                    crate::dialect::ControllerBodyItem::ClassMethod {
                        method,
                        configuration_slot: None,
                        ..
                    } => Some(method),
                    _ => None,
                })
                .nth(u.mi)
                .unwrap(),
        ),
    }
    .unwrap();
    body_hash(value)
}

pub(super) fn environment(app: &App) -> u64 {
    let _profile = phase(Phase::Fingerprint);
    // Global passes rebuild these non-unit inputs. Schema, route, view and
    // test-overlay edits start with a cold trace; class declarations and
    // constants are checked by the evaluations' input and read hashes.
    let mut value = serde_json::to_value(app).unwrap();
    let map = value.as_object_mut().unwrap();
    for key in [
        "sources",
        "root",
        "models",
        "library_classes",
        "controllers",
        "const_resolver",
    ] {
        map.remove(key);
    }
    strip(&mut value, false);
    hash(&value)
}

pub(super) fn value_hash(value: Option<&Ty>) -> u64 {
    let _profile = phase(Phase::ValueFingerprint);
    let value = serde_json::to_value(value).unwrap();
    hash(&encode(value).unwrap_or(Value::Null))
}

fn sorted_names(names: &std::collections::HashSet<Symbol>) -> Vec<&str> {
    let mut out: Vec<_> = names.iter().map(Symbol::as_str).collect();
    out.sort_unstable();
    out
}

pub(super) fn class_hash(info: Option<&ClassInfo>, names: &[Symbol], wild: bool) -> u64 {
    let _profile = phase(Phase::ClassFingerprint);
    let Some(c) = info else {
        return hash(&Value::Null);
    };
    let methods = |m: &HashMap<Symbol, Ty>| -> BTreeMap<String, Value> {
        m.iter()
            .filter(|(n, _)| wild || names.contains(n))
            .map(|(n, t)| {
                (
                    n.as_str().to_owned(),
                    encode(serde_json::to_value(t).unwrap()).unwrap_or(Value::Null),
                )
            })
            .collect()
    };
    let mut extensions: Vec<_> = c
        .assoc_extensions
        .iter()
        .map(|((a, m), t)| (a.as_str(), m.as_str(), value_hash(Some(t))))
        .collect();
    extensions.sort_unstable();
    let value = serde_json::json!({
        "is_module": c.is_module, "scheduler_index": c.sccq_idx,
        "constants": encode(serde_json::to_value(&c.constants).unwrap()),
        "table": c.table, "attributes": encode(serde_json::to_value(&c.attributes).unwrap()),
        "class_methods": methods(&c.class_methods), "instance_methods": methods(&c.instance_methods),
        "constructor": c.declares_constructor, "derived": sorted_names(&c.relation_derived),
        "materializing": sorted_names(&c.materializing_scopes), "block": sorted_names(&c.block_value_methods),
        "class_kinds": c.class_method_kinds, "instance_kinds": c.instance_method_kinds,
        "parent": c.parent, "includes": c.includes, "extensions": extensions,
        "gem_boundary": c.gem_boundary, "open": c.open, "app_declared": c.app_declared,
    });
    hash(&value)
}

pub(super) fn inputs(
    expr: &Expr,
    ctx: &Ctx,
    classes: &HashMap<ClassId, ClassInfo>,
    extra: &components::Extra,
) -> (u64, Option<u64>) {
    let _profile = phase(Phase::InputFingerprint);
    components::count("input_calls", 1);
    let stamps = {
        let _profile = phase(Phase::FpExpression);
        let mut stamps = serde_json::to_value(expr).unwrap();
        strip(&mut stamps, false);
        let bytes = serde_json::to_vec(&encode(stamps)).unwrap();
        components::bytes("expression_bytes", &bytes);
        bytes
    };
    let context = {
        let _profile = phase(Phase::FpContext);
        let mut objects: Vec<_> = ctx.class_objects.iter().map(Symbol::as_str).collect();
        objects.sort_unstable();
        let context = [
            serde_json::to_vec(&[ctx.annotate_self_dispatch, ctx.in_view, ctx.class_side,
                ctx.claimed_macro_template, ctx.instance_body]).unwrap(),
            serde_json::to_vec(&encode(serde_json::to_value(&ctx.ivar_bindings).unwrap())).unwrap(),
            serde_json::to_vec(&encode(serde_json::to_value(&ctx.local_bindings).unwrap())).unwrap(),
            serde_json::to_vec(&objects).unwrap(),
            serde_json::to_vec(&encode(serde_json::to_value(&ctx.self_ty).unwrap())).unwrap(),
        ];
        for bytes in &context { components::bytes("context_bytes", bytes); }
        context
    };
    let keys = components::keys(classes);
    let mode = components::mode();
    // serde_json's pinned map representation sorts these fields lexically.
    // Copy canonical fragments; never substitute a body/syntax hash for stamps.
    let bytes = {
        let _profile = phase(Phase::FpFinalEncode);
        let parts: [(&str, &[u8]); 9] = [
            ("extra", &extra.bytes), ("flags", &context[0]), ("ivars", &context[1]),
            ("keys", &keys), ("locals", &context[2]), ("mode", &mode),
            ("objects", &context[3]), ("self", &context[4]), ("stamps", &stamps),
        ];
        let mut bytes = Vec::with_capacity(parts.iter().map(|(k, b)| k.len() + b.len() + 4).sum::<usize>() + 2);
        bytes.push(b'{');
        for (i, (key, value)) in parts.iter().enumerate() {
            if i != 0 { bytes.push(b','); }
            bytes.push(b'"'); bytes.extend_from_slice(key.as_bytes());
            bytes.extend_from_slice(b"\":"); bytes.extend_from_slice(value);
        }
        bytes.push(b'}');
        components::bytes("final_bytes", &bytes);
        bytes
    };
    let current = {
        let _profile = phase(Phase::FpFinalHash);
        hash_bytes(&bytes)
    };
    let original = components::audit().then(|| {
        let _profile = phase(Phase::FpAudit);
        let old_value = original_inputs(expr, ctx, classes, extra.original.as_ref().unwrap());
        let old = serde_json::to_vec(&old_value).unwrap();
        assert_eq!(bytes, old, "RH_WARM_FP_AUDIT: canonical input bytes");
        let old_hash = hash(&old_value);
        assert_eq!(current, old_hash, "RH_WARM_FP_AUDIT: input hash");
        components::count("input_audit_comparisons", 1);
        components::bytes("audit_bytes", &old);
        old_hash
    });
    (current, original)
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

// Retained verbatim input construction, for the fail-hard dual computation.
fn original_inputs(expr: &Expr, ctx: &Ctx, classes: &HashMap<ClassId, ClassInfo>, extra: &Value) -> Value {
    let mut stamps = serde_json::to_value(expr).unwrap();
    strip(&mut stamps, false);
    let mut keys: Vec<_> = classes.keys().map(|c| c.0.as_str()).collect();
    keys.sort_unstable();
    let mut objects: Vec<_> = ctx.class_objects.iter().map(Symbol::as_str).collect();
    objects.sort_unstable();
    serde_json::json!({
        "stamps": encode(stamps), "self": encode(serde_json::to_value(&ctx.self_ty).unwrap()),
        "ivars": encode(serde_json::to_value(&ctx.ivar_bindings).unwrap()),
        "locals": encode(serde_json::to_value(&ctx.local_bindings).unwrap()),
        "objects": objects, "keys": keys, "extra": extra,
        "mode": super::super::fold::warm_structure(),
        "flags": [ctx.annotate_self_dispatch, ctx.in_view, ctx.class_side,
            ctx.claimed_macro_template, ctx.instance_body],
    })
}

pub(super) fn covered(
    reads: &BTreeMap<String, u64>,
    ctx: &Ctx,
    classes: &HashMap<ClassId, ClassInfo>,
    extra: &Value,
) -> bool {
    let names = super::names();
    reads.iter().all(|(key, expected)| {
        let Some((kind, name)) = key.split_once(':') else {
            return false;
        };
        let current = match kind {
            "class" | "wild" => class_hash(
                classes.get(&ClassId(Symbol::from(name))),
                &names,
                kind == "wild",
            ),
            "const" => value_hash(ctx.constants.get(&Symbol::from(name))),
            "own" => value_hash(ctx.constants.get_own(&Symbol::from(name))),
            "global" => value_hash(ctx.constants.get_global(&Symbol::from(name))),
            "decl" => {
                if !name.parse::<u64>().is_ok_and(|id| id != 0) {
                    return false;
                }
                extra
                    .get("constants")
                    .and_then(Value::as_array)
                    .and_then(|values| values.iter().find(|pair| pair[0].as_str() == Some(name)))
                    .and_then(|pair| pair[1].as_u64())
                    .unwrap_or_else(|| value_hash(None))
            }
            "fold" => {
                let Ok(wire) = serde_json::from_str(name) else {
                    return false;
                };
                let Some(key) = wire::decode_key(&wire) else {
                    return false;
                };
                value_hash(super::super::fold::warm_value(&key, classes).as_ref())
            }
            _ => return false,
        };
        current == *expected
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::{ExprNode, Literal};

    fn expr() -> Expr {
        Expr::new(crate::span::Span::synthetic(), ExprNode::Lit { value: Literal::Int { value: 1 } })
    }

    fn input(expr: &Expr, ctx: &Ctx, classes: &HashMap<ClassId, ClassInfo>, generation: u64, value: Value) -> u64 {
        let extra = components::extra(Some(generation), || value);
        let (current, original) = inputs(expr, ctx, classes, &extra);
        assert_eq!(original, Some(current), "the canary must exercise dual computation");
        current
    }

    #[test]
    fn warm_fp_receiver_side_and_instance_body_are_never_memoized() {
        components::start_audit_for_test();
        let expr = expr();
        let mut ctx = Ctx::default();
        let classes = HashMap::new();
        let a = input(&expr, &ctx, &classes, 1, Value::Null);
        ctx.class_side = true;
        let b = input(&expr, &ctx, &classes, 1, Value::Null);
        ctx.instance_body = true;
        let c = input(&expr, &ctx, &classes, 1, Value::Null);
        assert_ne!(a, b); assert_ne!(b, c);
        let stats = components::stop();
        assert_eq!(stats["counters"]["extra_generation_hits"], 2);
        assert_eq!(stats["counters"]["input_audit_comparisons"], 3);
    }

    #[test]
    fn warm_fp_constant_generation_and_scope_guards_follow_mutation() {
        components::start_audit_for_test();
        let classes = HashMap::new();
        let mut ctx = Ctx::default();
        let before = serde_json::json!({"constants": [["1", value_hash(Some(&Ty::Int))]]});
        let after = serde_json::json!({"constants": [["1", value_hash(Some(&Ty::Str))]]});
        let a = input(&expr(), &ctx, &classes, 1, before.clone());
        let b = input(&expr(), &ctx, &classes, 2, after.clone());
        assert_ne!(a, b);
        let reads = BTreeMap::from([("decl:1".into(), value_hash(Some(&Ty::Int)))]);
        assert!(covered(&reads, &ctx, &classes, &before));
        assert!(!covered(&reads, &ctx, &classes, &after));
        ctx.constants = super::super::super::body::ConstScope::global(HashMap::from([(Symbol::from("VALUE"), Ty::Int)]));
        let reads = BTreeMap::from([("global:VALUE".into(), value_hash(Some(&Ty::Int)))]);
        assert!(covered(&reads, &ctx, &classes, &Value::Null));
        ctx.constants = super::super::super::body::ConstScope::global(HashMap::from([(Symbol::from("VALUE"), Ty::Str)]));
        assert!(!covered(&reads, &ctx, &classes, &Value::Null));
        components::stop();
    }

    #[test]
    #[ignore = "requires RH_FOLD=1; warm_tests explicitly runs this canary"]
    fn warm_fp_alias_recursion_deactivation_and_reset_advance_generations() {
        let fold = super::super::super::fold::warm_structure;
        super::super::super::fold::reset();
        components::start_audit_for_test();
        let classes = HashMap::new();
        let ctx = Ctx::default();
        let module = ClassId(Symbol::from("Module"));
        let other = ClassId(Symbol::from("Other"));
        let class = ClassId(Symbol::from("Class"));
        let method = Symbol::from("answer");
        let a = input(&expr(), &ctx, &classes, 1, Value::Null);
        super::super::super::fold::add_rec_methods(vec![(module.clone(), method.clone())]);
        let b = input(&expr(), &ctx, &classes, 1, Value::Null);
        assert_ne!(a, b);
        super::super::super::fold::note_alias(&class, &method, &module);
        let c = input(&expr(), &ctx, &classes, 1, Value::Null);
        let generation = super::super::super::fold::warm_structure_generation();
        super::super::super::fold::note_alias(&class, &method, &module);
        assert_eq!(generation, super::super::super::fold::warm_structure_generation());
        assert_eq!(c, input(&expr(), &ctx, &classes, 1, Value::Null));
        super::super::super::fold::note_alias(&class, &method, &other);
        let d = input(&expr(), &ctx, &classes, 1, Value::Null);
        assert_ne!(c, d);
        assert_ne!(b, c);
        super::super::super::fold::deactivate();
        let e = input(&expr(), &ctx, &classes, 1, Value::Null);
        assert_ne!(d, e);
        super::super::super::fold::reset();
        assert_eq!(a, input(&expr(), &ctx, &classes, 1, Value::Null));
        assert_eq!(fold()["active"], false);
        components::stop();
    }

    #[test]
    fn warm_fp_same_length_registry_key_replacement_is_a_new_generation() {
        components::start_audit_for_test();
        let ctx = Ctx::default();
        let mut classes = HashMap::from([(ClassId(Symbol::from("First")), ClassInfo::default())]);
        let a = input(&expr(), &ctx, &classes, 1, Value::Null);
        classes.clear();
        classes.insert(ClassId(Symbol::from("Second")), ClassInfo::default());
        let b = input(&expr(), &ctx, &classes, 1, Value::Null);
        assert_ne!(a, b);
        assert_eq!(components::stop()["counters"]["class_key_generations"], 2);
    }

    #[test]
    #[should_panic(expected = "RH_WARM_FP_AUDIT: extra bytes")]
    fn warm_fp_audit_fails_hard_on_a_missing_constant_generation() {
        components::start_audit_for_test();
        components::extra(Some(7), || serde_json::json!({"constants": [["1", 1]]}));
        components::extra(Some(7), || serde_json::json!({"constants": [["1", 2]]}));
    }

    #[test]
    fn warm_fp_expression_stamps_and_context_values_are_fresh() {
        components::start_audit_for_test();
        let classes = HashMap::new();
        let mut ctx = Ctx::default();
        let mut expr = expr();
        expr.ty = Some(Ty::Int);
        let a = input(&expr, &ctx, &classes, 1, Value::Null);
        expr.ty = Some(Ty::Str);
        let b = input(&expr, &ctx, &classes, 1, Value::Null);
        ctx.local_bindings.insert(Symbol::from("local"), Ty::Int);
        let c = input(&expr, &ctx, &classes, 1, Value::Null);
        ctx.ivar_bindings.insert(Symbol::from("@value"), Ty::Str);
        let d = input(&expr, &ctx, &classes, 1, Value::Null);
        assert_ne!(a, b); assert_ne!(b, c); assert_ne!(c, d);
        components::stop();
    }
}
