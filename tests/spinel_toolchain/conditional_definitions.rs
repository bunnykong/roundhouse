//! A class body's load-time `if` / `unless` around `def`s, which Spinel compiles as Ruby loads it: the branch the
//! environment selects defines the method (the Ruby tree's `tests/conditional_definitions.rs`).

use super::emit_and_run;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn the_branch_the_environment_selects_defines_the_method_natively() {
    let run = emit_and_run::empty_app()
        .write(
            "lib/cache_store.rb",
            r#"class CacheStore
  if Rails.env.production?
    def clear
      "production"
    end
  elsif Rails.env.development?
    def clear
      "development"
    end
  end

  unless Rails.env.development?
    def verify = "strict"
  else
    def verify = "lenient"
  end

  def report
    [clear, verify]
  end
end
"#,
        )
        .run_spinel("p CacheStore.new.report\n");
    run.assert_passes();
    assert_eq!(run.stdout, "[\"development\", \"lenient\"]\n");
}
