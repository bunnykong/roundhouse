//! Selected type exporter for public reproductions.
//! `sound-types APP [SELECTION.json]`: with a selection (the selected-slot
//! format, alias -> [Class, method, index|"return"]) print the selected types
//! as JSON; without one, print every user method's parameter row and return.
//! `SOUND_VAR=1` renders an unbound `Var` as `?var` instead of `untyped`.
use roundhouse::{analyze::Analyzer, ident::{ClassId, Symbol}, ty::Ty};
use std::{collections::BTreeMap, path::Path, process::ExitCode};

fn render(t: &Ty, var: bool) -> String {
    let r = |t: &Ty| render(t, var);
    match t {
        Ty::Int => "Integer".into(),
        Ty::Float => "Float".into(),
        Ty::Bool => "bool".into(),
        Ty::Str => "String".into(),
        Ty::Sym => "Symbol".into(),
        Ty::Nil => "nil".into(),
        Ty::Array { elem } => format!("Array[{}]", r(elem)),
        Ty::Hash { key, value } => format!("Hash[{}, {}]", r(key), r(value)),
        Ty::Tuple { elems } => format!("[{}]", elems.iter().map(|e| r(e)).collect::<Vec<_>>().join(", ")),
        Ty::Record { row } => format!(
            "{{ {} }}",
            row.fields.iter().map(|(k, v)| format!("{}: {}", k.as_str(), r(v))).collect::<Vec<_>>().join(", ")
        ),
        Ty::Union { variants } => variants.iter().map(|v| r(v)).collect::<Vec<_>>().join(" | "),
        Ty::Fn { ret, .. } => r(ret),
        Ty::Var { .. } if var => "?var".into(),
        Ty::Var { .. } if std::env::var("SOUND_MISSING").is_ok() => "__var__".into(),
        // ⊥ admits no value: a record that requires a key no runtime Hash has.
        Ty::Bottom if std::env::var("SOUND_MISSING").is_ok() => "__bot__".into(),
        Ty::Class { id, .. } if var => format!("<{}>", id.0.as_str()),
        _ => "untyped".into(),
    }
}

