//! `config.<key> = config_for(:name)` in `config/application.rb`: Rails
//! reads `config/<name>.yml` through ERB, merges the `shared` section
//! with the section of the current environment, and answers an
//! `ActiveSupport::OrderedOptions` with symbol keys. Dontfile keeps its
//! upload limit this way and reads it from a model on every page.

use super::emit_and_run;

const APPLICATION: &str = "module Demo\nclass Application < Rails::Application\nconfig.uploads = config_for(:uploads)\nend\nend\n";

const UPLOADS_YML: &str = "shared:\n  max_file_size: <%= 100.megabytes %>\n  label: shared\n\ntest:\n  max_file_size: <%= 16.kilobytes %>\n";

const LIMITS: &str = "class Limits
  def self.max_file_size
    Rails.configuration.uploads.max_file_size
  end

  def self.label
    Rails.configuration.uploads.label
  end

  def self.from_application_config
    Rails.application.config.uploads.max_file_size
  end

  def self.from_direct_call
    Rails.application.config_for(:uploads)[:max_file_size]
  end
end
";

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("config/application.rb", APPLICATION)
        .write("config/uploads.yml", UPLOADS_YML)
        .write("app/models/limits.rb", LIMITS)
}

#[test]
fn the_shared_section_answers_outside_an_overriding_environment() {
    app()
        .run_ruby_env(
            r#"
raise Limits.max_file_size.inspect unless Limits.max_file_size == 104857600
raise Limits.label.inspect unless Limits.label == "shared"
puts "shared section passed"
"#,
            &[("RAILS_ENV", "development")],
        )
        .assert_passes();
}

#[test]
fn the_environment_section_overrides_the_shared_section() {
    app()
        .run_ruby_env(
            r#"
raise Limits.max_file_size.inspect unless Limits.max_file_size == 16384
raise Limits.label.inspect unless Limits.label == "shared"
puts "environment section passed"
"#,
            &[("RAILS_ENV", "test")],
        )
        .assert_passes();
}

#[test]
fn the_application_config_and_a_direct_call_read_the_same_values() {
    app()
        .run_ruby_env(
            r#"
raise Limits.from_application_config.inspect unless Limits.from_application_config == 16384
raise Limits.from_direct_call.inspect unless Limits.from_direct_call == 16384
puts "direct call passed"
"#,
            &[("RAILS_ENV", "test")],
        )
        .assert_passes();
}

/// `app()` with `app/models/<name>.rb` added.
fn app_with_model(name: &str, source: &str) -> emit_and_run::Overlay {
    app().write(&format!("app/models/{name}.rb"), source)
}

/// `check` errors, each of which contains `needle`.
fn assert_errors_name(errors: &[String], needle: &str) {
    assert!(
        !errors.is_empty() && errors.iter().all(|e| e.contains(needle)),
        "expected errors that name {needle:?}, got:\n{}",
        errors.join("\n")
    );
}

#[test]
fn a_direct_call_reads_a_key_as_a_method() {
    app_with_model(
        "direct",
        "class Direct\n  def self.max_file_size\n    Rails.application.config_for(:uploads).max_file_size\n  end\nend\n",
    )
    .run_ruby_env(
        "raise Direct.max_file_size.inspect unless Direct.max_file_size == 16384\nputs \"direct read passed\"\n",
        &[("RAILS_ENV", "test")],
    )
    .assert_passes();
}

#[test]
fn an_options_object_the_app_builds_reads_and_writes_keys() {
    let overlay = || {
        app_with_model(
            "built",
            "class Built\n  def self.value\n    options = ActiveSupport::OrderedOptions.new\n    options.size_limit = 3\n    options.size_limit\n  end\nend\n",
        )
    };
    overlay()
        .run_ruby("raise Built.value.inspect unless Built.value == 3\nputs \"built options passed\"\n")
        .assert_passes();
    // Spinel leaves the YAML file, and with it the class, out of its boot.
    let (_, errors) = overlay().emit(roundhouse::project::BuildTarget::Spinel);
    assert!(
        errors.iter().any(|e| e.contains("ActiveSupport::OrderedOptions is not available")),
        "spinel must refuse the class:\n{}",
        errors.join("\n")
    );
}

