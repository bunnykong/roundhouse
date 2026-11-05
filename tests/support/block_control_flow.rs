//! #1774: a block helper whose template body starts with control flow
//! (`<% if … %>`, an `each`, a `case`) was taken for a non-template block,
//! so its markup never reached the output (`<div></div>`). Expected output
//! is Rails 8.1.4's.

use super::emit_and_run;

pub const VIEW: &str = concat!(
    "<%= content_tag :div do %><% if true %>yes<% end %><% end %>\n",
    "<%= content_tag :ul do %><% [1, 2].each do |i| %><li><%= i %></li><% end %><% end %>\n",
    "<% n = 2 %><%= content_tag :p do %><% case n %><% when 2 %>two<% end %><% end %>\n",
    "<%= content_tag :span do %><% if false %>no<% else %>else<% end %><% end %>\n",
    "<%= link_to \"/x\" do %><% if true %>go<% end %><% end %>\n",
);

/// What Rails 8.1.4 renders for each line of `VIEW`.
pub const RENDERS: &[&str] = &[
    "<div>yes</div>",
    "<ul><li>1</li><li>2</li></ul>",
    "<p>two</p>",
    "<span>else</span>",
    "<a href=\"/x\">go</a>",
];

/// real-blog with `GET /probe` rendering `VIEW`.
pub fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/controllers/probes_controller.rb", "class ProbesController < ApplicationController\n  def show\n  end\nend\n")
        .write("app/views/probes/show.html.erb", VIEW)
        .edit("config/routes.rb", "  root \"articles#index\"\n", "  root \"articles#index\"\n  get \"/probe\", to: \"probes#show\", as: :probe\n")
}

/// A minitest integration test asserting each of `RENDERS`.
pub fn integration_test() -> String {
    let asserts: String = RENDERS.iter().map(|r| format!("    assert_includes response.body, {}\n", ruby_str(r))).collect();
    format!(
        "require \"test_helper\"\n\nclass ProbesControllerTest < ActionDispatch::IntegrationTest\n  test \"a block starting with control flow renders\" do\n    get probe_url\n    assert_response :success\n{asserts}  end\nend\n"
    )
}

/// Statements that raise unless `body` holds each of `RENDERS`.
pub fn body_checks() -> String {
    RENDERS.iter().map(|r| format!("raise body unless body.include?({})\n", ruby_str(r))).collect()
}

fn ruby_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}