fn main() -> ExitCode {
    roundhouse::stack::run(|| {
        let args: Vec<String> = std::env::args().collect();
        assert!(args.len() == 2 || args.len() == 3, "sound-types APP [SELECTION.json] (public reproductions only)");
        let var = std::env::var("SOUND_VAR").is_ok();
        roundhouse::ingest::survey::activate();
        let mut app = roundhouse::ingest::app::ingest_app(Path::new(&args[1])).unwrap();
        let mut analyzer = Analyzer::new(&app);
        analyzer.analyze(&mut app);
        if args.len() == 3 {
            let selections: BTreeMap<String, serde_json::Value> =
                serde_json::from_str(&std::fs::read_to_string(&args[2]).unwrap()).unwrap();
            let mut result = BTreeMap::new(); let mut audit = BTreeMap::new();
            for (alias, spec) in selections {
                let class = ClassId(Symbol::from(spec[0].as_str().unwrap()));
                let method = Symbol::from(spec[1].as_str().unwrap());
                let param_row;
                let ty = if let Some(index) = spec[2].as_u64() {
                    param_row = analyzer.inferred_param_types(&class, &method);
                    param_row.as_deref().and_then(|row| row.get(index as usize))
                } else if spec[2].as_str() == Some("self_return") {
                    analyzer.class_registry().get(&class).and_then(|c| c.class_methods.get(&method))
                } else {
                    analyzer.class_registry().get(&class).and_then(|c| c.instance_methods.get(&method))
                };
                // `SOUND_MISSING=1`: an absent slot prints `__missing__` (the sweep counts it, then
                // checks it as `untyped`, the selected-slot convention).
                let missing = if std::env::var("SOUND_MISSING").is_ok() { "__missing__" } else { "untyped" };
                audit.insert(alias.clone(), dossier3_audit(ty));
                result.insert(alias, ty.map(|t| render(t, var)).unwrap_or(missing.into()));
            }
            std::fs::write(std::env::var("D3_AUDIT_PATH").unwrap(), serde_json::to_string_pretty(&audit).unwrap()).unwrap();
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
            return ExitCode::SUCCESS;
        }
        // `SOUND_COUNT=1`: aggregates only. Method returns that end as ⊥ at the top or at any depth.
        if std::env::var("SOUND_COUNT").is_ok() {
            fn has_bot(t: &Ty) -> bool {
                match t {
                    Ty::Bottom => true,
                    Ty::Array { elem } => has_bot(elem),
                    Ty::Hash { key, value } => has_bot(key) || has_bot(value),
                    Ty::Tuple { elems } => elems.iter().any(|e| has_bot(e)),
                    Ty::Union { variants } => variants.iter().any(|v| has_bot(v)),
                    Ty::Record { row } => row.fields.iter().any(|(_, v)| has_bot(v)),
                    Ty::Fn { ret, .. } => has_bot(ret),
                    _ => false,
                }
            }
            let (mut total, mut top, mut any, mut untyped_top) = (0u64, 0u64, 0u64, 0u64);
            for info in analyzer.class_registry().values() {
                for t in info.instance_methods.values().chain(info.class_methods.values()) {
                    total += 1;
                    let r = match t { Ty::Fn { ret, .. } => &**ret, other => other };
                    if matches!(r, Ty::Bottom) { top += 1 } else if has_bot(r) { any += 1 }
                    if matches!(r, Ty::Untyped { .. }) { untyped_top += 1 }
                }
            }
            println!("{{\"methods\":{total},\"bottom_return\":{top},\"bottom_inside\":{any},\"untyped_return\":{untyped_top}}}");
            return ExitCode::SUCCESS;
        }
        let wanted = std::env::var("SOUND_CLASSES").unwrap_or_else(|_| "TreesController,ApplicationHelper".into());
        for cname in wanted.split(',') {
            let class = ClassId(Symbol::from(cname));
            let Some(info) = analyzer.class_registry().get(&class) else { continue };
            let mut methods: Vec<_> = info.instance_methods.iter().collect();
            methods.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
            for (m, ret) in methods {
                let row = analyzer.inferred_param_types(&class, m).unwrap_or_default();
                let ps = row.iter().map(|t| render(t, var)).collect::<Vec<_>>().join("; ");
                println!("{}#{}({}) -> {}", cname, m.as_str(), ps, render(ret, var));
            }
        }
        ExitCode::SUCCESS
    })
}

// Extra observational output for the lab's selected-type exporter.
fn dossier3_audit(t: Option<&Ty>) -> serde_json::Value {
    fn walk(t: &Ty, tags: &mut std::collections::BTreeSet<String>, unsupported: &mut bool) {
        match t {
            Ty::Untyped { why } => { for reason in why.reasons() { tags.insert(reason.to_owned()); } }
            Ty::Var { .. } => { tags.insert("Var".into()); }
            Ty::Array { elem } => walk(elem, tags, unsupported),
            Ty::Hash { key, value } => { walk(key, tags, unsupported); walk(value, tags, unsupported); }
            Ty::Tuple { elems } | Ty::Union { variants: elems } => {
                for e in elems { walk(e, tags, unsupported); }
            }
            Ty::Record { row } => {
                for e in row.fields.values() { walk(e, tags, unsupported); }
                if row.rest.is_some() { *unsupported = true; }
            }
            Ty::Fn { ret, .. } => walk(ret, tags, unsupported),
            Ty::Int | Ty::Float | Ty::Bool | Ty::Str | Ty::Sym | Ty::Nil | Ty::Bottom => {}
            _ => { *unsupported = true; }
        }
    }
    let mut tags = std::collections::BTreeSet::new();
    let mut unsupported = false;
    if let Some(t) = t { walk(t, &mut tags, &mut unsupported); }
    serde_json::json!({
        "present": t.is_some(), "tags": tags,
        "origins": t.map(roundhouse::analyze::producer_evidence::origins), "unsupported_by_exporter": unsupported,
        "debug": t.map(|t| format!("{t:?}"))
    })
}
