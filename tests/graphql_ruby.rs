//! graphql-ruby object types (`ingest::graphql_ruby`, `analyze::graphql`).
//!
//! howtographql's `check` used to report nothing under `app/graphql`:
//! library class bodies are outside `diagnose`, and a type's `object`
//! had no type. The ingest pass gives each field the method graphql-ruby
//! would resolve it through, so inference carries the record class from
//! the schema's root down, and `check` reports a field with nothing
//! behind it and a `null: false` field that can be nil.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::{diagnose, Analyzer};
use roundhouse::diagnostic::{Diagnostic, DiagnosticKind, Severity};
use roundhouse::dialect::GraphqlResolution;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::App;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :users do |t|\n    t.string :name, null: false\n    t.string :nickname\n  end\n  \
    create_table :posts do |t|\n    t.string :title, null: false\n    \
    t.integer :user_id, null: false\n    t.integer :editor_id\n    t.integer :board_id, null: false\n  end\n  \
    create_table :boards do |t|\n    t.string :name, null: false\n  end\n  \
    add_foreign_key :posts, :users\n  add_foreign_key :posts, :users, column: :editor_id\nend\n";

const BASE: &[(&str, &str)] = &[
    ("db/schema.rb", SCHEMA),
    (
        "config/routes.rb",
        "Rails.application.routes.draw do\n  post '/graphql', to: 'graphql#execute'\nend\n",
    ),
    (
        "app/models/application_record.rb",
        "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
    ),
    ("app/models/user.rb", "class User < ApplicationRecord\n  has_many :posts\nend\n"),
    (
        "app/models/post.rb",
        "class Post < ApplicationRecord\n  belongs_to :user\n  \
         belongs_to :editor, class_name: \"User\", optional: true\n  belongs_to :board\nend\n",
    ),
    ("app/models/board.rb", "class Board < ApplicationRecord\nend\n"),
    (
        "app/controllers/graphql_controller.rb",
        "class GraphqlController < ActionController::Base\n  def execute\n    \
         render json: AppSchema.execute(params[:query])\n  end\nend\n",
    ),
    (
        "app/graphql/app_schema.rb",
        "class AppSchema < GraphQL::Schema\n  query Types::QueryType\nend\n",
    ),
    (
        "app/graphql/types/base_object.rb",
        "module Types\n  class BaseObject < GraphQL::Schema::Object\n  end\nend\n",
    ),
    (
        "app/graphql/types/query_type.rb",
        "module Types\n  class QueryType < BaseObject\n    \
         field :posts, [PostType], null: false\n\n    def posts\n      Post.all\n    end\n  end\nend\n",
    ),
];

fn post_type(fields: &str) -> String {
    format!("module Types\n  class PostType < BaseObject\n{fields}  end\nend\n")
}

const USER_TYPE: &str = "module Types\n  class UserType < BaseObject\n    \
    field :name, String, null: false\n    field :posts, [PostType], null: false\n  end\nend\n";

fn analyzed(extra: &[(&str, &str)]) -> App {
    let mut files: HashMap<&str, &str> = BASE.iter().copied().collect();
    files.insert("app/graphql/types/user_type.rb", USER_TYPE);
    files.extend(extra.iter().copied());
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .into_iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    // As `check` does: analyze, no lowering.
    Analyzer::new(&app).analyze(&mut app);
    app
}

/// Diagnostics anchored in `app/graphql`, as `(code, message)`.
fn graphql_diagnostics(app: &App) -> Vec<(String, String)> {
    diagnose(app)
        .into_iter()
        .filter(|d| in_graphql(app, d))
        .map(|d| (d.code().to_owned(), d.message.clone()))
        .collect()
}

fn in_graphql(app: &App, d: &Diagnostic) -> bool {
    let index = d.span.file.0 as usize;
    index > 0 && app.sources[index - 1].path.contains("app/graphql/")
}

fn nullable_fields(app: &App) -> Vec<String> {
    diagnose(app)
        .into_iter()
        .filter_map(|d| match &d.kind {
            DiagnosticKind::GraphqlNullableField { field, .. } => {
                assert_eq!(d.severity, Severity::Warning, "{d}");
                Some(field.as_str().to_owned())
            }
            _ => None,
        })
        .collect()
}

#[test]
fn a_field_backed_by_a_not_null_column_or_association_is_clean() {
    let post = post_type(
        "    field :title, String, null: false\n    \
         field :author, UserType, null: false, method: :user\n",
    );
    let app = analyzed(&[("app/graphql/types/post_type.rb", &post)]);
    assert!(
        graphql_diagnostics(&app).is_empty(),
        "{:?}",
        graphql_diagnostics(&app)
    );
}

/// The record class reaches each type from the root, cycles included:
/// `PostType` wraps a `Post`, `UserType` a `User`.
#[test]
fn the_object_type_flows_from_the_root() {
    let post = post_type("    field :author, UserType, null: false, method: :user\n");
    let app = analyzed(&[("app/graphql/types/post_type.rb", &post)]);
    let object = |class: &str| {
        let lc = app
            .library_classes
            .iter()
            .find(|c| c.name.0.as_str() == class)
            .unwrap();
        let m = lc
            .methods
            .iter()
            .find(|m| m.name.as_str() == "object")
            .unwrap();
        format!("{:?}", m.signature)
    };
    assert!(
        object("Types::PostType").contains("\"Post\""),
        "{}",
        object("Types::PostType")
    );
    assert!(
        object("Types::UserType").contains("\"User\""),
        "{}",
        object("Types::UserType")
    );
}

