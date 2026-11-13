//! A serializer that reads `scope`, or `current_user` (the name the
//! default `serialization_scope :current_user` gives it), renders with
//! the scope its controller passes, as active_model_serializers 0.10.16
//! does on Rails 8.1.4: the controller's `current_user` under the default
//! scope, nil without one, and nil under `serialization_scope nil`.

use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write(
            "app/serializers/viewer_serializer.rb",
            r#"class ViewerSerializer < ActiveModel::Serializer
  attributes :id, :title
  attribute :viewer, if: :current_user?
  attribute(:scoped) { scope.nil? ? "none" : scope }

  def viewer
    current_user.upcase
  end

  def current_user?
    !current_user.nil?
  end
end
"#,
        )
        .write(
            "app/serializers/scope_only_serializer.rb",
            "class ScopeOnlySerializer < ActiveModel::Serializer\n  attributes :id\n  attribute(:scoped) { scope.nil? ? \"none\" : scope }\nend\n",
        )
        .write(
            "app/controllers/viewer_json_controller.rb",
            r#"class ViewerJsonController < ApplicationController
  def show
    render json: Article.find(params[:id]), serializer: ViewerSerializer
  end

  def index
    render json: Article.order(:id), each_serializer: ViewerSerializer
  end

  def scope_only
    render json: Article.find(params[:id]), serializer: ScopeOnlySerializer
  end

  private

  def current_user
    params[:who]
  end
end
"#,
        )
        .write(
            "app/controllers/anonymous_json_controller.rb",
            r#"class AnonymousJsonController < ApplicationController
  def show
    render json: Article.find(params[:id]), serializer: ScopeOnlySerializer
  end

  def viewer
    render json: Article.find(params[:id]), serializer: ViewerSerializer
  end
end
"#,
        )
        .write(
            "app/controllers/quiet_json_controller.rb",
            r#"class QuietJsonController < ApplicationController
  serialization_scope nil

  def show
    render json: Article.find(params[:id]), serializer: ScopeOnlySerializer
  end

  private

  def current_user
    "ignored"
  end
end
"#,
        )
        .edit(
            "config/routes.rb",
            "  resources :articles do",
            "  get \"viewer\" => \"viewer_json#index\"\n  get \"viewer/:id/scope_only\" => \"viewer_json#scope_only\"\n  get \"viewer/:id\" => \"viewer_json#show\"\n  get \"anonymous/:id/viewer\" => \"anonymous_json#viewer\"\n  get \"anonymous/:id\" => \"anonymous_json#show\"\n  get \"quiet/:id\" => \"quiet_json#show\"\n  resources :articles do",
        )
}

/// The bodies are active_model_serializers 0.10.16's for the same
/// controllers and records.
const SCRIPT: &str = r##"
%w[viewer_json anonymous_json quiet_json].each { |c| require_relative "app/controllers/#{c}_controller" }
a = Article.create!(title: "Hello", body: "A long body")
b = Article.create!(title: "World", body: "Another body")
call = ->(controller, action, params) { c = controller.new; c.params = params; c.process_action(action); c.body }
check = ->(label, got, want) { raise "#{label}: #{got.inspect}, want #{want.inspect}" unless got == want }
ida, idb = a.id.to_s, b.id.to_s
check.("default scope, signed in", call.(ViewerJsonController, :show, { "id" => ida, "who" => "ann" }),
  '{"id":' + ida + ',"title":"Hello","viewer":"ANN","scoped":"ann"}')
check.("default scope, signed out", call.(ViewerJsonController, :show, { "id" => ida }),
  '{"id":' + ida + ',"title":"Hello","scoped":"none"}')
check.("each_serializer", call.(ViewerJsonController, :index, { "who" => "ann" }),
  '[{"id":' + ida + ',"title":"Hello","viewer":"ANN","scoped":"ann"},{"id":' + idb + ',"title":"World","viewer":"ANN","scoped":"ann"}]')
check.("scope only, signed in", call.(ViewerJsonController, :scope_only, { "id" => ida, "who" => "ann" }),
  '{"id":' + ida + ',"scoped":"ann"}')
check.("no current_user, scope only", call.(AnonymousJsonController, :show, { "id" => ida }),
  '{"id":' + ida + ',"scoped":"none"}')
check.("no current_user, viewer", call.(AnonymousJsonController, :viewer, { "id" => ida }),
  '{"id":' + ida + ',"title":"Hello","scoped":"none"}')
check.("serialization_scope nil", call.(QuietJsonController, :show, { "id" => ida }),
  '{"id":' + ida + ',"scoped":"none"}')
puts "serializer scope OK"
"##;

#[test]
fn a_serializer_reads_the_scope_its_controller_passes() {
    let run = app().run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("serializer scope OK"), "{}", run.stdout);
}

