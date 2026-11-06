//! #1780: a non-block `content_tag name, content[, opts]` in a template,
//! shared by the targets' runners. Strict targets kept it as a bare call
//! the view module has no method for; it now reaches the runtime's
//! `content_tag_html` with the content escaped, and is appended unescaped.
//! `RENDERS` is what Rails 8.1.4 renders for each line of `VIEW`.

use super::emit_and_run;

pub const VIEW: &str = concat!(
    "<%= content_tag :p, \"<b>x</b>\" %>\n",
    "<% s = \"<i>\" + \"y\" %><%= content_tag :span, s, class: \"c\" %>\n",
    "<%= content_tag :p, \"y\", class: [\"btn\", \"primary\"] %>\n",
    "<%= content_tag :p, \"t\", id: \"a&b\" %>\n",
    "<%= content_tag \"em\", \"e\" %>\n",
    "<%= content_tag :p, \"y\", { \"data x\" => \"1\" } %>\n",
    "<%= content_tag :p, link_to(\"a\", \"/x\") %>\n",
    "<%= content_tag :p, \"<b>s</b>\".html_safe %>\n",
    "<%= content_tag :p, raw(\"<i>r</i>\") %>\n",
    "<%= content_tag :p, content_tag(:b, \"<n>\") %>\n",
    "<% w = \"<b>x</b>\" %><%= content_tag :p, pluralize(1, w) %>\n",
    "<%= content_tag :p, truncate(\"<b>xx</b>\", length: 5) %>\n",
    "<%= content_tag :p, h(\"<x>\") %>\n",
    "<% hs = \"<y>\" %><%= content_tag :p, h(hs) %>\n",
    "<%= content_tag :p, h(\"<s>t</s>\".html_safe) %>\n",
    "<%= content_tag :p, h(raw(\"<s>v</s>\")) %>\n",
    "<%= content_tag :p, ERB::Util.html_escape(\"<z>\") %>\n",
    "<%= content_tag :p, h(content_tag(:b, \"x\")) %>\n",
    "<%= content_tag :p, ERB::Util.h(content_tag(:b, \"y\")) %>\n",
    "<%= content_tag :p, ERB::Util.html_escape(link_to(\"<l>\", \"/l\")) %>\n",
    "<%= content_tag :p, h(truncate(\"<i>yy</i>\", length: 5)) %>\n",
    "<%= content_tag :div, turbo_stream_from(\"room\") %>\n",
    "<% content_for :t do %><i>m</i><% end %><%= content_tag :p, content_for(:t) %>\n",
    "<% attrs = { class: \"c\" } %><%= content_tag :p, \"x\", attrs %>\n",
    "<%= content_tag :div, class: \"x\" do %>b<% end %>\n",
);

/// What Rails 8.1.4 renders for each line of `VIEW`: the content escaped
/// unless it is html-safe (a helper's markup, including a
/// `turbo_stream_from`'s element and a `content_for`'s capture;
/// `html_safe`; `raw`; an `h` of any of these): a `pluralize`'s, a
/// `truncate`'s and an `h`'s text is escaped only once; options can be a
/// Hash local. The last line is #1771's block form with options, which Go
/// didn't compile. The
/// `turbo_stream_from` render is the element's start: its signed name is
/// the runtime's.
pub const RENDERS: &[&str] = &[
    "<p>&lt;b&gt;x&lt;/b&gt;</p>",
    "<span class=\"c\">&lt;i&gt;y</span>",
    "<p class=\"btn primary\">y</p>",
    "<p id=\"a&amp;b\">t</p>",
    "<em>e</em>",
    "<p data_x=\"1\">y</p>",
    "<p><a href=\"/x\">a</a></p>",
    "<p><b>s</b></p>",
    "<p><i>r</i></p>",
    "<p><b>&lt;n&gt;</b></p>",
    "<p>1 &lt;b&gt;x&lt;/b&gt;</p>",
    "<p>&lt;b...</p>",
    "<p>&lt;x&gt;</p>",
    "<p>&lt;y&gt;</p>",
    "<p><s>t</s></p>",
    "<p><s>v</s></p>",
    "<p>&lt;z&gt;</p>",
    "<p><b>x</b></p>",
    "<p><b>y</b></p>",
    "<p><a href=\"/l\">&lt;l&gt;</a></p>",
    "<p>&lt;i...</p>",
    "<div><turbo-cable-stream-source channel=\"Turbo::StreamsChannel\"",
    "<p><i>m</i></p>",
    "<p class=\"c\">x</p>",
    "<div class=\"x\">b</div>",
];

/// real-blog with `GET /probe` rendering `view`, and an integration test
/// asserting each of `RENDERS` in its body.
pub fn app(view: &str) -> emit_and_run::Overlay {
    app_rendering(view, RENDERS)
}

/// `app` with the integration test asserting `renders` instead.
pub fn app_rendering(view: &str, renders: &[&str]) -> emit_and_run::Overlay {
    let asserts: String = renders
        .iter()
        .map(|r| format!("    assert_includes response.body, {}\n", ruby_str(r)))
        .collect();
    emit_and_run::real_blog()
        .write("app/controllers/probes_controller.rb", "class ProbesController < ApplicationController\n  def show\n  end\nend\n")
        .write("app/views/probes/show.html.erb", view)
        .edit("config/routes.rb", "  root \"articles#index\"\n", "  root \"articles#index\"\n  get \"/probe\", to: \"probes#show\", as: :probe\n")
        .write(
            "test/controllers/probes_controller_test.rb",
            &format!(
                "require \"test_helper\"\n\nclass ProbesControllerTest < ActionDispatch::IntegrationTest\n  test \"content tag renders\" do\n    get probe_url\n    assert_response :success\n{asserts}  end\nend\n"
            ),
        )
}

/// A script rendering `GET /probe` in the process and raising unless the
/// body holds each of `RENDERS`, for a runner with no test framework
/// (the Spinel binary).
pub fn render_script() -> String {
    let mut script = String::from(concat!(
        "Db.configure(\":memory:\")\n",
        "Schema.statements.each { |sql| Db.exec(sql) }\n",
        "ActiveRecord.adapter = SqliteAdapter\n",
        "controller = ProbesController.new\n",
        "controller.request_method = \"GET\"\n",
        "controller.process_action(:show)\n",
        "body = controller.body\n",
    ));
    for r in RENDERS {
        script.push_str(&format!("raise body unless body.include?({})\n", ruby_str(r)));
    }
    script.push_str("puts \"content_tag options rendered\"\n");
    script
}

fn ruby_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}
