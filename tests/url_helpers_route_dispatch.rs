//! `Rails.application.routes.url_helpers` answers every named route helper
//! of the drawn route set, as Rails' does: `<name>_path` is the emitted
//! RouteHelpers' answer, `<name>_url` the same path under
//! `protocol://host[:port]`, on the module itself and through a class that
//! includes it. The module was a stub with `root_url` alone, so Core's
//! `UrlHelpers.root_path` (`graphql_preloadable_assertions.rb`) was an
//! undefined method. A name no drawn route explains stays a NoMethodError.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const SCRIPT: &str = r##"helpers = Rails.application.routes.url_helpers
puts helpers.articles_path
puts helpers.article_path(3)
puts helpers.root_path
puts helpers.articles_url(host: "blog.test", protocol: "https")
puts helpers.article_url(3, host: "blog.test", port: 3000)
puts helpers.article_url(3, only_path: true)
puts helpers.respond_to?(:articles_path)
puts helpers.respond_to?(:nope_path)
puts helpers.respond_to?(:articles_url)
begin
  helpers.articles_url
rescue ArgumentError => e
  puts "ArgumentError #{e.message[0, 25].strip}"
end
begin
  helpers.nope_path
rescue NoMethodError => e
  puts "NoMethodError #{e.name}"
end
includer = Class.new { include Rails.application.routes.url_helpers }
puts includer.new.article_path(5)
extender = Module.new { class << self; include Rails.application.routes.url_helpers; end }
puts extender.article_path(6)
"##;

#[test]
fn url_helpers_answer_every_named_route_helper() {
    let run = emit_and_run::real_blog().run_ruby(SCRIPT);
    run.assert_passes();
    let expected = "/articles\n/articles/3\n/\nhttps://blog.test/articles\nhttp://blog.test:3000/articles/3\n/articles/3\ntrue\nfalse\ntrue\n\
ArgumentError Missing host to link to!\nNoMethodError nope_path\n/articles/5\n/articles/6\n";
    assert_eq!(run.stdout, expected);
}

