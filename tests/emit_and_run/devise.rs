//! Devise compiled from its installed source: `devise_for` draws what
//! `Devise::Mapping` and the model's modules choose, as in Rails.

use roundhouse::build_target::BuildTarget;

use super::emit_and_run;

const GEMS: [(&str, &str); 4] = [("devise", "4.9.4"), ("warden", "1.2.9"), ("orm_adapter", "0.5.0"), ("responders", "3.1.1")];

const SPECS: &str = "    bcrypt (3.1.20)
    devise (4.9.4)
      bcrypt (~> 3.0)
      orm_adapter (~> 0.1)
      railties (>= 4.1.0)
      responders
      warden (~> 1.2.3)
    orm_adapter (0.5.0)
    responders (3.1.1)
      actionpack (>= 5.2)
      railties (>= 5.2)
    warden (1.2.9)
      rack (>= 2.0.9)
";

const USERS: &str = r#"  create_table "users", force: :cascade do |t|
    t.string "email", default: "", null: false
    t.string "encrypted_password", default: "", null: false
    t.string "reset_password_token"
    t.datetime "reset_password_sent_at"
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end

"#;

fn installed(name: &str, version: &str) -> std::path::PathBuf {
    let out = std::process::Command::new("ruby")
        .args(["-e", &format!("print Gem::Specification.find_by_name('{name}', '{version}').gem_dir")])
        .output()
        .expect("spawn ruby");
    assert!(out.status.success(), "{name} {version} is not installed: {}", String::from_utf8_lossy(&out.stderr));
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

fn all_files(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            all_files(root, &path, out);
        } else {
            out.push(path.strip_prefix(root).unwrap().to_path_buf());
        }
    }
}

fn app(modules: &str, routes: &str) -> emit_and_run::Overlay {
    let fixture = roundhouse::fixtures::real_blog();
    let lock = std::fs::read_to_string(fixture.join("Gemfile.lock")).unwrap();
    // Not a bare "    rack (": a dependency line ("      rack (>= 2.2.4)") holds it too.
    let lock = lock.replacen("\n    rack (", &format!("\n{SPECS}    rack ("), 1).replacen("DEPENDENCIES\n", "DEPENDENCIES\n  devise\n", 1);
    let gemfile = std::fs::read_to_string(fixture.join("Gemfile")).unwrap() + "gem \"devise\"\n";
    let mut overlay = emit_and_run::real_blog()
        .write("Gemfile", &gemfile)
        .write("Gemfile.lock", &lock)
        .write(".bundle/config", "---\nBUNDLE_PATH: \"vendor/bundle\"\n")
        .edit("db/schema.rb", "  create_table \"articles\"", &format!("{USERS}  create_table \"articles\""))
        // Not without it: a Devise app's generator writes it, and it is what names `Devise` to the gems compiled from source.
        .write("config/initializers/devise.rb", "Devise.setup do |config|\n  config.mailer_sender = \"please-change-me@example.com\"\n  require \"devise/orm/active_record\"\nend\n")
        .write("app/models/user.rb", &format!("class User < ApplicationRecord\n  devise {modules}\nend\n"))
        .edit("config/routes.rb", "  root \"articles#index\"\n", &format!("  root \"articles#index\"\n{routes}"));
    for (name, version) in GEMS {
        let gem = installed(name, version);
        let mut files = Vec::new();
        rb_files(&gem, &gem.join("lib"), &mut files);
        // Not lib/ alone: an installed engine has its app/ too, devise's controllers and views.
        all_files(&gem, &gem.join("app"), &mut files);
        // And config/: an engine's locales are on Rails' load path (devise's `devise.sessions.signed_in`).
        all_files(&gem, &gem.join("config"), &mut files);
        for file in files {
            let text = std::fs::read_to_string(gem.join(&file)).unwrap();
            overlay = overlay.write(&format!("vendor/bundle/ruby/3.4.0/gems/{name}-{version}/{}", file.display()), &text);
        }
    }
    overlay
}

/// A route by its verb, path, controller and action, which a guard's arguments may follow: not a longer action's name.
fn has_route(routes: &str, route: &str) -> bool {
    routes.contains(&format!("{route})")) || routes.contains(&format!("{route},"))
}

fn drawn(overlay: emit_and_run::Overlay) -> (String, String) {
    let (tree, _) = overlay.emit(BuildTarget::Ruby);
    let routes = std::fs::read_to_string(tree.join("config/routes.rb")).expect("routes.rb");
    let helpers = std::fs::read_to_string(tree.join("app/route_helpers.rb")).expect("route_helpers.rb");
    (routes, helpers)
}

