//! A method whose body ends in `.html_safe` is written unescaped in a
//! template only on a receiver that has that definition: its class, a
//! subclass of it, or the module itself. Another class's method of the
//! same name is escaped, as Rails 8.1.4 escapes it. Each line of `VIEW`
//! is tagged, and `RENDERS` is what Rails 8.1.4 renders for it.

use super::emit_and_run;

pub const VIEW: &str = concat!(
    "A:<%= @article.label %>\n",
    "C:<%= @comment.label %>\n",
    "F:<%= Formatting.badge %>\n",
    "P:<%= Plain.badge %>\n",
    "U:<%= @article.tag_html %>\n",
    "X:<% x = @comment.nil? ? @article : @comment %><%= x.label %>\n",
    "Z:<% z = @comment.nil? ? @article : @comment %><%= z.tag_html %>\n",
    "B:<%= @base.label %>\n",
    "S:<%= @special.label %>\n",
    "M:<%= @mixed.label %>\n",
    "Y:<%= @article.shiny %>\n",
    "T:<%= Twin1.badge %>\n",
    "W:<%= Twin2.badge %>\n",
    "I:<%= @twin2.badge %>\n",
    "R:<%= @fronted.label %>\n",
    "E:<%= @readered.label %>\n",
    "G:<%= @delegating.label %>\n",
    "K:<% @article.body = \"<k>\" %><%= @article.body %>\n",
    "Q:<% @comment.note_html = \"<q>\" %><%= @comment.note_html %>\n",
    "N:<%= @shelf.first %>\n",
    "O:<%= Loud.badge %>\n",
    "V:<%= Fmt.badge %>\n",
    "H:<%= @recon.label %>\n",
    "L:<%= @relaid.label %>\n",
    "J:<%= content_tag(:span, @comment.label) %>\n",
    "D:<%= content_tag(:span, @article.label) %>\n",
    "w1:<%= @opted.label %>\n",
    "w2:<%= @opted2.label %>\n",
    "s1:<%= @sent.inner_label %>\n",
    "s2:<%= @sent2.inner_label %>\n",
    "c1:<%= content_tag(:p, @article.label + pluralize(1, @comment.label)) %>\n",
    "c2:<%= content_tag(:p, truncate(@article.label, length: 5)) %>\n",
    "c3:<%= content_tag(:p, truncate(@comment.label, length: 20)) %>\n",
    "c4:<%= content_tag(:p, @comment.label + truncate(\"<b>xx</b>\", length: 5)) %>\n",
    "c5:<%= content_tag(:p, pluralize(1, @article.label)) %>\n",
    "c6:<%= content_tag(:p, @article.label + truncate(\"<b>xx</b>\", length: 5)) %>\n",
    "c7:<%= @article.label + pluralize(1, @comment.label) %>\n",
    "c8:<%= link_to @article.label + pluralize(1, @comment.label), \"/x\" %>\n",
    "c9:<%= tag.p(@article.label + pluralize(1, @comment.label)) %>\n",
    "c10:<% x = @comment.nil? ? @article : @comment %><%= content_tag(:p, x.label + \"!\") %>\n",
    "i1:<%= link_to Safe.html_escape(\"<s>\"), \"/z\" %>\n",
    "i2:<%= link_to_if true, Safe.html_escape(\"<r>\"), \"/z\" %>\n",
    "f1:<% f = @article.nil? %><%= link_to_if f, @article.label + @comment.label, \"/x\" %>\n",
    "f2:<% f = @article.nil? %><%= link_to_if f, truncate(\"<b>xx</b>\", length: 5) + \"!\", \"/x\" %>\n",
    "f3:<% f = @article.nil? %><%= link_to_if f, \"<s>\".html_safe + \"<t>\", \"/x\" %>\n",
    "f4:<% f = @article.nil? %><%= link_to_if f, @article.label, \"/x\" %>\n",
    "f5:<% f = @article.nil? %><%= link_to_if f, @comment.label, \"/x\" %>\n",
);

