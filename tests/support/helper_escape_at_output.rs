//! #1796: a view helper inside a larger template expression. Rails 8.1.4
//! marks `truncate`'s and `word_wrap`'s result html-safe and `pluralize`'s
//! not; `safe + x` stays safe and escapes `x`; interpolation drops the mark.
//! Each line of `VIEW` is tagged `L<n>:`, and `RENDERS` is what Rails 8.1.4
//! renders after the tag, `content_tag`'s content among them.

use super::emit_and_run;

pub const VIEW: &str = concat!(
    "L0:<%= truncate(\"<b>xx</b>\", length: 5) + \"!\" %>\n",
    "L1:<% w = \"<b>x</b>\" %><%= pluralize(1, w) + \"!\" %>\n",
    "L2:<% w = \"<b>x</b>\" %><%= link_to \"#{pluralize(1, w)}\", \"/x\" %>\n",
    "L3:<%= link_to \"#{truncate(\"<b>xx</b>\", length: 5)}\", \"/x\" %>\n",
    "L4:<%= word_wrap(\"<i>a b</i>\", line_width: 3) + \"!\" %>\n",
    "L5:<% w = \"<b>x</b>\" %><%= \"#{pluralize(1, w)}!\" %>\n",
    "L6:<%= \"#{truncate(\"<b>xx</b>\", length: 5)}!\" %>\n",
    "L7:<%= \"!\" + truncate(\"<b>xx</b>\", length: 5) %>\n",
    "L8:<%= truncate(\"<b>xx</b>\", length: 5) + \"<i>\" %>\n",
    "L9:<% w = \"<b>x</b>\" %><%= link_to pluralize(1, w) + \"!\", \"/x\" %>\n",
    "L10:<%= link_to truncate(\"<b>xx</b>\", length: 5) + \"!\", \"/x\" %>\n",
    "L11:<%= link_to \"<s>\".html_safe, \"/x\" %>\n",
    "L12:<%= \"<s>\".html_safe + \"<t>\" %>\n",
    "L13:<%= link_to \"<s>\".html_safe + \"<t>\", \"/x\" %>\n",
    "L14:<% w = \"<b>x</b>\" %><%= \"!\" + pluralize(1, w) %>\n",
    "L15:<%= tag.p(truncate(\"<b>xx</b>\", length: 5)) %>\n",
    "L16:<% w = \"<b>x</b>\" %><%= tag.p(pluralize(1, w)) %>\n",
    "L17:<%= tag.p(truncate(\"<b>xx</b>\", length: 5) + \"!\") %>\n",
    "L18:<% w = \"<b>x</b>\" %><%= content_tag(:p, pluralize(1, w)) %>\n",
    "L19:<%= content_tag(:p, truncate(\"<b>xx</b>\", length: 5)) %>\n",
    "L20:<%= content_tag(:p, truncate(\"<b>xx</b>\", length: 5) + \"!\") %>\n",
);

pub const RENDERS: &[&str] = &[
    "L0:&lt;b...!",
    "L1:1 &lt;b&gt;x&lt;/b&gt;!",
    "L2:<a href=\"/x\">1 &lt;b&gt;x&lt;/b&gt;</a>",
    "L3:<a href=\"/x\">&amp;lt;b...</a>",
    "L4:&lt;i&gt;a\nb&lt;/i&gt;!",
    "L5:1 &lt;b&gt;x&lt;/b&gt;!",
    "L6:&amp;lt;b...!",
    "L7:!&amp;lt;b...",
    "L8:&lt;b...&lt;i&gt;",
    "L9:<a href=\"/x\">1 &lt;b&gt;x&lt;/b&gt;!</a>",
    "L10:<a href=\"/x\">&lt;b...!</a>",
    "L11:<a href=\"/x\"><s></a>",
    "L12:<s>&lt;t&gt;",
    "L13:<a href=\"/x\"><s>&lt;t&gt;</a>",
    "L14:!1 &lt;b&gt;x&lt;/b&gt;",
    "L15:<p>&lt;b...</p>",
    "L16:<p>1 &lt;b&gt;x&lt;/b&gt;</p>",
    "L17:<p>&lt;b...!</p>",
    "L18:<p>1 &lt;b&gt;x&lt;/b&gt;</p>",
    "L19:<p>&lt;b...</p>",
    "L20:<p>&lt;b...!</p>",
];

/// real-blog with `GET /probe` rendering `view`.
pub fn app(view: &str) -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/controllers/probes_controller.rb", "class ProbesController < ApplicationController\n  def show\n  end\nend\n")
        .write("app/views/probes/show.html.erb", view)
        .edit("config/routes.rb", "  root \"articles#index\"\n", "  root \"articles#index\"\n  get \"/probe\", to: \"probes#show\", as: :probe\n")
}

/// The Ruby script that renders the probe and prints each `L<n>:` line's
/// verdict: `ok`, or the rendered text.
pub fn render_script(renders: &[&str]) -> String {
    let wants: String = renders.iter().map(|r| format!("{},", ruby_str(r))).collect();
    format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\ncontroller = ProbesController.new\ncontroller.request_method = \"GET\"\ncontroller.process_action(:show)\nbody = controller.body\nbad = 0\n[{wants}].each_with_index do |want, i|\n  tag = \"L#{{i}}:\"\n  at = body.index(tag)\n  nxt = body.index(\"L#{{i + 1}}:\") || body.index(\"</main>\") || body.length\n  got = at ? body[at...nxt].rstrip : \"(missing)\"\n  if got == want\n    puts \"#{{tag}} ok\"\n  else\n    bad += 1\n    puts \"#{{tag}} got #{{got.inspect}}\"\n  end\nend\nraise \"#{{bad}} lines differ\" if bad > 0\nputs \"all lines as Rails\"\n"
    )
}

/// A minitest integration test for `app`'s page, printing each line's
/// verdict as `render_script` does and failing when one differs.
pub fn integration_test(renders: &[&str]) -> String {
    let wants: String = renders.iter().map(|r| format!("{},", ruby_str(r))).collect();
    format!(
        "require \"test_helper\"\n\nclass ProbesControllerTest < ActionDispatch::IntegrationTest\n  test \"nested helpers escape as Rails\" do\n    get probe_url\n    body = response.body\n    bad = 0\n    [{wants}].each_with_index do |want, i|\n      tag = \"L#{{i}}:\"\n      at = body.index(tag)\n      nxt = body.index(\"L#{{i + 1}}:\") || body.index(\"</main>\") || body.length\n      got = at ? body[at...nxt].rstrip : \"(missing)\"\n      if got == want\n        puts \"#{{tag}} ok\"\n      else\n        bad += 1\n        puts \"#{{tag}} got #{{got.inspect}}\"\n      end\n    end\n    assert_equal 0, bad\n  end\nend\n"
    )
}

fn ruby_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('#', "\\#"))
}