#[test]
fn a_non_null_field_on_a_nullable_column_warns() {
    let user = "module Types\n  class UserType < BaseObject\n    \
        field :nickname, String, null: false\n    field :posts, [PostType], null: false\n  end\nend\n";
    let post = post_type("    field :author, UserType, null: false, method: :user\n");
    let app = analyzed(&[
        ("app/graphql/types/post_type.rb", &post),
        ("app/graphql/types/user_type.rb", user),
    ]);
    assert_eq!(nullable_fields(&app), ["nickname"]);
    // Declared nullable: nothing to report.
    let user = user.replace(
        "null: false\n    field :posts",
        "null: true\n    field :posts",
    );
    let app = analyzed(&[
        ("app/graphql/types/post_type.rb", &post),
        ("app/graphql/types/user_type.rb", &user),
    ]);
    assert!(nullable_fields(&app).is_empty());
}

/// A `belongs_to` is non-nil for a stored row only when the database
/// says so: NOT NULL and a foreign key. `editor` is optional and
/// nullable; `board` is NOT NULL with no foreign key.
#[test]
fn a_belongs_to_is_proven_only_by_not_null_and_a_foreign_key() {
    let post = post_type(
        "    field :author, UserType, null: false, method: :user\n    \
         field :editor, UserType, null: false\n    field :board_name, String, null: false\n\n    \
         def board_name\n      object.board&.name\n    end\n",
    );
    let app = analyzed(&[("app/graphql/types/post_type.rb", &post)]);
    assert_eq!(nullable_fields(&app), ["editor", "board_name"]);
}

/// graphql-ruby's "Failed to implement": neither the type nor the
/// object has the method. Reported at the `field` call.
#[test]
fn a_field_with_nothing_behind_it_is_a_dispatch_failure() {
    let post = post_type("    field :headline, String, null: false\n");
    let app = analyzed(&[("app/graphql/types/post_type.rb", &post)]);
    let found = graphql_diagnostics(&app);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, "send_dispatch_failed");
    assert!(found[0].1.contains("`headline` on Post"), "{found:?}");
}

/// The bodies of the type's own methods are checked like a controller's.
#[test]
fn a_type_method_body_is_checked() {
    let post = post_type(
        "    field :score, Integer, null: false\n\n    def score\n      object.title + 1\n    end\n",
    );
    let app = analyzed(&[("app/graphql/types/post_type.rb", &post)]);
    let codes: Vec<String> = graphql_diagnostics(&app)
        .into_iter()
        .map(|(c, _)| c)
        .collect();
    assert_eq!(codes, ["incompatible_binop"]);
}

/// search_object's `scope { … }` is what a resolver class answers.
#[test]
fn a_search_object_resolver_carries_its_scope() {
    let query = "module Types\n  class QueryType < BaseObject\n    \
        field :posts, resolver: Resolvers::PostsSearch\n  end\nend\n";
    let resolver = "module Resolvers\n  class PostsSearch < GraphQL::Schema::Resolver\n    \
        scope { Post.all }\n    type [Types::PostType]\n  end\nend\n";
    let post = post_type("    field :headline, String, null: false\n");
    let app = analyzed(&[
        ("app/graphql/types/query_type.rb", query),
        ("app/graphql/resolvers/posts_search.rb", resolver),
        ("app/graphql/types/post_type.rb", &post),
    ]);
    let found = graphql_diagnostics(&app);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].1.contains("`headline` on Post"), "{found:?}");
}

/// No `argument` declared: graphql-ruby calls the method with no
/// keywords, so an optional one takes its default. `length` is nil
/// here, and `length[:x]` raises on every request for the field.
#[test]
fn an_undeclared_optional_parameter_takes_its_default() {
    let post = post_type(
        "    field :excerpt, String, null: false\n\n    \
         def excerpt(length: nil)\n      length[:x]\n    end\n",
    );
    let app = analyzed(&[("app/graphql/types/post_type.rb", &post)]);
    let found = graphql_diagnostics(&app);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].1.contains("`[]` on nil"), "{found:?}");
}

/// A type nothing constructs has no known object, so nothing is claimed.
#[test]
fn an_unreachable_type_reports_nothing() {
    let post = post_type("    field :author, UserType, null: false, method: :user\n");
    let orphan = "module Types\n  class OrphanType < BaseObject\n    \
        field :anything, String, null: false\n  end\nend\n";
    let app = analyzed(&[
        ("app/graphql/types/post_type.rb", &post),
        ("app/graphql/types/orphan_type.rb", orphan),
    ]);
    assert!(
        graphql_diagnostics(&app).is_empty(),
        "{:?}",
        graphql_diagnostics(&app)
    );
}

/// The synthesized methods are for the analyzer: lowering removes them.
#[test]
fn lowering_removes_the_synthesized_methods() {
    let post = post_type(
        "    field :title, String, null: false\n\n    def shout\n      object.title.upcase\n    end\n",
    );
    let mut app = analyzed(&[("app/graphql/types/post_type.rb", &post)]);
    roundhouse::session::analyze_and_lower(&mut app);
    let lc = app
        .library_classes
        .iter()
        .find(|c| c.name.0.as_str() == "Types::PostType")
        .unwrap();
    let names: Vec<&str> = lc.methods.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["shout"]);
}

/// A method an included app module defines is the type's own.
#[test]
fn a_method_from_an_included_app_module_resolves_the_field() {
    let concern = "module PostFields\n  def headline\n    object.title.upcase\n  end\nend\n";
    let post = "module Types\n  class PostType < BaseObject\n    include PostFields\n\n    \
        field :headline, String, null: false\n  end\nend\n";
    let app = analyzed(&[
        ("app/graphql/post_fields.rb", concern),
        ("app/graphql/types/post_type.rb", post),
    ]);
    assert!(
        graphql_diagnostics(&app).is_empty(),
        "{:?}",
        graphql_diagnostics(&app)
    );
}

