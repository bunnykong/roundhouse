//! The initializer ledger (RFC rubys/roundhouse#841, E0): a WARNING per
//! boot statement no recognizer reads, in host initializers, engine class
//! bodies and `config/application.rb`. No behaviour change: every test
//! here also pins that nothing the recognizers produce moved.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::diagnostic::{Diagnostic, DiagnosticKind, Severity};
use roundhouse::ingest::{ingest_app_from_tree, prism, survey};
use roundhouse::App;

const SCHEMA: &str = "ActiveRecord::Schema.define do\nend\n";

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files.iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect()
}

/// Ingest `files` and return the app with its ledger diagnostics.
fn ingest(files: &[(&str, &str)]) -> (App, Vec<Diagnostic>) {
    let mut all: Vec<(&str, &str)> = vec![("db/schema.rb", SCHEMA)];
    all.extend_from_slice(files);
    let (app, diags) = prism::scope(|| ingest_app_from_tree(tree(&all)));
    let app = app.expect("ingest");
    let ledger = diags
        .into_iter()
        .filter(|d| matches!(d.kind, DiagnosticKind::UnconsumedInitializer { .. }))
        .collect();
    (app, ledger)
}

fn statement(d: &Diagnostic) -> &str {
    match &d.kind {
        DiagnosticKind::UnconsumedInitializer { statement, .. } => statement.as_str(),
        _ => unreachable!(),
    }
}

fn owner(d: &Diagnostic) -> &str {
    match &d.kind {
        DiagnosticKind::UnconsumedInitializer { owner, .. } => owner.as_str(),
        _ => unreachable!(),
    }
}

fn holder(d: &Diagnostic) -> &str {
    match &d.kind {
        DiagnosticKind::UnconsumedInitializer { holder, .. } => holder.as_str(),
        _ => unreachable!(),
    }
}

fn location(d: &Diagnostic) -> &str {
    match &d.kind {
        DiagnosticKind::UnconsumedInitializer { location, .. } => location.as_str(),
        _ => unreachable!(),
    }
}

fn statements(diags: &[Diagnostic]) -> Vec<&str> {
    diags.iter().map(statement).collect()
}

/// The source text a diagnostic's span covers.
fn spanned<'a>(app: &'a App, d: &Diagnostic) -> &'a str {
    let file = &app.sources[d.span.file.0 as usize - 1];
    &file.text[d.span.start as usize..d.span.end as usize]
}

#[test]
fn a_setting_inside_an_unconsumed_configure_is_reported_with_its_span_and_a_consumed_mixin_is_silent() {
    let (app, ledger) = ingest(&[
        (
            "config/initializers/boot.rb",
            "Turbo::StreamsChannel.prepend RoomStreamsAreAuthorized\n\
             \n\
             Foo.configure do |c|\n  c.timeout = 5\nend\n",
        ),
    ]);
    // The recognizer still reads the mixin ...
    assert_eq!(app.module_mixins.len(), 1);
    // ... and the ledger reports exactly what nothing reads: the setting
    // inside the `Foo.configure` block, which no recognizer reads whole
    // or in part.
    assert_eq!(statements(&ledger), ["c.timeout ="]);
    let d = &ledger[0];
    assert_eq!(d.severity, Severity::Warning);
    assert_eq!(d.code(), "unconsumed_initializer");
    assert_eq!(location(d), "config/initializers/boot.rb:4:3");
    assert_eq!(owner(d), "host initializer, inside Foo.configure");
    assert_eq!(spanned(&app, d), "c.timeout = 5");
}

#[test]
fn a_configure_block_is_listed_by_its_statements_and_a_callback_body_stays_one_statement() {
    let (app, ledger) = ingest(&[(
        "config/initializers/gadget.rb",
        "Gadget.configure do |config|\n  config.queue = \"low\"\n  config.events.after_save do |event|\n    Indexer.call(event.record)\n    Audit.log(event)\n  end\nend\n",
    )]);
    // The handler registered by `after_save` is one statement; its body is
    // not a list of settings, and the container itself is not listed twice.
    assert_eq!(statements(&ledger), ["config.queue =", "config.events.after_save"]);
    assert_eq!(location(&ledger[1]), "config/initializers/gadget.rb:3:3");
    assert!(spanned(&app, &ledger[1]).starts_with("config.events.after_save do |event|"));
    assert!(spanned(&app, &ledger[1]).ends_with("end"));
}

#[test]
fn a_to_prepare_block_is_unwrapped_like_the_recognizers_and_only_the_unread_statement_is_reported() {
    let (app, ledger) = ingest(&[(
        "config/initializers/prepare.rb",
        "Rails.application.config.to_prepare do\n  Turbo::StreamsChannel.prepend Guard\n  Audit.record_boot\nend\n",
    )]);
    assert_eq!(app.module_mixins.len(), 1);
    assert_eq!(statements(&ledger), ["Audit.record_boot"]);
    assert_eq!(owner(&ledger[0]), "host initializer, inside to_prepare");
    assert_eq!(location(&ledger[0]), "config/initializers/prepare.rb:3:3");
}

// A statement's node carries offsets into its whole file, past the end of
// the statement's own text once anything comes before it. The recognizers
// that read a constant receiver slice the file, not that text.