#[test]
fn a_serializer_reading_its_scope_emits_for_spinel() {
    let (_emitted, errors) = app().emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
}

/// How active_model_serializers 0.10.16 reads the scope (Rails 8.1.4):
/// once per render, after the rendered value, also for an
/// `each_serializer` collection; from the controller's own
/// `serialization_scope` when it defines one; and from a `current_user`
/// an included concern defines.
fn read_app() -> emit_and_run::Overlay {
    app()
        .write(
            "app/controllers/counting_json_controller.rb",
            r#"class CountingJsonController < ApplicationController
  def index
    render json: items, each_serializer: ScopeOnlySerializer
  end

  def show
    render json: item, serializer: ScopeOnlySerializer
  end

  private

  def items
    (@reads ||= []) << "items"
    Article.order(:id)
  end

  def item
    (@reads ||= []) << "item"
    Article.order(:id).first
  end

  def current_user
    (@reads ||= []) << "current_user"
    "ann"
  end
end
"#,
        )
        .write(
            "app/controllers/override_json_controller.rb",
            r#"class OverrideJsonController < ApplicationController
  def show
    render json: Article.find(params[:id]), serializer: ScopeOnlySerializer
  end

  def viewer
    render json: Article.find(params[:id]), serializer: ViewerSerializer
  end

  private

  def serialization_scope
    "override"
  end

  def current_user
    "ignored"
  end
end
"#,
        )
        .write(
            "app/controllers/concerns/current_user_concern.rb",
            "module CurrentUserConcern\n  extend ActiveSupport::Concern\n\n  private\n\n  def current_user\n    \"concern\"\n  end\nend\n",
        )
        .write(
            "app/controllers/concern_json_controller.rb",
            r#"class ConcernJsonController < ApplicationController
  include CurrentUserConcern

  def show
    render json: Article.find(params[:id]), serializer: ScopeOnlySerializer
  end

  def viewer
    render json: Article.find(params[:id]), serializer: ViewerSerializer
  end
end
"#,
        )
        .edit(
            "config/routes.rb",
            "  resources :articles do",
            "  get \"counting\" => \"counting_json#index\"\n  get \"counting/one\" => \"counting_json#show\"\n  get \"override/:id/viewer\" => \"override_json#viewer\"\n  get \"override/:id\" => \"override_json#show\"\n  get \"concern/:id/viewer\" => \"concern_json#viewer\"\n  get \"concern/:id\" => \"concern_json#show\"\n  resources :articles do",
        )
}

/// The bodies and reads are active_model_serializers 0.10.16's.
const READ_SCRIPT: &str = r##"
%w[counting_json override_json concern_json].each { |c| require_relative "app/controllers/#{c}_controller" }
a = Article.create!(title: "Hello", body: "A long body")
b = Article.create!(title: "World", body: "Another body")
run = ->(controller, action, params) { c = controller.new; c.params = params; c.process_action(action); c }
failures = []
check = ->(label, got, want) { failures << "#{label}: #{got.inspect}, want #{want.inspect}" unless got == want }
ida, idb = a.id.to_s, b.id.to_s
c = run.(CountingJsonController, :index, {})
check.("each_serializer body", c.body, '[{"id":' + ida + ',"scoped":"ann"},{"id":' + idb + ',"scoped":"ann"}]')
check.("each_serializer reads", c.instance_variable_get(:@reads), ["items", "current_user"])
c = run.(CountingJsonController, :show, {})
check.("serializer body", c.body, '{"id":' + ida + ',"scoped":"ann"}')
check.("serializer reads", c.instance_variable_get(:@reads), ["item", "current_user"])
check.("override, scope only", run.(OverrideJsonController, :show, { "id" => ida }).body, '{"id":' + ida + ',"scoped":"override"}')
check.("override, viewer", run.(OverrideJsonController, :viewer, { "id" => ida }).body,
  '{"id":' + ida + ',"title":"Hello","viewer":"OVERRIDE","scoped":"override"}')
check.("concern, scope only", run.(ConcernJsonController, :show, { "id" => ida }).body, '{"id":' + ida + ',"scoped":"concern"}')
check.("concern, viewer", run.(ConcernJsonController, :viewer, { "id" => ida }).body,
  '{"id":' + ida + ',"title":"Hello","viewer":"CONCERN","scoped":"concern"}')
raise failures.join("\n") unless failures.empty?
puts "scope reads OK"
"##;

#[test]
fn a_render_reads_the_scope_as_the_gem_does() {
    let run = read_app().run_ruby(READ_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("scope reads OK"), "stdout:\n{}\nstderr:\n{}", run.stdout, run.stderr);
}

