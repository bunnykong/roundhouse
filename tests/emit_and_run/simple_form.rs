//! simple_form compiled from its installed source: `simple_form_for` runs on a view context at request time, its
//! `SimpleForm::FormBuilder` over Rails' FormBuilder, and the initializer's `SimpleForm.setup` configures it at boot.
//! The expected strings are Rails 8.1.4's own output for the same record, validations and setup.

use roundhouse::build_target::BuildTarget;

use super::emit_and_run;

const VERSION: &str = "5.4.1";

fn installed_simple_form() -> std::path::PathBuf {
    let out = std::process::Command::new("ruby")
        .args(["-e", &format!("print Gem::Specification.find_by_name('simple_form', '{VERSION}').gem_dir")])
        .output()
        .expect("spawn ruby");
    assert!(out.status.success(), "simple_form {VERSION} is not installed: {}", String::from_utf8_lossy(&out.stderr));
    std::path::PathBuf::from(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn rb_files(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            rb_files(root, &path, out);
        } else if path.extension().is_some_and(|e| e == "rb") {
            out.push(path.strip_prefix(root).unwrap().to_path_buf());
        }
    }
}

const INITIALIZER: &str = r#"SimpleForm.setup do |config|
  config.wrappers :default, class: :input, hint_class: :field_with_hint, error_class: :field_with_errors do |b|
    b.use :html5
    b.use :placeholder
    b.optional :maxlength
    b.use :label_input
    b.use :hint,  wrap_with: { tag: :span, class: :hint }
    b.use :error, wrap_with: { tag: :span, class: :error }
  end
  config.default_wrapper = :default
  config.boolean_style = :nested
  config.button_class = 'btn'
  config.error_notification_tag = :div
  config.browser_validations = false
end
"#;

const FORM: &str = r#"<%= simple_form_for(article) do |f| %>
  <%= f.input :title %>
  <%= f.input :body, as: :text, hint: "Ten characters at least" %>
  <%= f.button :submit %>
<% end %>
"#;

const SCRIPT: &str = r##"def rendered(article)
  io = +""
  Views::Articles.form_into(io, article, nil, nil)
  io
end
form = "<form class=\"simple_form new_article\" id=\"new_article\" novalidate=\"novalidate\" action=\"/articles\" accept-charset=\"UTF-8\" method=\"post\">"
fresh = rendered(Article.new(title: "Hi <b>", body: "Body"))
[form,
 "<div class=\"input string required article_title\"><label class=\"string required\" for=\"article_title\"><abbr title=\"required\">*</abbr> Title</label><input class=\"string required\" type=\"text\" value=\"Hi &lt;b&gt;\" name=\"article[title]\" id=\"article_title\" /></div>",
 "<div class=\"input text required article_body field_with_hint\"><label class=\"text required\" for=\"article_body\"><abbr title=\"required\">*</abbr> Body</label><textarea class=\"text required\" name=\"article[body]\" id=\"article_body\">\nBody</textarea><span class=\"hint\">Ten characters at least</span></div>",
 "<input type=\"submit\" name=\"commit\" value=\"Create Article\" class=\"btn\" data-disable-with=\"Create Article\" />"].each do |part|
  raise "fresh:\n#{part}\nin\n#{fresh}" unless fresh.include?(part)
end
blank = Article.new(title: "", body: "short")
blank.valid?
invalid = rendered(blank)
["<div class=\"input string required article_title field_with_errors\"><label class=\"string required\" for=\"article_title\"><abbr title=\"required\">*</abbr> Title</label><input class=\"string required\" aria-invalid=\"true\" type=\"text\" value=\"\" name=\"article[title]\" id=\"article_title\" /><span class=\"error\">can&#39;t be blank</span></div>",
 "<div class=\"input text required article_body field_with_errors field_with_hint\"><label class=\"text required\" for=\"article_body\"><abbr title=\"required\">*</abbr> Body</label><textarea class=\"text required\" aria-invalid=\"true\" name=\"article[body]\" id=\"article_body\">\nshort</textarea><span class=\"hint\">Ten characters at least</span><span class=\"error\">is too short (minimum is 10 characters)</span></div>"].each do |part|
  raise "invalid:\n#{part}\nin\n#{invalid}" unless invalid.include?(part)
end
puts "simple_form passed"
"##;

fn app() -> emit_and_run::Overlay {
    let gem = installed_simple_form();
    let fixture = roundhouse::fixtures::real_blog();
    let lock = std::fs::read_to_string(fixture.join("Gemfile.lock")).unwrap();
    let lock = lock
        // Not a bare "    rack (": a dependency line ("      rack (>= 2.2.4)") holds it too.
        .replacen("\n    rack (", &format!("\n    simple_form ({VERSION})\n      actionpack (>= 7.0)\n      activemodel (>= 7.0)\n    rack ("), 1)
        .replacen("DEPENDENCIES\n", "DEPENDENCIES\n  simple_form\n", 1);
    let gemfile = std::fs::read_to_string(fixture.join("Gemfile")).unwrap() + "gem \"simple_form\"\n";
    let mut overlay = emit_and_run::real_blog()
        .write("Gemfile", &gemfile)
        .write("Gemfile.lock", &lock)
        .write(".bundle/config", "---\nBUNDLE_PATH: \"vendor/bundle\"\n")
        .write("config/initializers/simple_form.rb", INITIALIZER)
        .write("app/views/articles/_form.html.erb", FORM);
    let mut files = Vec::new();
    rb_files(&gem, &gem.join("lib"), &mut files);
    for file in files {
        let text = std::fs::read_to_string(gem.join(&file)).unwrap();
        overlay = overlay.write(&format!("vendor/bundle/ruby/3.4.0/gems/simple_form-{VERSION}/{}", file.display()), &text);
    }
    overlay
}

/// What the gem still does that no tree answers, none of which the form above reaches: `simple_fields_for`'s
/// `self.class < FormBuilder`, the date inputs' `DateTimeSelector::POSITION`, the grouped select's
/// `sample.instance_methods` over a collection's classes, and `include_component`'s `include` at run time.
const KNOWN_GAPS: [&str; 4] = [
    "`<` with incompatible operand types",
    "constant not supported (all targets): ActionView::Helpers::DateTimeSelector::POSITION",
    "Module protocol not supported (all targets): instance_methods requires a proven class/module object receiver",
    "Module mixin at run time not supported (all targets)",
];

#[test]
fn simple_form_for_renders_as_rails_on_ruby() {
    let run = app().run_ruby(SCRIPT);
    let unknown: Vec<_> = run.errors.iter().filter(|e| !KNOWN_GAPS.iter().any(|gap| e.contains(gap))).collect();
    assert!(unknown.is_empty(), "{unknown:#?}");
    for gap in KNOWN_GAPS {
        assert!(run.errors.iter().any(|e| e.contains(gap)), "{gap} is answered now: drop it from KNOWN_GAPS\n{:#?}", run.errors);
    }
    assert!(run.program_succeeded(), "{}\n{}", run.stdout, run.stderr);
    assert!(run.stdout.contains("simple_form passed"), "{}", run.stdout);
}

/// No other tree carries Rails' FormBuilder at request time yet: the builder's `input` is reported, not left to
/// raise at render.
#[test]
fn simple_form_inputs_are_reported_off_the_ruby_family() {
    let (_, errors) = app().emit(BuildTarget::Spinel);
    assert!(errors.iter().any(|e| e.contains("custom form builder method") && e.contains("input")), "{errors:#?}");
}