#[test]
fn a_prepended_module_new_after_other_statements_is_reported_not_a_panic() {
    let (_, ledger) = ingest(&[(
        "config/initializers/document.rb",
        "# Extensions for the JSON:API document.\n\
         # They run once the app's classes are loaded,\n\
         # after every other initializer has run.\n\
         \n\
         Rails.application.config.to_prepare do\n  Audit.record_boot\n  \
         Document.prepend(Module.new do\n    def to_hash\n      super\n    end\n  end)\nend\n",
    )]);
    let locations: Vec<&str> = ledger.iter().map(location).collect();
    assert_eq!(locations, ["config/initializers/document.rb:6:3", "config/initializers/document.rb:7:3"]);
}

#[test]
fn a_content_helper_allow_list_after_other_statements_is_read() {
    let (_, ledger) = ingest(&[(
        "config/initializers/rich_text.rb",
        "# Allow video embeds in rich text.\n\
         \n\
         Rails.application.config.to_prepare do\n  Audit.record_boot\n  \
         ActionText::ContentHelper.allowed_tags = Set[\"video\"]\nend\n",
    )]);
    assert_eq!(statements(&ledger), ["Audit.record_boot"]);
}

#[test]
fn an_after_initialize_hook_is_carried_unless_it_needs_its_surroundings() {
    // The first runs at boot (ingest::after_initialize) and is consumed;
    // `do |app|` takes the application Rails yields and the third reads the
    // initializer's own local, which a carried body has neither of.
    let (app, ledger) = ingest(&[(
        "config/initializers/hooks.rb",
        "limit = 5\n\
         Rails.application.config.after_initialize do\n  Mailer.verify!\nend\n\
         Rails.application.config.after_initialize do |app|\n  Mailer.verify!(app)\nend\n\
         Rails.application.config.after_initialize do\n  Mailer.verify!(limit)\nend\n",
    )]);
    let hooks: Vec<&str> = ledger
        .iter()
        .filter(|d| statement(d) == "Rails.application.config.after_initialize")
        .map(location)
        .collect();
    assert_eq!(hooks, ["config/initializers/hooks.rb:5:1", "config/initializers/hooks.rb:8:1"]);
    assert!(app.runs_after_initialize);
}

#[test]
fn inert_allowlisted_statements_are_not_reported_but_a_computed_value_is() {
    let (_, ledger) = ingest(&[(
        "config/initializers/inert.rb",
        "Rails.application.config.filter_parameters += [:passw, :secret]\n\
         Rails.application.config.action_dispatch.cookies_serializer = :json\n\
         Rails.application.config.assets.version = \"1.0\"\n\
         Rails.application.config.filter_parameters += Secrets.names\n\
         Rails.application.config.generators do |g|\n  g.orm :active_record\nend\n\
         ActiveStorage::Transformers::Vips\n",
    )]);
    // The computed `Secrets.names` could do anything; the literal ones cannot.
    assert_eq!(statements(&ledger), ["Rails.application.config.filter_parameters +="]);
    assert_eq!(location(&ledger[0]), "config/initializers/inert.rb:4:1");
}

#[test]
fn load_edges_and_definitions_are_not_boot_statements() {
    let (_, ledger) = ingest(&[(
        "config/initializers/defs.rb",
        "require \"set\"\nrequire_relative \"../../lib/thing\"\n\nmodule Helper\n  def self.x = 1\nend\n\nclass Marker; end\n",
    )]);
    assert!(ledger.is_empty(), "{:?}", statements(&ledger));
}

#[test]
fn every_recognizer_that_reads_an_initializer_keeps_its_statement_out_of_the_ledger() {
    // One statement per recognizer, each in the file the recognizer reads.
    let consumed: &[(&str, &str)] = &[
        ("config/initializers/session_store.rb", "Rails.application.config.session_store :cookie_store, key: \"_app\"\n"),
        ("config/initializers/formats.rb", "Time::DATE_FORMATS[:short_date] = \"%b %e\"\n"),
        ("config/initializers/mixins.rb", "Target.include Concern\n"),
        ("config/initializers/filters.rb", "Target.before_action :guard, only: [:show]\n"),
        (
            "config/initializers/headers.rb",
            "ActiveSupport.on_load(:active_storage_blob) do\n  \
             Target.after_action only: :show do\n    response.set_header(\"Cache-Control\", \"public\")\n  end\nend\n",
        ),
        (
            "config/initializers/sqlite.rb",
            "conn.create_function(\"regexp\", 2) do |fn, pattern, expr|\n  fn.result = 1\nend\n",
        ),
        ("config/initializers/kaminari.rb", "Kaminari.configure do |config|\n  config.default_per_page = 10\nend\n"),
        ("config/initializers/lift.rb", "Rails.application.config.app_version = \"1\"\nRails.application.config.x.vapid.key = \"k\"\n"),
        ("config/initializers/storage.rb", "Rails.application.config.active_storage.variable_content_types -= %w[image/svg+xml]\n"),
        ("config/initializers/vips.rb", "Vips.block_untrusted(true)\n"),
        ("config/initializers/wrap.rb", "Rails.application.config.action_controller.wrap_parameters_by_default = true\n"),
        (
            "config/initializers/forgery.rb",
            "Rails.application.configure do\n  config.action_controller.forgery_protection_verification_strategy = :header_only\nend\n",
        ),
        (
            "config/initializers/inflections.rb",
            "ActiveSupport::Inflector.inflections(:en) do |inflect|\n  inflect.acronym \"API\"\n  inflect.irregular \"person\", \"people\"\nend\n",
        ),
        ("config/initializers/mapper.rb", "ActionDispatch::Routing::Mapper.prepend Extras\n"),
        (
            "config/initializers/cable.rb",
            "Rails.application.config.action_cable.allowed_request_origins = [\"https://a.example\", /https:\\/\\/b/]\n",
        ),
        (
            "config/initializers/content.rb",
            "ActionText::ContentHelper.allowed_tags = ActionText::ContentHelper.allowed_tags | %w[mark]\n",
        ),
        ("config/initializers/view_paths.rb", "ActionController::Base.prepend_view_path \"app/views/themes/next\"\n"),
        (
            "config/initializers/mail_views.rb",
            "ActiveSupport.on_load(:action_mailer) do\n  append_view_path \"app/views/mail\"\nend\n",
        ),
    ];
    for (path, source) in consumed {
        let (_, ledger) = ingest(&[(path, source)]);
        assert!(ledger.is_empty(), "{path}: ledger listed {:?}", statements(&ledger));
    }
}

