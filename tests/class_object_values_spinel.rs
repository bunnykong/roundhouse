//! Native consumer of #956's emitted singleton sidecars. Credits
//! eddygarcas's default, namespaced class and keyword-splat reproduction.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
#[ignore = "requires the native Spinel compiler"]
fn class_arguments_and_defaults_build_and_run_with_emitted_rbs() {
    let run = emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\nend\n")
        .write("app/lib/class_object_values.rb", include_str!("support/class_object_values.rb"))
        .run_spinel_with_rbs("puts Probe.result\nputs ClientUser.result\nWidgets.result.each { |answer| puts answer }\n");
    run.assert_passes();
    assert_eq!(run.stdout, "1\nclient\npong 1h\npong 2h\n");
}
