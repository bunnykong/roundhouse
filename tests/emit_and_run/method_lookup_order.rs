//! A method is looked up as Ruby does: in the last-included module first,
//! and in a module before the modules it includes; `super` answers what the
//! next method in that order answers; and an app reopening
//! `ActiveJob::Base` reads its adapter through `queue_adapter`, which an
//! override's `super` reaches (mission_control-jobs:
//! `SolidQueueExt#find_job` over its `Adapter`'s raising default, and
//! `current_queue_adapter || super`).

use super::emit_and_run;

const PROBE: &str = r#"module Base
  def label
    "base"
  end
end

module Near
  include Base

  def label
    "near"
  end
end

module Later
  def tag
    "later"
  end
end

module Earlier
  def tag
    "earlier"
  end
end

module Extra
  def boost
    1
  end
end

class LookupProbe
  include Near
  include Earlier
  include Later
  extend Extra

  def self.boost
    super + 1
  end

  def self.run
    [new.label, new.tag, boost, ActiveJob::Base.queue_adapter.class.name, ActiveJob::Base.queue_adapter.equal?(ActiveJob::Base.queue_adapter)]
  end
end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("config/environments/production.rb", "  config.active_job.queue_adapter = :solid_queue\n", "")
        .write("app/models/lookup_probe.rb", PROBE)
        .write("app/models/active_job_label.rb", "module ActiveJob\n  class Base\n    def self.adapter_label\n      queue_adapter.class.name\n    end\n  end\nend\n")
}

#[test]
fn methods_and_super_resolve_in_rubys_lookup_order() {
    let (emitted, errors) = app().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    let sig = std::fs::read_to_string(emitted.join("sig/app/models/lookup_probe.rbs")).expect("lookup_probe.rbs");
    assert!(sig.contains("def self.boost: () -> Integer"), "{sig}");
}

#[test]
fn methods_and_super_run_in_rubys_lookup_order() {
    let run = app().run_ruby(
        "raise LookupProbe.run.inspect unless LookupProbe.run == [\"near\", \"later\", 2, \"ActiveJob::QueueAdapters::AsyncAdapter\", true]\nputs \"lookup order passed\"\n",
    );
    run.assert_passes();
    assert!(run.stdout.contains("lookup order passed"), "{}", run.stdout);
}
