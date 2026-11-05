//! `ActiveJob::Arguments.deserialize` reads a payload's arguments back as
//! activejob 8.1.3 does: a GlobalID through the app's models, a Symbol and
//! a Time from their serializers, a Hash's symbol keys, and a gone record
//! or an unknown serializer as DeserializationError (mission_control-jobs
//! shows a job's arguments through it).

use super::emit_and_run;

const PROBE: &str = "class ArgumentsProbe\n  def self.read(arguments)\n    ActiveJob::Arguments.deserialize(arguments)\n  end\nend\n";

const ASSERTIONS: &str = r##"
a = Article.create!(title: "hello world", body: "b" * 20)
uri = a.to_gid_uri
args = JSON.parse("[" + ActiveJob::Arguments.record(uri) + ",1,\"s\",null,true," + ActiveJob::Arguments.sym(:x) + "," + ActiveJob::Arguments.time(Time.utc(2026, 1, 2, 3, 4, 5)) + ",{\"k\":1,\"j\":[2],\"_aj_symbol_keys\":[\"k\"]}]")
out = ArgumentsProbe.read(args)
raise "record" unless out[0].id == a.id
raise "scalars" unless out[1] == 1 && out[2] == "s" && out[3].nil? && out[4] == true
raise "symbol" unless out[5] == :x
raise "time" unless out[6] == Time.utc(2026, 1, 2, 3, 4, 5)
raise "hash" unless out[7].keys == [:k, "j"] && out[7][:k] == 1 && out[7]["j"] == [2]
begin
  ArgumentsProbe.read(JSON.parse("[{\"_aj_globalid\":\"" + uri[0, uri.rindex("/") + 1] + "999\"}]"))
  raise "missing record read"
rescue ActiveJob::DeserializationError => e
  raise e.message unless e.message == "Error while trying to deserialize arguments: Couldn't find Article with 'id'=999"
end
begin
  ArgumentsProbe.read(JSON.parse("[{\"_aj_serialized\":\"ActiveJob::Serializers::DateSerializer\",\"value\":\"2026-01-02\"}]"))
  raise "unknown serializer read"
rescue ActiveJob::DeserializationError => e
  raise e.message unless e.message == "Error while trying to deserialize arguments: Serializer ActiveJob::Serializers::DateSerializer is not known"
end
puts "arguments deserialize passed"
"##;

#[test]
fn a_payloads_arguments_deserialize_as_rails_reads_them() {
    let run = emit_and_run::real_blog().write("app/models/arguments_probe.rb", PROBE).run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("arguments deserialize passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_payloads_arguments_deserialize_on_spinel() {
    let run = emit_and_run::real_blog().write("app/models/arguments_probe.rb", PROBE).run_spinel_with_db(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("arguments deserialize passed"), "{}", run.stdout);
}
