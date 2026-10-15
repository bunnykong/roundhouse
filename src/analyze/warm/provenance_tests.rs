//! Direct native read-guard control; uses private production replay machinery.
use super::*;
use crate::analyze::{
    Analyzer,
    body::{BodyTyper, ConstScope},
};
use crate::ty::Provenance;

struct Environment(Vec<(String, Option<std::ffi::OsString>)>);
impl Environment {
    fn set(values: &[(&str, &str)]) -> Self {
        let old = values
            .iter()
            .map(|(key, value)| {
                let previous = std::env::var_os(key);
                // This test runs in a dedicated, single-threaded test process.
                unsafe {
                    std::env::set_var(key, value);
                }
                ((*key).to_owned(), previous)
            })
            .collect();
        Self(old)
    }
}
impl Drop for Environment {
    fn drop(&mut self) {
        for (key, value) in &self.0 {
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

fn evaluate(app: &App, why: Provenance) -> (Ty, Value) {
    assert!(start(app).unwrap());
    let mut analyzer = Analyzer::new(app);
    analyzer.sccq_init(app);
    let unit = analyzer.sccq_lib_unit(0, 0).expect("reader unit missing");
    analyzer.sccq_rec_begin(Some(unit));
    let mut expr = app.library_classes[0].methods[0].body.clone();
    let ctx = Ctx {
        constants: ConstScope::global(
            [(Symbol::from("FLAG"), Ty::untyped(why))]
                .into_iter()
                .collect(),
        ),
        ..Ctx::default()
    };
    let ty = BodyTyper::new(analyzer.class_registry()).analyze_expr(&mut expr, &ctx);
    analyzer.sccq_rec_end(Some(unit), true, Some(&expr));
    let pending = stop().unwrap();
    // This isolated body control has no whole-program shadow. The CLI fixture
    // and app tests retain mandatory shadow validation and publication checks.
    let stats = serde_json::to_value(&pending.stats).unwrap();
    assert_eq!(pending.cache.records.len(), 1);
    assert!(pending.cache.records[0].reads.contains_key("global:FLAG"));
    assert_eq!(pending.cache.schema, 3);
    pending.save().unwrap();
    (ty, stats)
}

#[test]
fn warm_provenance_invalidation() {
    if std::env::var("WARM_STORAGE_PROVENANCE_CHILD").as_deref() != Ok("1") {
        assert_eq!(
            WarmCodec::selected(),
            WarmCodec::LosslessV3,
            "explicitly select the storage candidate"
        );
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        for (key, _) in std::env::vars()
            .filter(|(key, _)| key.starts_with("RH_") || key == "ROUNDHOUSE_TIMINGS")
        {
            command.env_remove(key);
        }
        let output = command
            .args([
                "--exact",
                "analyze::warm::provenance_tests::warm_provenance_invalidation",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("WARM_STORAGE_PROVENANCE_CHILD", "1")
            .env("RH_WARM_LOSSLESS_CODEC", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "native reader control failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        print!("{}", String::from_utf8_lossy(&output.stdout));
        return;
    }
    assert_eq!(
        WarmCodec::selected(),
        WarmCodec::LosslessV3,
        "explicitly select the storage candidate"
    );
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(".warm-tests")
        .join(format!("{}-provenance-control", std::process::id()));
    let _environment = Environment::set(&[
        ("RH_SCHED", "sccq"),
        ("RH_BRK_ALLARMS", "1"),
        ("RH_FOLD_JOIN", "1"),
        ("RH_FOLD", "1"),
        ("RH_FOLD_SLOTS", "1"),
        ("RH_FOLD_TAIL", "1"),
        ("RH_WARM_SHADOW", "1"),
        ("RH_FIXPOINT_VERIFY", "0"),
    ]);
    crate::ingest::sources::reset();
    let mut app = App::new();
    app.library_classes = vec![
        crate::ingest::ingest_library_class(
            b"class Chain\ndef link0\nFLAG\nend\nend\n",
            "app/services/chain.rb",
        )
        .unwrap()
        .unwrap(),
    ];
    app.sources = crate::ingest::sources::drain();
    let mut rows = Vec::new();
    for (i, (before, after)) in [
        (Provenance::Pending, Provenance::Unresolved),
        (
            Provenance::Gradual,
            Provenance::Gradual.union(Provenance::Unresolved),
        ),
        (
            Provenance::Pending
                .union(Provenance::Gradual)
                .union(Provenance::Unresolved),
            Provenance::Gradual,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let cache = root.join(i.to_string());
        if cache.exists() {
            std::fs::remove_dir_all(&cache).unwrap();
        }
        let _cache = Environment::set(&[("RH_WARM", cache.to_str().unwrap())]);
        let (old, seed) = evaluate(&app, before);
        let (same, repeat) = evaluate(&app, before);
        assert_eq!(repeat["replayed"], 1);
        assert_eq!(same.provenance(), Some(before));
        let (changed, revisit) = evaluate(&app, after);
        assert_eq!(
            old, changed,
            "reason-only control must retain ordinary type equality"
        );
        assert_ne!(old.provenance(), changed.provenance());
        assert_eq!(changed.provenance(), Some(after));
        assert_eq!(revisit["loaded"], 1);
        assert_eq!(revisit["invalidated"], 0, "syntax/body identity changed");
        assert_eq!(
            revisit["input_invalidated"], 0,
            "reason change was hidden in input invalidation"
        );
        assert_eq!(revisit["read_invalidated"], 1);
        assert_eq!(revisit["typed"], 1);
        assert_eq!(
            revisit["replayed"], 0,
            "stale reason-bearing reader replayed"
        );
        let (stable, replay) = evaluate(&app, after);
        assert_eq!(stable.provenance(), Some(after));
        assert_eq!(replay["replayed"], 1);
        assert!(replay["codec"]["decoded_reason_leaves"].as_u64().unwrap() > 0);
        rows.push(
            serde_json::json!({"before": before.reasons(), "after": after.reasons(),
            "seed": seed, "repeat": repeat, "revisit": revisit, "stable": replay}),
        );
    }
    let summary = serde_json::json!({"cases": rows, "reason_only_read_invalidations": 3,
        "reader_revisits": 3, "ordinary_equal": true, "codec_identity": WarmCodec::LosslessV3.identity()});
    println!("warm-provenance-invalidation: {summary}");
    if let Some(receipts) = std::env::var_os("WARM_TEST_RECEIPTS") {
        let receipts = PathBuf::from(receipts);
        std::fs::create_dir_all(&receipts).unwrap();
        std::fs::write(
            receipts.join("provenance-invalidation.json"),
            serde_json::to_vec_pretty(&summary).unwrap(),
        )
        .unwrap();
    }
    std::fs::remove_dir_all(root).unwrap();
}
