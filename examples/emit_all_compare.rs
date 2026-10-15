use std::path::Path;
use roundhouse::project::{target_files, BuildTarget};
fn main() {
    roundhouse::stack::run(|| {
        let out = std::env::args().nth(1).expect("output directory");
        for fixture in ["tiny-blog", "tiny-api", "tiny-blog-uuid", "real-blog", "store", "roda-blog", "gem-capabilities/historical/base"] {
            let input = format!("fixtures/{fixture}");
            let raw = roundhouse::ingest::ingest_app(Path::new(&input)).expect("ingest fixture");
            let mut app = raw.clone();
            roundhouse::session::analyze_and_lower(&mut app);
            let mut targets = BuildTarget::ALL.to_vec();
            for target in BuildTarget::TRANSPILE { if !targets.contains(target) { targets.push(*target); } }
            for target in targets {
                let (files, diagnostics) = roundhouse::emit::diagnostics::scope(|| target_files(if target == BuildTarget::Roda { &raw } else { &app }, Path::new(&input), target));
                let dir = Path::new(&out).join(fixture.replace('/', "-")).join(target.as_str());
                match files {
                    Ok(files) => {
                        for (name, contents) in &files {
                            let path = dir.join(name);
                            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                            std::fs::write(path, contents).unwrap();
                        }
                        println!("{}", serde_json::json!({"fixture":fixture, "target":target.as_str(), "files":files.len(), "diagnostics":diagnostics.len()}));
                    }
                    Err(error) => println!("{}", serde_json::json!({"fixture":fixture, "target":target.as_str(), "error":error})),
                }
            }
        }
    });
}
