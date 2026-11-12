use super::emit_and_run;

/// A form whose builder is the app's own subclass (`form_with(builder: ProbeFormBuilder)`): a method of that class
/// runs at request time on Rails' FormBuilder, and the stock fields it calls back (`label`, `text_field`, `text_area`,
/// `hidden_field`, `password_field`, `email_field`, `check_box`, `submit`, `button`, `@template.content_tag`,
/// `@template.safe_join`, and a safe `+` of two of them), each
/// wrapped in `field_with_errors` for an attribute with an error, answer the markup Rails 8.1.4 renders for the same
/// record. The expected strings are Rails' own output.
const BUILDER: &str = r##"class ProbeFormBuilder < ActionView::Helpers::FormBuilder
  def everything
    @template.safe_join([label(:title), text_field(:title, placeholder: "x"), text_area(:body, rows: 3), hidden_field(:title),
                         password_field(:title), email_field(:body), check_box(:title), submit, button], "|")
  end

  def boxed(attribute)
    @template.content_tag(:p, label(attribute) + text_field(attribute), class: "row")
  end
end
"##;

const FORM: &str = "<%= form_with(model: article, builder: ProbeFormBuilder) do |form| %>\n<%= form.everything %>\n<%= form.boxed :title %>\n<% end %>\n";

const SCRIPT: &str = r##"def rendered(article)
  io = +""
  Views::Articles.form_into(io, article, nil, nil)
  io
end
fresh = rendered(Article.new(title: "Hi <b>", body: "Long\nbody"))
everything = "<label for=\"article_title\">Title</label>|<input placeholder=\"x\" type=\"text\" value=\"Hi &lt;b&gt;\" name=\"article[title]\" id=\"article_title\" />|<textarea rows=\"3\" name=\"article[body]\" id=\"article_body\">\nLong\nbody</textarea>|<input autocomplete=\"off\" type=\"hidden\" value=\"Hi &lt;b&gt;\" name=\"article[title]\" id=\"article_title\" />|<input type=\"password\" name=\"article[title]\" id=\"article_title\" />|<input type=\"email\" value=\"Long\nbody\" name=\"article[body]\" id=\"article_body\" />|<input name=\"article[title]\" type=\"hidden\" value=\"0\" autocomplete=\"off\" /><input type=\"checkbox\" value=\"1\" name=\"article[title]\" id=\"article_title\" />|<input type=\"submit\" name=\"commit\" value=\"Create Article\" data-disable-with=\"Create Article\" />|<button name=\"button\" type=\"submit\">Create Article</button>"
raise "everything:\n#{fresh}" unless fresh.include?(everything)
blank = Article.new(title: "", body: "long enough body")
blank.valid?
boxed = "<p class=\"row\"><div class=\"field_with_errors\"><label for=\"article_title\">Title</label></div><div class=\"field_with_errors\"><input type=\"text\" value=\"\" name=\"article[title]\" id=\"article_title\" /></div></p>"
raise "boxed:\n#{rendered(blank)}" unless rendered(blank).include?(boxed)
puts "custom builder passed"
"##;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().write("app/helpers/probe_form_builder.rb", BUILDER).write("app/views/articles/_form.html.erb", FORM)
}

#[test]
fn a_custom_builders_own_methods_render_as_rails_on_ruby() {
    let run = app().run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("custom builder passed"), "{}", run.stdout);
}

/// No other tree carries Rails' FormBuilder at request time: the builder's own method is reported, not left to
/// raise at render.
#[test]
fn a_custom_builders_own_method_is_reported_off_the_ruby_family() {
    let (_, errors) = app().emit(roundhouse::build_target::BuildTarget::Spinel);
    assert!(errors.iter().any(|e| e.contains("custom form builder method") && e.contains("form.everything")), "{errors:#?}");
}