#[test]
fn a_view_path_the_build_cannot_read_is_an_error_not_a_ledger_line() {
    let files = tree(&[
        ("db/schema.rb", SCHEMA),
        ("config/initializers/view_paths.rb", "ActionController::Base.prepend_view_path(ThemeResolver.new)\n"),
    ]);
    let (app, diags) = prism::scope(|| ingest_app_from_tree(files));
    app.expect("ingest");
    assert!(!diags.iter().any(|d| matches!(d.kind, DiagnosticKind::UnconsumedInitializer { .. })), "{diags:#?}");
    let errors: Vec<&Diagnostic> = diags.iter().filter(|d| d.severity == Severity::Error).collect();
    assert_eq!(errors.len(), 1, "{diags:#?}");
    assert!(errors[0].message.contains("a view path that is not a string literal"), "{:?}", errors[0].message);
}

#[test]
fn inflect_calls_nothing_reads_are_reported_inside_inflections_rb() {
    let (_, ledger) = ingest(&[(
        "config/initializers/inflections.rb",
        "ActiveSupport::Inflector.inflections(:en) do |inflect|\n  inflect.acronym \"API\"\n  inflect.human \"rhino\", \"rhinoceros\"\nend\n",
    )]);
    assert_eq!(statements(&ledger), ["inflect.human"]);
    assert_eq!(owner(&ledger[0]), "host initializer, inside inflections");
    // The same statement outside the host's inflections.rb is read by nothing.
    let (_, elsewhere) = ingest(&[(
        "config/initializers/other.rb",
        "ActiveSupport::Inflector.inflections(:en) do |inflect|\n  inflect.acronym \"API\"\nend\n",
    )]);
    assert_eq!(statements(&elsewhere), ["ActiveSupport::Inflector.inflections"]);
}

#[test]
fn a_spelling_the_recognizer_does_not_read_stays_in_the_ledger() {
    // The forgery reader matches lines that START with `config.`; the long
    // `Rails.application.config.` spelling of the same setting is read by
    // nothing, and the ledger says so rather than assuming it is.
    let (_, ledger) = ingest(&[(
        "config/initializers/forgery.rb",
        "Rails.application.config.action_controller.forgery_protection_verification_strategy = :header_only\n",
    )]);
    assert_eq!(ledger.len(), 1);
    assert!(statement(&ledger[0]).starts_with("Rails.application.config.action_controller.forgery_"));
}

#[test]
fn a_recognizer_that_does_not_apply_leaves_its_statement_in_the_ledger() {
    // The long forgery setting spelling is unread; a time format is read only
    // at the top level; `config.x` assignments only at the top or in configure.
    let (_, ledger) = ingest(&[
        ("config/initializers/other.rb", "Rails.application.config.action_controller.forgery_protection_verification_strategy = :header_only\n"),
        (
            "config/initializers/nested.rb",
            "Rails.application.config.to_prepare do\n  Time::DATE_FORMATS[:short_date] = \"%b %e\"\n  Rails.application.config.app_version = \"1\"\nend\n",
        ),
    ]);
    let mut got = statements(&ledger);
    got.sort();
    assert_eq!(
        got,
        [
            "Rails.application.config.action_controller.forgery_protection_verification_strat",
            "Rails.application.config.app_version =",
            "Time::DATE_FORMATS[] =",
        ]
    );
}

#[test]
fn a_session_store_in_another_initializer_reaches_the_cookie_key() {
    let (app, ledger) = ingest(&[(
        "config/initializers/other.rb",
        "Rails.application.config.session_store :cookie_store, key: \"_app\"\n",
    )]);
    assert!(ledger.is_empty());
    let application = app.rails_application.expect("initializer application reopen");
    let method = application.methods.iter().find(|m| m.name.as_str() == "session_cookie_key").unwrap();
    assert!(roundhouse::emit::ruby::emit_method(method).contains("_app"));
}

#[test]
fn a_mixed_conditional_with_session_store_stays_in_the_ledger() {
    // A session_store sibling must not consume an unread assignment that
    // shares its conditional — the ledger has to keep reporting the whole if.
    let (_, ledger) = ingest(&[(
        "config/initializers/mixed.rb",
        "if Rails.env.production?\n\
           Rails.application.config.session_store :cookie_store, key: \"_prod\"\n\
           Rails.application.config.action_controller.forgery_protection_verification_strategy = :header_only\n\
         end\n",
    )]);
    assert_eq!(ledger.len(), 1);
    assert!(statement(&ledger[0]).starts_with("if "));
}

