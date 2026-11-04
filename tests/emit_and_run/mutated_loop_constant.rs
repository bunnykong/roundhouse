//! A `define_method` loop over a constant the class body changes in place
//! is not unrolled from the literal it was first assigned: the loop would
//! run over what the constant holds then. Spinel reports it as the
//! runtime-defined methods it is.

use super::emit_and_run;
use roundhouse::project::BuildTarget;

#[test]
fn a_loop_over_a_constant_appended_to_is_not_unrolled_from_its_first_value() {
    let (_emitted, errors) = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  KINDS = %w[draft]\n  KINDS << \"live\"\n\n  KINDS.each do |kind|\n    define_method(\"#{kind}_label\") { kind.upcase }\n  end\n\n",
        )
        .emit(BuildTarget::Spinel);
    assert!(errors.iter().any(|e| e.contains("runtime-defined method")), "{errors:#?}");
}

/// The Ruby emit has no runtime-defined methods to fall back on either: the loop it cannot unroll is reported, not dropped silently.
#[test]
fn the_ruby_emit_reports_the_loop_it_drops() {
    let root = std::env::temp_dir().join(format!("roundhouse-dropped-define-loop-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (path, body) in [
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        (
            "app/models/post.rb",
            "class Post < ApplicationRecord\n  def self.kinds\n    %w[draft live]\n  end\n\n  kinds.each do |kind|\n    define_method(\"#{kind}_label\") { kind.upcase }\n  end\nend\n",
        ),
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"title\"\n  end\nend\n"),
    ] {
        let file = root.join("app_src").join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, body).unwrap();
    }
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "ruby", "--allow-unsupported", "-o"])
        .arg(root.join("out"))
        .arg(root.join("app_src"))
        .output()
        .expect("run roundhouse");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let _ = std::fs::remove_dir_all(&root);
    assert!(stderr.contains("runtime-defined method not supported"), "{stderr}");
}
