use super::super::fold::{SiteId, SlotKey};
use super::*;

#[derive(Default)]
pub(super) struct SiteMap {
    forward: HashMap<SiteId, Value>,
    reverse: HashMap<String, SiteId>,
    files: HashMap<u32, (String, u64)>,
}

impl SiteMap {
    pub(super) fn new(app: &App) -> Self {
        Self {
            files: app
                .sources
                .iter()
                .enumerate()
                .map(|(i, f)| {
                    (
                        (i + 1) as u32,
                        (
                            f.path.clone(),
                            fingerprint::hash(&Value::String(f.text.clone())),
                        ),
                    )
                })
                .collect(),
            ..Self::default()
        }
    }

    pub(super) fn add_unit(&mut self, app: &App, u: &Unit, key: &str) {
        let mut roots = vec![super::super::sccq::unit_body(app, u.family, u.ci, u.mi)];
        let defaults: Vec<&Expr> = match u.family {
            Family::Lib => app.library_classes[u.ci].methods[u.mi]
                .params
                .iter()
                .filter_map(|p| p.default.as_ref())
                .collect(),
            Family::ModelMethod => app.models[u.ci]
                .methods()
                .nth(u.mi)
                .unwrap()
                .params
                .iter()
                .filter_map(|p| p.default.as_ref())
                .collect(),
            Family::CtrlAction => {
                let a = app.controllers[u.ci].actions().nth(u.mi).unwrap();
                a.opt_params
                    .iter()
                    .map(|(_, e)| e)
                    .chain(a.kw_params.iter().filter_map(|(_, e)| e.as_ref()))
                    .collect()
            }
            Family::CtrlClassMethod => app.controllers[u.ci]
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
                .unwrap()
                .params
                .iter()
                .filter_map(|p| p.default.as_ref())
                .collect(),
            Family::ModelScope => Vec::new(),
        };
        roots.extend(defaults);
        fn walk(map: &mut SiteMap, expr: &Expr, key: &str, root: usize, position: &mut usize) {
            let site = super::super::fold::site_of(&expr.span);
            let wire = serde_json::json!({"unit": key, "root": root, "position": *position});
            if !expr.span.is_synthetic() {
                map.forward.entry(site).or_insert_with(|| wire.clone());
                map.reverse.insert(wire.to_string(), site);
            }
            *position += 1;
            expr.node
                .for_each_child(&mut |e| walk(map, e, key, root, position));
        }
        for (root, expr) in roots.into_iter().enumerate() {
            walk(self, expr, key, root, &mut 0);
        }
    }

    fn site(&self, site: SiteId) -> Option<Value> {
        if site.0 == 0 || site.0 == u32::MAX {
            return Some(serde_json::json!({"sentinel": site}));
        }
        if let Some(wire) = self.forward.get(&site) {
            return Some(wire.clone());
        }
        let (path, file_hash) = self.files.get(&site.0)?;
        Some(serde_json::json!({"file": path, "hash": file_hash, "start": site.1, "end": site.2}))
    }

    fn resolve(&self, wire: &Value) -> Option<SiteId> {
        if let Some(site) = wire.get("sentinel") {
            return serde_json::from_value(site.clone()).ok();
        }
        if let Some(site) = self.reverse.get(&wire.to_string()) {
            return Some(*site);
        }
        let path = wire.get("file")?.as_str()?;
        let hash = wire.get("hash")?.as_u64()?;
        let (&file, _) = self
            .files
            .iter()
            .find(|(_, (p, h))| p == path && *h == hash)?;
        Some((
            file,
            u32::try_from(wire.get("start")?.as_u64()?).ok()?,
            u32::try_from(wire.get("end")?.as_u64()?).ok()?,
        ))
    }
}

pub(super) fn slot_key(key: &SlotKey) -> Option<Value> {
    let mut value = serde_json::to_value(key).ok()?;
    if let Some(map) = value.as_object_mut() {
        for nested in map.values_mut().filter_map(Value::as_object_mut) {
            if let Some(site) = nested.get_mut("site") {
                let raw: SiteId = serde_json::from_value(site.clone()).ok()?;
                *site = SESSION.with(|s| s.borrow().as_ref()?.sites.site(raw))?;
            }
        }
    }
    Some(value)
}

pub(super) fn decode_key(value: &Value) -> Option<SlotKey> {
    let mut value = value.clone();
    if let Some(map) = value.as_object_mut() {
        for nested in map.values_mut().filter_map(Value::as_object_mut) {
            if let Some(site) = nested.get_mut("site") {
                let raw = SESSION.with(|s| s.borrow().as_ref()?.sites.resolve(site))?;
                *site = serde_json::to_value(raw).ok()?;
            }
        }
    }
    serde_json::from_value(value).ok()
}