#[test]
fn application_rb_reports_config_statements_nothing_reads() {
    let (app, ledger) = ingest(&[(
        "config/application.rb",
        "require_relative \"boot\"\nrequire \"rails/all\"\n\nmodule Shop\n  class Application < Rails::Application\n    \
         config.load_defaults 8.1\n    config.time_zone = \"Central Time (US & Canada)\"\n    \
         config.eager_load = true\n    config.i18n.enforce_available_locales = false\n    \
         config.after_initialize do\n      Warmup.run\n    end\n\n    def read_only?\n      false\n    end\n  end\nend\n",
    )]);
    // time_zone and the method still ingest ...
    let application = app.rails_application.as_ref().expect("Rails::Application reopen");
    assert!(application.methods.iter().any(|m| m.name.as_str() == "config_time_zone"));
    assert!(application.methods.iter().any(|m| m.name.as_str() == "read_only?"));
    // ... load_defaults / time_zone are consumed, eager_load is inert, defs are methods.
    // `config.after_initialize` runs at boot (ingest::after_initialize), so it is consumed too.
    assert_eq!(statements(&ledger), ["config.i18n.enforce_available_locales ="]);
    assert_eq!(owner(&ledger[0]), "host config/application.rb");
    assert!(app.runs_after_initialize);
}

/// The tree of an in-tree PATH engine: its class body, initializers and
/// whatever extra files `extra` adds.
fn engine_tree<'a>(class_body: &'a str, extra: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
    let mut files = vec![
        ("Gemfile.lock", "PATH\n  remote: vendor/catalog\n  specs:\n    catalog (0.1.0)\n\nDEPENDENCIES\n  catalog!\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("vendor/catalog/lib/catalog/engine.rb", class_body),
        ("vendor/catalog/app/models/catalog/product.rb", "module Catalog\n  class Product\n  end\nend\n"),
        ("vendor/catalog/config/routes.rb", "Catalog::Engine.routes.draw do\nend\n"),
    ];
    files.extend_from_slice(extra);
    files
}

#[test]
fn an_engine_class_body_reports_initializers_and_hooks_but_not_requires() {
    let engine = "require \"catalog/version\"\n\
        module Catalog\n  class Engine < ::Rails::Engine\n    isolate_namespace Catalog\n\n    \
        initializer \"catalog.assets\" do |app|\n      app.config.assets.paths << root.join(\"assets\")\n    end\n\n    \
        config.to_prepare { Catalog.boot! }\n\n    config.catalog_flag = true\n  end\nend\n";
    let (_, ledger) = ingest(&engine_tree(engine, &[]));
    assert_eq!(
        statements(&ledger),
        ["isolate_namespace", "initializer \"catalog.assets\"", "config.to_prepare", "config.catalog_flag ="]
    );
    assert_eq!(owner(&ledger[1]), "engine Catalog::Engine class body");
    assert_eq!(location(&ledger[1]), "vendor/catalog/lib/catalog/engine.rb:6:5");
    // The span points at the statement itself.
    assert!(ledger.iter().all(|d| d.severity == Severity::Warning));
}

const SETTLED_ENGINE: &str = r##"module Catalog
  mattr_accessor :label, default: "x"

  class Engine < Rails::Engine
    isolate_namespace Catalog

    rake_tasks do
      load "catalog/tasks.rb"
    end

    console do
      puts "catalog"
    end

    config.catalog = ActiveSupport::OrderedOptions.new unless config.try(:catalog)

    config.before_initialize do
      config.catalog.each do |key, value|
        Catalog.public_send("#{key}=", value)
      end
    end

    initializer "catalog.extensions" do
      ActiveSupport.on_load :active_job do
        ActiveJob.extend Catalog::Lookup
      end
    end

    initializer "catalog.middleware" do |app|
      if app.config.api_only
        config.middleware.use ActionDispatch::Flash
      end
    end
  end
end
"##;

/// An engine's statements the engine's own boot accounts for leave the
/// ledger: blocks only a `rails` command runs, the config namespace it
/// copies onto a module, the load-hook mixins the ingest carries, an
/// initializer whose only branch the host's config never takes, and the
/// `before_initialize` boot replays; the engine is then admitted, so its
/// `isolate_namespace` is consumed too.
#[test]
fn an_engines_settled_boot_statements_leave_the_ledger() {
    let (_, ledger) = ingest(&engine_tree(SETTLED_ENGINE, &[]));
    assert!(statements(&ledger).is_empty(), "{:?}", statements(&ledger));
}

#[test]
fn an_engine_initializer_whose_branch_the_host_takes_stays_on_the_ledger() {
    let application = "module Shop\n  class Application < Rails::Application\n    config.api_only = true\n  end\nend\n";
    let (_, ledger) = ingest(&engine_tree(SETTLED_ENGINE, &[("config/application.rb", application)]));
    assert!(statements(&ledger).contains(&"initializer \"catalog.middleware\""), "{:?}", statements(&ledger));
}