pub const RENDERS: &[&str] = &[
    "A:<b>a</b>", "C:&lt;c&gt;", "F:<s>", "P:&lt;p&gt;", "U:<u>", "X:&lt;c&gt;", "Z:<u>",
    "B:<b>", "S:&lt;x&gt;", "M:&lt;m&gt;", "Y:<y>", "T:&lt;t1&gt;", "W:<t2>", "I:&lt;i2&gt;",
    "R:&lt;f&gt;", "E:&lt;r&gt;", "G:&lt;d&gt;", "Q:&lt;q&gt;",
    "N:&lt;e&gt;", "O:&lt;o&gt;", "V:<v>", "H:&lt;cr&gt;",
    "J:<span>&lt;c&gt;</span>", "D:<span><b>a</b></span>",
    "c1:<p><b>a</b>1 &lt;c&gt;</p>", "c2:<p>&lt;b...</p>", "c3:<p>&lt;c&gt;</p>",
    "c4:<p>&lt;c&gt;&amp;lt;b...</p>", "c5:<p>1 &lt;b&gt;a&lt;/b&gt;</p>",
    "c6:<p><b>a</b>&lt;b...</p>", "c7:<b>a</b>1 &lt;c&gt;",
    "c8:<a href=\"/x\"><b>a</b>1 &lt;c&gt;</a>", "c9:<p><b>a</b>1 &lt;c&gt;</p>",
    "c10:<p>&lt;c&gt;!</p>",
    "i1:<a href=\"/z\">&lt;s&gt;</a>", "i2:<a href=\"/z\">&lt;r&gt;</a>",
    "f1:<b>a</b>&lt;c&gt;", "f2:&lt;b...!", "f3:<s>&lt;t&gt;", "f4:<b>a</b>", "f5:&lt;c&gt;",
];

/// What trusting a mark too widely wrote: another class's `label`,
/// another module's `badge`, `label` on an `Article | Comment` receiver
/// (only `Article` has the marked one), and each value whose closer
/// definition shadows a marked one, unescaped, also as `content_tag`'s
/// content.
pub const ABSENT: &[&str] = &["C:<c>", "P:<p>", "X:<c>", "S:<x>", "M:<m>", "T:<t1>", "I:<i2>", "R:<g>", "R:<f>", "E:<r>", "G:<d>", "K:<k>", "Q:<q>", "N:<e>", "N:<w>", "O:<o>", "O:<t2>", "H:<cr>", "H:<b>", "L:<cr>", "L:<l>", "J:<span><c></span>", "w1:<ow>", "w1:<b>", "w2:<ow>", "w2:<b>", "s1:<d>", "s1:<il>", "s2:<d>", "s2:<il>", "c1:<p><b>a</b>1 <c></p>", "c2:<p><b", "c3:<p><c></p>", "c4:<p><c>", "c5:<p>1 <b>a</b></p>", "c6:<p><b>a</b><b...</p>", "c7:<b>a</b>1 <c>", "c8:<a href=\"/x\"><b>a</b>1 <c></a>", "c9:<p><b>a</b>1 <c></p>", "c10:<p><c>!</p>", "i1:<a href=\"/z\">&amp;", "i2:<a href=\"/z\">&amp;", "f1:<b>a</b><c>", "f2:&amp;lt;b", "f3:<s><t>", "f4:&lt;b&gt;a", "f5:<c>"];