/// A render's other options run before the scope is read: with a
/// `status:` that changes `current_user`, active_model_serializers
/// 0.10.16 (Rails 8.1.4) reads `item, status, current_user` for `json:,
/// serializer:, status:`, `status, item, current_user` with `status:`
/// first, and `items, status, current_user` for a collection, and the
/// scope is the changed user's.
fn switching_app() -> emit_and_run::Overlay {
    app()
        .write(
            "app/controllers/switching_json_controller.rb",
            r#"class SwitchingJsonController < ApplicationController
  def show
    render json: item, serializer: ScopeOnlySerializer, status: switch_user
  end

  def first
    render status: switch_user, json: item, serializer: ScopeOnlySerializer
  end

  def index
    render json: items, each_serializer: ScopeOnlySerializer, status: switch_user
  end

  private

  def item
    (@reads ||= []) << "item"
    Article.order(:id).first
  end

  def items
    (@reads ||= []) << "items"
    Article.order(:id)
  end

  def switch_user
    (@reads ||= []) << "status"
    @who = "bob"
    :ok
  end

  def current_user
    (@reads ||= []) << "current_user"
    @who || "ann"
  end
end
"#,
        )
        .edit(
            "config/routes.rb",
            "  resources :articles do",
            "  get \"switching\" => \"switching_json#show\"\n  get \"switching/first\" => \"switching_json#first\"\n  get \"switching/all\" => \"switching_json#index\"\n  resources :articles do",
        )
}

const SWITCHING_SCRIPT: &str = r##"
require_relative "app/controllers/switching_json_controller"
a = Article.create!(title: "Hello", body: "A long body")
b = Article.create!(title: "World", body: "Another body")
run = ->(action) { c = SwitchingJsonController.new; c.params = {}; c.process_action(action); c }
failures = []
check = ->(label, got, want) { failures << "#{label}: #{got.inspect}, want #{want.inspect}" unless got == want }
ida, idb = a.id.to_s, b.id.to_s
c = run.(:show)
check.("show body", c.body, '{"id":' + ida + ',"scoped":"bob"}')
check.("show reads", c.instance_variable_get(:@reads), ["item", "status", "current_user"])
c = run.(:first)
check.("status first body", c.body, '{"id":' + ida + ',"scoped":"bob"}')
check.("status first reads", c.instance_variable_get(:@reads), ["status", "item", "current_user"])
c = run.(:index)
check.("collection body", c.body, '[{"id":' + ida + ',"scoped":"bob"},{"id":' + idb + ',"scoped":"bob"}]')
check.("collection reads", c.instance_variable_get(:@reads), ["items", "status", "current_user"])
raise failures.join("\n") unless failures.empty?
puts "option order OK"
"##;

#[test]
fn a_renders_other_options_run_before_the_scope_is_read() {
    let run = switching_app().run_ruby(SWITCHING_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("option order OK"), "stdout:\n{}\nstderr:\n{}", run.stdout, run.stderr);
}

/// The bindings the option order needs are the rewrite's own: a local
/// the action already has, whatever its name, keeps its value. Rails
/// renders this with status 202 and the controller's `current_user` as
/// the scope; the action's `ams_json` and `ams_scope` are its own
/// locals, which the generated bindings must not take over.
fn locals_app() -> emit_and_run::Overlay {
    app().write(
        "app/controllers/locals_json_controller.rb",
        r#"class LocalsJsonController < ApplicationController
  def show
    ams_json = :accepted
    ams_scope = "local"
    render json: item, serializer: ScopeOnlySerializer, status: ams_json
    @after = [ams_json, ams_scope]
  end

  private

  def item
    Article.order(:id).first
  end

  def current_user
    "ann"
  end
end
"#,
    )
    .edit("config/routes.rb", "  resources :articles do", "  get \"locals\" => \"locals_json#show\"\n  resources :articles do")
}

#[test]
fn a_renders_bindings_leave_the_actions_own_locals_alone() {
    let run = locals_app().run_ruby(
        r##"
require_relative "app/controllers/locals_json_controller"
a = Article.create!(title: "Hello", body: "A long body")
c = LocalsJsonController.new
c.params = {}
c.process_action(:show)
failures = []
failures << "status #{c.status.inspect}, want 202" unless c.status == 202
want = '{"id":' + a.id.to_s + ',"scoped":"ann"}'
failures << "body #{c.body.inspect}, want #{want.inspect}" unless c.body == want
after = c.instance_variable_get(:@after)
failures << "locals after #{after.inspect}, want [:accepted, \"local\"]" unless after == [:accepted, "local"]
raise failures.join("\n") unless failures.empty?
puts "locals OK"
"##,
    );
    run.assert_passes();
    assert!(run.stdout.contains("locals OK"), "stdout:\n{}\nstderr:\n{}", run.stdout, run.stderr);
}