#[test]
fn an_admitted_engines_literal_isolate_namespace_is_consumed() {
    // Exactly one literal `isolate_namespace`, plain lib, no initializers:
    // the shape `engine_route_sources` admits for a mount.
    let engine = "module Catalog\n  class Engine < Rails::Engine\n    isolate_namespace Catalog\n  end\nend\n";
    let (_, ledger) = ingest(&engine_tree(engine, &[]));
    assert!(ledger.is_empty(), "{:?}", statements(&ledger));
}

#[test]
fn an_engines_own_initializer_directory_is_listed_whether_or_not_the_mount_is_refused() {
    let engine = "module Catalog\n  class Engine < Rails::Engine\n    isolate_namespace Catalog\n  end\nend\n";
    let extra = [(
        "vendor/catalog/config/initializers/catalog.rb",
        "require \"catalog\"\nCatalog.configure do |c|\n  c.name = \"x\"\nend\nTime::DATE_FORMATS[:c] = \"%d\"\n",
    )];
    let (_, ledger) = ingest(&engine_tree(engine, &extra));
    // Nothing reads an engine's initializer directory, not even a shape the
    // host's recognizers read (the time format). The directory also makes
    // the mount refused, so the class's `isolate_namespace` is unread too.
    assert_eq!(statements(&ledger), ["c.name =", "Time::DATE_FORMATS[] =", "isolate_namespace"]);
    assert_eq!(owner(&ledger[0]), "engine vendor/catalog initializer, inside Catalog.configure");
}

#[test]
fn the_ledger_is_a_warning_and_never_a_survey_gap() {
    survey::activate();
    let (app, diags) = prism::scope(|| {
        ingest_app_from_tree(tree(&[
            ("db/schema.rb", SCHEMA),
            ("config/initializers/x.rb", "Foo.configure { |c| c.a = 1 }\n"),
        ]))
    });
    let recorded = survey::drain();
    app.expect("ingest");
    assert!(
        recorded.iter().all(|e| !e.to_string().contains("Foo.configure")),
        "a ledger line must not be an ingest gap (attribution would downgrade real errors): {recorded:?}"
    );
    let ledger: Vec<_> = diags
        .iter()
        .filter(|d| matches!(d.kind, DiagnosticKind::UnconsumedInitializer { .. }))
        .collect();
    assert_eq!(ledger.len(), 1);
    assert!(diags.iter().all(|d| d.severity != Severity::Error));
    let summary = roundhouse::ingest::initializer_ledger::render_summary(&diags);
    assert!(summary.contains("Initializer ledger: 1 unconsumed statement(s)"), "{summary}");
    assert!(summary.contains("[1×] c.a ="), "{summary}");
    assert!(summary.contains("by owner: unknown 1"), "{summary}");
}

#[test]
fn the_inflections_probe_records_no_survey_gap_of_its_own() {
    // An inflections statement holding a construct `ingest_expr` rejects is
    // recorded once, by inflections ingest, against the real file. The
    // ledger's own probe of that statement must not record it again.
    survey::activate();
    let (app, _diags) = prism::scope(|| {
        ingest_app_from_tree(tree(&[
            ("db/schema.rb", SCHEMA),
            (
                "config/initializers/inflections.rb",
                "ActiveSupport::Inflector.inflections(:en) do |inflect|\n  inflect.acronym(\"API\") if (a = 1)..(a == 2)\n  inflect.acronym([1].each { redo }.to_s)\nend\n",
            ),
        ]))
    });
    let recorded = survey::drain();
    app.expect("ingest");
    assert!(
        recorded.iter().all(|e| !format!("{e:?}").contains("initializer_ledger")),
        "the ledger probe recorded a gap of its own: {recorded:?}"
    );
}

#[test]
fn the_recognizer_table_names_are_unique() {
    let names = roundhouse::ingest::initializer_ledger::recognizer_names();
    let unique: std::collections::HashSet<_> = names.iter().collect();
    assert_eq!(unique.len(), names.len(), "{names:?}");
}

#[test]
fn only_an_initializer_that_really_loads_an_ignored_lib_dir_vouches_for_it() {
    let application = (
        "config/application.rb",
        "module Shop\n  class Application < Rails::Application\n    config.autoload_lib(ignore: %w[assets tasks])\n  end\nend\n",
    );
    // campfire's shape: the directory is a literal, and it is `require`d.
    let loads = (
        "config/initializers/extensions.rb",
        "%w[ assets ].each do |dir|\n  Dir[\"#{Rails.root}/lib/#{dir}/*\"].each { |path| require \"#{dir}/#{File.basename(path)}\" }\nend\n",
    );
    let (_, ledger) = ingest(&[application, loads]);
    assert!(ledger.is_empty(), "{:?}", statements(&ledger));
    // A configuration block that merely says "require" and "assets" does not:
    // the recognizer's substring test would have counted it.
    let incidental = (
        "config/initializers/setup.rb",
        "Gadget.setup do |config|\n  config.assets = true\n  config.require_login = false\nend\n",
    );
    let (_, ledger) = ingest(&[application, incidental]);
    assert_eq!(statements(&ledger), ["config.assets =", "config.require_login ="]);
}

