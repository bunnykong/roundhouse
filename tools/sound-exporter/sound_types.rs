//! Observation-only exporter for c210f226 and 38403140. No inference overlays.
//! sound-types APP SELECTION.json
//! Legacy [owner, method, index|"return"|"self_return"] selections are supported.
//! Expression selections use owner, method, side, file, start, end and ivar.
use roundhouse::{
    analyze::{Analyzer, LoopEnd}, app::App, dialect::MethodReceiver,
    expr::{Expr, ExprNode}, ident::{ClassId, Symbol}, ty::Ty,
};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path};

#[cfg(feature = "s3")]
fn provenance(t: &Ty) -> Option<String> { t.provenance().map(|p| format!("{p:?}").to_lowercase()) }
#[cfg(not(feature = "s3"))]
fn provenance(t: &Ty) -> Option<String> {
    if matches!(t, Ty::Untyped) { Some("untagged".into()) } else { None }
}

// Serde intentionally omits S3's reporting-only provenance. Retain it here,
// including nested uncertainty, alongside the original Debug representation.
fn raw(t: &Ty) -> Value {
    let mut v = serde_json::to_value(t).unwrap();
    if let Some(p) = provenance(t) { v["provenance"] = p.into(); }
    match t {
        Ty::Array { elem } => v["elem"] = raw(elem),
        Ty::Hash { key, value } => { v["key"] = raw(key); v["value"] = raw(value); }
        Ty::Tuple { elems } => v["elems"] = elems.iter().map(raw).collect(),
        Ty::Union { variants } => v["variants"] = variants.iter().map(raw).collect(),
        Ty::Record { row } => {
            for (k, ty) in &row.fields { v["row"]["fields"][k.as_str()] = raw(ty); }
        }
        Ty::Fn { ret, .. } => v["ret"] = raw(ret),
        Ty::Class { args, .. } => v["args"] = args.iter().map(raw).collect(),
        _ => {}
    }
    v
}

fn end(e: LoopEnd) -> Value {
    match e {
        LoopEnd::Settled(n) => json!({"end":"settled", "round":n}),
        LoopEnd::RanToCap => json!({"end":"ran_to_cap"}),
        LoopEnd::NotRun => json!({"end":"not_run"}),
    }
}

fn walk<'a>(e: &'a Expr, app: &App, s: &Value, matches: &mut Vec<&'a Expr>) {
    let file = app.sources.get(e.span.file.0.saturating_sub(1) as usize);
    let same_file = file.is_some_and(|f| {
        let wanted = s["file"].as_str().unwrap();
        f.path == wanted || f.path.ends_with(&format!("/{wanted}"))
    });
    if same_file && e.span.start as u64 == s["start"].as_u64().unwrap()
        && e.span.end as u64 == s["end"].as_u64().unwrap()
        && matches!(&*e.node, ExprNode::Ivar { name } if name.as_str() == s["ivar"].as_str().unwrap()) {
        matches.push(e);
    }
    e.node.for_each_child(&mut |c| walk(c, app, s, matches));
}

fn main() {
    roundhouse::stack::run(|| {
        let args: Vec<_> = std::env::args().collect();
        assert_eq!(args.len(), 3, "sound-types APP SELECTION.json");
        assert_eq!(std::env::var("SOUND_MISSING").as_deref(), Ok("1"), "require SOUND_MISSING=1");
        let selected: BTreeMap<String, Value> = serde_json::from_str(
            &std::fs::read_to_string(&args[2]).unwrap()).unwrap();
        assert!(!selected.is_empty(), "empty selection");
        roundhouse::ingest::survey::activate();
        let mut app = roundhouse::ingest::app::ingest_app(Path::new(&args[1])).unwrap();
        let mut analyzer = Analyzer::new(&app);
        analyzer.analyze(&mut app);
        let mut slots = BTreeMap::new();
        for (alias, s) in selected {
            let mut match_count = 0;
            let ty = if s.is_array() {
                let class = ClassId(Symbol::from(s[0].as_str().unwrap()));
                let method = Symbol::from(s[1].as_str().unwrap());
                if let Some(index) = s[2].as_u64() {
                    analyzer.inferred_param_types(&class, &method).and_then(|r| r.get(index as usize))
                } else {
                    assert!(s[2] == "return" || s[2] == "self_return", "unknown legacy selector");
                    analyzer.class_registry().get(&class).and_then(|c| {
                        if s[2] == "self_return" { c.class_methods.get(&method) }
                        else { c.instance_methods.get(&method) }
                    })
                }
            } else {
                assert_eq!(s["kind"], "ivar_read");
                let side = match s["side"].as_str().unwrap() {
                    "instance" => MethodReceiver::Instance,
                    "class" => MethodReceiver::Class,
                    _ => panic!("unknown receiver side"),
                };
                let mut found = Vec::new();
                for c in &app.library_classes {
                    if c.name.0.as_str() != s["owner"].as_str().unwrap() { continue; }
                    for m in &c.methods {
                        if m.name.as_str() == s["method"].as_str().unwrap() && m.receiver == side {
                            walk(&m.body, &app, &s, &mut found);
                        }
                    }
                }
                match_count = found.len();
                assert!(match_count <= 1, "ambiguous logical slot {alias}");
                found.first().and_then(|e| e.ty.as_ref())
            };
            let row = match ty {
                Some(t) => json!({"status":"typed", "type":raw(t), "debug":format!("{t:?}"),
                    "selection":s, "match_count":match_count}),
                None => json!({"status":"missing", "type":null, "raw":"__missing__",
                    "selection":s, "match_count":match_count}),
            };
            slots.insert(alias, row);
        }
        let rounds = analyzer.fixpoint_rounds();
        println!("{}", serde_json::to_string_pretty(&json!({"schema":2, "slots":slots,
            "loops":{"production":end(rounds.production),
                "views_and_tests":end(rounds.views_and_tests), "absorb":end(rounds.absorb)}})).unwrap());
    });
}
