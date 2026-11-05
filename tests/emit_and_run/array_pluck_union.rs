use super::emit_and_run;

const SOURCES: &str = r#"module ChoiceSources
  module Full
    def self.models
      [{ value: "alpha", label: "Alpha", levels: ["low"] }, { label: "Missing", levels: ["high"] }]
    end
  end

  module Numeric
    def self.models
      [{ value: 7 }]
    end
  end

  module Empty
    def self.models
      []
    end
  end

  def self.for(name)
    case name
    when "full" then Full
    when "numeric" then Numeric
    when "empty" then Empty
    else raise ArgumentError, "unknown source"
    end
  end

  def self.concrete(name)
    name == "full" ? Full : Numeric
  end
end
"#;

const SELECTION: &str = r#"class Selection
  include ActiveModel::Model

  def self.values(name)
    provider = ChoiceSources.for(name)
    provider.models.pluck(:value)
  end

  def self.values_with_local_key(name)
    provider = ChoiceSources.for(name)
    key = :value
    provider.models.pluck(key)
  end

  def self.concrete_values(name)
    provider = ChoiceSources.concrete(name)
    provider.models.pluck(:value)
  end
end
"#;

const CONTRACT: &str = r#"
raise "full values" unless Selection.values("full") == ["alpha", nil]
raise "numeric values" unless Selection.values("numeric") == [7]
raise "empty values" unless Selection.values("empty") == []
raise "local key" unless Selection.values_with_local_key("full") == ["alpha", nil]
raise "empty local key" unless Selection.values_with_local_key("empty") == []
raise "concrete hashes" unless Selection.concrete_values("full") == ["alpha", nil]
raise "concrete integers" unless Selection.concrete_values("numeric") == [7]
puts "union array pluck passed"
"#;

fn choices() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/models/choice_sources.rb", SOURCES)
        .write("app/models/selection.rb", SELECTION)
}

#[test]
fn union_hash_and_unresolved_array_elements_pluck_in_emitted_ruby() {
    choices().run_ruby(CONTRACT).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn spinel_gate_union_hash_and_unresolved_array_elements_pluck() {
    choices().run_spinel(CONTRACT).assert_passes();
}

#[test]
fn a_union_with_an_unsupported_element_keeps_the_pluck_error() {
    let sources = SOURCES
        .replace(
            "  module Empty\n",
            "  module Invalid\n    def self.models\n      [\"invalid\"]\n    end\n  end\n\n  module Empty\n",
        )
        .replace(
            "    when \"empty\" then Empty\n",
            "    when \"empty\" then Empty\n    when \"invalid\" then Invalid\n",
        );
    let errors = choices()
        .write("app/models/choice_sources.rb", &sources)
        .check_errors();
    assert!(
        errors
            .iter()
            .any(|error| error.contains("no known method `pluck`")),
        "{errors:?}"
    );
}

#[test]
fn a_computed_key_keeps_the_pluck_error_for_union_elements() {
    let errors = choices()
        .write(
            "app/models/selection.rb",
            r#"class Selection
  include ActiveModel::Model

  def self.key
    :value
  end

  def self.values(name)
    provider = ChoiceSources.for(name)
    provider.models.pluck(key)
  end
end
"#,
        )
        .check_errors();
    assert!(
        errors
            .iter()
            .any(|error| error.contains("no known method `pluck`")),
        "{errors:?}"
    );
}