#[test]
fn a_statement_the_ledger_calls_consumed_has_its_recognizers_effect_on_the_app() {
    // The ledger asks the recognizers' own code, so "consumed" must mean
    // the App changed. One effect per recognizer family.
    let (app, ledger) = ingest(&[
        (
            "config/application.rb",
            "module Shop\n  class Application < Rails::Application\n  end\nend\n",
        ),
        ("config/initializers/formats.rb", "Time::DATE_FORMATS[:short_date] = \"%b %e\"\n"),
        ("config/initializers/filters.rb", "Target.before_action :guard, only: [:show]\n"),
        ("config/initializers/headers.rb", "Target.after_action { headers[\"X-Seen\"] = \"1\" }\n"),
        (
            "config/initializers/sqlite.rb",
            "conn.create_function(\"regexp\", 2) do |fn, pattern, expr|\n  fn.result = 1\nend\n",
        ),
        ("config/initializers/kaminari.rb", "Kaminari.configure do |config|\n  config.default_per_page = 10\nend\n"),
        ("config/initializers/lift.rb", "Rails.application.config.app_version = \"1\"\n"),
        ("config/initializers/cable.rb", "Rails.application.config.action_cable.allow_same_origin_as_host = false\n"),
    ]);
    assert!(ledger.is_empty(), "{:?}", statements(&ledger));
    assert!(!app.time_formats.is_empty());
    assert_eq!(app.initializer_filters.len(), 1);
    assert_eq!(app.initializer_response_headers.len(), 1);
    assert_eq!(app.sql_functions.len(), 1);
    let names: Vec<&str> = app
        .rails_application
        .as_ref()
        .expect("Rails::Application reopen")
        .methods
        .iter()
        .map(|m| m.name.as_str())
        .collect();
    assert!(
        names.contains(&"default_per_page")
            && names.contains(&"app_version")
            && names.contains(&"action_cable_allow_same_origin_as_host"),
        "{names:?}"
    );
}

const HOLDER_LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    devise (4.9.4)\n    doorkeeper (5.8.0)\n    lograge (0.14.0)\n    omniauth (2.1.2)\n    rack (3.1.8)\n    rack-attack (6.7.0)\n    railties (8.0.1)\n\nDEPENDENCIES\n  devise\n  doorkeeper\n  lograge\n  omniauth\n  rack-attack\n  railties\n";