/// A module computed at load time (`include Resolvers.for(:post)`), or
/// one the app does not define, may hold the method: a field with none
/// in sight is skipped, not reported as missing on the record. The
/// computed form is never added to `includes` at all (it's replayed as
/// an unknown call instead), so it reports nothing; the named-but-
/// unresolved form is a real `include` refusal — #58 gave it a location,
/// so it now surfaces here as the one diagnostic in this file, with
/// `headline`/`title` still skipped rather than reported missing.
#[test]
fn an_include_out_of_sight_makes_unanswered_fields_skipped() {
    for (include, expect_diagnostic) in [
        ("include Resolvers.for(:post)", false),
        ("include SomeGem::PostFields", true),
    ] {
        let post = format!(
            "module Types\n  class PostType < BaseObject\n    {include}\n\n    \
             field :headline, String, null: false\n    field :title, String, null: false\n  end\nend\n"
        );
        let app = analyzed(&[("app/graphql/types/post_type.rb", &post)]);
        let found = graphql_diagnostics(&app);
        if expect_diagnostic {
            assert_eq!(found.len(), 1, "{include}: {found:?}");
            assert!(
                found[0].1.contains("Types::PostType includes unresolved SomeGem::PostFields"),
                "{include}: {found:?}"
            );
        } else {
            assert!(found.is_empty(), "{include}: {found:?}");
        }
        let post_type = app
            .graphql_types
            .iter()
            .find(|t| t.class.0.as_str() == "Types::PostType");
        let resolution = |name: &str| {
            post_type
                .unwrap()
                .fields
                .iter()
                .find(|f| f.name.as_str() == name)
                .unwrap()
                .resolution
                .clone()
        };
        assert!(
            matches!(resolution("headline"), GraphqlResolution::Skipped { .. }),
            "{include}"
        );
        assert!(
            matches!(resolution("title"), GraphqlResolution::Skipped { .. }),
            "{include}"
        );
    }
}

/// The summary `check` prints: a clean run's denominator, and the
/// reasons the rest was not followed.
#[test]
fn coverage_counts_checked_unreached_arguments_and_skipped() {
    let post = post_type(
        "    field :title, String, null: false\n    \
         field :excerpt, String, null: false\n\n    def excerpt(length: nil)\n      object.title\n    end\n    \
         field :tags, String, null: false, hash_key: :tags\n",
    );
    let orphan = "module Types\n  class OrphanType < BaseObject\n    \
        field :anything, String, null: false\n  end\nend\n";
    let app = analyzed(&[
        ("app/graphql/types/post_type.rb", &post),
        ("app/graphql/types/orphan_type.rb", orphan),
    ]);
    let coverage = roundhouse::analyze::graphql::coverage(&app).expect("graphql coverage");
    // Checked: QueryType.posts, PostType.title and .excerpt (called as
    // graphql-ruby would, with no keywords). Nothing here leads to
    // UserType (no author field) or OrphanType: their 3 are unreached.
    assert_eq!(
        coverage.summary(),
        "graphql: 4 object type(s), 7 field(s): 3 checked, 3 on types nothing reaches, \
         0 take arguments, 1 skipped (hash lookup 1)"
    );
}

const BASE_OBJECT_WITH_FIELD_CLASS: &str = "module Types\n  class BaseObject < GraphQL::Schema::Object\n    \
    field_class Types::BaseField\n  end\nend\n";

/// A field class consuming the app's own options, GitLab's shape, plus `body`.
fn base_field(body: &str) -> String {
    format!(
        "module Types\n  class BaseField < GraphQL::Schema::Field\n    \
         def initialize(**kwargs, &block)\n      \
         @authorize = kwargs.delete(:authorize)\n      @scopes = kwargs.delete(:scopes)\n      \
         @calls_gitaly = kwargs.delete(:calls_gitaly)\n      \
         @experiment = kwargs.delete(:experiment)\n      @deprecated = kwargs.delete(:deprecated)\n      \
         super(**kwargs, &block)\n    end\n{body}  end\nend\n"
    )
}

/// Coverage of one `post_type` field under the given field class.
fn title_with_option(field_class: &str, option: &str, extra: &[(&str, &str)]) -> String {
    let post = post_type(&format!("    field :title, String, null: false, {option}\n"));
    let mut files = vec![
        ("app/graphql/types/post_type.rb", post.as_str()),
        ("app/graphql/types/base_object.rb", BASE_OBJECT_WITH_FIELD_CLASS),
        ("app/graphql/types/base_field.rb", field_class),
    ];
    files.extend_from_slice(extra);
    coverage_line(&analyzed(&files))
}

/// Without a field class to consume it, nothing says what an option does.
#[test]
fn an_app_option_with_no_field_class_is_skipped() {
    let post = post_type("    field :title, String, null: false, authorize: :read_post\n");
    let line = coverage_line(&analyzed(&[("app/graphql/types/post_type.rb", &post)]));
    assert!(line.contains("`authorize:` option 1"), "{line}");
}

/// Whatever the option is called, a class that rewrites the value keyword
/// has moved the value.
#[test]
fn a_field_class_rewriting_a_value_keyword_vouches_for_nothing() {
    let class = "module Types\n  class BaseField < GraphQL::Schema::Field\n    \
        def initialize(**kwargs, &block)\n      @a = kwargs.delete(:authorize)\n      \
        kwargs[:method] = :other\n      super(**kwargs, &block)\n    end\n  end\nend\n";
    let line = title_with_option(class, "authorize: :read_post", &[]);
    assert!(line.contains("`authorize:` option 1"), "{line}");
}

#[test]
fn a_field_class_overriding_resolve_vouches_for_nothing() {
    let class = base_field("    def resolve(object, args, ctx)\n      nil\n    end\n");
    let line = title_with_option(&class, "authorize: :read_post", &[]);
    assert!(line.contains("`authorize:` option 1"), "{line}");
}

#[test]
fn a_field_class_prepending_a_module_vouches_for_nothing() {
    let class = base_field("    prepend SomeGem::Field\n");
    let line = title_with_option(&class, "authorize: :read_post", &[]);
    assert!(line.contains("`authorize:` option 1"), "{line}");
}

