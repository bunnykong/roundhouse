//! JSON.dump's destination and limit overloads must survive native dispatch.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
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

fn compare_native(script: &str) {
    let oracle = app().run_ruby(script);
    oracle.assert_passes();
    let native = app().run_spinel_with_rbs(script);
    native.assert_passes();
    assert_eq!(native.stdout, oracle.stdout, "native JSON.dump differs from CRuby");
}

#[test]
fn cruby_dump_contracts() {
    for script in [IO, LIMIT, OPTIONS] {
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
fn dump_return_types_follow_the_destination() {
    let (emitted, errors) = app().write("app/services/json_probe.rb", r#"
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
class JsonProbe
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
  def self.unknown(io)
    JSON.dump({}, io)
  end
end
"#).emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    assert!(emitted.join("runtime/json_impl.rbs").is_file());
    let rbs = std::fs::read_to_string(emitted.join("app/models/json_probe.rbs")).unwrap();
    for name in ["text", "limit", "options", "absent"] {
        assert!(rbs.contains(&format!("def self.{name}: () -> String\n")), "{rbs}");
    }
    for name in ["stream", "converted"] {
        assert!(rbs.contains(&format!("def self.{name}: () -> StringIO\n")), "{rbs}");
    }
    assert!(rbs.contains("def self.write_count: () -> Integer\n"), "{rbs}");
    assert!(rbs.contains("def self.unknown: (untyped io) -> untyped\n"), "{rbs}");
    let optional = rbs.lines().find(|line| line.contains("def self.maybe_stream:")).unwrap();
    assert!(optional.contains("String | StringIO") || optional.contains("StringIO | String"), "{rbs}");
}
