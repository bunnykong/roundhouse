//! Statement-shaped values keep grouping in case subjects and defaults.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const EFFECTS: &str = r#"
  def self.condition(trace, ready)
    trace << :condition
    ready
  end
  def self.value(trace)
    trace << :value
    'ready'
  end
"#;

const CASE: &str = r#"
  def self.choose(ready, trace)
    case (self.value(trace) if self.condition(trace, ready))
    when 'ready' then 'yes'
    else 'no'
    end
  end
  def self.choose_unless(ready, trace)
    case (self.value(trace) unless self.condition(trace, ready))
    when 'ready' then 'yes'
    else 'no'
    end
  end
"#;

const DEFAULTS: &str = r#"
  def self.read(ready, trace, value: (self.value(trace) if self.condition(trace, ready)), fallback: nil)
    value || fallback
  end
  def self.read_unless(ready, trace, value: (self.value(trace) unless self.condition(trace, ready)), fallback: nil)
    value || fallback
  end
"#;

const CLOSURES: &str = r#"
  def self.lambda_read(ready, trace)
    callback = ->(value: (self.value(trace) if self.condition(trace, ready))) { value || 'none' }
    [callback.call, callback.call(value: 'explicit')]
  end
  def self.lambda_positional(ready, trace)
    callback = ->(value = (self.value(trace) unless self.condition(trace, ready))) { value || 'none' }
    [callback.call, callback.call('explicit')]
  end
  def self.block_read(ready, trace)
    [0].map { |index, value: (self.value(trace) unless self.condition(trace, ready))| value || 'none' }
  end
  def self.block_positional(ready, trace)
    [0].map do |index, value = (self.value(trace) if self.condition(trace, ready))|
      value || 'none'
    end
  end
"#;

const LAMBDA_SEQUENCES: &str = r#"
  def self.keyword_sequence(trace)
    callback = ->(value: (marker = (trace << :default; 42))) { value }
    [callback.call, callback.call(value: 7)]
  end
  def self.positional_sequence(trace)
    callback = ->(value = (marker = (trace << :default; 42))) { value }
    [callback.call, callback.call(7)]
  end
"#;

const SEQUENCE_REPORT: &str = r#"
trace = []
p [ConditionalProbe.keyword_sequence(trace), trace]
trace = []
p [ConditionalProbe.positional_sequence(trace), trace]
"#;

const CASE_REPORT: &str = r#"
[true, false].each do |ready|
  trace = []
  p [ConditionalProbe.choose(ready, trace), trace]
  trace = []
  p [ConditionalProbe.choose_unless(ready, trace), trace]
end
"#;
const CASE_EXPECTED: &str = "[\"yes\", [:condition, :value]]\n[\"no\", [:condition]]\n[\"no\", [:condition]]\n[\"yes\", [:condition, :value]]\n";

const DEFAULT_REPORT: &str = r#"
[true, false].each do |ready|
  trace = []
  p [ConditionalProbe.read(ready, trace, fallback: 'none'), trace]
  trace = []
  p [ConditionalProbe.read_unless(ready, trace, fallback: 'none'), trace]
  trace = []
  p [ConditionalProbe.read(ready, trace, value: 'explicit'), trace]
  p [ConditionalProbe.read_unless(ready, trace, value: 'explicit'), trace]
end
"#;

const CLOSURE_REPORT: &str = r#"
[true, false].each do |ready|
  trace = []
  p [ConditionalProbe.lambda_read(ready, trace), trace]
  trace = []
  p [ConditionalProbe.lambda_positional(ready, trace), trace]
  trace = []
  p [ConditionalProbe.block_read(ready, trace), trace]
  trace = []
  p [ConditionalProbe.block_positional(ready, trace), trace]
end
"#;

fn source(methods: &str) -> String {
    format!("class ConditionalProbe\n{EFFECTS}{methods}end\n")
}

fn oracle(source: &str, report: &str) -> String {
    let output = std::process::Command::new("ruby")
        .args(["-e", &format!("{source}\n{report}")])
        .output()
        .expect("CRuby control");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn check_emitted(methods: &str, report: &str, expected: &str) {
    let source = source(methods);
    assert_eq!(oracle(&source, report), expected, "CRuby control");
    for target in [BuildTarget::Ruby, BuildTarget::Jruby, BuildTarget::Spinel] {
        let (emitted, errors) = emit_and_run::real_blog()
            .write("app/lib/conditional_probe.rb", &source)
            .emit(target);
        assert!(errors.is_empty(), "{}", errors.join("\n"));
        let output = std::process::Command::new("ruby")
            .args([
                "-e",
                &format!("require_relative 'app/models/conditional_probe'\n{report}"),
            ])
            .current_dir(&emitted)
            .output()
            .expect("emitted Ruby");
        assert!(
            output.status.success(),
            "{target:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            expected,
            "{target:?}"
        );
    }
}

#[test]
fn conditional_case_subjects_run_as_written() {
    check_emitted(CASE, CASE_REPORT, CASE_EXPECTED);
}

#[test]
fn conditional_pattern_case_subjects_run_as_written() {
    check_emitted(
        &CASE.replace("when 'ready'", "in 'ready'"),
        CASE_REPORT,
        CASE_EXPECTED,
    );
}

#[test]
fn conditional_keyword_defaults_run_only_when_omitted() {
    let expected = "[\"ready\", [:condition, :value]]\n[\"none\", [:condition]]\n[\"explicit\", []]\n[\"explicit\", []]\n[\"none\", [:condition]]\n[\"ready\", [:condition, :value]]\n[\"explicit\", []]\n[\"explicit\", []]\n";
    check_emitted(DEFAULTS, DEFAULT_REPORT, expected);
}

#[test]
fn conditional_lambda_and_block_defaults_run_as_written() {
    let expected = "[[\"ready\", \"explicit\"], [:condition, :value]]\n[[\"none\", \"explicit\"], [:condition]]\n[[\"none\"], [:condition]]\n[[\"ready\"], [:condition, :value]]\n[[\"none\", \"explicit\"], [:condition]]\n[[\"ready\", \"explicit\"], [:condition, :value]]\n[[\"ready\"], [:condition, :value]]\n[[\"none\"], [:condition]]\n";
    check_emitted(CLOSURES, CLOSURE_REPORT, expected);
}

#[test]
fn lambda_default_sequences_keep_source_markers_on_their_own_lines() {
    check_emitted(
        LAMBDA_SEQUENCES,
        SEQUENCE_REPORT,
        "[[42, 7], [:default]]\n[[42, 7], [:default]]\n",
    );
}

#[test]
#[ignore = "requires Spinel"]
fn conditional_values_run_natively() {
    for (methods, report) in [
        (CASE.to_string(), CASE_REPORT),
        (DEFAULTS.to_string(), DEFAULT_REPORT),
        (CLOSURES.to_string(), CLOSURE_REPORT),
        (LAMBDA_SEQUENCES.to_string(), SEQUENCE_REPORT),
        (CASE.replace("when 'ready'", "in 'ready'"), CASE_REPORT),
    ] {
        let source = source(&methods);
        let expected = oracle(&source, report);
        let run = emit_and_run::real_blog()
            .write("app/lib/conditional_probe.rb", &source)
            .run_spinel_slim(&[], report);
        run.assert_passes();
        assert_eq!(run.stdout, expected);
    }
}
