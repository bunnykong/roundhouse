//! `ActionController::Base#json_render_escape` is transpiled into every
//! target's controller base. Only the ruby family and Spinel load
//! runtime/ruby/rails.rb, so on the other targets the hook must not read
//! `Rails.application` (Go: `undefined: Rails_application`, Rust: E0433,
//! C#: CS0103).

use super::emit_and_run;
use roundhouse::project::BuildTarget;
use std::path::Path;

fn files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read emitted dir").flatten() {
        let path = entry.path();
        if path.is_dir() {
            files(&path, out);
        } else {
            out.push(path);
        }
    }
}

#[test]
fn the_json_render_escape_hook_reads_no_rails_on_strict_targets() {
    let mut offenders = Vec::new();
    for target in [
        BuildTarget::Go,
        BuildTarget::Crystal,
        BuildTarget::Kotlin,
        BuildTarget::Swift,
        BuildTarget::CSharp,
        BuildTarget::Rust,
        BuildTarget::Python,
        BuildTarget::Elixir,
    ] {
        let (emitted, errors) = emit_and_run::real_blog().emit(target);
        assert!(errors.is_empty(), "{target:?}: emit errors {errors:?}");
        let mut all = Vec::new();
        files(&emitted, &mut all);
        let mut seen = false;
        for path in all {
            let Ok(source) = std::fs::read_to_string(&path) else { continue };
            let lines: Vec<&str> = source.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                if !line.to_ascii_lowercase().replace('_', "").contains("jsonrenderescape") {
                    continue;
                }
                seen = true;
                let body = lines[i..(i + 3).min(lines.len())].join("\n");
                if body.contains("Rails") {
                    offenders.push(format!("{target:?} {}:{}\n{body}", path.display(), i + 1));
                }
            }
        }
        assert!(seen, "{target:?}: no json_render_escape in the emitted tree");
    }
    assert!(offenders.is_empty(), "{}", offenders.join("\n\n"));
}
