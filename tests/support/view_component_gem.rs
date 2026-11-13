//! view_component compiled from its installed source: a view's `render(GreetingComponent.new(...))` reaches the gem's
//! `render_in`, and the component's sidecar template, compiled into its `call` when the app is read, writes the markup
//! view_component 3.23.2 renders for it on Rails 7.2.2.1 (maybe's versions): `render?`, a component method, an ivar and
//! a view helper; and with a block, its `content` and its `renders_one` / `renders_many` slots (a lambda's among
//! them, whose `html_safe` markup is not escaped again, and a block's markup, which Rails captures safe), escaped as
//! Rails does.

#[path = "prerequisite.rs"]
mod prerequisite;
use super::emit_and_run;

const GEMS: [(&str, &str); 2] = [("view_component", "3.23.2"), ("method_source", "1.1.0")];

fn installed(name: &str, version: &str) -> Option<std::path::PathBuf> {
    let out = std::process::Command::new("ruby")
        .args(["-e", &format!("print Gem::Specification.find_by_name('{name}', '{version}').gem_dir")])
        .output();
    match out {
        Ok(out) if out.status.success() => Some(std::path::PathBuf::from(String::from_utf8_lossy(&out.stdout).into_owned())),
        _ => {
            prerequisite::missing(&format!("ruby with {name} {version} installed"));
            None
        }
    }
}

fn files(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            files(root, &path, out);
        } else {
            out.push(path.strip_prefix(root).unwrap().to_path_buf());
        }
    }
}

const COMPONENT: &str = "class GreetingComponent < ViewComponent::Base\n  def initialize(name:, count: 0)\n    @name = name\n    @count = count\n  end\n\n  def shout = @name.upcase\n\n  def render? = @count >= 0\nend\n";

const TEMPLATE: &str = "<p class=\"greeting\">Hello, <%= shout %>! (<%= pluralize(@count, \"article\") %>)</p>\n";

const CARD: &str = "class CardComponent < ViewComponent::Base\n  renders_one :header\n  renders_many :items\n  renders_one :footer, ->(note:, &block) { \"<small>#{note}</small>\".html_safe }\n\n  def initialize(tone: \"plain\")\n    @tone = tone\n  end\nend\n";

const CARD_TEMPLATE: &str = "<div class=\"card card-<%= @tone %>\">\n  <% if header? %><h2><%= header %></h2><% end %>\n  <ul><% items.each do |item| %><li><%= item %></li><% end %></ul>\n  <div class=\"body\"><%= content %></div>\n  <%= footer %>\n</div>\n";

pub const SCRIPT: &str = r#"io = +""
Views::Articles.index_into(io, Article.all, nil, nil)
expected = "<p class=\"greeting\">Hello, BLOG! (0 articles)</p>"
raise "component:\n#{io}" unless io.include?(expected)
card = "<div class=\"card card-warm\">\n  <h2>Title &lt;b&gt;</h2>\n  <ul><li>one</li><li>two</li><li><b>three</b></li></ul>\n  <div class=\"body\">\n  Body text\n</div>\n  <small>fine <print></small>\n</div>"
raise "card:\n#{io}" unless io.include?(card)
puts "view_component passed"
"#;

pub fn app() -> Option<emit_and_run::Overlay> {
    let fixture = roundhouse::fixtures::real_blog();
    let lock = std::fs::read_to_string(fixture.join("Gemfile.lock")).unwrap();
    let lock = lock
        // Not a bare "    rack (": a dependency line ("      rack (>= 2.2.4)") holds it too.
        .replacen(
            "\n    rack (",
            "\n    method_source (1.1.0)\n    view_component (3.23.2)\n      activesupport (>= 5.2.0, < 8.1)\n      concurrent-ruby (~> 1)\n      method_source (~> 1.0)\n    rack (",
            1,
        )
        .replacen("DEPENDENCIES\n", "DEPENDENCIES\n  view_component\n", 1);
    let gemfile = std::fs::read_to_string(fixture.join("Gemfile")).unwrap() + "gem \"view_component\"\n";
    let mut overlay = emit_and_run::real_blog()
        .write("Gemfile", &gemfile)
        .write("Gemfile.lock", &lock)
        .write(".bundle/config", "---\nBUNDLE_PATH: \"vendor/bundle\"\n")
        .write("app/components/greeting_component.rb", COMPONENT)
        .write("app/components/greeting_component.html.erb", TEMPLATE)
        .write("app/components/card_component.rb", CARD)
        .write("app/components/card_component.html.erb", CARD_TEMPLATE)
        .edit(
            "app/views/articles/index.html.erb",
            "<% content_for :title, \"Articles\" %>\n",
            "<% content_for :title, \"Articles\" %>\n<%= render(GreetingComponent.new(name: \"blog\", count: @articles.size)) %>\n<%= render(CardComponent.new(tone: \"warm\")) do |card| %>\n  <% card.with_header { \"Title <b>\" } %>\n  <% card.with_item { \"one\" } %>\n  <% card.with_item { \"two\" } %>\n  <% card.with_item do %><b>three</b><% end %>\n  <% card.with_footer(note: \"fine <print>\") %>\n  Body text\n<% end %>\n",
        );
    for (name, version) in GEMS {
        let gem = installed(name, version)?;
        let mut list = Vec::new();
        files(&gem, &gem.join("lib"), &mut list);
        files(&gem, &gem.join("app"), &mut list);
        for file in list {
            let Ok(text) = std::fs::read_to_string(gem.join(&file)) else { continue };
            overlay = overlay.write(&format!("vendor/bundle/ruby/3.4.0/gems/{name}-{version}/{}", file.display()), &text);
        }
    }
    Some(overlay)
}

/// What the gem still does that no tree answers, none of which these renders reach: its previews (a development-only
/// page of the engine) and test helpers, the engine's Railtie and `Rails::Engine`, the runtime template compiler this
/// tree replaces (`ActionView::Template`, its `class_eval`s), translations (`I18n` internals, keyword splats through
/// `super`), collections (`ActionView::PartialIteration`), and method_source's computed requires.
pub const KNOWN_GAPS: [&str; 19] = [
    "@render_args has no known type",
    "@example_name has no known type",
    "@preview has no known type",
    "@view_renderer has no known type",
    "render of an object not supported (all targets)",
    "conditional or dynamic visibility declarations are not modeled",
    "railtie middleware not supported (all targets)",
    "constant not supported (all targets): Rails::Engine",
    "constant not supported (all targets): ActionView::Template",
    "Module protocol not supported (all targets): class_eval requires",
    "Module protocol not supported (all targets): define_method requires",
    "Module protocol not supported (all targets): method_defined? requires",
    "constant not supported (all targets): I18n::RESERVED_KEYS",
    "constant not supported (all targets): I18n::MissingTranslation",
    "keyword splat in ordinary super not supported",
    "keyword splat into full argument forwarding not supported",
    "constant not supported (all targets): ActionView::PartialIteration",
    "`<` with incompatible operand types",
    "file_statement_unsupported not supported (ruby)",
];