/// GitLab's `late_extensions:`: the option handed to `extension` can do
/// anything; the options beside it cannot.
#[test]
fn an_option_handed_to_extension_is_skipped_and_its_neighbours_are_not() {
    let class = "module Types\n  class BaseField < GraphQL::Schema::Field\n    \
        def initialize(**kwargs, &block)\n      @a = kwargs.delete(:authorize)\n      \
        late = kwargs.delete(:late_extensions) || []\n      super(**kwargs, &block)\n      \
        late.each { extension _1 }\n    end\n  end\nend\n";
    let moved = title_with_option(class, "late_extensions: [Foo]", &[]);
    assert!(moved.contains("`late_extensions:` option 1"), "{moved}");
    let kept = title_with_option(class, "authorize: :read_post", &[]);
    assert!(!kept.contains("option"), "{kept}");
}

const KEEPING_EXTENSION: &str = "class KeepExtension < GraphQL::Schema::FieldExtension\n  \
    def resolve(object:, arguments:, context:)\n    yield(object, arguments)\n  end\n  \
    def after_resolve(value:, **rest)\n    return value if value.nil?\n    value = value.to_a\n    value\n  end\nend\n";

const SWAPPING_EXTENSION: &str = "class SwapExtension < GraphQL::Schema::FieldExtension\n  \
    def after_resolve(value:, **rest)\n    value.to_s\n  end\nend\n";

/// An extension every field passes through is read, not assumed: one that
/// keeps the value is quiet, and one that may not is named on the line.
#[test]
fn a_field_class_extension_that_may_change_the_value_is_named() {
    let keeping = base_field("    def setup\n      extension KeepExtension\n    end\n");
    let line = title_with_option(
        &keeping,
        "authorize: :read_post",
        &[("app/graphql/keep_extension.rb", KEEPING_EXTENSION)],
    );
    assert!(!line.contains("not shown"), "{line}");
    let swapping = base_field("    def setup\n      extension SwapExtension\n    end\n");
    let line = title_with_option(
        &swapping,
        "authorize: :read_post",
        &[("app/graphql/swap_extension.rb", SWAPPING_EXTENSION)],
    );
    assert!(line.contains("not shown to keep the value: SwapExtension on"), "{line}");
}

/// `extension X` in a field's block counts only if X keeps the value.
#[test]
fn a_block_extension_is_read_like_a_field_class_one() {
    let with = |name: &str| {
        post_type(&format!(
            "    field :title, String, null: false do\n      extension {name}\n    end\n"
        ))
    };
    let kept = with("KeepExtension");
    let line = coverage_line(&analyzed(&[
        ("app/graphql/types/post_type.rb", &kept),
        ("app/graphql/keep_extension.rb", KEEPING_EXTENSION),
    ]));
    assert!(!line.contains("field block"), "{line}");
    let swapped = with("SwapExtension");
    let line = coverage_line(&analyzed(&[
        ("app/graphql/types/post_type.rb", &swapped),
        ("app/graphql/swap_extension.rb", SWAPPING_EXTENSION),
    ]));
    assert!(line.contains("field block 1"), "{line}");
}

/// `::Types::UserType` names the same type as `Types::UserType`, and its
/// fields are reached through it.
#[test]
fn a_rooted_field_type_is_followed() {
    let post = post_type("    field :user, ::Types::UserType, null: false\n");
    let app = analyzed(&[("app/graphql/types/post_type.rb", &post)]);
    let post_type = app
        .graphql_types
        .iter()
        .find(|t| t.class.0.as_str() == "Types::PostType")
        .expect("PostType");
    let user = post_type
        .fields
        .iter()
        .find(|f| f.name.as_str() == "user")
        .expect("user");
    assert_eq!(
        user.object_type.as_ref().map(|c| c.0.as_str()),
        Some("Types::UserType")
    );
    // A rooted name does not find a nested class of the same name.
    let nested = "module Types\n  module Types\n    class UserType < BaseObject\n    end\n  end\nend\n";
    let app = analyzed(&[
        ("app/graphql/types/post_type.rb", &post),
        ("app/graphql/types/types/user_type.rb", nested),
    ]);
    let post_type = app
        .graphql_types
        .iter()
        .find(|t| t.class.0.as_str() == "Types::PostType")
        .expect("PostType");
    assert_eq!(
        post_type
            .fields
            .iter()
            .find(|f| f.name.as_str() == "user")
            .and_then(|f| f.object_type.as_ref())
            .map(|c| c.0.as_str()),
        Some("Types::UserType")
    );
}

/// What a real schema writes on a field that does not move its value: who
/// may read it, a connection, a type passed by keyword, a built scalar.
/// None of it should cost the field its check.
#[test]
fn declarative_options_and_written_types_keep_a_field_checked() {
    let post = post_type(
        "    field :title, String, null: false, authorize: :read_post, scopes: [:api], \
         calls_gitaly: true, experiment: { milestone: '1.0' }, deprecated: { reason: 'x' }\n    \
         field :nickname, 'The title again', type: String, null: true, method: :title\n    \
         field :global_id, Types::GlobalIDType[Post], null: true, method: :id\n    \
         field :author_links, Types::UserType.connection_type, null: true, method: :user\n    \
         field :missing_thing, String, null: true, authorize: :read_post\n",
    );
    let app = analyzed(&[
        ("app/graphql/types/post_type.rb", &post),
        ("app/graphql/types/base_object.rb", BASE_OBJECT_WITH_FIELD_CLASS),
        ("app/graphql/types/base_field.rb", &base_field("")),
    ]);
    let post_type = app
        .graphql_types
        .iter()
        .find(|t| t.class.0.as_str() == "Types::PostType")
        .expect("PostType");
    for field in &post_type.fields {
        assert!(
            matches!(field.resolution, GraphqlResolution::Value { .. }),
            "{} was not modeled: {:?}",
            field.name.as_str(),
            field.resolution
        );
    }
    let connection = post_type
        .fields
        .iter()
        .find(|f| f.name.as_str() == "author_links")
        .expect("author_links");
    assert_eq!(
        connection.object_type.as_ref().map(|c| c.0.as_str()),
        Some("Types::UserType")
    );
    assert!(connection.list, "a connection answers many");
    // An `authorize:` on it does not excuse a field nothing backs.
    assert!(
        graphql_diagnostics(&app)
            .iter()
            .any(|(_, message)| message.contains("missing_thing")),
        "{:?}",
        graphql_diagnostics(&app)
    );
}

