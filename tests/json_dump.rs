//! JSON.dump's destination and limit overloads must survive native dispatch.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write("db/schema.rb", "ActiveRecord::Schema.define do\nend\n")
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\nend\n",
        )
}

const IO: &str = r#"
require "json"
require "stringio"
class JsonSink
  attr_reader :text, :flushes
  def initialize
    @text = +"prefix:"
    @flushes = 0
  end
  def write(text)
    @text << text
    123
  end
  def flush
    @flushes += 1
    self
  end
end
class JsonDestination
  def initialize(io)
    @io = io
  end
  def to_io
    @io
  end
end
class JsonAbsentDestination
  def to_io
    nil
  end
end
class JsonFailingSink
  def write(text)
    raise IOError, "write failed"
  end
  def flush
    raise "flush after failed write"
  end
end
stream = StringIO.new
p JSON.dump({"a" => [1]}, stream).equal?(stream)
p stream.string
sink = JsonSink.new
p JSON.dump([true, nil], sink).equal?(sink)
p sink.text
p sink.flushes
adapter = JsonDestination.new(stream)
p JSON.dump({}, adapter).equal?(stream)
p stream.string
p JSON.dump({}, StringIO.new).write("!")
p JSON.dump({}, JsonAbsentDestination.new)
File.open("dump.json", "w") do |file|
  p JSON.dump({"file" => 2}, file).equal?(file)
end
p File.read("dump.json")
begin
  JSON.dump({}, JsonFailingSink.new)
rescue IOError => dump_error
  puts dump_error.class.to_s
end
"#;

const LIMIT: &str = r#"
require "json"
require "stringio"
p JSON.dump([{"a" => [1]}], nil, 3)
p JSON.dump([{"a" => [1]}], 3)
p JSON.dump([[[1]]], nil, 0)
p JSON.dump([[[1]]], nil, false)
p JSON.dump([[[1]]], nil, "ignored")
stream = StringIO.new
begin
  JSON.dump([{"a" => [1]}], stream, 2)
rescue ArgumentError => dump_error
  puts dump_error.class.to_s + ":" + dump_error.message
end
p stream.string
begin
  JSON.dump([[[1]]], 1)
rescue ArgumentError => dump_error
  puts dump_error.class.to_s + ":" + dump_error.message
end
"#;

// json 2.18 accepts a fourth options argument. The audit's fourth-argument
// probe raises because -1 is its nesting limit, not because of its arity.
const OPTIONS: &str = r#"
require "json"
p JSON.dump({}, nil, nil, {})
p JSON.dump([[[1]]], nil, 1, {max_nesting: false})
p JSON.dump([1], {max_nesting: 1})
p JSON.dump([1], nil, {max_nesting: 1})
begin
  JSON.dump({}, nil, -1, nil)
rescue ArgumentError => dump_error
  puts dump_error.class.to_s + ":" + dump_error.message
end
begin
  JSON.dump([[1]], nil, nil, {max_nesting: 1})
rescue ArgumentError => dump_error
  puts dump_error.class.to_s + ":" + dump_error.message
end
begin
  JSON.dump({}, nil, nil, {}, nil)
rescue ArgumentError => dump_error
  puts dump_error.class.to_s
end
"#;

// A destination can be absent directly or after one to_io conversion. Exercise
// every overload with scalars, arrays, hashes and alternating nested containers.
const FALSEY: &str = r#"
require "json"
class JsonFalseDestination
  attr_reader :conversions
  def initialize
    @conversions = 0
  end
  def to_io
    @conversions += 1
    false
  end
end
class JsonNilDestination
  def to_io
    nil
  end
end
class JsonInheritedFalseDestination < JsonFalseDestination
end
module JsonFalseConversion
  def to_io
    false
  end
end
class JsonIncludedFalseDestination
  include JsonFalseConversion
end
class JsonAliasDestination
  def absent_destination
    false
  end
  alias to_io absent_destination
end
class JsonAliasMethodDestination
  def absent_destination
    nil
  end
  alias_method :to_io, :absent_destination
end
class JsonConvertedDestination
  def initialize(io)
    @io = io
  end
  def to_io
    @io
  end
end
JsonEncoder = JSON
destinations = [false, nil, JsonFalseDestination.new, JsonNilDestination.new,
                JsonInheritedFalseDestination.new, JsonIncludedFalseDestination.new,
                JsonAliasDestination.new, JsonAliasMethodDestination.new]
destinations.each do |destination|
  [nil, false, 7, [], [1], {"a" => [1]}, [{"a" => [1]}]].each do |value|
    p JSON.dump(value, destination)
    p JSON.dump(value, destination, 4)
    p JSON.dump(value, destination, 0)
    p JSON.dump(value, destination, false)
    p JSON.dump(value, destination, {max_nesting: 4})
    p JSON.dump(value, destination, nil, {max_nesting: 4})
    p JSON.dump(value, destination, 1, {max_nesting: false})
  end
  begin
    JSON.dump([{"a" => [1]}], destination, 1)
  rescue ArgumentError => dump_error
    puts dump_error.class.to_s + ":" + dump_error.message
  end