/// The scope a serializer holds, and the controller declarations that
/// set it, as active_model_serializers 0.10.16 answers them on Rails
/// 8.1.4: the gem keeps the scope in its own accessor from construction,
/// so a later write to `instance_options[:scope]` leaves it
/// (`{"scoped":"ann"}`), and `self.scope = "other"` changes it, for
/// `current_user` too (`{"scoped":"other"}`, `{"viewer":"other"}`).
/// `self.serialization_scope nil` is `serialization_scope nil`
/// (`{"scoped":"none"}`), and `attr_reader :current_user` is the
/// controller's `current_user` (`{"scoped":"cat"}`).
fn holds_app() -> emit_and_run::Overlay {
    app()
        .write(
            "app/serializers/options_write_serializer.rb",
            "class OptionsWriteSerializer < ActiveModel::Serializer\n  attributes :id\n  attribute(:scoped) { instance_options[:scope] = \"other\"; scope }\nend\n",
        )
        .write(
            "app/serializers/scope_write_serializer.rb",
            "class ScopeWriteSerializer < ActiveModel::Serializer\n  attributes :id\n  attribute(:scoped) { self.scope = \"other\"; scope }\nend\n",
        )
        .write(
            "app/serializers/scope_write_viewer_serializer.rb",
            "class ScopeWriteViewerSerializer < ActiveModel::Serializer\n  attributes :id\n  attribute(:viewer) { self.scope = \"other\"; current_user }\nend\n",
        )
        .write(
            "app/controllers/writes_json_controller.rb",
            r#"class WritesJsonController < ApplicationController
  def options_write
    render json: Article.find(params[:id]), serializer: OptionsWriteSerializer
  end

  def scope_write
    render json: Article.find(params[:id]), serializer: ScopeWriteSerializer
  end

  def scope_write_viewer
    render json: Article.find(params[:id]), serializer: ScopeWriteViewerSerializer
  end

  private

  def current_user
    "ann"
  end
end
"#,
        )
        .write(
            "app/controllers/self_quiet_json_controller.rb",
            r#"class SelfQuietJsonController < ApplicationController
  self.serialization_scope nil

  def show
    render json: Article.find(params[:id]), serializer: ScopeOnlySerializer
  end

  private

  def current_user
    "ann"
  end
end
"#,
        )
        .write(
            "app/controllers/attr_user_json_controller.rb",
            r#"class AttrUserJsonController < ApplicationController
  attr_reader :current_user
  before_action :set_user

  def show
    render json: Article.find(params[:id]), serializer: ScopeOnlySerializer
  end

  private

  def set_user
    @current_user = "cat"
  end
end
"#,
        )
        .edit(
            "config/routes.rb",
            "  resources :articles do",
            "  get \"writes/:id/options_write\" => \"writes_json#options_write\"\n  get \"writes/:id/scope_write\" => \"writes_json#scope_write\"\n  get \"writes/:id/scope_write_viewer\" => \"writes_json#scope_write_viewer\"\n  get \"self_quiet/:id\" => \"self_quiet_json#show\"\n  get \"attr_user/:id\" => \"attr_user_json#show\"\n  resources :articles do",
        )
}

const HOLDS_SCRIPT: &str = r##"
%w[writes_json self_quiet_json attr_user_json].each { |c| require_relative "app/controllers/#{c}_controller" }
a = Article.create!(title: "Hello", body: "A long body")
run = ->(controller, action, params) { c = controller.new; c.params = params; c.process_action(action); c.body }
failures = []
check = ->(label, got, want) { failures << "#{label}: #{got.inspect}, want #{want.inspect}" unless got == want }
id = a.id.to_s
check.("instance_options[:scope] write", (run.(WritesJsonController, :options_write, { "id" => id }) rescue $!.class.name), '{"id":' + id + ',"scoped":"ann"}')
check.("self.scope write, scope", (run.(WritesJsonController, :scope_write, { "id" => id }) rescue $!.class.name), '{"id":' + id + ',"scoped":"other"}')
check.("self.scope write, current_user", (run.(WritesJsonController, :scope_write_viewer, { "id" => id }) rescue $!.class.name), '{"id":' + id + ',"viewer":"other"}')
check.("self.serialization_scope nil", (run.(SelfQuietJsonController, :show, { "id" => id }) rescue $!.class.name), '{"id":' + id + ',"scoped":"none"}')
check.("attr_reader :current_user", (run.(AttrUserJsonController, :show, { "id" => id }) rescue $!.class.name), '{"id":' + id + ',"scoped":"cat"}')
raise failures.join("\n") unless failures.empty?
puts "scope holds OK"
"##;

#[test]
fn a_serializer_holds_its_scope_as_the_gem_does() {
    let run = holds_app().run_ruby(HOLDS_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("scope holds OK"), "stdout:\n{}\nstderr:\n{}", run.stdout, run.stderr);
}