/// GitLab's `Resolvers::BaseResolver#object` is `super.tap { … }`: it
/// decorates the value, so a subclass still reads the record it was given.
#[test]
fn an_ancestor_object_through_super_keeps_a_resolver_modeled() {
    let base = "module Resolvers\n  class BaseResolver < GraphQL::Schema::Resolver\n    \
        def object\n      super.tap { |o| break o.itself if o.nil? }\n    end\n  end\nend\n";
    let resolver = "module Resolvers\n  class AuthorResolver < BaseResolver\n    \
        type Types::UserType, null: false\n    def resolve\n      object.user\n    end\n  end\nend\n";
    let post = post_type("    field :author, resolver: Resolvers::AuthorResolver\n");
    let app = analyzed(&[
        ("app/graphql/resolvers/base_resolver.rb", base),
        ("app/graphql/resolvers/author_resolver.rb", resolver),
        ("app/graphql/types/post_type.rb", &post),
        ("app/graphql/types/user_type.rb", USER_TYPE),
    ]);
    let post_type = app
        .graphql_types
        .iter()
        .find(|t| t.class.0.as_str() == "Types::PostType")
        .expect("PostType");
    let author = post_type
        .fields
        .iter()
        .find(|f| f.name.as_str() == "author")
        .expect("author");
    assert!(
        matches!(author.resolution, GraphqlResolution::Value { .. }),
        "{:?}",
        author.resolution
    );
    assert_eq!(
        author.object_type.as_ref().map(|c| c.0.as_str()),
        Some("Types::UserType")
    );
    let user = coverage_line(&app);
    assert!(!user.contains("resolver defines"), "{user}");
}

/// An `object` that does not go through `super` answers something else.
#[test]
fn an_object_override_without_super_stays_unread() {
    let base = "module Resolvers\n  class BaseResolver < GraphQL::Schema::Resolver\n    \
        def object\n      context[:other]\n    end\n  end\nend\n";
    let resolver = "module Resolvers\n  class AuthorResolver < BaseResolver\n    \
        type Types::UserType, null: false\n    def resolve\n      object.user\n    end\n  end\nend\n";
    let post = post_type("    field :author, resolver: Resolvers::AuthorResolver\n");
    let app = analyzed(&[
        ("app/graphql/resolvers/base_resolver.rb", base),
        ("app/graphql/resolvers/author_resolver.rb", resolver),
        ("app/graphql/types/post_type.rb", &post),
        ("app/graphql/types/user_type.rb", USER_TYPE),
    ]);
    assert!(coverage_line(&app).contains("resolver defines"), "{}", coverage_line(&app));
}

/// A failure on a type behind an extension that may swap its `object` is a
/// question about the analysis: it stays visible as a note, and names why.
#[test]
fn a_failure_behind_an_unproven_extension_is_a_note_naming_it() {
    let swapping = base_field("    def setup\n      extension SwapExtension\n    end\n");
    let post = post_type("    field :nothing_has_this, String, null: true\n");
    let app = analyzed(&[
        ("app/graphql/types/post_type.rb", &post),
        ("app/graphql/types/base_object.rb", BASE_OBJECT_WITH_FIELD_CLASS),
        ("app/graphql/types/base_field.rb", &swapping),
        ("app/graphql/swap_extension.rb", SWAPPING_EXTENSION),
    ]);
    let failure = diagnose(&app)
        .into_iter()
        .find(|d| d.message.contains("`nothing_has_this`"))
        .expect("the failure stays visible");
    assert_eq!(failure.severity, Severity::Info, "{}", failure.message);
    assert!(failure.message.contains("`SwapExtension`"), "{}", failure.message);
}

/// With every extension proven, the same failure is still an error.
#[test]
fn a_failure_behind_proven_extensions_stays_an_error() {
    let keeping = base_field("    def setup\n      extension KeepExtension\n    end\n");
    let post = post_type("    field :nothing_has_this, String, null: true\n");
    let app = analyzed(&[
        ("app/graphql/types/post_type.rb", &post),
        ("app/graphql/types/base_object.rb", BASE_OBJECT_WITH_FIELD_CLASS),
        ("app/graphql/types/base_field.rb", &keeping),
        ("app/graphql/keep_extension.rb", KEEPING_EXTENSION),
    ]);
    let failure = diagnose(&app)
        .into_iter()
        .find(|d| d.message.contains("`nothing_has_this`"))
        .expect("the failure");
    assert_eq!(failure.severity, Severity::Error, "{}", failure.message);
}

/// A field class that cannot be read leaves every field through it in doubt,
/// including a field with no option for it to consume.
#[test]
fn a_field_class_that_cannot_be_read_is_named_for_every_field() {
    for body in [
        "    prepend SomeGem::Field\n",
        "    def resolve(object, args, ctx)\n      nil\n    end\n",
        "    def setup\n      extension some_extension\n    end\n",
    ] {
        let class = base_field(body);
        let post = post_type("    field :nothing_has_this, String, null: true\n");
        let app = analyzed(&[
            ("app/graphql/types/post_type.rb", &post),
            ("app/graphql/types/base_object.rb", BASE_OBJECT_WITH_FIELD_CLASS),
            ("app/graphql/types/base_field.rb", &class),
        ]);
        let line = coverage_line(&app);
        assert!(line.contains("not shown to keep the value: Types::BaseField on"), "{body}: {line}");
        let failure = diagnose(&app)
            .into_iter()
            .find(|d| d.message.contains("`nothing_has_this`"))
            .expect("the failure stays visible");
        assert_eq!(failure.severity, Severity::Info, "{body}: {}", failure.message);
        assert!(failure.message.contains("`Types::BaseField`"), "{}", failure.message);
    }
}