end
adapter = JsonFalseDestination.new
destination_alias = adapter
p ::JSON.dump([1], destination_alias, 2)
p adapter.conversions
p JsonEncoder.dump([1], false, 2)
p JSON.dump(JSON.dump([1], false, 2), false, 2)
# Truthy non-writers still fail, including after exactly one conversion.
[true, 0, "", JsonFalseDestination.new].each do |value|
  begin
    JSON.dump([1], JsonConvertedDestination.new(value), 2)
  rescue NoMethodError => dump_error
    puts dump_error.class.to_s
  end
end
[true, 0, ""].each do |destination|
  begin
    JSON.dump([1], destination, 2)
  rescue NoMethodError => dump_error
    puts dump_error.class.to_s
  end
end
"#;

// These limits used to gain unintended coercions: Float#to_i raises for
// Infinity/NaN, and Rational#to_i invents a limit the oracle does not enforce.
// Fractional Float limits remain an existing bundled-encoder limitation.
const NUMERIC_BOUNDARIES: &str = r#"
require "json"
require "stringio"
[Float::INFINITY, -Float::INFINITY, Float::NAN, Rational(1, 1), Rational(-1, 1)].each do |limit|
  value = [{"a" => [1]}]
  p JSON.dump(value, limit)
  p JSON.dump(value, nil, limit)
  p JSON.dump(value, {max_nesting: limit})
  p JSON.dump(value, nil, {max_nesting: limit})
  p JSON.dump(value, nil, nil, {max_nesting: limit})
  stream = StringIO.new
  p JSON.dump(value, stream, limit).equal?(stream)
  p stream.string
  p JSON.dump(value, nil, 1, {max_nesting: limit})
end
"#;

fn compare_native(script: &str) {
    let oracle = app().run_ruby(script);
    oracle.assert_passes();
    let native = app().run_spinel_with_rbs(script);
    native.assert_passes();
    assert_eq!(
        native.stdout, oracle.stdout,
        "native JSON.dump differs from CRuby"
    );
}

#[test]
fn cruby_dump_contracts() {
    for script in [IO, LIMIT, OPTIONS, FALSEY, NUMERIC_BOUNDARIES] {
        app().run_ruby(script).assert_passes();
    }
}

#[test]
#[ignore = "requires the native Spinel compiler; set SPINEL"]
fn dump_writes_and_returns_its_destination() {
    compare_native(IO);
}

#[test]
#[ignore = "requires the native Spinel compiler; set SPINEL"]
fn dump_honors_the_nesting_limit_before_writing() {
    compare_native(LIMIT);
}

#[test]
#[ignore = "requires the native Spinel compiler; set SPINEL"]
fn dump_distinguishes_options_from_invalid_nesting_and_arity() {
    compare_native(OPTIONS);
}

#[test]
#[ignore = "requires the native Spinel compiler; set SPINEL"]
fn dump_treats_falsey_destinations_as_absent() {
    compare_native(FALSEY);
}

#[test]
#[ignore = "requires the native Spinel compiler; set SPINEL"]
fn dump_does_not_coerce_noninteger_limits() {
    compare_native(NUMERIC_BOUNDARIES);
}

const RETURN_TYPES: &str = r#"
require "json"
require "stringio"
class JsonDestination
  def to_io
    StringIO.new
  end
end
class JsonAbsentDestination
  def to_io
    nil
  end
end
class JsonFalseDestination
  def to_io
    false
  end
end
class JsonInheritedFalseDestination < JsonFalseDestination
end
module JsonFalseConversion
  def to_io
    false
  end
end
class JsonIncludedFalseDestination
  include JsonFalseConversion
end
class JsonMaybeDestination
  def initialize(flag)
    @flag = flag
  end
  def to_io
    @flag ? StringIO.new : false
  end
end
class JsonThreeWayDestination
  def initialize(writer, absent)
    @writer = writer
    @absent = absent
  end
  def to_io
    @writer ? StringIO.new : (@absent ? false : nil)
  end
end
class JsonFalseOrNilDestination
  def initialize(flag)
    @flag = flag
  end
  def to_io
    @flag ? false : nil
  end