#[test]
fn the_yaml_text_reaches_the_runtime_as_written() {
    let yml = "# a #{host} in a comment\nshared:\n  greet: \"mail me #@x now\"\n  path: 'a\\b'\n  tag: \"<%% x\"\n  size: <%= 2 * 3 %>\n";
    app()
        .write("config/uploads.yml", yml)
        .write(
            "app/models/text.rb",
            "class Text\n  def self.greet\n    Rails.configuration.uploads.greet\n  end\n\n  def self.path\n    Rails.configuration.uploads.path\n  end\n\n  def self.tag\n    Rails.configuration.uploads.tag\n  end\n\n  def self.size\n    Rails.configuration.uploads.size_value\n  end\nend\n",
        )
        .edit("config/uploads.yml", "  size:", "  size_value:")
        .run_ruby(
            r#"
raise Text.greet.inspect unless Text.greet == "mail me \#@x now"
raise Text.path.inspect unless Text.path == "a\\b"
raise Text.tag.inspect unless Text.tag == "<% x"
raise Text.size.inspect unless Text.size == 6
puts "text passed"
"#,
        )
        .assert_passes();
}

#[test]
fn an_unterminated_erb_tag_is_an_error() {
    let errors = app().write("config/uploads.yml", "shared:\n  limit: <%= 5\n").check_errors();
    assert_errors_name(&errors, "the ERB tag on line 2 has no `%>`");
}

#[test]
fn a_file_that_does_not_compile_leaves_the_other_files() {
    let overlay = || {
        app()
            .write(
                "config/application.rb",
                "module Demo\nclass Application < Rails::Application\nconfig.uploads = config_for(:uploads)\nconfig.extra = config_for(:extra)\nend\nend\n",
            )
            .write("config/uploads.yml", "shared:\n  limit: <%= 1 + %>\n")
            .write("config/extra.yml", "shared:\n  e: 2\n")
    };
    assert_errors_name(&overlay().check_errors(), "config/uploads.yml, which does not compile");
    let (emitted, _) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let application = std::fs::read_to_string(emitted.join("config/application.rb")).expect("config/application.rb");
    assert!(application.contains("when \"extra\""), "{application}");
}

#[test]
fn a_call_with_env_is_an_error() {
    let errors = app_with_model(
        "production_limits",
        "class ProductionLimits\n  def self.all\n    Rails.application.config_for(:uploads, env: \"production\")\n  end\nend\n",
    )
    .check_errors();
    assert_errors_name(&errors, "arguments other than the file name");
}

#[test]
fn the_options_behave_as_rails_hash_subclass() {
    const OPTIONS: &str = r#"class Options
  def self.read_limit(opts)
    opts.max_file_size
  end

  def self.through_a_parameter
    read_limit(Rails.configuration.uploads)
  end

  def self.safe_navigation
    Rails.configuration.uploads&.max_file_size
  end

  def self.op_assign
    o = Rails.configuration.uploads.dup
    o.max_file_size += 1
    o.newkey ||= 7
    [o.max_file_size, o.newkey]
  end

  def self.through_an_ivar
    @settings = Rails.application.config_for(:uploads)
    @settings.label
  end

  def self.copy_write
    o = Rails.configuration.uploads.dup
    o.other = 5
    [o.other, Rails.configuration.uploads.key?(:other)]
  end

  def self.hash_methods
    o = Rails.application.config_for(:uploads)
    [o.fetch(:label), o.dig(:label), o == { max_file_size: 16384, label: "shared" }, o.is_a?(Hash), o.key?("label"), o.to_h.equal?(o), o.label!]
  end
end
"#;
    app_with_model("options", OPTIONS)
        .run_ruby_env(
            r#"
raise Options.through_a_parameter.inspect unless Options.through_a_parameter == 16384
raise Options.safe_navigation.inspect unless Options.safe_navigation == 16384
raise Options.op_assign.inspect unless Options.op_assign == [16385, 7]
raise Options.through_an_ivar.inspect unless Options.through_an_ivar == "shared"
raise Options.copy_write.inspect unless Options.copy_write == [5, false]
raise Options.hash_methods.inspect unless Options.hash_methods == ["shared", "shared", true, true, false, false, "shared"]
puts "options passed"
"#,
            &[("RAILS_ENV", "test")],
        )
        .assert_passes();
}