/// The routes are the modules' (`Devise::Mapping#used_routes`): no `:confirmable`, no confirmation routes.
#[test]
fn devise_for_draws_the_routes_of_the_models_modules() {
    let (routes, helpers) = drawn(app(":database_authenticatable, :registerable, :recoverable", "  devise_for :users\n"));
    for route in [
        r#"Route.new("GET", "/users/sign_in", :devise__sessions, :new"#,
        r#"Route.new("DELETE", "/users/sign_out", :devise__sessions, :destroy"#,
        r#"Route.new("GET", "/users/cancel", :devise__registrations, :cancel"#,
        r#"Route.new("PUT", "/users/password", :devise__passwords, :update"#,
    ] {
        assert!(has_route(&routes, route), "{route}\n{routes}");
    }
    assert!(!routes.contains("confirmation"), "{routes}");
    for helper in ["new_user_session_path", "destroy_user_session_path", "cancel_user_registration_path", "edit_user_password_path"] {
        assert!(helpers.contains(&format!("def self.{helper}")), "{helper}\n{helpers}");
    }

    let (routes, _) = drawn(app(":database_authenticatable", "  devise_for :users\n"));
    assert!(has_route(&routes, r#"Route.new("GET", "/users/sign_in", :devise__sessions, :new"#), "{routes}");
    assert!(!routes.contains("registrations") && !routes.contains("passwords"), "{routes}");

    let (routes, _) = drawn(app(":database_authenticatable, :registerable, :recoverable", "  devise_for :users, skip: [:registrations]\n"));
    assert!(has_route(&routes, r#"Route.new("GET", "/users/password/new", :devise__passwords, :new"#), "{routes}");
    assert!(!routes.contains("registrations"), "{routes}");
}

/// Under `scope module:` Devise takes the scope's module in place of its own `devise` default,
/// and a `controllers:` override is the controller as written (`with_devise_exclusive_scope`).
#[test]
fn devise_for_options_and_an_enclosing_module_are_devises() {
    let (routes, _) = drawn(app(
        ":database_authenticatable, :registerable",
        "  scope module: :auth do\n    devise_for :users, path: \"account\", path_names: { sign_in: \"login\" }, controllers: { registrations: \"users/registrations\" }\n  end\n",
    ));
    for route in [
        r#"Route.new("GET", "/account/login", :auth__sessions, :new"#,
        r#"Route.new("DELETE", "/account/sign_out", :auth__sessions, :destroy"#,
        r#"Route.new("GET", "/account/sign_up", :users__registrations, :new"#,
    ] {
        assert!(has_route(&routes, route), "{route}\n{routes}");
    }
    assert!(!routes.contains("auth/devise/") && !routes.contains("auth__users__"), "{routes}");
}

/// A subclass with no table of its own (`SuperAdmin < User`) has its superclass's modules, as `devise_modules` is a `class_attribute`.
#[test]
fn devise_for_a_subclass_draws_its_superclasss_modules() {
    let (routes, helpers) = drawn(
        app(":database_authenticatable, :recoverable", "  devise_for :admins, path: \"admin\"\n").write("app/models/admin.rb", "class Admin < User\nend\n"),
    );
    assert!(has_route(&routes, r#"Route.new("GET", "/admin/sign_in", :devise__sessions, :new"#), "{routes}");
    assert!(has_route(&routes, r#"Route.new("GET", "/admin/password/new", :devise__passwords, :new"#), "{routes}");
    assert!(helpers.contains("def self.new_admin_session_path"), "{helpers}");
}

/// Devise's engine stacks Warden's `Warden::Manager` (`config.app_middleware.use Warden::Manager do
/// |config| … end`), so a request carries `env["warden"]` as in Rails; the block configures it.
#[test]
fn devise_puts_warden_on_the_middleware_stack_with_its_configuring_block() {
    let (tree, errors) = app(":database_authenticatable", "  devise_for :users\n").emit(BuildTarget::Ruby);
    let stack = std::fs::read_to_string(tree.join("runtime/gem_middleware.rb")).expect("gem_middleware.rb");
    assert!(stack.contains("Warden::Manager.new(app) { |config|\nDevise.warden_config = config\n}"), "{stack}\n{errors:#?}");
    // Not refused: with no `config.omniauth`, the engine's loop over `Devise.omniauth_configs` inserts nothing.
    assert!(!errors.iter().any(|e| e.contains("railtie middleware")), "{errors:#?}");
}

/// Devise's `require 'responders'` compiles the responders gem too, though its `ActionController::Responder`
/// names no namespace of its own: Devise's Responder then settles `respond_to?(:error_status=)` on it.
#[test]
fn devise_compiles_the_responders_it_requires() {
    let (tree, errors) = app(":database_authenticatable", "  devise_for :users\n").emit(BuildTarget::Spinel);
    let responder = std::fs::read_to_string(tree.join("app/models/action_controller/responder.rb")).expect("responder.rb");
    assert!(responder.contains("class Responder"), "{responder}");
    assert!(!errors.iter().any(|e| e.contains("devise/controllers/responder.rb")), "{errors:#?}");
}

/// A provider the app configures is a middleware the engine's loop inserts, which the stack does not hold.
#[test]
fn an_omniauth_provider_keeps_the_engines_middleware_loop_refused() {
    let (_, errors) = app(":database_authenticatable", "  devise_for :users\n")
        .write(
            "config/initializers/devise.rb",
            "Devise.setup do |config|\n  config.mailer_sender = \"please-change-me@example.com\"\n  require \"devise/orm/active_record\"\n  config.omniauth :github, \"id\", \"secret\"\nend\n",
        )
        .emit(BuildTarget::Ruby);
    assert!(errors.iter().any(|e| e.contains("railtie middleware") && e.contains("inserted under a condition")), "{errors:#?}");
}

/// `authenticated :user` with no block: Devise's `block.nil? || block.call(…)` is `true` once the
/// block is written in, so the constraint asks only the Warden proxy Devise's engine stacks.
#[test]
fn devise_authenticated_asks_the_warden_proxy_its_engine_stacks() {
    let (tree, errors) = app(":database_authenticatable", "  devise_for :users\n  authenticated :user do\n    get \"/dash\", to: \"articles#index\"\n  end\n")
        .emit(BuildTarget::Ruby);
    assert!(!errors.iter().any(|e| e.contains("request constraint")), "{errors:#?}");
    let guards = std::fs::read_to_string(tree.join("app/models/roundhouse_route_guards.rb")).expect("guards");
    assert!(guards.contains("request.env[\"warden\"].send(:authenticate?, scope: :user) && true"), "{guards}");
}

/// A gem's `Devise.token` (devise-jwt's `Devise.jwt` is written this way) yields a settings object of the gem's own, not `Devise`:
/// that `Devise.setup` cannot run it leaves none of Devise's settings unsure, and the routes that read them are still drawn.
#[test]
fn devise_for_is_drawn_past_a_setup_block_configuring_another_object() {
    let gem = "module Devise\n  def self.token\n    yield(Devise::Token.config)\n  end\n\n  module Token\n  end\nend\n";
    let setup = "Devise.setup do |config|\n  config.mailer_sender = \"please-change-me@example.com\"\n  require \"devise/orm/active_record\"\n  config.token do |token|\n    token.secret = \"x\"\n  end\nend\n";
    let (routes, helpers) = drawn(
        app(":database_authenticatable", "  devise_for :users\n")
            .write("lib/devise_token.rb", gem)
            .write("config/initializers/devise.rb", setup),
    );
    assert!(has_route(&routes, r#"Route.new("GET", "/users/sign_in", :devise__sessions, :new"#), "{routes}");
    assert!(helpers.contains("def self.new_user_session_path"), "{helpers}");
}

/// A gem can register its module with a bare `add_module` inside `module Devise` (devise-jwt does): the module is one Devise knows, as `Devise.add_module` makes it.
#[test]
fn devise_for_knows_a_module_registered_inside_module_devise() {
    let (routes, _) = drawn(
        app(":database_authenticatable, :token_authenticatable", "  devise_for :users\n")
            .edit("Gemfile.lock", "    devise (4.9.4)\n", "    devise-token (0.1.0)\n      devise (~> 4.0)\n    devise (4.9.4)\n")
            .edit("Gemfile.lock", "DEPENDENCIES\n", "DEPENDENCIES\n  devise-token\n")
            .edit("Gemfile", "gem \"devise\"\n", "gem \"devise\"\ngem \"devise-token\"\n")
            .write("vendor/bundle/ruby/3.4.0/gems/devise-token-0.1.0/lib/devise-token.rb", "module Devise\n  add_module(:token_authenticatable, strategy: :token)\nend\n"),
    );
    assert!(has_route(&routes, r#"Route.new("GET", "/users/sign_in", :devise__sessions, :new"#), "{routes}");
}

/// A `devise` option sets that option on the model and no other: `stretches:` leaves the unlock strategy Devise's, so its unlock routes are drawn.
/// The option it does read, set on the model, is the model's own: no route is guessed from Devise's default.
#[test]
fn devise_for_reads_past_a_model_option_it_does_not_ask_about() {
    let (routes, _) = drawn(app(":database_authenticatable, :lockable, stretches: 1", "  devise_for :users\n"));
    assert!(has_route(&routes, r#"Route.new("GET", "/users/unlock/new", :devise__unlocks, :new"#), "{routes}");

    let (routes, _) = drawn(app(":database_authenticatable, :lockable, unlock_strategy: :time", "  devise_for :users\n"));
    assert!(!routes.contains("/users/unlock"), "{routes}");
}

/// `new_session_path(resource_or_scope, *args)`, which Devise's `generate_helpers!` defines by a computed name, calls the scope's own helper.
/// The expected answers are Rails' (Devise 4.9.4 on Rails 8.1), with `:admin` a mapping that uses no password routes and has its session routes skipped.
#[test]
fn devise_generated_url_helpers_answer_for_a_scope_and_a_resource_as_in_rails() {
    let admins = "  create_table \"admins\", force: :cascade do |t|\n    t.string \"email\", default: \"\", null: false\n    t.string \"encrypted_password\", default: \"\", null: false\n  end\n\n";
    let (tree, _) = app(":database_authenticatable, :recoverable", "  devise_for :users\n  devise_for :admins, skip: [:sessions]\n")
        .edit("db/schema.rb", "  create_table \"articles\"", &format!("{admins}  create_table \"articles\""))
        .write("app/models/admin.rb", "class Admin < ApplicationRecord\n  devise :database_authenticatable\nend\n")
        // A query passed to the scope's own helper gives it a keyword parameter, which is no path segment.
        .write("app/controllers/gadgets_controller.rb", "class GadgetsController < ApplicationController\n  def show\n    redirect_to new_user_session_path(foo: 1)\n  end\nend\n")
        .edit("config/routes.rb", "  root \"articles#index\"\n", "  root \"articles#index\"\n  get \"gadget\", to: \"gadgets#show\"\n")
        .emit(BuildTarget::Spinel);
    // The Spinel tree is the one holding the gem compiled from source; `RouteHelpers` and its query rendering load on their own.
    let script = r##"
module ActiveSupport; end
class User; end
class Manager < User; end
class Admin; end
require "./runtime/action_view/view_helpers"
require "./runtime/hash_to_query"
require "./app/route_helpers"
def expect(label, want)
  got = begin
    yield
  rescue Exception => e
    e.class
  end
  raise "#{label}: #{got.inspect}, want #{want.inspect}" unless got == want
end
R = RouteHelpers
expect("symbol", "/users/sign_in") { R.new_session_path(:user) }
expect("string", "/users/sign_in") { R.new_session_path("user") }
expect("instance", "/users/sign_in") { R.new_session_path(User.new) }
expect("class", "/users/sign_in") { R.new_session_path(User) }
expect("subclass instance", "/users/sign_in") { R.new_session_path(Manager.new) }
expect("destroy", "/users/sign_out") { R.destroy_session_path(:user) }
expect("no action", "/users/sign_in") { R.session_path(:user) }
expect("query", "/users/password/edit?reset_password_token=x+y") { R.edit_password_path(:user, reset_password_token: "x y") }
expect("instance and query", "/users/password/edit?extra%5B%5D=1&extra%5B%5D=2&reset_password_token=x") { R.edit_password_path(User.new, reset_password_token: "x", extra: [1, 2]) }
# Rails answers these two, which the helper does not write: refused rather than left out of the URL.
expect("format", ArgumentError) { R.new_session_path(:user, format: :json) }
expect("positional format", ArgumentError) { R.new_session_path(:user, "json") }
# A helper this mapping does not use, one whose routes are skipped, one no mapping uses, an unknown scope and an object that maps to none: Rails' errors.
expect("helper the mapping does not use", NoMethodError) { R.new_password_path(:admin) }
expect("routes skipped", NoMethodError) { R.new_session_path(:admin) }
expect("no mapping uses it", NoMethodError) { R.new_confirmation_path(:user) }
expect("unknown scope", NoMethodError) { R.new_session_path(:ghost) }
expect("no mapping for the object", RuntimeError) { R.new_session_path(Object.new) }
expect("nil", RuntimeError) { R.new_session_path(nil) }
puts "devise url helpers passed"
"##;
    let out = std::process::Command::new("ruby").args(["-e", script]).current_dir(&tree).output().expect("spawn ruby");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("devise url helpers passed"), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
}

/// `Devise.mappings` is a Hash, so a scope drawn again keeps its first place, and `find_scope!` walks it in that order.
#[test]
fn devise_url_helpers_keep_a_redrawn_scopes_first_place() {
    let admins = "  create_table \"admins\", force: :cascade do |t|\n    t.string \"email\", default: \"\", null: false\n    t.string \"encrypted_password\", default: \"\", null: false\n  end\n\n";
    let (tree, _) = app(":database_authenticatable", "  devise_for :users, skip: :all\n  devise_for :admins\n  devise_for :users\n")
        .edit("db/schema.rb", "  create_table \"articles\"", &format!("{admins}  create_table \"articles\""))
        .write("app/models/admin.rb", "class Admin < ApplicationRecord\n  devise :database_authenticatable\nend\n")
        .emit(BuildTarget::Spinel);
    let script = r##"
module ActiveSupport; end
class User; end
class Admin < User; end
require "./runtime/action_view/view_helpers"
require "./runtime/hash_to_query"
require "./app/route_helpers"
got = RouteHelpers.new_session_path(Admin.new)
raise "got #{got}" unless got == "/users/sign_in"
puts "first place kept"
"##;
    let out = std::process::Command::new("ruby").args(["-e", script]).current_dir(&tree).output().expect("spawn ruby");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("first place kept"), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
}

/// The helper asks the record itself for its scope, so a caller's record reaches it whole: `@user.id` would map to no scope.
#[test]
fn devise_generated_url_helpers_take_a_record_as_written() {
    let (tree, _) = app(":database_authenticatable", "  devise_for :users\n")
        .write("app/controllers/gadgets_controller.rb", "class GadgetsController < ApplicationController\n  def show\n    @user = User.new\n    @path = new_session_path(@user)\n    @url = new_session_url(@user)\n  end\nend\n")
        .write("app/views/gadgets/show.html.erb", "<%= new_session_path(@user) %>\n<%= render \"row\", user: @user %>\n")
        .write("app/views/gadgets/_row.html.erb", "<%= link_to \"in\", new_session_path(user) %>\n")
        .edit("config/routes.rb", "  root \"articles#index\"\n", "  root \"articles#index\"\n  get \"gadget\", to: \"gadgets#show\"\n")
        .emit(BuildTarget::Spinel);
    let controller = std::fs::read_to_string(tree.join("app/controllers/gadgets_controller.rb")).expect("controller");
    assert!(controller.contains("RouteHelpers.new_session_path(@user)"), "{controller}");
    assert!(!controller.contains("@user.id"), "{controller}");
    let view = std::fs::read_to_string(tree.join("app/views/gadgets/show.rb")).expect("view");
    assert!(!view.contains(".id"), "{view}");
    let partial = std::fs::read_to_string(tree.join("app/views/gadgets/_row.rb")).expect("partial");
    assert!(partial.contains("RouteHelpers.new_session_path(user)"), "{partial}");
}

/// Without a `define_method` by a computed name left for Spinel to refuse: the generator's bodies are written out.
#[test]
fn devise_generate_helpers_is_emptied_once_its_helpers_are_written() {
    let (tree, errors) = app(":database_authenticatable", "  devise_for :users\n")
        .write("app/models/widget_links.rb", "class WidgetLinks\n  include Devise::Controllers::UrlHelpers\nend\n")
        .emit(BuildTarget::Spinel);
    let module = std::fs::read_to_string(tree.join("app/models/devise/controllers/url_helpers.rb")).expect("url_helpers.rb");
    assert!(!module.contains("define_method"), "{module}");
    assert!(!errors.iter().any(|e| e.contains("url helpers")), "{errors:#?}");
}

/// `devise_for … router_name:`: the helper sends to another router, which `RouteHelpers` does not hold. Refused, its generator left standing.
#[test]
fn devise_url_helpers_of_a_named_router_are_refused() {
    let (tree, errors) = app(":database_authenticatable", "  devise_for :users, router_name: :other\n")
        .write("app/models/widget_links.rb", "class WidgetLinks\n  include Devise::Controllers::UrlHelpers\nend\n")
        .emit(BuildTarget::Spinel);
    assert!(errors.iter().any(|e| e.contains("url helpers") && e.contains("router_name")), "{errors:#?}");
    let module = std::fs::read_to_string(tree.join("app/models/devise/controllers/url_helpers.rb")).expect("url_helpers.rb");
    assert!(module.contains("define_method"), "{module}");
    let helpers = std::fs::read_to_string(tree.join("app/route_helpers.rb")).expect("route_helpers.rb");
    assert!(!helpers.contains("def self.new_session_path"), "{helpers}");
}

/// `devise_for` runs `Devise.add_mapping` at boot, as Rails draws the routes then, and `devise_scope`'s
/// constraint writes the mapping into the env each routed request carries, where Devise's controllers read it.
#[test]
fn devise_for_maps_at_boot_and_its_scope_writes_the_mapping_a_request_carries() {
    let (tree, _) = app(":database_authenticatable", "  devise_for :users\n").emit(BuildTarget::Spinel);
    let boot = std::fs::read_to_string(tree.join("config/initializers.rb")).expect("initializers.rb");
    assert!(boot.contains("Devise.add_mapping(:users, "), "{boot}");
    let guards = std::fs::read_to_string(tree.join("app/models/roundhouse_route_guards.rb")).expect("guards");
    assert!(guards.contains("request.env[\"devise.mapping\"] = Devise.mappings[:user]"), "{guards}");
}

/// A Devise controller no route dispatches to (no model is `:recoverable`, so no password routes) runs no
/// action: it is left out of the Spinel tree, and is there once a model's modules route to it.
#[test]
fn a_devise_controller_no_route_reaches_is_left_out() {
    let (tree, _) = app(":database_authenticatable", "  devise_for :users\n").emit(BuildTarget::Spinel);
    assert!(tree.join("app/controllers/devise/sessions_controller.rb").exists());
    assert!(!tree.join("app/controllers/devise/passwords_controller.rb").exists());
    let (tree, _) = app(":database_authenticatable, :recoverable", "  devise_for :users\n").emit(BuildTarget::Spinel);
    assert!(tree.join("app/controllers/devise/passwords_controller.rb").exists());
}

/// What the app's mappings answer alike is a constant: no model here is `:validatable` or `:omniauthable`, so
/// `if devise_mapping.validatable?` and the shared links' `if devise_mapping.omniauthable?` take their `else`, and
/// what only those branches call (`resource_class.password_length`, `OmniAuth::Utils`) is not reported. The mailer
/// the gem's `app/` declares inside `if defined?(ActionMailer)` is in the tree (`Devise.mailer` names it by a string),
/// with the action `after_update :send_email_changed_notification` sends.
#[test]
fn devise_answers_its_mappings_modules_and_leaves_out_what_no_model_uses() {
    let (tree, errors) = app(":database_authenticatable", "  devise_for :users\n").emit(BuildTarget::Spinel);
    for absent in ["password_length", "OmniAuth::Utils", "confirmation_instructions", "unlock_instructions"] {
        assert!(!errors.iter().any(|e| e.contains(absent)), "{absent}: {errors:#?}");
    }
    let mut files = Vec::new();
    all_files(&tree, &tree, &mut files);
    let has = |needle: &str| files.iter().any(|f| f.to_string_lossy().contains(needle));
    assert!(has("devise/mailer"), "the gem's mailer: {files:#?}");
    assert!(has("email_changed"), "an action DatabaseAuthenticatable's callback sends: {files:#?}");
}

/// The same requests over Rack on the ruby family, which compiles Devise, Warden and responders from source as the
/// Spinel tree does (`project::compiled_on_the_ruby_family`: a gem that needs railties cannot load as the real gem).
const RACK_SIGN_IN: &str = r##"require "uri"
$jar = {}
def req(verb, path, body = "")
  env = { "REQUEST_METHOD" => verb, "PATH_INFO" => path, "QUERY_STRING" => "", "SERVER_NAME" => "localhost",
          "SERVER_PORT" => "80", "HTTP_HOST" => "localhost", "rack.url_scheme" => "http", "rack.input" => StringIO.new(body),
          "HTTP_COOKIE" => $jar.map { |k, v| "#{k}=#{v}" }.join("; ") }
  unless body.empty?
    env["CONTENT_TYPE"] = "application/x-www-form-urlencoded"
    env["CONTENT_LENGTH"] = body.bytesize.to_s
  end
  status, headers, chunks = Main.run_rack(env)
  Array(headers["set-cookie"]).flat_map { |line| line.split("\n") }.each do |line|
    name, rest = line.split("=", 2)
    value = rest.split(";").first.to_s
    value.empty? ? $jar.delete(name) : $jar[name] = value
  end
  [status, headers, chunks.join]
end
form = ->(token, password) { "authenticity_token=#{URI.encode_www_form_component(token)}&user%5Bemail%5D=a%40example.com&user%5Bpassword%5D=#{password}&commit=Log+in" }

status, = req("GET", "/account/new")
raise "account #{status}" unless status == 200
status, headers, = req("GET", "/secret")
raise "secret #{status} #{headers.inspect}" unless status == 302 && headers["location"].end_with?("/users/sign_in")
status, _, page = req("GET", "/users/sign_in")
raise "sign-in page #{status}" unless status == 200
token = page[/name="authenticity_token" value="([^"]*)"/, 1]
status, _, failed = req("POST", "/users/sign_in", form.(token, "wrong"))
raise "wrong password #{status}" unless status == 200
raise "email kept #{failed}" unless failed.include?(%(name="user[email]" id="user_email" value="a@example.com"))
status, headers, = req("POST", "/users/sign_in", form.(token, "secret123"))
raise "sign in #{status} #{headers.inspect}" unless status == 302 && headers["location"].end_with?("/secret")
status, = req("GET", "/secret")
raise "secret signed in #{status}" unless status == 200
status, = req("GET", "/dash")
raise "dash signed in #{status}" unless status == 200
puts "devise sign-in over rack passed"
"##;

#[test]
fn a_devise_user_signs_in_over_rack_on_the_ruby_family() {
    let run = app(
        ":database_authenticatable",
        "  devise_for :users\n  get \"/account/new\", to: \"accounts#create\"\n  authenticate :user do\n    get \"/secret\", to: \"articles#index\"\n  end\n  authenticated :user do\n    get \"/dash\", to: \"articles#index\"\n  end\n",
    )
    .write("app/controllers/accounts_controller.rb", ACCOUNTS)
    .run_ruby(RACK_SIGN_IN);
    run.assert_passes();
    assert!(run.stdout.contains("devise sign-in over rack passed"), "{}", run.stdout);
}

/// Two mappings: Devise's shared views and its failure app read `resource_name` / `scope` per request, which the
/// ingest spells out as a `case` over the mappings (`gem_hooks::fold_devise_view_scope`, `fold_devise_failure_route`).
const RACK_TWO_MAPPINGS: &str = r##"require "uri"
$jar = {}
def req(verb, path, body = "")
  env = { "REQUEST_METHOD" => verb, "PATH_INFO" => path, "QUERY_STRING" => "", "SERVER_NAME" => "localhost",
          "SERVER_PORT" => "80", "HTTP_HOST" => "localhost", "rack.url_scheme" => "http", "rack.input" => StringIO.new(body),
          "HTTP_COOKIE" => $jar.map { |k, v| "#{k}=#{v}" }.join("; ") }
  unless body.empty?
    env["CONTENT_TYPE"] = "application/x-www-form-urlencoded"
    env["CONTENT_LENGTH"] = body.bytesize.to_s
  end
  status, headers, chunks = Main.run_rack(env)
  Array(headers["set-cookie"]).flat_map { |line| line.split("\n") }.each do |line|
    name, rest = line.split("=", 2)
    value = rest.split(";").first.to_s
    value.empty? ? $jar.delete(name) : $jar[name] = value
  end
  [status, headers, chunks.join]
end
def sign_in(scope, email, password)
  _, _, page = req("GET", "/#{scope}s/sign_in")
  token = page[/name="authenticity_token" value="([^"]*)"/, 1]
  [page, req("POST", "/#{scope}s/sign_in", "authenticity_token=#{URI.encode_www_form_component(token)}&#{scope}%5Bemail%5D=#{URI.encode_www_form_component(email)}&#{scope}%5Bpassword%5D=#{password}&commit=Log+in")]
end

req("GET", "/account/new")
req("GET", "/account/admin")
status, headers, = req("GET", "/console")
raise "console #{status} #{headers.inspect}" unless status == 302 && headers["location"].end_with?("/admins/sign_in")
page, (status, _, failed) = sign_in("admin", "root@example.com", "wrong")
raise "admin form" unless page.include?(%(name="admin[email]" id="admin_email"))
raise "admin wrong #{status}" unless status == 200 && failed.include?(%(value="root@example.com"))
_, (status, headers, _) = sign_in("admin", "root@example.com", "secret123")
raise "admin sign in #{status} #{headers.inspect}" unless status == 302 && headers["location"].end_with?("/console")
status, = req("GET", "/console")
raise "console admin #{status}" unless status == 200
status, headers, = req("GET", "/secret")
raise "secret as admin #{status}" unless status == 302 && headers["location"].end_with?("/users/sign_in")
page, (status, headers, _) = sign_in("user", "a@example.com", "secret123")
raise "user form" unless page.include?(%(name="user[email]" id="user_email"))
raise "user sign in #{status} #{headers.inspect}" unless status == 302 && headers["location"].end_with?("/secret")
status, = req("GET", "/secret")
raise "secret user #{status}" unless status == 200
puts "two devise mappings over rack passed"
"##;

const TWO_ACCOUNTS: &str = "class AccountsController < ApplicationController\n  def create\n    User.create!(email: \"a@example.com\", password: \"secret123\")\n    head :ok\n  end\n\n  def admin\n    Admin.create!(email: \"root@example.com\", password: \"secret123\")\n    head :ok\n  end\nend\n";

#[test]
fn two_devise_mappings_sign_in_over_rack_on_the_ruby_family() {
    let run = app(
        ":database_authenticatable",
        "  devise_for :users\n  devise_for :admins\n  get \"/account/new\", to: \"accounts#create\"\n  get \"/account/admin\", to: \"accounts#admin\"\n  authenticate :admin do\n    get \"/console\", to: \"articles#index\"\n  end\n  authenticate :user do\n    get \"/secret\", to: \"articles#index\"\n  end\n",
    )
    .edit("db/schema.rb", "  create_table \"articles\"", &format!("{}  create_table \"articles\"", USERS.replace("\"users\"", "\"admins\"")))
    .write("app/models/admin.rb", "class Admin < ApplicationRecord\n  devise :database_authenticatable\nend\n")
    .write("app/controllers/accounts_controller.rb", TWO_ACCOUNTS)
    .run_ruby(RACK_TWO_MAPPINGS);
    run.assert_passes();
    assert!(run.stdout.contains("two devise mappings over rack passed"), "{}", run.stdout);
}

const ACCOUNTS: &str = "class AccountsController < ApplicationController\n  def create\n    User.create!(email: \"a@example.com\", password: \"secret123\")\n    head :ok\n  end\nend\n";

fn between<'a>(text: &'a str, start: &str, end: &str) -> &'a str {
    let at = text.find(start).unwrap_or_else(|| panic!("no {start:?} in:\n{text}")) + start.len();
    &text[at..at + text[at..].find(end).expect("unterminated")]
}

// The cookies a browser keeps: each Set-Cookie's `name=value`, the later one of a name in place of the earlier.
fn kept(jar: &mut Vec<(String, String)>, set_cookies: &[String]) {
    for cookie in set_cookies {
        let pair = cookie.split(';').next().unwrap_or_default();
        let Some((name, value)) = pair.split_once('=') else { continue };
        jar.retain(|(n, _)| n != name);
        jar.push((name.to_string(), value.to_string()));
    }
}

fn header(jar: &[(String, String)]) -> String {
    jar.iter().map(|(n, v)| format!("{n}={v}")).collect::<Vec<_>>().join("; ")
}

fn form_encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Devise compiled from its source, served natively: the user's password is hashed by its `password=`, the sign-in
/// page is its view, its strategy authenticates the form's params through Warden, the session keeps the user
/// (`[[id], salt]`), and `authenticated :user` routes only a signed-in request, as in Rails.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_devise_user_signs_in_over_http_natively() {
    let tree = app(
        ":database_authenticatable",
        "  devise_for :users\n  get \"/account/new\", to: \"accounts#create\"\n  authenticate :user do\n    get \"/secret\", to: \"articles#index\"\n  end\n  authenticated :user do\n    get \"/dash\", to: \"articles#index\"\n  end\n",
    )
    .write("app/controllers/accounts_controller.rb", ACCOUNTS)
    .native_http_tree();
    let server = super::native_http::Server::start(&tree);
    assert_eq!(server.get("/account/new").status, 200, "{}", server.log());
    assert_eq!(server.get("/dash").status, 404, "{}", server.log());
    // `authenticate` sends a visitor to sign in through Warden's failure app (Devise's FailureApp#redirect).
    let secret = server.get("/secret");
    assert_eq!(secret.status, 302, "{}\n{}", secret.body, server.log());
    assert!(secret.headers.get("location").is_some_and(|l| l.ends_with("/users/sign_in")), "{:?}", secret.headers);

    let mut jar = Vec::new();
    let (page, cookies) = server.request_with_set_cookies("GET", "/users/sign_in", &[], "");
    assert_eq!(page.status, 200, "{}", server.log());
    assert!(page.body.contains(r#"<form class="new_user" id="new_user" action="/users/sign_in" accept-charset="UTF-8" method="post">"#), "{}", page.body);
    assert!(page.body.contains(r#"name="user[email]" id="user_email""#), "{}", page.body);
    kept(&mut jar, &cookies);
    let token = between(&page.body, "name=\"authenticity_token\" value=\"", "\"");
    // A wrong password renders the form again (FailureApp#recall, at the responder's `error_status`, :ok).
    let wrong = format!("authenticity_token={}&user%5Bemail%5D=a%40example.com&user%5Bpassword%5D=wrong&commit=Log+in", form_encode(token));
    let cookie = header(&jar);
    let (failed, cookies) = server.request_with_set_cookies(
        "POST",
        "/users/sign_in",
        &[("Content-Type", "application/x-www-form-urlencoded"), ("Cookie", cookie.as_str())],
        &wrong,
    );
    assert_eq!(failed.status, 200, "{}\n{}", failed.body, server.log());
    assert!(failed.body.contains(r#"<form class="new_user" id="new_user" action="/users/sign_in""#), "{}", failed.body);
    assert!(failed.body.contains(r#"name="user[email]" id="user_email" value="a@example.com""#), "{}", failed.body);
    kept(&mut jar, &cookies);
    let form = format!("authenticity_token={}&user%5Bemail%5D=a%40example.com&user%5Bpassword%5D=secret123&commit=Log+in", form_encode(token));
    let cookie = header(&jar);
    let (signed, cookies) = server.request_with_set_cookies(
        "POST",
        "/users/sign_in",
        &[("Content-Type", "application/x-www-form-urlencoded"), ("Cookie", cookie.as_str())],
        &form,
    );
    assert_eq!(signed.status, 302, "{}\n{}", signed.body, server.log());
    assert!(signed.headers.get("location").is_some_and(|l| l.ends_with("/")), "{:?}", signed.headers);
    kept(&mut jar, &cookies);

    let cookie = header(&jar);
    let (home, _) = server.request_with_set_cookies("GET", "/", &[("Cookie", cookie.as_str())], "");
    assert!(home.body.contains("Signed in successfully."), "{}", home.body);
    let (dash, _) = server.request_with_set_cookies("GET", "/dash", &[("Cookie", cookie.as_str())], "");
    assert_eq!(dash.status, 200, "{}", server.log());
    let (secret, _) = server.request_with_set_cookies("GET", "/secret", &[("Cookie", cookie.as_str())], "");
    assert_eq!(secret.status, 200, "{}", server.log());
    // Signed in, the sign-in page sends the user on (`require_no_authentication`).
    let (again, _) = server.request_with_set_cookies("GET", "/users/sign_in", &[("Cookie", cookie.as_str())], "");
    assert_eq!(again.status, 302, "{}", server.log());
}

/// Devise names its helpers from the route mapping: `devise_for :users`
/// gives `current_user`, the scope active_model_serializers' default
/// `serialization_scope` reads. `class_name:`, `as:` or `singular:`
/// changes what the mapping is called or which model it answers, and a
/// `devise` model with no mapping drawn gives no helper. Only a single
/// `devise_for :users` drawn unconditionally at the top of the routes
/// counts: under a namespace, a condition or a second one, as under any
/// of the others, the scope is left unknown, and the render as written,
/// reported.
#[test]
fn a_serializer_scope_is_devises_current_user_only_under_devise_for_users() {
    let serializer = "class CardSerializer < ActiveModel::Serializer\n  attributes :id\n  attribute(:scoped) { scope }\nend\n";
    let controller = "class CardsController < ApplicationController\n  def show\n    render json: Article.find(params[:id]), serializer: CardSerializer\n  end\nend\n";
    let rendered = |routes: &str| {
        let (tree, errors) = app(":database_authenticatable", &format!("{routes}  get \"/cards/:id\", to: \"cards#show\"\n"))
            .write("app/serializers/card_serializer.rb", serializer)
            .write("app/controllers/cards_controller.rb", controller)
            .emit(BuildTarget::Ruby);
        (std::fs::read_to_string(tree.join("app/controllers/cards_controller.rb")).expect("cards_controller.rb"), errors)
    };
    let (cards, errors) = rendered("  devise_for :users\n");
    assert!(cards.contains("CardSerializer.new(Article.find(@params[\"id\"]), { scope: current_user }).serializable_hash"), "{cards}\n{errors:#?}");
    for routes in [
        "  devise_for :users, class_name: \"User\"\n",
        "  devise_for :users, as: :member\n",
        "  devise_for :users, singular: :member\n",
        "",
        // Not one unconditional mapping drawn at the top of the routes:
        // under a namespace Devise maps `admin_user`, a conditional one
        // may not be drawn, and a second `devise_for :users` redraws it.
        "  namespace :admin do\n    devise_for :users\n  end\n",
        "  if ENV[\"DEVISE\"]\n    devise_for :users\n  end\n",
        "  devise_for :users\n  devise_for :users, path: \"accounts\"\n",
    ] {
        let (cards, errors) = rendered(routes);
        assert!(!cards.contains("serializable_hash"), "{routes:?}: {cards}");
        assert!(errors.iter().any(|e| e.contains("rendering through an active_model_serializers")), "{routes:?}: {errors:#?}");
    }
}

/// `:validatable`: Devise's `included` hook `class_eval`s its `validates_*_of` calls onto the model, with
/// `email_regexp` and `password_length` read as the hook runs. Each case's errors are Rails 8.1.4's with Devise 4.9.4.
const CHECKS: &str = "class AccountsController < ApplicationController\n  def create\n    User.create!(email: \"a@example.com\", password: \"secret123\")\n    head :ok\n  end\n\n  def check\n    user = User.new(email: params[:email], password: params[:password])\n    user.password_confirmation = params[:confirmation] if params[:confirmation]\n    user.valid?\n    render plain: user.errors.full_messages.join(\"|\")\n  end\nend\n";

const VALIDATABLE_ROUTES: &str = "  devise_for :users\n  get \"/account/new\", to: \"accounts#create\"\n  get \"/account/check\", to: \"accounts#check\"\n";

/// The query of each case, and the errors Rails answers for it.
const VALIDATABLE_CASES: [(&str, &str); 6] = [
    ("email=bad&password=123", "Email is invalid|Password is too short (minimum is 6 characters)"),
    ("email=&password=", "Email can't be blank|Password can't be blank"),
    ("email=b%40example.com&password=secret123&confirmation=other", "Password confirmation doesn't match Password"),
    ("email=b%40example.com&password=secret123&confirmation=secret123", ""),
    ("email=b%40example.com&password=xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx", "Password is too long (maximum is 128 characters)"),
    ("email=a%40example.com&password=secret123", "Email has already been taken"),
];

#[test]
fn devise_validatable_answers_rails_errors_on_the_ruby_family() {
    let cases = VALIDATABLE_CASES.iter().map(|(query, want)| format!("[{query:?}, {want:?}]")).collect::<Vec<_>>().join(",\n  ");
    let script = format!(
        r##"def get(path, query)
  env = {{ "REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => query, "SERVER_NAME" => "localhost",
          "SERVER_PORT" => "80", "HTTP_HOST" => "localhost", "rack.url_scheme" => "http", "rack.input" => StringIO.new("") }}
  status, _headers, chunks = Main.run_rack(env)
  [status, chunks.join]
end
status, = get("/account/new", "")
raise "account #{{status}}" unless status == 200
[
  {cases}
].each do |query, want|
  status, body = get("/account/check", query)
  raise "#{{query}}: #{{status}} #{{body.inspect}}" unless status == 200 && body == want
end
puts "devise validatable passed"
"##
    );
    let run = app(":database_authenticatable, :validatable", VALIDATABLE_ROUTES).write("app/controllers/accounts_controller.rb", CHECKS).run_ruby(&script);
    run.assert_passes();
    assert!(run.stdout.contains("devise validatable passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn devise_validatable_answers_rails_errors_natively() {
    let tree = app(":database_authenticatable, :validatable", VALIDATABLE_ROUTES)
        .write("app/controllers/accounts_controller.rb", CHECKS)
        .native_http_tree();
    let server = super::native_http::Server::start(&tree);
    assert_eq!(server.get("/account/new").status, 200, "{}", server.log());
    for (query, want) in VALIDATABLE_CASES {
        let answer = server.get(&format!("/account/check?{query}"));
        assert_eq!((answer.status, answer.body.as_str()), (200, want), "{query}\n{}", server.log());
    }
}

/// `:recoverable`, the whole reset as Rails 8.1.4 with Devise 4.9.4 runs it: the request redirects to sign in and mails a
/// link from `config.mailer_sender`, the link's token opens the edit form, and the new password signs the user in.
const RESETS: &str = "class AccountsController < ApplicationController\n  def create\n    User.create!(email: \"a@example.com\", password: \"secret123\")\n    head :ok\n  end\n\n  def mailed\n    mail = Devise::Mailer.deliveries.last\n    render plain: \"#{mail.subject}|#{mail.from.join(\",\")}|#{mail.to.join(\",\")}|#{mail.body}\"\n  end\nend\n";

const RECOVERABLE_ROUTES: &str = "  devise_for :users\n  get \"/account/new\", to: \"accounts#create\"\n  get \"/account/mailed\", to: \"accounts#mailed\"\n  authenticate :user do\n    get \"/secret\", to: \"articles#index\"\n  end\n";

const RACK_RESET: &str = r##"require "uri"
$jar = {}
def req(verb, path, body = "")
  path, query = path.split("?", 2)
  env = { "REQUEST_METHOD" => verb, "PATH_INFO" => path, "QUERY_STRING" => query.to_s, "SERVER_NAME" => "localhost",
          "SERVER_PORT" => "80", "HTTP_HOST" => "localhost", "rack.url_scheme" => "http", "rack.input" => StringIO.new(body),
          "HTTP_COOKIE" => $jar.map { |k, v| "#{k}=#{v}" }.join("; ") }
  unless body.empty?
    env["CONTENT_TYPE"] = "application/x-www-form-urlencoded"
    env["CONTENT_LENGTH"] = body.bytesize.to_s
  end
  status, headers, chunks = Main.run_rack(env)
  Array(headers["set-cookie"]).flat_map { |line| line.split("\n") }.each do |line|
    name, rest = line.split("=", 2)
    value = rest.split(";").first.to_s
    value.empty? ? $jar.delete(name) : $jar[name] = value
  end
  [status, headers, chunks.join]
end
token_of = ->(page) { URI.encode_www_form_component(page[/name="authenticity_token" value="([^"]*)"/, 1]) }

status, = req("GET", "/account/new")
raise "account #{status}" unless status == 200
status, _, page = req("GET", "/users/password/new")
raise "request page #{status}" unless status == 200 && page.include?(%(action="/users/password"))
status, headers, = req("POST", "/users/password", "authenticity_token=#{token_of.(page)}&user%5Bemail%5D=a%40example.com")
raise "request #{status} #{headers.inspect}" unless status == 302 && headers["location"].end_with?("/users/sign_in")
_, _, mailed = req("GET", "/account/mailed")
subject, from, to, body = mailed.split("|", 4)
raise "mail #{[subject, from, to].inspect}" unless [subject, from, to] == ["Reset password instructions", "please-change-me@example.com", "a@example.com"]
raw = body[/reset_password_token=([^"&]+)"/, 1]
raise "no token in #{body}" if raw.nil?
status, _, edit = req("GET", "/users/password/edit?reset_password_token=#{raw}")
raise "edit #{status}" unless status == 200 && edit.include?(%(value="#{raw}"))
form = "_method=put&authenticity_token=#{token_of.(edit)}&user%5Breset_password_token%5D=#{raw}&user%5Bpassword%5D=newsecret1&user%5Bpassword_confirmation%5D=newsecret1"
status, headers, = req("POST", "/users/password", form)
raise "reset #{status} #{headers.inspect}" unless status == 302 && headers["location"] == "http://localhost/"
status, = req("GET", "/secret")
raise "signed in #{status}" unless status == 200
raise "new password" unless User.find_by(email: "a@example.com").valid_password?("newsecret1")
puts "devise reset over rack passed"
"##;

#[test]
fn a_devise_password_resets_over_rack_on_the_ruby_family() {
    let run = app(":database_authenticatable, :recoverable, :validatable", RECOVERABLE_ROUTES)
        .write("app/controllers/accounts_controller.rb", RESETS)
        .run_ruby(RACK_RESET);
    run.assert_passes();
    assert!(run.stdout.contains("devise reset over rack passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_devise_password_resets_over_http_natively() {
    let tree = app(":database_authenticatable, :recoverable, :validatable", RECOVERABLE_ROUTES)
        .write("app/controllers/accounts_controller.rb", RESETS)
        .native_http_tree();
    let server = super::native_http::Server::start(&tree);
    assert_eq!(server.get("/account/new").status, 200, "{}", server.log());
    let form_type = ("Content-Type", "application/x-www-form-urlencoded");
    let mut jar = Vec::new();

    let (page, cookies) = server.request_with_set_cookies("GET", "/users/password/new", &[], "");
    assert_eq!(page.status, 200, "{}", server.log());
    kept(&mut jar, &cookies);
    let token = between(&page.body, "name=\"authenticity_token\" value=\"", "\"").to_string();
    let cookie = header(&jar);
    let (requested, cookies) = server.request_with_set_cookies(
        "POST",
        "/users/password",
        &[form_type, ("Cookie", cookie.as_str())],
        &format!("authenticity_token={}&user%5Bemail%5D=a%40example.com", form_encode(&token)),
    );
    assert_eq!(requested.status, 302, "{}\n{}", requested.body, server.log());
    assert!(requested.headers.get("location").is_some_and(|l| l.ends_with("/users/sign_in")), "{:?}", requested.headers);
    kept(&mut jar, &cookies);

    let mailed = server.get("/account/mailed");
    let mut parts = mailed.body.splitn(4, '|');
    let head: Vec<&str> = parts.by_ref().take(3).collect();
    assert_eq!(head, ["Reset password instructions", "please-change-me@example.com", "a@example.com"], "{}", mailed.body);
    let body = parts.next().unwrap_or_default();
    let raw = between(body, "reset_password_token=", "\"").to_string();

    let cookie = header(&jar);
    let (edit, cookies) = server.request_with_set_cookies("GET", &format!("/users/password/edit?reset_password_token={raw}"), &[("Cookie", cookie.as_str())], "");
    assert_eq!(edit.status, 200, "{}", server.log());
    assert!(edit.body.contains(&format!("value=\"{raw}\"")), "{}", edit.body);
    kept(&mut jar, &cookies);
    let token = between(&edit.body, "name=\"authenticity_token\" value=\"", "\"").to_string();
    let cookie = header(&jar);
    let (reset, cookies) = server.request_with_set_cookies(
        "POST",
        "/users/password",
        &[form_type, ("Cookie", cookie.as_str())],
        &format!(
            "_method=put&authenticity_token={}&user%5Breset_password_token%5D={raw}&user%5Bpassword%5D=newsecret1&user%5Bpassword_confirmation%5D=newsecret1",
            form_encode(&token)
        ),
    );
    assert_eq!(reset.status, 302, "{}\n{}", reset.body, server.log());
    assert!(reset.headers.get("location").is_some_and(|l| l.ends_with("/")), "{:?}", reset.headers);
    kept(&mut jar, &cookies);
    let cookie = header(&jar);
    let (secret, _) = server.request_with_set_cookies("GET", "/secret", &[("Cookie", cookie.as_str())], "");
    assert_eq!(secret.status, 200, "{}", server.log());
}
