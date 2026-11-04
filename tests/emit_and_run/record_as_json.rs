//! A record's `as_json` answers each attribute as Rails' does: string
//! keys, and a timestamp as `xmlschema(3)` text rather than the Time
//! (mission_control-jobs shows a solid_queue row's `as_json` as raw data).

use super::emit_and_run;

#[test]
fn a_records_as_json_answers_json_ready_values() {
    let run = emit_and_run::real_blog().run_ruby(
        "a = Article.create!(title: \"hello world\", body: \"b\" * 20)\nh = a.as_json\nraise h.inspect unless h.keys == [\"id\", \"title\", \"body\", \"created_at\", \"updated_at\"]\nraise h[\"created_at\"].inspect unless h[\"created_at\"].is_a?(String) && h[\"created_at\"].match?(/\\A\\d{4}-\\d\\d-\\d\\dT\\d\\d:\\d\\d:\\d\\d\\.\\d{3}(Z|[+-]\\d\\d:\\d\\d)\\z/)\nputs \"record as_json passed\"\n",
    );
    run.assert_passes();
    assert!(run.stdout.contains("record as_json passed"), "{}", run.stdout);
}