#[test]
fn each_dropped_statement_names_who_it_belongs_to() {
    let (_, ledger) = ingest(&[
        ("Gemfile.lock", HOLDER_LOCK),
        ("app/models/audit.rb", "class Audit
  def self.record_boot; end
end
"),
        (
            "config/initializers/boot.rb",
            "Rails.application.config.middleware.use Rack::Deflater
             Rails.application.config.assets.paths << \"vendor\"
             Rails.autoloaders.main.ignore(\"lib/tasks\")
             Devise.setup do |config|
  config.password_length = 8..128
end
             Doorkeeper.configure do
  orm :active_record
end
             Rack::Attack.throttle(\"req/ip\", limit: 300, period: 300) { |req| req.ip }
             OmniAuth.config.logger = Rails.logger
             Audit.record_boot
             Rails.application.config.after_initialize { |app| Audit.record_boot }
             if ENV[\"SLOW\"]
  Rails.application.config.middleware.insert_before 0, Rack::Runtime
end
             Unheard::Of.start
",
        ),
    ]);
    let got: Vec<(&str, &str)> = ledger.iter().map(|d| (statement(d), holder(d))).collect();
    assert_eq!(
        got,
        [
            ("Rails.application.config.middleware.use", "rails"),
            ("Rails.application.config.assets.paths.<<", "rails-boot-only"),
            ("Rails.autoloaders.main.ignore", "rails-boot-only"),
            ("config.password_length =", "gem:devise:unknown"),
            ("orm :active_record", "gem:doorkeeper:unknown"),
            ("Rack::Attack.throttle", "gem:rack-attack:unknown"),
            ("OmniAuth.config.logger =", "gem:omniauth:unknown"),
            // Not listed: an app-owned top-level call is no longer dropped, it runs at boot (`ingest::boot_program`).
            ("Rails.application.config.after_initialize", "app"),
            ("if ENV[\"SLOW\"]", "rails"),
            ("Unheard::Of.start", "unknown"),
        ]
    );
    let middleware = &ledger[0];
    assert_eq!(middleware.severity, Severity::Warning);
    assert!(middleware.message.ends_with("it is Rails behaviour the running app answers without"), "{}", middleware.message);
    assert!(ledger[5].message.ends_with("it belongs to the gem `rack-attack` (unknown)"), "{}", ledger[5].message);
}

#[test]
fn a_rooted_configure_block_and_an_in_repo_gem_keep_their_gems() {
    let (_, ledger) = ingest(&[
        ("Gemfile.lock", "PATH\n  remote: vendor/catalog\n  specs:\n    catalog (0.1.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n    devise (4.9.4)\n\nDEPENDENCIES\n  catalog!\n  devise\n"),
        (
            "config/initializers/boot.rb",
            "::Devise.setup do |config|\n  config.password_length = 8..128\nend\nCatalog.configure do |c|\n  c.size = 3\nend\n",
        ),
    ]);
    let got: Vec<(&str, &str)> = ledger.iter().map(|d| (statement(d), holder(d))).collect();
    assert_eq!(got, [("config.password_length =", "gem:devise:unknown"), ("c.size =", "gem:catalog:in-repo")]);
}

#[test]
fn a_filter_parameters_list_with_a_regexp_and_comments_is_read() {
    let (_, ledger) = ingest(&[(
        "config/initializers/filter_parameter_logging.rb",
        "Rails.application.config.filter_parameters += [\n  # keys named like an API key, not their ids\n  :passw,\n  /\\bapi_key(?!_id\\b)/,\n]\n\
         Rails.application.config.filter_parameters += [/(?:card|cvv)[_.]code{1,2}/, :pin]\n\
         Rails.application.config.filter_parameters += [/#{Secrets.prefix}_key/]\n",
    )]);
    // Plain Regexps are read, `,` and `]` inside them included; an
    // interpolated one could be anything.
    assert_eq!(statements(&ledger), ["Rails.application.config.filter_parameters +="]);
    assert_eq!(location(&ledger[0]), "config/initializers/filter_parameter_logging.rb:7:1");
}

#[test]
fn a_filter_parameters_operand_after_a_comment_or_blank_line_is_read() {
    let (_, ledger) = ingest(&[(
        "config/initializers/filter_parameter_logging.rb",
        "Rails.application.config.filter_parameters +=\n  # credentials\n  [:password]\n\
         Rails.application.config.filter_parameters +=\n\n  [:token]\n\
         Rails.application.config.filter_parameters += [\n  # secrets\n  :secret,\n]\n",
    )]);
    assert!(ledger.is_empty(), "{:?}", statements(&ledger));
}

#[test]
fn a_constant_a_class_body_reads_stays_on_the_ledger_since_initializers_load_after_app() {
    let (app, ledger) = ingest(&[
        ("config/initializers/settings.rb", "SETTINGS = { default: 3 }.freeze\nGREETING = \"hi\"\n"),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\n  DEFAULT = SETTINGS[:default]\n  def greeting = GREETING\nend\n"),
    ]);
    assert!(statements(&ledger).iter().any(|s| s.starts_with("SETTINGS")), "{:?}", statements(&ledger));
    assert!(!statements(&ledger).iter().any(|s| s.starts_with("GREETING")), "{:?}", statements(&ledger));
    assert_eq!(app.boot_program.len(), 1);
}

#[test]
fn a_condition_on_a_rooted_or_quoted_constant_stays_on_the_ledger() {
    let (app, ledger) = ingest(&[(
        "config/initializers/guards.rb",
        "if ::Gizmo.ready?\n  READY = true\nend\nif \"'\" == \"x\" && Widgetry.on?\n  ON = true\nend\nif ENV[\"BOOT\"]\n  BOOTED = true\nend\n",
    )]);
    assert_eq!(statements(&ledger), ["if ::Gizmo.ready?", "if \"'\" == \"x\" && Widgetry.on?"]);
    assert_eq!(app.boot_program.len(), 1);
}

#[test]
fn a_constant_a_class_body_reads_stays_on_the_ledger_when_a_conditional_assigns_it() {
    let (app, ledger) = ingest(&[
        ("config/initializers/settings.rb", "if ENV[\"BOOT\"]\n  SETTINGS = {}\nend\n"),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\n  DEFAULT = SETTINGS\nend\n"),
    ]);
    assert_eq!(statements(&ledger), ["if ENV[\"BOOT\"]"]);
    assert!(app.boot_program.is_empty());
}

#[test]
fn a_constant_a_conditional_assigns_does_not_admit_a_later_mutation() {
    let (app, ledger) = ingest(&[(
        "config/initializers/flags.rb",
        "if ENV[\"BOOT\"]\n  FLAGS = []\nend\nFLAGS << :late\n",
    )]);
    assert_eq!(statements(&ledger), ["FLAGS.<<"]);
    assert_eq!(app.boot_program.len(), 1);
}

/// Rails registers a hook under `if defined?(Rails::Server)` under `rails
/// server` only; it is carried as a server-only callback
/// (`ingest::after_initialize`), so the ledger no longer reports it, and it
/// is still no statement of the boot program.
#[test]
fn a_hook_under_the_server_guard_is_carried_off_the_ledger() {
    let (app, ledger) = ingest(&[(
        "config/initializers/recover_exports.rb",
        "if defined?(Rails::Server)\n  Rails.application.config.after_initialize do\n    BootProbe.recover_pending!\n  end\nend\n",
    )]);
    assert!(ledger.is_empty(), "{:?}", statements(&ledger));
    assert!(app.runs_after_initialize);
    assert!(app.boot_program.is_empty(), "{:?}", app.boot_program);
}

/// The other spellings of that guard: rooted, as a modifier, and around
/// two hooks. Each is carried, server-only, and nothing is ledgered.
#[test]
fn the_server_guard_is_carried_rooted_as_a_modifier_and_around_several_hooks() {
    let hook = "Rails.application.config.after_initialize do\n  BootProbe.recover_pending!\nend\n";
    for (name, source) in [
        ("rooted", format!("if defined?(::Rails::Server)\n{hook}end\n")),
        ("then", format!("if defined? Rails::Server then\n{hook}end\n")),
        ("modifier", format!("Rails.application.config.after_initialize do\n  BootProbe.recover_pending!\nend if defined?(Rails::Server)\n")),
        ("several", format!("if defined?(Rails::Server)\n{hook}{hook}end\n")),
    ] {
        let (app, ledger) = ingest(&[("config/initializers/server_hooks.rb", &source)]);
        assert!(ledger.is_empty(), "{name}: {:?}", statements(&ledger));
        assert!(app.runs_after_initialize, "{name}");
        assert!(app.boot_program.is_empty(), "{name}: {:?}", app.boot_program);
    }
}

/// Only that exact guard is carried. An `else` or `elsif`, `unless`,
/// another predicate or constant, a mixed body, a hook inside `configure`,
/// a hook whose block takes the app, and a hook with a static part (a
/// reopen or a `require`, which would apply in every process) each stay on
/// the ledger, and nothing runs.
#[test]
fn a_hook_under_any_other_guard_stays_on_the_ledger() {
    let hook = "  Rails.application.config.after_initialize do\n    BootProbe.recover_pending!\n  end\n";
    let guarded = |body: &str| format!("if defined?(Rails::Server)\n{body}end\n");
    for (name, source) in [
        ("else", format!("if defined?(Rails::Server)\n{hook}else\n  BOOTED = true\nend\n")),
        ("elsif", format!("if defined?(Rails::Server)\n{hook}elsif ENV[\"SERVE\"]\n{hook}end\n")),
        ("unless", format!("unless defined?(Rails::Server)\n{hook}end\n")),
        ("console", format!("if defined?(Rails::Console)\n{hook}end\n")),
        ("nested constant", format!("if defined?(Boot::Rails::Server)\n{hook}end\n")),
        ("and", format!("if defined?(Rails::Server) && ENV[\"SERVE\"]\n{hook}end\n")),
        ("mixed", guarded(&format!("{hook}  BootProbe.warm!\n"))),
        ("configure", guarded("  Rails.application.configure do\n    config.after_initialize do\n      BootProbe.recover_pending!\n    end\n  end\n")),
        ("block parameter", guarded("  Rails.application.config.after_initialize do |app|\n    BootProbe.recover_pending!(app)\n  end\n")),
        ("reopen", guarded("  Rails.application.config.after_initialize do\n    BootProbe.module_eval do\n      def self.ready? = true\n    end\n  end\n")),
        ("require", guarded("  Rails.application.config.after_initialize do\n    require \"ext/boot_patch\"\n  end\n")),
    ] {
        let (app, ledger) = ingest(&[("config/initializers/guarded.rb", &source)]);
        assert_eq!(ledger.len(), 1, "{name}: {:?}", statements(&ledger));
        assert!(!app.runs_after_initialize, "{name}");
        assert!(app.boot_program.is_empty(), "{name}: {:?}", app.boot_program);
    }
}

#[test]
fn a_hook_name_outside_a_guard_branch_is_not_a_hook() {
    let (app, _) = ingest(&[
        (
            "config/initializers/limits.rb",
            "LIMITS = Limits.configure { |c| c }\n\ndef reconfigure_limits\n  Limits.configure { |c| c }\nend\n",
        ),
        ("app/models/limits.rb", "class Limits\n  def self.configure\n    yield 1\n  end\nend\n"),
    ]);
    assert_eq!(app.boot_program.len(), 1, "{:?}", app.boot_program);
    assert!(
        app.library_classes.iter().any(|lc| lc.name.0.as_str() == "RoundhouseInitializers"
            && lc.methods.iter().any(|m| m.name.as_str() == "reconfigure_limits")),
        "the def is still taken"
    );
}

#[test]
fn a_hook_in_a_case_branch_stays_on_the_ledger() {
    let (app, ledger) = ingest(&[(
        "config/initializers/by_env.rb",
        "case ENV[\"MODE\"]\nwhen \"server\"\n  Rails.application.config.after_initialize do\n    BootProbe.recover_pending!\n  end\nend\n",
    )]);
    assert!(!ledger.is_empty(), "the hook is reported");
    assert!(app.boot_program.is_empty(), "{:?}", app.boot_program);
}

/// The app's own `config.middleware.use` of a gem compiled from source is stacked, so it is no
/// dropped statement; one of a class no source gem defines still is.
#[test]
fn a_stacked_application_middleware_use_is_not_dropped() {
    let (app, ledger) = ingest(&[
        ("Gemfile.lock", "GEM\n  remote: https://rubygems.org/\n  specs:\n    warden (1.2.9)\n\nDEPENDENCIES\n  warden\n"),
        (".bundle/config", "---\nBUNDLE_PATH: \"vendor/bundle\"\n"),
        (
            "vendor/bundle/ruby/3.4.0/gems/warden-1.2.9/lib/warden.rb",
            "module Warden\n  class Proxy\n    def initialize(env)\n      @env = env\n    end\n  end\n  class Manager\n    def initialize(app)\n      @app = app\n    end\n\n    def call(env)\n      env[\"warden\"] = Proxy.new(env)\n      @app.call(env)\n    end\n  end\nend\n",
        ),
        (
            "config/application.rb",
            "module Blog\n  class Application < Rails::Application\n    config.middleware.use Rack::Deflater\n    config.middleware.use Warden::Manager\n  end\nend\n",
        ),
    ]);
    assert_eq!(app.gem_middlewares, ["Warden::Manager"]);
    assert_eq!(app.rack_env_writes.get("warden").map(String::as_str), Some("Warden::Proxy"));
    let got: Vec<&str> = ledger.iter().map(|d| statement(d)).collect();
    assert_eq!(got, ["config.middleware.use"], "{ledger:#?}");
    assert!(ledger[0].message.contains("config/application.rb:3"), "{}", ledger[0].message);
}