fn line_for_declared_field_class(declaration: &str) -> String {
    let post = post_type("    field :title, String, null: false\n");
    let base = format!(
        "module Types\n  class BaseObject < GraphQL::Schema::Object\n    {declaration}\n  end\nend\n"
    );
    coverage_line(&analyzed(&[
        ("app/graphql/types/post_type.rb", &post),
        ("app/graphql/types/base_object.rb", &base),
    ]))
}

/// A gem's field class is out of sight; graphql-ruby's own is the default.
#[test]
fn a_field_class_out_of_sight_is_named_and_the_default_is_not() {
    let line = line_for_declared_field_class("field_class SomeGem::Field");
    assert!(line.contains("not shown to keep the value: SomeGem::Field on"), "{line}");
    let line = line_for_declared_field_class("field_class field_class_for(self)");
    assert!(line.contains("a computed `field_class`"), "{line}");
    for default in ["GraphQL::Schema::Field", "::GraphQL::Schema::Field"] {
        let line = line_for_declared_field_class(&format!("field_class {default}"));
        assert!(!line.contains("not shown"), "{default}: {line}");
    }
}

fn line_with_extension_body(body: &str) -> String {
    let extension = format!("class CustomExtension < GraphQL::Schema::FieldExtension\n{body}\nend\n");
    let class = base_field("    def setup\n      extension CustomExtension\n    end\n");
    title_with_option(
        &class,
        "authorize: :read_post",
        &[("app/graphql/custom_extension.rb", &extension)],
    )
}

/// `resolve` keeps the value only by answering what the block answers.
#[test]
fn an_extension_resolve_keeps_the_value_only_by_answering_the_yield() {
    let resolve = |body: &str| {
        line_with_extension_body(&format!(
            "  def resolve(object:, arguments:, context:)\n{body}\n  end"
        ))
    };
    for kept in [
        "    yield(object, arguments)",
        "    raise 'nope' if arguments.nil?\n    yield(object, arguments)",
        "    return yield(object, arguments) if context\n    yield(object, arguments)",
    ] {
        let line = resolve(kept);
        assert!(!line.contains("not shown"), "{kept}: {line}");
    }
    for replaced in [
        "    nil",
        "    yield(object, arguments)\n    nil",
        "    yield(object, arguments).to_s",
        "    yield(other_object, arguments)",
        "    return nil if context\n    yield(object, arguments)",
    ] {
        let line = resolve(replaced);
        assert!(line.contains("not shown to keep the value: CustomExtension on"), "{replaced}: {line}");
    }
}

/// `after_resolve` must not change `value`, whether by reassigning it or in
/// place. Reading it is fine.
#[test]
fn an_extension_after_resolve_that_changes_its_value_is_named() {
    let after = |body: &str| {
        line_with_extension_body(&format!("  def after_resolve(value:, **rest)\n{body}\n    value\n  end"))
    };
    let line = after("    log(value.size)");
    assert!(!line.contains("not shown"), "{line}");
    for changed in [
        "    value ||= []",
        "    value += [1]",
        "    value << 1",
        "    value.push(1)",
        "    value.map!(&:to_s)",
        "    value[0] = 1",
        "    value.name = 'x'",
    ] {
        let line = after(changed);
        assert!(line.contains("not shown to keep the value: CustomExtension on"), "{changed}: {line}");
    }
}

/// A class-level call this pass does not know is named with how many types
/// make it; the pass does not act on what it might mean.
#[test]
fn an_unread_macro_is_reported_with_its_count() {
    let user = "module Types\n  class UserType < BaseObject\n    present_using UserPresenter\n    \
        field :name, String, null: true\n  end\nend\n";
    let post = "module Types\n  class PostType < BaseObject\n    present_using PostPresenter\n    \
        field :title, String, null: false\n  end\nend\n";
    let app = analyzed(&[
        ("app/graphql/types/post_type.rb", post),
        ("app/graphql/types/user_type.rb", user),
    ]);
    assert!(
        coverage_line(&app).contains("macros not read: present_using on 2"),
        "{}",
        coverage_line(&app)
    );
}

const LAZY: &str = "class Lazy\n  def initialize(&block)\n    @proc = block\n  end\n  \
    def force\n    self.class.force(@proc.call)\n  end\n  \
    def self.force(value)\n    case value\n    when Lazy then value.force\n    else value\n    end\n  end\nend\n";

/// A helper that calls `Lazy.force` on unrelated values, as an app does.
const LAZY_CALLER: &str = "class Elsewhere\n  def go\n    Lazy.force(\"text\")\n    Lazy.force([:a, 1])\n  end\nend\n";

const LAZY_SCHEMA: &str = "class AppSchema < GraphQL::Schema\n  query Types::QueryType\n  lazy_resolve Lazy, :force\nend\n";

fn lazy_author(body: &str) -> App {
    let post = post_type(&format!(
        "    field :author, Types::UserType, null: true\n    def author\n      {body}\n    end\n"
    ));
    let elsewhere_type = "module Types\n  class ElsewhereType < BaseObject\n    def go\n      Elsewhere.new.go\n    end\n  end\nend\n";
    analyzed(&[
        ("app/graphql/types/post_type.rb", &post),
        ("app/graphql/types/elsewhere_type.rb", elsewhere_type),
        ("app/graphql/app_schema.rb", LAZY_SCHEMA),
        ("app/models/lazy.rb", LAZY),
        ("app/models/elsewhere.rb", LAZY_CALLER),
    ])
}

