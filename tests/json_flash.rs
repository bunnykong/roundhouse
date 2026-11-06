#[path = "support/json_flash.rs"]
mod contract;
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn json_flash_survives_one_redirect_on_ruby() {
    contract::app()
        .run_ruby(&format!(
            "{}\n{}",
            contract::RACK_ADAPTER,
            contract::ASSERTIONS
        ))
        .assert_passes();
}

#[test]
fn json_flash_keeps_string_cookie_bytes_and_authenticates_the_format() {
    contract::app()
        .run_ruby(contract::CODEC_ASSERTIONS)
        .assert_passes();
}
