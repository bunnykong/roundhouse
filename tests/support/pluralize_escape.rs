//! `pluralize` in a template: Rails 8.1.4 escapes its String, which isn't
//! html-safe, where the emitted views appended it as it was. A label that
//! `link_to` or `button_to` escape is escaped once, not twice, an `h` or
//! `ERB::Util.html_escape` label too, and a label through an app's own
//! `html_escape` is still escaped.

use super::emit_and_run;

pub const VIEW: &str = concat!(
    "<% w = \"<b>x</b>\" %><%= pluralize(1, w) %>\n",
    "<%= pluralize(2, \"<i>\") %>\n",
    "<%= pluralize(2, \"comment\") %>\n",
    "<%= link_to pluralize(4, \"<s>\"), \"/x\" %>\n",
    "<%= button_to pluralize(3, \"<u>\"), \"/x\" %>\n",
    "<%= link_to h(\"<v>\"), \"/z\" %>\n",
    "<%= button_to ERB::Util.html_escape(\"<k>\"), \"/z\" %>\n",
);

/// What Rails 8.1.4 renders for the lines of `VIEW` (the `button_to`
/// label as the text it escapes).
pub const RENDERS: &[&str] = &[
    "1 &lt;b&gt;x&lt;/b&gt;",
    "2 &lt;i&gt;s",
    "2 comments",
    "<a href=\"/x\">4 &lt;s&gt;s</a>",
    "3 &lt;u&gt;s",
    "<a href=\"/z\">&lt;v&gt;</a>",
    "&lt;k&gt;",
];

/// Neither the raw words nor a second escape of them.
pub const ABSENT: &[&str] = &["<b>x</b>", "<i>s", "<s>s", "<u>s", "<v>", "<k>", "&amp;lt;"];

/// A label through an app's own `html_escape`, which returns its argument
/// as it is: Rails 8.1.4 still escapes it. Ruby and Spinel only, since
/// Crystal's emit has no plain app module (`undefined constant Formatting`).
pub const APP_ESCAPE_LINE: &str = "<%= link_to Formatting.html_escape(\"<q>\"), \"/y\" %>\n";
pub const APP_ESCAPE_RENDERS: &str = "<a href=\"/y\">&lt;q&gt;</a>";
pub const APP_ESCAPE_ABSENT: &str = "<q>";

/// real-blog with `GET /probe` rendering `VIEW`.
pub fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/controllers/probes_controller.rb", "class ProbesController < ApplicationController\n  def show\n  end\nend\n")
        .write("app/views/probes/show.html.erb", VIEW)
        .edit("config/routes.rb", "  root \"articles#index\"\n", "  root \"articles#index\"\n  get \"/probe\", to: \"probes#show\", as: :probe\n")
}

/// `app` with `APP_ESCAPE_LINE` in the view and the app module it calls.
pub fn app_with_app_escape() -> emit_and_run::Overlay {
    app()
        .write("app/views/probes/show.html.erb", &format!("{VIEW}{APP_ESCAPE_LINE}"))
        .write("lib/formatting.rb", "module Formatting\n  def self.html_escape(s)\n    s\n  end\nend\n")
}

/// A minitest integration test asserting every `RENDERS` line and none
/// of `ABSENT` in `GET /probe`'s body, with `app_with_app_escape`'s line.
pub fn integration_test() -> String {
    let mut asserts = String::new();
    for r in RENDERS {
        asserts.push_str(&format!("    assert_includes response.body, {}\n", ruby_str(r)));
    }
    for a in ABSENT {
        asserts.push_str(&format!("    refute_includes response.body, {}\n", ruby_str(a)));
    }
    asserts.push_str(&format!("    assert_includes response.body, {}\n", ruby_str(APP_ESCAPE_RENDERS)));
    asserts.push_str(&format!("    refute_includes response.body, {}\n", ruby_str(APP_ESCAPE_ABSENT)));
    format!(
        "require \"test_helper\"\n\nclass ProbesControllerTest < ActionDispatch::IntegrationTest\n  test \"pluralize is escaped\" do\n    get probe_url\n    assert_response :success\n{asserts}  end\nend\n"
    )
}

/// Ruby statements that raise unless `body` holds every `RENDERS` line and
/// none of `ABSENT`, with `app_with_app_escape`'s line.
pub fn body_checks() -> String {
    let mut checks = String::new();
    for r in RENDERS {
        checks.push_str(&format!("raise body unless body.include?({})\n", ruby_str(r)));
    }
    for a in ABSENT {
        checks.push_str(&format!("raise body if body.include?({})\n", ruby_str(a)));
    }
    checks.push_str(&format!("raise body unless body.include?({})\n", ruby_str(APP_ESCAPE_RENDERS)));
    checks.push_str(&format!("raise body if body.include?({})\n", ruby_str(APP_ESCAPE_ABSENT)));
    checks
}

fn ruby_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}