fn dispatch_failures(app: &App) -> Vec<String> {
    graphql_diagnostics(app)
        .into_iter()
        .filter(|(code, _)| code == "send_dispatch_failed")
        .map(|(_, message)| message)
        .collect()
}

/// graphql-ruby forces a lazy the schema declares before the next type wraps
/// it, so the type's fields are not read off the lazy.
#[test]
fn a_declared_lazy_is_forced_before_the_type_sees_it() {
    // Either a lazy or the record, chosen at run time. A value that is only
    // ever the lazy still reads off the lazy: `is_a?` narrowing leaves a lone
    // variant alone on its `else` side, and GitLab reports those falsely.
    let failures = dispatch_failures(&lazy_author("object.title ? Lazy.new { object.user } : object.user"));
    assert!(failures.is_empty(), "{failures:?}");
}

/// GitLab's `authorized_find!`: `Lazy.force(x)` must not answer with what
/// every other caller in the app passed it.
#[test]
fn forcing_a_lazy_does_not_answer_with_every_callers_value() {
    let failures = dispatch_failures(&lazy_author("Lazy.force(object.user)"));
    assert!(failures.is_empty(), "{failures:?}");
}

const MOUNTING_MODULE: &str = "module MountMutation\n  extend ActiveSupport::Concern\n  class_methods do\n    \
    def mount_mutation(mutation_class, **custom_kwargs)\n      \
    field mutation_class.graphql_name.underscore.to_sym, mutation: mutation_class, **custom_kwargs\n    \
    end\n  end\nend\n";

fn mutation_app(include: &str) -> App {
    let mutation = |module: &str| {
        format!(
            "module Mutations\n  module {module}\n    class Create < GraphQL::Schema::RelayClassicMutation\n      \
             def resolve\n        {{ errors: [] }}\n      end\n    end\n  end\nend\n"
        )
    };
    let posts = mutation("Posts");
    let users = mutation("Users");
    let root = format!(
        "module Types\n  class MutationType < BaseObject\n    {include}\n    \
         mount_mutation Mutations::Posts::Create\n    mount_mutation Mutations::Users::Create\n  end\nend\n"
    );
    let schema = "class AppSchema < GraphQL::Schema\n  query Types::QueryType\n  mutation Types::MutationType\nend\n";
    analyzed(&[
        ("app/graphql/mutations/posts/create.rb", &posts),
        ("app/graphql/mutations/users/create.rb", &users),
        ("app/graphql/types/mutation_type.rb", &root),
        ("app/graphql/app_schema.rb", schema),
        ("app/graphql/mount_mutation.rb", MOUNTING_MODULE),
    ])
}

/// A macro the app defines around `field` is read from its own definition,
/// and two mutations with the same last segment are two fields.
#[test]
fn a_macro_the_app_defines_around_field_declares_its_fields() {
    let app = mutation_app("include MountMutation");
    let root = app
        .graphql_types
        .iter()
        .find(|t| t.class.0.as_str() == "Types::MutationType")
        .expect("MutationType");
    let names: Vec<&str> = root.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["mutations_posts_create", "mutations_users_create"]);
    assert!(root
        .fields
        .iter()
        .all(|f| matches!(f.resolution, GraphqlResolution::Value { .. })));
    assert!(!coverage_line(&app).contains("mount_mutation"), "{}", coverage_line(&app));
}

/// Nothing to read it from: no fields, and the line says so.
#[test]
fn a_field_macro_out_of_sight_is_reported_not_guessed() {
    let app = mutation_app("");
    let root = app
        .graphql_types
        .iter()
        .find(|t| t.class.0.as_str() == "Types::MutationType")
        .expect("MutationType");
    assert!(root.fields.is_empty());
    assert!(coverage_line(&app).contains("mount_mutation on 1"), "{}", coverage_line(&app));
}

fn coverage_line(app: &App) -> String {
    roundhouse::analyze::graphql::coverage(app)
        .expect("graphql coverage")
        .summary()
}

fn codes(app: &App) -> Vec<String> {
    graphql_diagnostics(app)
        .into_iter()
        .map(|(c, _)| c)
        .collect()
}

/// graphql-ruby passes a field's `argument`s as keywords: the method
/// is called with them, typed as declared, and its body is checked.
#[test]
fn field_arguments_type_the_method_parameters() {
    let post = post_type(
        "    field :excerpt, String, null: false do\n      argument :length, Integer\n    end\n\n    \
         def excerpt(length:)\n      object.title[0, length] + length\n    end\n",
    );
    let app = analyzed(&[("app/graphql/types/post_type.rb", &post)]);
    assert_eq!(
        codes(&app),
        ["incompatible_binop"],
        "{:?}",
        graphql_diagnostics(&app)
    );
    let coverage = roundhouse::analyze::graphql::coverage(&app).unwrap();
    assert_eq!(coverage.arguments, 0, "{}", coverage.summary());
}

/// An optional keyword (`length: nil`) is flattened at ingest; its slot
/// is filled in order with the declared argument.
#[test]
fn an_optional_argument_fills_a_flattened_keyword() {
    let post = post_type(
        "    field :excerpt, String, null: false do\n      argument :length, Integer, required: false\n      \
         argument :suffix, String, required: true\n    end\n\n    \
         def excerpt(length: nil, suffix: \"...\")\n      suffix + 1\n    end\n",
    );
    let app = analyzed(&[("app/graphql/types/post_type.rb", &post)]);
    assert_eq!(
        codes(&app),
        ["incompatible_binop"],
        "{:?}",
        graphql_diagnostics(&app)
    );
    assert!(
        graphql_diagnostics(&app)[0].1.contains("String + Integer"),
        "{:?}",
        graphql_diagnostics(&app)
    );
}

