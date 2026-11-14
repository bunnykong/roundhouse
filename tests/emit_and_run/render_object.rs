//! A view's `render` of an object. Rails asks an object that answers `render_in` to render itself, and renders a
//! partial otherwise: one typed as answering it is rendered through `render_in(view_context)`; any other object is
//! reported, not emitted as a bare `render(…)` the view's module does not answer.

use roundhouse::build_target::BuildTarget;

use super::emit_and_run;

const BADGE: &str = "class Badge\n  def initialize(label)\n    @label = label\n  end\n\n  def render_in(_view_context)\n    \"<b>#{@label}</b>\".html_safe\n  end\nend\n";

const PLAIN: &str = "class Plain\n  def initialize(label)\n    @label = label\n  end\nend\n";

fn app(object: &str) -> emit_and_run::Overlay {
    emit_and_run::real_blog().write("app/models/badge.rb", BADGE).write("app/models/plain.rb", PLAIN).edit(
        "app/views/articles/index.html.erb",
        "<% content_for :title, \"Articles\" %>\n",
        &format!("<% content_for :title, \"Articles\" %>\n<%= render({object}.new(\"new\")) %>\n"),
    )
}

#[test]
fn an_object_that_renders_itself_is_rendered_on_ruby() {
    let run = app("Badge").run_ruby("io = +\"\"\nViews::Articles.index_into(io, Article.all, nil, nil)\nraise io unless io.include?(\"<b>new</b>\")\nputs \"rendered\"\n");
    run.assert_passes();
    assert!(run.stdout.contains("rendered"), "{}", run.stdout);
}

#[test]
fn an_object_that_does_not_render_itself_is_reported_on_every_target() {
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let (_, errors) = app("Plain").emit(target);
        assert!(errors.iter().any(|e| e.contains("render of an object not supported")), "{target:?}: {errors:#?}");
    }
}