/// real-blog with `GET /probe` rendering `VIEW`:
/// - `Article#label` and `ApplicationRecord#tag_html` return html-safe
///   markup; `Comment#label` returns a plain String;
/// - `Formatting.badge` is html-safe, `Plain.badge` isn't;
/// - `content_tag(:span, …)` takes `Comment#label` and `Article#label`
///   as its content, escaping the first only;
/// - `x` and `z` are typed `Article | Comment`: `label` is marked only
///   on `Article`, so it is escaped; `tag_html` is marked on both
///   classes' parent, so it is trusted (which shows the receiver's
///   classes are known, not untyped);
/// - `Base#label` is html-safe; `Special < Base` overrides it with a
///   plain String, and so does `Plainer`, which `Mixed < Base` includes;
/// - `Labelled`, a concern `Article` includes, defines html-safe `shiny`;
/// - `Twin1`'s instance `badge` is html-safe and its `self.badge` isn't;
///   `Twin2`'s `self.badge` is html-safe and its instance `badge` isn't;
/// - `Fronted#label` is html-safe, but `Fronted` prepends `PlainFront`,
///   whose plain `label` comes first in the lookup;
/// - `Readered < Base` overrides `label` with an `attr_reader`, and
///   `Delegating < Base` with a `delegate` to an `InnerLabel`;
/// - `ApplicationRecord#note_html` is html-safe, and `Comment` overrides
///   it with an `attr_accessor`;
/// - `Base#first` is html-safe, and `Shelf < Base` includes `Enumerable`,
///   whose `first` comes first; `Loud < Twin2` extends `Quiet`, whose
///   plain `badge` answers `Loud.badge` ahead of `Twin2.badge`;
/// - `Fmt.badge` is an `extend self` module's html-safe method;
/// - `Relabel`'s `included do` block defines a plain `label` on its
///   includer: on `Recon < Base`, over `Base#label`, and on `Relaid`,
///   after `Relaid`'s own html-safe `label`. Rails 8.1.4 renders
///   `L:&lt;cr&gt;`; the emitted `Relaid` answers with its own `label`,
///   so the line only checks that neither value is written unescaped;
/// - `Opted < Base` and `Opted2 < Base` define `label` with a `delegate`
///   inside a `with_options` block, through the block's param and through
///   its implicit receiver. Rails 8.1.4 renders `&lt;ow&gt;` for both;
///   the emitted classes answer with `Base#label`, so the lines only
///   check that neither value is written unescaped;
/// - `Sent < Base` and `Sent2 < Base` make that call through `send` and
///   `public_send`, with `prefix: :inner`, over `Base#inner_label`, which
///   is html-safe. Rails 8.1.4 renders `&lt;d&gt;`; the lines check that
///   neither `<d>` nor `<il>` is written unescaped;
/// - lines `c1`–`c10` combine a marked (`Article#label`) or unmarked
///   (`Comment#label`) value with a nested `pluralize` or `truncate`, as
///   `content_tag`'s content, `link_to`'s label, `tag.p`'s content and
///   bare output: a `+` onto the marked value stays html-safe and escapes
///   the other side; `+` onto the unmarked value, or onto `label` on an
///   `Article | Comment` receiver, is escaped whole; `truncate` escapes an
///   unmarked value, and a marked one it cuts; `pluralize` drops the
///   mark;
/// - `Safe.html_escape` (#1808's reduction) escapes its argument and
///   returns it html-safe; lines `i1` and `i2` take it as `link_to`'s and
///   `link_to_if`'s label, which Rails 8.1.4 shows as it is, escaped once;
/// - lines `f1` to `f5` take `link_to_if`'s false branch, which shows
///   the label as a bare `<%= %>` does: a marked value, a `truncate` or an
///   `x.html_safe`, alone or on the left of a `+`, stays as it is and the
///   `+`'s plain operand is escaped; a plain value is escaped;
/// - `ApplicationRecord#body` is html-safe, and `articles.body` is a
///   column. Rails 8.1.4 renders `K:<j>`: Active Record doesn't generate
///   a reader that a class between the model and `ActiveRecord::Base`
///   already defines. The emitted model's column reader is the one
///   called, so the line only checks that its value isn't written
///   unescaped.
pub fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "app/models/application_record.rb",
            "  primary_abstract_class\n",
            "  primary_abstract_class\n\n  def tag_html\n    \"<u>\".html_safe\n  end\n\n  def body\n    \"<j>\".html_safe\n  end\n\n  def note_html\n    \"<n>\".html_safe\n  end\n",
        )
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  include Labelled\n\n  validates :title, presence: true\n\n  def label\n    \"<b>a</b>\".html_safe\n  end\n",
        )
        .edit(
            "app/models/comment.rb",
            "  validates :commenter, presence: true\n",
            "  attr_accessor :note_html\n\n  validates :commenter, presence: true\n\n  def label\n    \"<c>\"\n  end\n",
        )
        .write("lib/safe.rb", "module Safe\n  def self.html_escape(s)\n    s.gsub(\"<\", \"&lt;\").gsub(\">\", \"&gt;\").html_safe\n  end\nend\n")
        .write("lib/formatting.rb", "module Formatting\n  def self.badge\n    \"<s>\".html_safe\n  end\nend\n")
        .write("lib/plain.rb", "module Plain\n  def self.badge\n    \"<p>\"\n  end\nend\n")
        .write("lib/base.rb", "class Base\n  def label\n    \"<b>\".html_safe\n  end\n\n  def first\n    \"<w>\".html_safe\n  end\n\n  def inner_label\n    \"<il>\".html_safe\n  end\nend\n")
        .write("lib/special.rb", "class Special < Base\n  def label\n    \"<x>\"\n  end\nend\n")
        .write("lib/plainer.rb", "module Plainer\n  def label\n    \"<m>\"\n  end\nend\n")
        .write("lib/mixed.rb", "class Mixed < Base\n  include Plainer\nend\n")
        .write("lib/twin1.rb", "module Twin1\n  def self.badge\n    \"<t1>\"\n  end\n\n  def badge\n    \"<i1>\".html_safe\n  end\nend\n")
        .write("lib/twin2.rb", "class Twin2\n  def self.badge\n    \"<t2>\".html_safe\n  end\n\n  def badge\n    \"<i2>\"\n  end\nend\n")
        .write("lib/plain_front.rb", "module PlainFront\n  def label\n    \"<f>\"\n  end\nend\n")
        .write("lib/fronted.rb", "class Fronted\n  prepend PlainFront\n\n  def label\n    \"<g>\".html_safe\n  end\nend\n")
        .write("lib/readered.rb", "class Readered < Base\n  attr_reader :label\n\n  def initialize\n    @label = \"<r>\"\n  end\nend\n")
        .write("lib/inner_label.rb", "class InnerLabel\n  def label\n    \"<d>\"\n  end\nend\n")
        .write("lib/delegating.rb", "class Delegating < Base\n  delegate :label, to: :inner\n\n  def inner\n    InnerLabel.new\n  end\nend\n")
        .write("lib/shelf.rb", "class Shelf < Base\n  include Enumerable\n\n  def each\n    yield \"<e>\"\n  end\nend\n")
        .write("lib/quiet.rb", "module Quiet\n  def badge\n    \"<o>\"\n  end\nend\n")
        .write("lib/loud.rb", "class Loud < Twin2\n  extend Quiet\nend\n")
        .write("lib/fmt.rb", "module Fmt\n  extend self\n\n  def badge\n    \"<v>\".html_safe\n  end\nend\n")
        .write("app/models/concerns/relabel.rb", "module Relabel\n  extend ActiveSupport::Concern\n\n  included do\n    def label\n      \"<cr>\"\n    end\n  end\nend\n")
        .write("lib/recon.rb", "class Recon < Base\n  include Relabel\nend\n")
        .write("lib/relaid.rb", "class Relaid\n  def label\n    \"<l>\".html_safe\n  end\n\n  include Relabel\nend\n")
        .write("lib/inner_ow.rb", "class InnerOw\n  def label\n    \"<ow>\"\n  end\nend\n")
        .write("lib/opted.rb", "class Opted < Base\n  with_options to: :inner do |o|\n    o.delegate :label\n  end\n\n  def inner\n    InnerOw.new\n  end\nend\n")
        .write("lib/sent.rb", "class Sent < Base\n  send(:with_options, prefix: :inner) { |o| o.delegate :label, to: :x }\n\n  def x\n    InnerLabel.new\n  end\nend\n")
        .write("lib/sent2.rb", "class Sent2 < Base\n  public_send(:with_options, prefix: :inner) { |o| o.delegate :label, to: :x }\n\n  def x\n    InnerLabel.new\n  end\nend\n")
        .write("lib/opted2.rb", "class Opted2 < Base\n  with_options to: :inner do\n    delegate :label\n  end\n\n  def inner\n    InnerOw.new\n  end\nend\n")
        .write("app/models/concerns/labelled.rb", "module Labelled\n  extend ActiveSupport::Concern\n\n  def shiny\n    \"<y>\".html_safe\n  end\nend\n")
        .write(
            "app/controllers/probes_controller.rb",
            "class ProbesController < ApplicationController\n  def show\n    @article = Article.new\n    @comment = Comment.new\n    @base = Base.new\n    @special = Special.new\n    @mixed = Mixed.new\n    @twin2 = Twin2.new\n    @fronted = Fronted.new\n    @readered = Readered.new\n    @delegating = Delegating.new\n    @shelf = Shelf.new\n    @recon = Recon.new\n    @relaid = Relaid.new\n    @opted = Opted.new\n    @opted2 = Opted2.new\n    @sent = Sent.new\n    @sent2 = Sent2.new\n  end\nend\n",
        )
        .write("app/views/probes/show.html.erb", VIEW)
        .edit("config/routes.rb", "  root \"articles#index\"\n", "  root \"articles#index\"\n  get \"/probe\", to: \"probes#show\", as: :probe\n")
}

/// A minitest integration test asserting each of `RENDERS`, and none of
/// `ABSENT`, in the probe's body.
pub fn integration_test() -> String {
    let mut asserts = String::new();
    for r in RENDERS {
        asserts.push_str(&format!("    assert_includes response.body, {}\n", ruby_str(r)));
    }
    for a in ABSENT {
        asserts.push_str(&format!("    refute_includes response.body, {}\n", ruby_str(a)));
    }
    format!(
        "require \"test_helper\"\n\nclass ProbesControllerTest < ActionDispatch::IntegrationTest\n  test \"html safe methods by owner\" do\n    get probe_url\n    assert_response :success\n{asserts}  end\nend\n"
    )
}

/// The same checks as a script, for the Spinel binary.
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
        script.push_str(&format!("raise \"missing \" + {0} + \" in \" + body unless body.include?({0})\n", ruby_str(r)));
    }
    for a in ABSENT {
        script.push_str(&format!("raise \"unexpected \" + {0} + \" in \" + body if body.include?({0})\n", ruby_str(a)));
    }
    script.push_str("puts \"html safe owners rendered\"\n");
    script
}

fn ruby_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}
