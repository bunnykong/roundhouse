//! Prism → Roundhouse IR.
//!
//! Reads Ruby source (a single file or a Rails app directory) and produces an
//! [`App`](crate::App). This is the reverse of [`crate::emit::ruby`]; together
//! they form the round-trip forcing function.
//!
//! Scope for the initial landing is the tiny-blog fixture: a single model, a
//! single controller with one action, a trivial routes file, and a schema.
//! The ingester deliberately panics on unrecognized constructs — a failed
//! ingest is a signal that the IR (or the recognizer) needs to grow.
//!
//! Organized one submodule per Rails concern: [`app`] orchestrates the
//! whole-directory walk, and [`model`], [`controller`], [`routes`],
//! [`schema`], [`view`], [`test`], [`fixture`] each handle a single source
//! type. The expression-level recursive descent lives in [`expr`]; small
//! cross-cutting Prism AST helpers live in [`util`].

mod alba;
mod built_send;
mod conditional_permit;
pub(crate) mod unreachable_views;
mod view_paths;
mod uncalled_helper_copies;
mod written_names;
mod pagy;
pub(crate) mod gem_package_settings;
mod graphql_ruby;
mod guarded_include;
mod lock_versions;
pub(crate) mod validates_with;
pub(crate) mod each_validator;
pub(crate) mod mailer_callbacks;
pub(crate) mod mailer_defaults;
pub(crate) mod has_many_hold;
pub(crate) mod mailer_helpers;
pub(crate) mod class_attribute;
mod class_configuration;
pub(crate) use concern_accessors::{library_classes_by_name, unconsumed_class_side_hook, LibraryClassesByName};
mod dry_initializer;
mod dry_struct;
mod dry_types;
mod oj;
pub(crate) mod source_gems;
mod auto_source_gems;
mod aliased_includes;
mod forwardable;
mod railtie_middleware;
pub(crate) mod absence_guards;
mod stdlib_features;
pub(crate) mod source_gem_autoload;
mod metal_actions;
mod uncalled_definers;
mod job_class_lookup;
mod settled_conditionals;
mod dry_monads;
mod dry_validation;
mod representable;
mod active_model_serializer;
mod action_cable_config;
mod recurring_commands;
mod session_config;
pub(crate) mod database_ownership;
pub(crate) mod database_yml;
pub mod sidekiq_yml;
pub mod cable_yml;
pub(crate) mod config_for;
mod public_file_server_config;
mod action_controller_caching_config;
mod json_response_config;
mod active_job_config;
mod caches_page;
mod boot_program;
mod engine_config;
mod engine_helpers;
mod interpolated_renders;
mod container_specialize;
mod controller_class_methods;
mod controller_delegates;
mod active_model_super;
mod queue_adapter_reader;
mod library_concern_items;
mod unreached_gem_modules;
mod gem_load_program;
mod option_merger;
pub(crate) mod puma_boot;
pub mod allow_browser;
pub mod app;
mod concern_accessors;
pub mod controller;
mod class_new_factory;
mod data_factory;
mod file_constant_hoist;
mod module_factory;
mod define_method_loop;
pub mod expr;
pub mod fixture;
pub(crate) mod forwarding;
pub(crate) mod controller_macro_synth;
pub mod impersonates;
pub mod invisible_captcha;
pub mod generated_helpers;
pub mod jbuilder;
pub mod library_class;
mod file_program;
pub(crate) use file_program::{normalize_source, resolve_gem_require};
pub(crate) fn source_gems_mount() -> &'static str {
    source_gems::MOUNT
}
mod glob_autoload;
pub mod channel_callbacks;
mod active_model_attributes;
pub mod current_attributes;
pub mod delegate;
mod acts_as_tenant;
mod acts_as_votable;
mod discard;
mod model_callbacks;
mod model_delegate;
pub mod thread_mattr;
pub mod model;
mod delegated_type;
mod app_extension_calls;
mod body_statements;
mod error_site;
mod external_include;
mod hook_class_attribute;
pub(crate) mod inherited_alias;
mod class_macros;
mod configured_superclass;
mod gem_hooks;
pub(crate) use gem_hooks::method_missing_readers;
mod gem_routes;
mod column_accessors;
mod model_macros;
mod referenced_carriers;
mod extend_splice;
mod included_extend;
pub mod on_load_reopen;
pub mod prism;
pub(crate) mod prism_api;
mod propshaft;
pub mod rbi;
pub mod rate_limit;
pub mod inertia;
pub mod initializer_ledger;
pub mod roda_app;
pub mod routes;
pub mod singleton_class;
pub mod schema;
pub mod sequel_migration;
pub mod sequel_model;
pub mod sorbet_sig;
pub mod sources;
pub mod spec;
pub mod type_ascription;
pub mod sql_functions;
mod select_aliases;
pub mod structure_sql;
pub mod survey;
mod tableless_inheritance;
mod sti_validations;
mod with_options;
pub(crate) mod nested_attributes;
pub mod test;
pub(crate) mod rake_tasks;
pub mod util;
pub mod view;
pub mod vite;
pub mod current_page;
pub mod time_tag;
mod visibility;