pub(super) fn encode(mut value: Value) -> Option<Value> {
    fn walk(value: &mut Value) -> Option<()> {
        match value {
            Value::Object(map) => {
                if map.get("kind").and_then(Value::as_str) == Some("rec") {
                    let slot = u32::try_from(map.get("slot")?.as_u64()?).ok()?;
                    let key = super::super::fold::key_of(slot)?;
                    map.insert(
                        "slot".into(),
                        serde_json::json!({"warm_slot": slot_key(&key)?}),
                    );
                    return Some(());
                }
                if map.len() == 3
                    && ["file", "start", "end"]
                        .iter()
                        .all(|key| map.get(*key).is_some_and(Value::is_u64))
                {
                    let span: crate::span::Span =
                        serde_json::from_value(Value::Object(map.clone())).ok()?;
                    let site = SESSION.with(|s| {
                        s.borrow()
                            .as_ref()?
                            .sites
                            .site(super::super::fold::site_of(&span))
                    })?;
                    *value = serde_json::json!({"warm_span": site});
                    return Some(());
                }
                for v in map.values_mut() {
                    walk(v)?;
                }
            }
            Value::Array(items) => {
                for v in items {
                    walk(v)?;
                }
            }
            _ => {}
        }
        Some(())
    }
    walk(&mut value)?;
    Some(value)
}

fn span_marker(value: &Value) -> Option<&Value> {
    let map = value.as_object()?;
    if map.len() != 1 {
        return None;
    }
    let site = map.get("warm_span")?;
    (site.get("unit").is_some_and(Value::is_string)
        || site.get("file").is_some_and(Value::is_string)
        || site.get("sentinel").is_some_and(Value::is_array))
    .then_some(site)
}

fn valid(value: &Value) -> bool {
    if let Some(site) = span_marker(value) {
        return SESSION.with(|s| {
            s.borrow()
                .as_ref()
                .is_some_and(|s| s.sites.resolve(site).is_some())
        });
    }
    match value {
        Value::Object(map) if map.get("kind").and_then(Value::as_str) == Some("rec") => map
            .get("slot")
            .and_then(|slot| slot.get("warm_slot"))
            .and_then(decode_key)
            .is_some(),
        Value::Object(map) => map.values().all(valid),
        Value::Array(items) => items.iter().all(valid),
        _ => true,
    }
}

/// Resolve every source/key dependency before restoring any fold identity.
pub(super) fn ready(write: &Value, writes: &[(Value, Value)]) -> bool {
    valid(write)
        && writes
            .iter()
            .all(|(key, value)| decode_key(key).is_some() && valid(value))
}

pub(super) fn decode(mut value: Value) -> Option<Value> {
    fn walk(value: &mut Value) -> Option<()> {
        if let Some(site) = span_marker(value) {
            let raw = SESSION.with(|s| s.borrow().as_ref()?.sites.resolve(site))?;
            *value = serde_json::json!({"file": raw.0, "start": raw.1, "end": raw.2});
            return Some(());
        }
        match value {
            Value::Object(map) => {
                if map.get("kind").and_then(Value::as_str) == Some("rec") {
                    let key = decode_key(map.get("slot")?.get("warm_slot")?)?;
                    map.insert(
                        "slot".into(),
                        Value::from(super::super::fold::warm_intern(key)),
                    );
                    return Some(());
                }
                for v in map.values_mut() {
                    walk(v)?;
                }
            }
            Value::Array(items) => {
                for v in items {
                    walk(v)?;
                }
            }
            _ => {}
        }
        Some(())
    }
    walk(&mut value)?;
    Some(value)
}

pub(super) fn restore(write: &Value, current: &Expr) -> Option<Expr> {
    // Spans are resolved by unit and syntax position. Rebind source-local
    // VarIds from the current syntax rather than importing an old file's ids.
    let mut output = write.clone();
    let input = serde_json::to_value(current).ok()?;
    fn bindings(value: &Value, out: &mut BTreeMap<String, Vec<Value>>) {
        match value {
            Value::Object(map) => {
                if matches!(map.get("kind").and_then(Value::as_str), Some("var" | "let")) {
                    if let (Some(name), Some(id)) =
                        (map.get("name").and_then(Value::as_str), map.get("id"))
                    {
                        out.entry(name.to_owned()).or_default().push(id.clone());
                    }
                }
                for v in map.values() {
                    bindings(v, out);
                }
            }
            Value::Array(items) => {
                for v in items {
                    bindings(v, out);
                }
            }
            _ => {}
        }
    }
    fn rebind(value: &mut Value, ids: &mut BTreeMap<String, Vec<Value>>) -> Option<()> {
        match value {
            Value::Object(map) => {
                if matches!(map.get("kind").and_then(Value::as_str), Some("var" | "let"))
                    && map.contains_key("id")
                    && map.get("name").is_some_and(Value::is_string)
                {
                    let name = map.get("name")?.as_str()?;
                    let ids = ids.get_mut(name)?;
                    if ids.is_empty() {
                        return None;
                    }
                    map.insert("id".into(), ids.remove(0));
                }
                for v in map.values_mut() {
                    rebind(v, ids)?;
                }
            }
            Value::Array(items) => {
                for v in items {
                    rebind(v, ids)?;
                }
            }
            _ => {}
        }
        Some(())
    }
    let mut ids = BTreeMap::new();
    bindings(&input, &mut ids);
    rebind(&mut output, &mut ids)?;
    serde_json::from_value(decode(output)?).ok()
}

pub(super) fn apply_fold_writes(writes: &[(Value, Value)]) -> bool {
    let Some(decoded): Option<Vec<_>> = writes
        .iter()
        .map(|(key, value)| {
            Some((
                decode_key(key)?,
                serde_json::from_value::<Ty>(decode(value.clone())?).ok()?,
            ))
        })
        .collect()
    else {
        return false;
    };
    for (key, value) in decoded {
        super::super::fold::warm_write(key, value);
    }
    true
}
