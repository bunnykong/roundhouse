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
    extra: &Value,
) -> u64 {
    let _profile = phase(Phase::InputFingerprint);
    let mut stamps = serde_json::to_value(expr).unwrap();
    strip(&mut stamps, false);
    let mut keys: Vec<_> = classes.keys().map(|c| c.0.as_str()).collect();
    keys.sort_unstable();
    let mut objects: Vec<_> = ctx.class_objects.iter().map(Symbol::as_str).collect();
    objects.sort_unstable();
    hash(&serde_json::json!({
        "stamps": encode(stamps), "self": encode(serde_json::to_value(&ctx.self_ty).unwrap()),
        "ivars": encode(serde_json::to_value(&ctx.ivar_bindings).unwrap()),
        "locals": encode(serde_json::to_value(&ctx.local_bindings).unwrap()),
        "objects": objects, "keys": keys, "extra": extra,
        "mode": super::super::fold::warm_structure(),
        "flags": [ctx.annotate_self_dispatch, ctx.in_view, ctx.class_side,
            ctx.claimed_macro_template, ctx.instance_body],
    }))
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