pub use app::{ingest_app, ingest_app_from_tree, ingest_app_with_vfs};
pub use concern_accessors::{defer as defer_accessor_surface, DeferredAccessorSurface};
pub use puma_boot::preserve_config as preserve_puma_config;

/// The construct an ActiveRecord class outside app/models is reported
/// under (`app::report_unmodelled_records`): the CLI keys its per-target
/// handling on it (#1278).
pub const UNMODELLED_RECORD: &str = "ActiveRecord model outside app/models";
pub use controller::ingest_controller;
pub use expr::ingest_expr;
pub use fixture::ingest_fixture_file;
pub use jbuilder::ingest_jbuilder;
pub use library_class::{
    classify_class_file, ingest_library_class, ingest_library_classes,
    ingest_nested_library_classes, ClassKind,
};
pub use model::ingest_model;
pub use roda_app::{ingest_roda_app_with_vfs, is_roda_app};
pub use routes::ingest_routes;
pub use schema::{
    ingest_migration, ingest_migration_with_generated_expression_dialect, ingest_schema,
    ingest_schema_with_generated_expression_dialect,
};
pub use sequel_migration::ingest_sequel_migration;
pub use sequel_model::ingest_sequel_model;
pub use structure_sql::{
    ingest_structure_sql, ingest_structure_sql_with_generated_expression_dialect,
};
pub use test::{ingest_test_file, ingest_test_files};
pub use view::ingest_view;

// Errors ----------------------------------------------------------------

#[derive(Debug)]
pub enum IngestError {
    Io(std::io::Error),
    Parse { file: String, message: String },
    Unsupported { file: String, message: String },
}

impl std::fmt::Display for IngestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::Parse { file, message } => write!(f, "parse error in {file}: {message}"),
            Self::Unsupported { file, message } => {
                write!(f, "unsupported construct in {file}: {message}")
            }
        }
    }
}

impl std::error::Error for IngestError {}

/// Render the current failure before another ingest replaces its noted source site.
pub fn render_error(err: &IngestError) -> String {
    if let IngestError::Unsupported { file, message } = err
        && let Some((start, _)) = error_site::of(err)
        && let Some(line) = sources::line_at(file, start) {
        return format!("unsupported construct in {file}:{line}: {message}");
    }
    err.to_string()
}

impl From<std::io::Error> for IngestError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

type IngestResult<T> = Result<T, IngestError>;

/// An empty reopen of `Rails::Application`, which ingest fills with the
/// methods it lifts from the app's config.
pub(crate) fn rails_application_class() -> crate::dialect::LibraryClass {
    crate::dialect::LibraryClass {
        name: crate::ident::ClassId(crate::ident::Symbol::from("Rails::Application")),
        is_module: false,
        parent: None,
        parent_span: Default::default(),
        includes: Vec::new(),
        include_spans: Vec::new(),
        methods: Vec::new(),
        nullable_columns: Vec::new(),
        origin: None,
        constants: Vec::new(),
        conditional_definitions: Vec::new(),
        unknown_calls: Vec::new(),
        class_ivar_initializers: Vec::new(),
    }
}
