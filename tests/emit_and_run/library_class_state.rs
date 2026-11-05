//! A plain class takes a concern's `included do` accessors as a model
//! does, and a module's `mattr_accessor` reads as the type of its
//! default. mission_control-jobs gives `ActiveJob::Base` its `status` and
//! `current_queue_adapter` through `ActiveJob::Executing`'s block, and
//! keeps its applications in `mattr_accessor :applications, default: …`.

use super::emit_and_run;
use roundhouse::project::BuildTarget;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write(
            "app/models/concerns/tracking.rb",
            "module Tracking\n  extend ActiveSupport::Concern\n\n  included do\n    attr_accessor :status\n    thread_cattr_accessor :current\n  end\nend\n",
        )
        .write("app/models/tracked_box.rb", "class TrackedBox\n  include Tracking\nend\n")
        .write(
            "app/models/box_registry.rb",
            "module BoxRegistry\n  mattr_accessor :label, default: \"boxes\"\n  mattr_accessor :boxes, default: {}\nend\n",
        )
        .write(
            "app/models/box_use.rb",
            "class BoxUse\n  def self.run\n    b = TrackedBox.new\n    b.status = :open\n    TrackedBox.current = 3\n    BoxRegistry.boxes[BoxRegistry.label] = b\n    [b.status, TrackedBox.current, BoxRegistry.label]\n  end\nend\n",
        )
}

#[test]
fn a_class_takes_a_concerns_accessors_and_a_module_attribute_its_defaults_type() {
    let (emitted, errors) = app().emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    let boxed = std::fs::read_to_string(emitted.join("sig/app/models/tracked_box.rbs")).expect("tracked_box.rbs");
    assert!(boxed.contains("attr_reader status: Symbol?"), "{boxed}");
    assert!(boxed.contains("def self.current: () -> Integer?"), "{boxed}");
    let registry = std::fs::read_to_string(emitted.join("sig/app/models/box_registry.rbs")).expect("box_registry.rbs");
    assert!(registry.contains("def self.label: () -> String"), "{registry}");
}

#[test]
fn a_class_runs_a_concerns_accessors() {
    let run = app().run_ruby("raise BoxUse.run.inspect unless BoxUse.run == [:open, 3, \"boxes\"]\nputs \"library class state passed\"\n");
    run.assert_passes();
    assert!(run.stdout.contains("library class state passed"), "{}", run.stdout);
}
