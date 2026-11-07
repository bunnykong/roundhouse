use super::puma_boot_contract as contract;
use roundhouse::project::BuildTarget;

#[test]
fn puma_boot_hooks_lease_a_connection_for_model_queries() {
    let run = contract::overlay()
        .write("config/puma.rb", "after_booted { puts Article.count }\n")
        .run_ruby_env(include_str!("puma_boot_lease.rb"), &[("RAILS_MAX_THREADS", "1")]);
    run.assert_passes();
    assert!(run.stdout.contains("Puma boot query waited for a connection"));
}

#[test]
fn puma_boot_captures_only_block_outputs_that_need_extracted_callbacks() {
    let output = super::emit_and_run::ruby()
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/emit_and_run/puma_boot_cli.rb"))
        .arg(env!("CARGO_BIN_EXE_roundhouse"))
        .output()
        .expect("run Puma source-preserving CLI contract");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn puma_boot_hooks_refuse_unresolved_calls_in_callback_bodies() {
    let (_tree, errors) = contract::overlay()
        .write(
            "config/puma.rb",
            "after_booted { JSON.parse('{}').no_such_puma_boot_hook }\n",
        )
        .emit(BuildTarget::Ruby);
    assert!(
        errors
            .iter()
            .any(|error| error.contains("no_such_puma_boot_hook")),
        "{errors:?}"
    );
}

#[test]
fn puma_boot_hooks_report_targets_without_a_boot_event() {
    let (_tree, errors) = contract::overlay().emit(BuildTarget::Rust);
    assert!(
        errors
            .iter()
            .any(|error| error.contains("no server boot event")),
        "{errors:?}"
    );
}

#[test]
fn puma_boot_hooks_wait_for_the_boot_event_and_keep_registration_conditions() {
    let script = r#"
raise "hook ran while loading libraries" if ENV["PUMA_HOOK_EVENTS"]
ENV.delete("PUMA_HOOK_SELECTION")
class PumaBootDsl
  def initialize
    @after_booted = []
  end

  def after_booted(&block)
    @after_booted << block
  end
  alias on_booted after_booted

  def fire_after_booted!
    @after_booted.each(&:call)
  end

  def threads(*)
  end

  def port(*)
  end

  def bind(*)
  end

  def environment(*)
  end

  def workers(*)
  end

  def preload_app!
  end

  def before_worker_boot(&block)
  end

  def plugin(*)
  end
end
config = PumaBootDsl.new
config.instance_eval(File.read("config/puma.rb"), "config/puma.rb")
raise "hook ran during registration" if ENV["PUMA_HOOK_EVENTS"]
require File.expand_path("main", Dir.pwd)
Main.configure_default_adapter!
config.fire_after_booted!
raise ENV["PUMA_HOOK_EVENTS"].inspect unless ENV["PUMA_HOOK_EVENTS"] == "first,second"
puts "puma boot hook contract passed"
"#;
    let run = contract::overlay().run_ruby(script);
    run.assert_passes();
    assert!(run.stdout.contains("puma boot hook contract passed"));
    let early_config = super::emit_and_run::ruby()
        .args(["-e", script])
        .current_dir(&run.emitted)
        .env("BLOG_DB", ":memory:")
        .output()
        .expect("load Puma config before the application");
    assert!(
        early_config.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&early_config.stdout),
        String::from_utf8_lossy(&early_config.stderr)
    );
}