/// A parameter no argument fills (graphql-ruby would raise) is not
/// modeled: recorded, not called, not checked.
#[test]
fn a_parameter_no_argument_fills_is_recorded_not_guessed() {
    let post = post_type(
        "    field :excerpt, String, null: false\n\n    def excerpt(length:)\n      length[:x]\n    end\n",
    );
    let app = analyzed(&[("app/graphql/types/post_type.rb", &post)]);
    assert!(
        graphql_diagnostics(&app).is_empty(),
        "{:?}",
        graphql_diagnostics(&app)
    );
    let coverage = roundhouse::analyze::graphql::coverage(&app).unwrap();
    assert_eq!(coverage.arguments, 1, "{}", coverage.summary());
}

/// An input object argument is an instance of its class, read by
/// method or by key; an enum argument is its value's name, a String.
#[test]
fn input_object_and_enum_arguments() {
    let input = "module Types\n  class PostFilter < GraphQL::Schema::InputObject\n    \
        argument :query, String\n  end\nend\n";
    let order = "module Types\n  class PostOrder < GraphQL::Schema::Enum\n    \
        value \"NEWEST\"\n    value \"OLDEST\"\n  end\nend\n";
    let query = "module Types\n  class QueryType < BaseObject\n    \
        field :posts, [PostType], null: false do\n      argument :filter, PostFilter\n      \
        argument :order, PostOrder\n    end\n\n    \
        def posts(filter:, order:)\n      filter[:query] + 1\n      filter.query + 2\n      order + 3\n      Post.all\n    end\n  end\nend\n";
    let post = post_type("    field :title, String, null: false\n");
    let app = analyzed(&[
        ("app/graphql/types/post_filter.rb", input),
        ("app/graphql/types/post_order.rb", order),
        ("app/graphql/types/query_type.rb", query),
        ("app/graphql/types/post_type.rb", &post),
    ]);
    let found = graphql_diagnostics(&app);
    assert_eq!(found.len(), 3, "{found:?}");
    assert!(
        found
            .iter()
            .all(|(c, m)| c == "incompatible_binop" && m.contains("String + Integer")),
        "{found:?}"
    );
}

/// A mutation's class-body `argument`s are its `resolve` keywords; the
/// record it returns types the payload type.
#[test]
fn a_mutation_resolves_with_its_arguments() {
    let schema = "class AppSchema < GraphQL::Schema\n  query Types::QueryType\n  \
        mutation Types::MutationType\nend\n";
    let mutation_type = "module Types\n  class MutationType < BaseObject\n    \
        field :create_post, mutation: Mutations::CreatePost\n  end\nend\n";
    let create = "module Mutations\n  class CreatePost < GraphQL::Schema::Mutation\n    \
        argument :title, String\n    type Types::PostType\n\n    \
        def resolve(title:)\n      title + 1\n      Post.create!(title: title)\n    end\n  end\nend\n";
    let post = post_type("    field :headline, String, null: false\n");
    let app = analyzed(&[
        ("app/graphql/app_schema.rb", schema),
        ("app/graphql/types/mutation_type.rb", mutation_type),
        ("app/graphql/mutations/create_post.rb", create),
        ("app/graphql/types/post_type.rb", &post),
    ]);
    let found = graphql_diagnostics(&app);
    // `title` arrives typed as declared, and the payload type is
    // reached through the mutation's return.
    assert!(
        found
            .iter()
            .any(|(c, m)| c == "incompatible_binop" && m.contains("String + Integer")),
        "{found:?}"
    );
    assert!(
        found.iter().any(|(_, m)| m.contains("`headline` on Post")),
        "{found:?}"
    );
}

/// The argument and reader signatures are for the analyzer: lowering
/// removes them with the synthesized methods.
#[test]
fn lowering_removes_the_argument_signatures() {
    let post = post_type(
        "    field :excerpt, String, null: false do\n      argument :length, Integer\n    end\n\n    \
         def excerpt(length:)\n      object.title\n    end\n",
    );
    let mut app = analyzed(&[("app/graphql/types/post_type.rb", &post)]);
    assert!(!app.graphql_signatures.is_empty());
    roundhouse::session::analyze_and_lower(&mut app);
    let leftover: Vec<_> = app
        .rbs_signatures
        .values()
        .flat_map(|t| t.keys())
        .filter(|k| k.as_str().starts_with("__gql_"))
        .collect();
    assert!(leftover.is_empty(), "{leftover:?}");
}

/// Generated inference methods borrow the field's span for diagnostics,
/// while authored declarations keep their real constant-reference identity.
#[test]
fn generated_graphql_constants_keep_provenance_when_borrowing_source_spans() {
    use roundhouse::expr::{Expr, ExprNode, GENERATED_CONST_REF};
    fn inspect(expr: &Expr, generated: bool, count: &mut usize) {
        if matches!(&*expr.node, ExprNode::Const { .. }) {
            assert!(expr.span.file.0 > 0, "expected a real borrowed/source span");
            assert_eq!(expr.decisions & GENERATED_CONST_REF != 0, generated);
            *count += 1;
        }
        expr.node.for_each_child(&mut |child| inspect(child, generated, count));
    }
    let post = post_type("    field :author, UserType, null: false, method: :user\n");
    let app = analyzed(&[("app/graphql/types/post_type.rb", &post)]);
    let mut generated = 0;
    let mut authored = 0;
    for ty in &app.graphql_types {
        let class = app.library_classes.iter().find(|c| c.name == ty.class).unwrap();
        for method in &class.methods {
            if ty.synthesized.contains(&method.name) {
                inspect(&method.body, true, &mut generated);
            }
        }
        for call in &class.unknown_calls {
            inspect(call, false, &mut authored);
        }
    }
    assert!(generated > 0, "must inspect generated class wrappers");
    assert!(authored > 0, "must inspect authored field declarations");
}
