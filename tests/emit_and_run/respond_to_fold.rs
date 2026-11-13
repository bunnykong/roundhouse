use super::emit_and_run;

/// `x.respond_to?(:m)` on a class nothing in whose chain answers `m` is false, so Ruby never runs what it guards:
/// the right of `&&`, the branch of an `if`, and the branch of an `if` over a predicate of the class's own whose body
/// is that condition (Devise's `unlockable?(resource)`). Those calls are not typed, and the action answers as CRuby. A local
/// only the untaken branch assigns is still defined, nil, as the parser met its assignment.
const PROBES: &str = r##"class ProbesController < ApplicationController
  def show
    article = Article.new(title: "t")
    first = answers?(article) ? article.nonexistent_method : "missing"
    late = article.nonexistent_method if article.respond_to?(:nonexistent_method)
    render plain: "#{first} skipped #{answers?(article).inspect} #{late.inspect}"
  end

  private

  def answers?(record)
    record.respond_to?(:nonexistent_method) && record.nonexistent_method
  end
end
"##;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/controllers/probes_controller.rb", PROBES)
        .edit("config/routes.rb", "  root \"articles#index\"\n", "  root \"articles#index\"\n  get \"/probe\", to: \"probes#show\"\n")
}

#[test]
fn a_guard_no_class_answers_folds_and_runs_as_ruby_does() {
    let run = app().run_ruby(
        r##"status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/probe", "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
raise "probe #{status} #{body.join}" unless status == 200 && body.join == "missing skipped false nil"
puts "respond_to fold passed"
"##,
    );
    run.assert_passes();
    assert!(run.stdout.contains("respond_to fold passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_guard_no_class_answers_folds_and_runs_natively() {
    let tree = app().native_http_tree();
    let server = super::native_http::Server::start(&tree);
    let answer = server.get("/probe");
    assert_eq!((answer.status, answer.body.as_str()), (200, "missing skipped false nil"), "{}", server.log());
}