end
class JsonProbe
  JsonEncoder = JSON
  def self.text
    JSON.dump({"a" => 1})
  end
  def self.stream
    JSON.dump({"a" => 1}, StringIO.new)
  end
  def self.limit
    JSON.dump([1], 1)
  end
  def self.options
    JSON.dump([1], {max_nesting: 1})
  end
  def self.converted
    JSON.dump({}, JsonDestination.new)
  end
  def self.absent
    JSON.dump({}, JsonAbsentDestination.new)
  end
  def self.maybe_stream(flag)
    JSON.dump({}, flag ? StringIO.new : nil)
  end
  def self.write_count
    JSON.dump({}, StringIO.new).write("more")
  end
  def self.false_destination
    JSON.dump([1], false, 2)
  end
  def self.false_options
    JSON.dump({"a" => [1]}, false, {max_nesting: 2})
  end
  def self.false_fourth
    JSON.dump([1], false, nil, {max_nesting: 2})
  end
  def self.false_alias
    destination = false
    ::JSON.dump([1], destination, 2)
  end
  def self.receiver_alias
    JsonEncoder.dump([1], false, 2)
  end
  def self.false_adapter
    JSON.dump([1], JsonFalseDestination.new, 2)
  end
  def self.false_inherited
    JSON.dump([1], JsonInheritedFalseDestination.new, 2)
  end
  def self.false_included
    JSON.dump([1], JsonIncludedFalseDestination.new, 2)
  end
  def self.false_nested
    JSON.dump(JSON.dump([1], false, 2), false, 2)
  end
  def self.false_count
    JSON.dump([1], JsonFalseDestination.new, 2).bytesize
  end
  def self.false_or_nil_adapter(flag)
    JSON.dump([1], JsonFalseOrNilDestination.new(flag), 2)
  end
  def self.maybe_false_stream(flag)
    JSON.dump([1], flag ? StringIO.new : false, 2)
  end
  def self.maybe_converted(flag)
    JSON.dump([1], JsonMaybeDestination.new(flag), 2)
  end
  def self.three_way_converted(writer, absent)
    JSON.dump([1], JsonThreeWayDestination.new(writer, absent), 2)
  end
  def self.unknown(io)
    JSON.dump({}, io)
  end
end
"#;

#[test]
fn dump_return_types_follow_the_destination() {
    let (emitted, errors) = app()
        .write("app/services/json_probe.rb", RETURN_TYPES)
        .emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    assert!(emitted.join("runtime/json_impl.rbs").is_file());
    let rbs = std::fs::read_to_string(emitted.join("app/models/json_probe.rbs")).unwrap();
    for name in [
        "text",
        "limit",
        "options",
        "absent",
        "false_destination",
        "false_options",
        "false_fourth",
        "false_alias",
        "receiver_alias",
        "false_adapter",
        "false_inherited",
        "false_included",
        "false_nested",
    ] {
        assert!(
            rbs.contains(&format!("def self.{name}: () -> String\n")),
            "{rbs}"
        );
    }
    for name in ["stream", "converted"] {
        assert!(
            rbs.contains(&format!("def self.{name}: () -> StringIO\n")),
            "{rbs}"
        );
    }
    assert!(
        rbs.contains("def self.write_count: () -> Integer\n"),
        "{rbs}"
    );
    assert!(
        rbs.contains("def self.false_count: () -> Integer\n"),
        "{rbs}"
    );
    let absent = rbs
        .lines()
        .find(|line| line.contains("def self.false_or_nil_adapter:"))
        .unwrap();
    assert!(absent.ends_with(" -> String"), "{rbs}");
    assert!(
        rbs.contains("def self.unknown: (untyped io) -> untyped\n"),
        "{rbs}"
    );
    for name in [
        "maybe_stream",
        "maybe_false_stream",
        "maybe_converted",
        "three_way_converted",
    ] {
        let optional = rbs
            .lines()
            .find(|line| line.contains(&format!("def self.{name}:")))
            .unwrap();
        assert!(
            optional.contains("String | StringIO") || optional.contains("StringIO | String"),
            "{rbs}"
        );
        assert!(
            !optional.contains("bool") && !optional.contains("nil"),
            "{rbs}"
        );
    }
}

#[test]
#[ignore = "requires the native Spinel compiler; set SPINEL"]
fn dump_falsey_returns_execute_with_application_sidecars() {
    let script = r#"
p JsonProbe.false_destination
p JsonProbe.false_options
p JsonProbe.false_fourth
p JsonProbe.false_alias
p JsonProbe.receiver_alias
p JsonProbe.false_adapter
p JsonProbe.false_inherited
p JsonProbe.false_included
p JsonProbe.false_nested
p JsonProbe.false_count
p JsonProbe.false_or_nil_adapter(true)
p JsonProbe.false_or_nil_adapter(false)
p JsonProbe.maybe_false_stream(false)
p JsonProbe.maybe_false_stream(true).string
p JsonProbe.maybe_converted(false)
p JsonProbe.maybe_converted(true).string
p JsonProbe.three_way_converted(false, true)
p JsonProbe.three_way_converted(false, false)
p JsonProbe.three_way_converted(true, false).string
"#;
    let overlay = || app().write("app/services/json_probe.rb", RETURN_TYPES);
    let oracle = overlay().run_ruby(script);
    oracle.assert_passes();
    let native = overlay().run_spinel_with_rbs(script);
    native.assert_passes();
    assert_eq!(native.stdout, oracle.stdout);
}