#[test]
fn a_call_on_the_application_held_in_a_variable_reads_its_file() {
    app()
        .write("config/other.yml", "shared:\n  label: other\n")
        .write(
            "app/models/held.rb",
            "class Held\n  def self.label\n    app = Rails.application\n    app.config_for(:other)[:label]\n  end\nend\n",
        )
        .run_ruby("raise Held.label.inspect unless Held.label == \"other\"\nputs \"held passed\"\n")
        .assert_passes();
}

#[test]
fn a_file_that_closes_its_method_is_an_error_and_leaves_the_other_files() {
    let run = app()
        .write(
            "config/application.rb",
            "module Demo\nclass Application < Rails::Application\nconfig.uploads = config_for(:uploads)\nconfig.broken = config_for(:broken)\nconfig.extra = config_for(:extra)\nend\nend\n",
        )
        .write("config/broken.yml", "test:\n  x: <% 1\nend\ndef zz\n2 %>\n")
        .write("config/extra.yml", "shared:\n  e: 2\n")
        .write("app/models/extra_reader.rb", "class ExtraReader\n  def self.e\n    Rails.configuration.extra.e\n  end\nend\n")
        .run_ruby("raise ExtraReader.e.inspect unless ExtraReader.e == 2\nputs \"extra passed\"\n");
    assert_errors_name(&run.errors, "config/broken.yml, which does not compile: its ERB closes the method that holds it");
    assert!(run.success, "{}\n{}", run.stdout, run.stderr);
}

#[test]
fn erb_in_the_yaml_does_not_see_the_generated_method() {
    let run = app()
        .write("config/who.yml", "shared:\n  who: <%= name %>\n")
        .write("app/models/who.rb", "class Who\n  def self.settings\n    Rails.application.config_for(:who)\n  end\nend\n")
        .run_ruby("begin\n  Who.settings\n  raise \"no NameError\"\nrescue NameError => e\n  raise e.message unless e.name == :name\nend\nputs \"who passed\"\n");
    assert!(run.success, "{}\n{}", run.stdout, run.stderr);
}

#[test]
fn a_call_on_self_in_the_application_class_reads_its_file() {
    app()
        .write(
            "config/application.rb",
            "module Demo\nclass Application < Rails::Application\nconfig.uploads = config_for(:uploads)\nconfig.own = self.config_for(:own)\nend\nend\n",
        )
        .write("config/own.yml", "shared:\n  label: own\n")
        .write("app/models/own_reader.rb", "class OwnReader\n  def self.label\n    Rails.configuration.own.label\n  end\nend\n")
        .run_ruby("raise OwnReader.label.inspect unless OwnReader.label == \"own\"\nputs \"self passed\"\n")
        .assert_passes();
}

#[test]
fn a_computed_name_on_a_receiver_that_may_be_the_application_is_an_error() {
    let errors = app_with_model(
        "loader",
        "class Loader\n  def self.load(name)\n    app = Rails.application\n    app.config_for(name)\n  end\n\n  def self.load_named(app, name)\n    app.config_for(name)\n  end\nend\n",
    )
    .check_errors();
    assert!(
        errors.len() == 2 && errors.iter().all(|e| e.contains("a name that is not a Symbol or String literal")),
        "{}",
        errors.join("\n")
    );
}

#[test]
fn a_file_named_only_by_another_files_erb_reads_its_file() {
    app()
        .write("config/outer.yml", "shared:\n  x: <%= Rails.application.config_for(:inner)[:y] %>\n")
        .write("config/inner.yml", "shared:\n  y: 41\n")
        .write("app/models/outer.rb", "class Outer\n  def self.x\n    Rails.application.config_for(:outer)[:x]\n  end\nend\n")
        .run_ruby("raise Outer.x.inspect unless Outer.x == 41\nputs \"nested passed\"\n")
        .assert_passes();
}

/// Psych loads an empty or comment-only document as `false`. Rails'
/// `ConfigurationFile.parse` reads that as `{}`, so the call answers nil.
#[test]
fn an_empty_file_answers_nil() {
    app()
        .write("config/empty.yml", "# nothing yet\n")
        .write(
            "app/models/empty_config.rb",
            "class EmptyConfig\n  def self.value\n    Rails.application.config_for(:empty)\n  end\nend\n",
        )
        .run_ruby_env(
            "raise EmptyConfig.value.inspect unless EmptyConfig.value.nil?\nputs \"empty file passed\"\n",
            &[("RAILS_ENV", "test")],
        )
        .assert_passes();
}
