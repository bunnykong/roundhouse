//! `link_to_if`'s label: Rails 8.1.4 escapes it in both branches. A label
//! already escaped by the view helpers (`truncate`) or by `ERB::Util` is
//! escaped once; one through an app's own `html_escape`, which may not
//! escape, is escaped.

use super::emit_and_run;

pub const VIEW: &str = concat!(
    "<%= link_to_if false, \"<i>\", \"/x\" %>\n",
    "<%= link_to_if true, \"<u>\", \"/x\" %>\n",
    "<%= link_to_if false, ERB::Util.html_escape(\"<v>\"), \"/x\" %>\n",
    "<%= link_to_if true, ERB::Util.h(\"<w>\"), \"/x\" %>\n",
);

/// What Rails 8.1.4 renders for the lines of `VIEW`.
pub const RENDERS: &[&str] = &["&lt;i&gt;", "<a href=\"/x\">&lt;u&gt;</a>", "&lt;v&gt;", "<a href=\"/x\">&lt;w&gt;</a>"];

/// Neither a raw label nor a second escape.
pub const ABSENT: &[&str] = &["<i>", "<u>", "<v>", "<w>", "&amp;lt;"];

/// A label the view helpers escape (`truncate`), taken once.
pub const TRUNCATE_LINE: &str = "<%= link_to_if false, truncate(\"<b>xx</b>\", length: 5), \"/x\" %>\n";
pub const TRUNCATE_RENDERS: &str = "&lt;b...";
pub const TRUNCATE_ABSENT: &str = "<b>xx";

/// Through an app's own `html_escape`, which returns its argument as it
/// is. Ruby and Spinel only: Crystal's emit has no plain app module
/// (`undefined constant Formatting`).
pub const APP_ESCAPE_VIEW: &str = concat!(
    "<%= link_to_if false, Formatting.html_escape(\"<q>\"), \"/y\" %>\n",
    "<%= link_to_if true, Formatting.html_escape(\"<r>\"), \"/y\" %>\n",
);
pub const APP_ESCAPE_RENDERS: &[&str] = &["&lt;q&gt;", "<a href=\"/y\">&lt;r&gt;</a>"];
pub const APP_ESCAPE_ABSENT: &[&str] = &["<q>", "<r>"];

/// Through an app's own `html_escape` that escapes and returns an
/// html_safe string: shown as it is, not escaped again. The false branch
/// only: the link branch's label is escaped again before this change too.
/// Its own overlay, as the html_safe mark is recorded per method name.
pub const SAFE_ESCAPE_VIEW: &str = "<%= link_to_if false, Safe.html_escape(\"<s>\"), \"/z\" %>\n";
pub const SAFE_ESCAPE_RENDERS: &str = "&lt;s&gt;";
pub const SAFE_ESCAPE_ABSENT: &[&str] = &["<s>", "&amp;lt;s"];

/// `app` with `SAFE_ESCAPE_VIEW` and the app module it calls.
pub fn app_with_safe_escape() -> emit_and_run::Overlay {
    app()
        .write("app/views/probes/show.html.erb", &format!("{VIEW}{SAFE_ESCAPE_VIEW}"))
        .write("lib/safe.rb", "module Safe\n  def self.html_escape(s)\n    s.gsub(\"<\", \"&lt;\").gsub(\">\", \"&gt;\").html_safe\n  end\nend\n")
}

/// A minitest integration test for `app_with_safe_escape()`'s page.
pub fn safe_integration_test() -> String {
    let mut asserts = format!("    assert_includes response.body, {}\n", ruby_str(SAFE_ESCAPE_RENDERS));
    for a in SAFE_ESCAPE_ABSENT {
        asserts.push_str(&format!("    refute_includes response.body, {}\n", ruby_str(a)));
    }
    format!(
        "require \"test_helper\"\n\nclass ProbesControllerTest < ActionDispatch::IntegrationTest\n  test \"link_to_if shows an html_safe label once\" do\n    get probe_url\n    assert_response :success\n{asserts}  end\nend\n"
    )
}

/// Ruby statements that raise unless `body`, `app_with_safe_escape()`'s
/// page, holds the safe label once escaped.
pub fn safe_body_checks() -> String {
    let mut checks = format!("raise body unless body.include?({})\n", ruby_str(SAFE_ESCAPE_RENDERS));
    for a in SAFE_ESCAPE_ABSENT {
        checks.push_str(&format!("raise body if body.include?({})\n", ruby_str(a)));
    }
    checks
}

/// real-blog with `GET /probe` rendering `VIEW`.
pub fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/controllers/probes_controller.rb", "class ProbesController < ApplicationController\n  def show\n  end\nend\n")
        .write("app/views/probes/show.html.erb", VIEW)
        .edit("config/routes.rb", "  root \"articles#index\"\n", "  root \"articles#index\"\n  get \"/probe\", to: \"probes#show\", as: :probe\n")
}

/// `app` with `TRUNCATE_LINE` too.
pub fn app_with_truncate() -> emit_and_run::Overlay {
    app().write("app/views/probes/show.html.erb", &format!("{VIEW}{TRUNCATE_LINE}"))
}

/// `app` with `APP_ESCAPE_VIEW` and `TRUNCATE_LINE` too, and the app
/// module it calls.
pub fn app_with_app_escape() -> emit_and_run::Overlay {
    app()
        .write("app/views/probes/show.html.erb", &format!("{VIEW}{APP_ESCAPE_VIEW}{TRUNCATE_LINE}"))
        .write("lib/formatting.rb", "module Formatting\n  def self.html_escape(s)\n    s\n  end\nend\n")
}

fn all_renders() -> Vec<&'static str> {
    RENDERS.iter().chain(APP_ESCAPE_RENDERS).copied().chain([TRUNCATE_RENDERS]).collect()
}

fn all_absent() -> Vec<&'static str> {
    ABSENT.iter().chain(APP_ESCAPE_ABSENT).copied().chain([TRUNCATE_ABSENT]).collect()
}

/// A minitest integration test for `app_with_app_escape()`'s page.
pub fn integration_test() -> String {
    let mut asserts = String::new();
    for r in all_renders() {
        asserts.push_str(&format!("    assert_includes response.body, {}\n", ruby_str(r)));
    }
    for a in all_absent() {
        asserts.push_str(&format!("    refute_includes response.body, {}\n", ruby_str(a)));
    }
    format!(
        "require \"test_helper\"\n\nclass ProbesControllerTest < ActionDispatch::IntegrationTest\n  test \"link_to_if escapes its label\" do\n    get probe_url\n    assert_response :success\n{asserts}  end\nend\n"
    )
}

/// Ruby statements that raise unless `body`, `app_with_app_escape()`'s
/// page, holds every render and none of the absent strings.
pub fn body_checks() -> String {
    let mut checks = String::new();
    for r in all_renders() {
        checks.push_str(&format!("raise body unless body.include?({})\n", ruby_str(r)));
    }
    for a in all_absent() {
        checks.push_str(&format!("raise body if body.include?({})\n", ruby_str(a)));
    }
    checks
}

fn ruby_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}
