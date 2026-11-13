//! Ingest-level checks for the Roda + Sequel front-end (issue #67)
//! against `fixtures/roda-blog`. These assert the shape of the
//! ingested `App` — routes linearized from the routing tree,
//! controllers synthesized with prologue filters, Sequel models and
//! migrations folded into the shared IR — per the mapping table in
//! `docs/roda-sequel-plan.md`.

use roundhouse::dialect::{
    Association, ControllerBodyItem, Dependent, FilterKind, LayoutDecl,
    RouteSpec, ValidationRule,
};
use roundhouse::ingest::ingest_app;
use roundhouse::schema::{ColumnType, ReferentialAction};
use std::path::Path;

fn ingest() -> roundhouse::App {
    ingest_app(Path::new("fixtures/roda-blog")).expect("roda-blog ingests")
}

#[test]
fn ingests_without_parse_diagnostics() {
    let (result, diags) =
        roundhouse::ingest::prism::scope(|| ingest_app(Path::new("fixtures/roda-blog")));
    result.expect("ingest ok");
    assert!(diags.is_empty(), "parse diagnostics: {diags:?}");
}

#[test]
fn schema_folds_from_sequel_migrations() {
    let app = ingest();
    assert_eq!(app.schema.tables.len(), 2);

    let articles = &app.schema.tables[&roundhouse::Symbol::from("articles")];
    let id = articles.columns.iter().find(|c| c.name.as_str() == "id").unwrap();
    assert!(id.primary_key);
    let title = articles.columns.iter().find(|c| c.name.as_str() == "title").unwrap();
    assert_eq!(title.col_type, ColumnType::String { limit: None });
    assert!(!title.nullable, "null: false carried");
    let body = articles.columns.iter().find(|c| c.name.as_str() == "body").unwrap();
    assert_eq!(body.col_type, ColumnType::Text, "String :body, text: true → Text");

    let comments = &app.schema.tables[&roundhouse::Symbol::from("comments")];
    let fk = comments.foreign_keys.first().expect("comments has an FK");
    assert_eq!(fk.from_columns[0].as_str(), "article_id");
    assert_eq!(fk.to_table.0.as_str(), "articles");
    assert_eq!(fk.on_delete, ReferentialAction::Cascade);
    assert_eq!(comments.indexes.len(), 1);
    let article_id =
        comments.columns.iter().find(|c| c.name.as_str() == "article_id").unwrap();
    assert!(matches!(&article_id.col_type, ColumnType::Reference { table } if table.0.as_str() == "articles"));
}

fn sequel_table(body: &str) -> Result<roundhouse::schema::Table, roundhouse::ingest::IngestError> {
    let source = format!("Sequel.migration do\n change do\n create_table(:links) do\n{body}\n end\n end\nend");
    let mut schema = roundhouse::schema::Schema::default();
    roundhouse::ingest::ingest_sequel_migration(source.as_bytes(), "db/migrate/001_links.rb", &mut schema)?;
    Ok(schema.tables.shift_remove(&roundhouse::Symbol::from("links")).unwrap())
}

#[test]
fn sequel_primary_key_arrays_are_constraints_on_existing_columns() {
    for keys in ["[:b, :a]", "%i[b a]", "[Sequel.identifier(:b), Sequel.identifier(:a)]"] {
        let table = sequel_table(&format!("Integer :a\nInteger :b\nprimary_key {keys}")).unwrap();
        assert_eq!(table.primary_key_columns.iter().map(|c| c.as_str()).collect::<Vec<_>>(), ["b", "a"]);
        assert_eq!(table.columns.len(), 2, "a constraint must not add columns");
        assert!(table.columns.iter().all(|c| !c.primary_key));
        assert!(table.columns.iter().all(|c| c.nullable == keys.contains("Sequel.identifier")));
    }
    let table = sequel_table("self.primary_key [:b, :a]\nInteger :a\nInteger :b").unwrap();
    assert!(table.columns.iter().all(|c| !c.nullable));
    assert_eq!(table.primary_key_columns[0].as_str(), "b");
}

#[test]
fn sequel_foreign_key_arrays_and_scalar_targets_keep_order_and_timing() {
    use roundhouse::schema::Deferrable::{Deferred, Immediate};
    for (timing, expected) in [("true", Some(Deferred)), (":deferred", Some(Deferred)),
        (":immediate", Some(Immediate)), ("false", None), ("nil", None)] {
        let table = sequel_table(&format!(
            "Integer :a\nInteger :b\nforeign_key [:b, :a], :parents, key: [:y, :x], deferrable: {timing}, on_delete: :cascade, on_update: :restrict"
        )).unwrap();
        assert_eq!(table.columns.len(), 2);
        let fk = &table.foreign_keys[0];
        assert_eq!(fk.from_columns.iter().map(|c| c.as_str()).collect::<Vec<_>>(), ["b", "a"]);
        assert_eq!(fk.to_columns.iter().map(|c| c.as_str()).collect::<Vec<_>>(), ["y", "x"]);
        assert_eq!(fk.to_table.0.as_str(), "parents");
        assert_eq!(fk.deferrable, expected);
        assert_eq!(fk.on_delete, ReferentialAction::Cascade);
        assert_eq!(fk.on_update, ReferentialAction::Restrict);
        for key in [":code", "[:code]"] {
            let table = sequel_table(&format!("foreign_key :parent_code, :parents, key: {key}, deferrable: {timing}")).unwrap();
            assert_eq!(table.columns.len(), 1);
            assert_eq!(table.foreign_keys[0].to_columns[0].as_str(), "code");
            assert_eq!(table.foreign_keys[0].deferrable, expected);
        }
    }
    let table = sequel_table("Integer :a\nforeign_key [:a], :parents, key: :code").unwrap();
    assert_eq!(table.columns.len(), 1, "an array with one name is still a constraint");
    assert_eq!(table.foreign_keys[0].to_columns[0].as_str(), "code");
    let table = sequel_table("self.foreign_key :parent_code, :parents, {key: :code, deferrable: true}").unwrap();
    assert_eq!(table.foreign_keys[0].to_columns[0].as_str(), "code");
    assert_eq!(table.foreign_keys[0].deferrable, Some(Deferred));
}

#[test]
fn sequel_implicit_foreign_keys_resolve_the_targets_primary_key() {
    use roundhouse::{Symbol, schema::Schema};
    for (parent, from, expected) in [
        ("Integer :x\nInteger :y\nprimary_key [:y, :x]", "[:b, :a]", vec!["y", "x"]),
        ("primary_key :code", "[:a]", vec!["code"]),
        ("primary_key :code", ":a", vec!["code"]),
        ("primary_key :id", "[:a]", vec!["id"]),
    ] {
        for options in ["", ", {deferrable: true}"] {
            let columns = if from.starts_with('[') { "Integer :a\nInteger :b" } else { "Integer :b" };
            let source = format!("Sequel.migration do\n change do\n create_table(:parents) do\n{parent}\n end\n create_table(:links) do\n{columns}\n self.foreign_key {from}, :parents{options}\n end\n end\nend");
            let mut schema = Schema::default();
            roundhouse::ingest::ingest_sequel_migration(source.as_bytes(), "db/migrate/001_links.rb", &mut schema).unwrap();
            let table = &schema.tables[&Symbol::from("links")];
            assert_eq!(table.columns.len(), 2, "constraint must reuse its existing columns");
            let fk = &table.foreign_keys[0];
            assert_eq!(fk.to_columns.iter().map(|c| c.as_str()).collect::<Vec<_>>(), expected);
        }
    }
}

#[test]
fn sequel_implicit_self_references_resolve_keys_declared_later() {
    for (body, expected) in [
        ("Integer :a\nInteger :b\nInteger :c\nInteger :d\nforeign_key [:c, :d], :links\nprimary_key [:b, :a]", vec!["b", "a"]),
        ("foreign_key [:c, :d], :links\nprimary_key [:b, :a]\nInteger :a\nInteger :b\nInteger :c\nInteger :d", vec!["b", "a"]),
        ("foreign_key :parent_code, :links\nprimary_key :code", vec!["code"]),
    ] {
        let table = sequel_table(body).unwrap();
        assert_eq!(table.foreign_keys[0].to_columns.iter().map(|c| c.as_str()).collect::<Vec<_>>(), expected);
    }
}

#[test]
fn sequel_implicit_foreign_keys_refuse_unknown_or_incompatible_keys() {
    use roundhouse::schema::Schema;
    for (parent, from) in [
        ("create_table(:parents) { Integer :id }", "[:a]"),
        ("create_table(:parents) { Integer :a; Integer :b; primary_key [:b, :a] }", "[:a]"),
        ("create_table(:parents) { Integer :a; Integer :b; primary_key [:b, :a] }", ":a"),
        ("create_table(:parents) { primary_key :id }", "[:missing]"),
    ] {
        let columns = if from.starts_with('[') { "Integer :a" } else { "" };
        let source = format!("Sequel.migration do\n change do\n{parent}\n create_table(:links) do\n{columns}\n foreign_key {from}, :parents\n end\n end\nend");
        let error = roundhouse::ingest::ingest_sequel_migration(source.as_bytes(), "db/migrate/001_links.rb", &mut Schema::default()).expect_err(&source).to_string();
        assert!(error.contains("db/migrate/001_links.rb"), "{error}");
    }
}

#[test]
fn scalar_sequel_keys_ignore_the_options_they_do_not_read() {
    use roundhouse::{Symbol, schema::Schema};
    let source = "Sequel.migration do\n change do\n create_table(:parents) do\n primary_key :id, type: :Bignum\n end\n create_table(:links) do\n primary_key :id, :Bignum, auto_increment: true\n foreign_key :parent_id, :parents, type: :Bignum, index: true, default: 1, null: false\n foreign_key :spread_id, :parents, **options\n foreign_key :hashed_id, :parents, key_options\n end\n end\nend";
    let mut schema = Schema::default();
    roundhouse::ingest::ingest_sequel_migration(source.as_bytes(), "db/migrate/001_links.rb", &mut schema).unwrap();
    let columns = |table: &str| schema.tables[&Symbol::from(table)].columns.iter()
        .map(|c| (c.name.to_string(), c.col_type.clone(), c.nullable, c.primary_key)).collect::<Vec<_>>();
    let parents = ColumnType::Reference { table: roundhouse::TableRef(Symbol::from("parents")) };
    assert_eq!(columns("parents"), [("id".to_string(), ColumnType::Integer, false, true)]);
    assert_eq!(columns("links"), [("id".to_string(), ColumnType::Integer, false, true), ("parent_id".to_string(), parents.clone(), false, false),
        ("spread_id".to_string(), parents.clone(), true, false), ("hashed_id".to_string(), parents, true, false)]);
    let links = &schema.tables[&Symbol::from("links")];
    assert!(links.indexes.is_empty() && links.primary_key_columns.is_empty());
    for (fk, from) in links.foreign_keys.iter().zip(["parent_id", "spread_id", "hashed_id"]) {
        assert_eq!((fk.from_columns.as_slice(), fk.to_table.0.as_str(), fk.to_columns.as_slice()), ([Symbol::from(from)].as_slice(), "parents", [Symbol::from("id")].as_slice()));
        assert_eq!((&fk.on_delete, &fk.on_update, fk.deferrable), (&ReferentialAction::NoAction, &ReferentialAction::NoAction, None));
    }
    assert_eq!(links.foreign_keys.len(), 3);
}

#[test]
fn sequel_unread_key_forms_are_refused() {
    for body in [
        "primary_key []", "primary_key [:a, key_column]", "primary_key key_columns",
        "primary_key [[ :a ]]", "primary_key [Sequel.identifier(:a, :b)]",
        "primary_key [Other.identifier(:a)]", "primary_key [Sequel.identifier(key_column)]",
        "primary_key [:missing]", "primary_key [:a], deferrable: true",
        "foreign_key [], :parents, key: []", "foreign_key [:a, :b], :parents, key: [:x]",
        "foreign_key [:a, key_column], :parents, key: [:x, :y]",
        "foreign_key [:a, :b], :parents, **options", "foreign_key [:a], :parents, key_options",
        "foreign_key [:a], :parents, key: :id, type: :String", "foreign_key :a, :parents, key: []",
        "foreign_key :a, :parents, key: key_column", "foreign_key :a, :parents, key: [:x, key_column]",
        "foreign_key :a, :parents, key: [:x, :y]", "foreign_key :a, :parents, deferrable: timing",
        "foreign_key :a, :parents, deferrable: :unknown",
        "foreign_key [:missing], :parents, key: :id",
        "primary_key [:a] do\n unsupported\nend", "Other.primary_key [:a]",
        "Other.foreign_key [:a], :parents, key: :id",
    ] {
        let error = sequel_table(&format!("Integer :a\nInteger :b\n{body}"))
            .expect_err(body).to_string();
        assert!(error.contains("db/migrate/001_links.rb"), "{error}");
    }
}

#[test]
fn sequel_models_converge_on_rails_shape() {
    let app = ingest();
    // Article + Comment + the synthesized ApplicationRecord base.
    assert_eq!(app.models.len(), 3);

    let article = app.models.iter().find(|m| m.name.0.as_str() == "Article").unwrap();
    assert_eq!(article.parent.as_ref().unwrap().0.as_str(), "ApplicationRecord");
    assert_eq!(article.table.0.as_str(), "articles");
    assert!(!article.attributes.fields.is_empty(), "attributes folded from schema");
    let has_many = article.associations().next().expect("one_to_many ingested");
    match has_many {
        Association::HasMany { name, target, foreign_key, dependent, scope, .. } => {
            assert_eq!(name.as_str(), "comments");
            assert_eq!(target.0.as_str(), "Comment");
            assert_eq!(foreign_key.as_str(), "article_id");
            // FK ON DELETE CASCADE in the migration expresses the same
            // behavior dependent: :destroy does on the AR runtime.
            assert_eq!(*dependent, Dependent::Destroy);
            assert!(scope.is_some(), "order: Sequel.desc(...) carried as assoc scope");
        }
        other => panic!("expected HasMany, got {other:?}"),
    }
    // `def validate` body → declarative Validations: presence on
    // title+body, min-length 10 with the custom message on body.
    let validations: Vec<_> = article.validations().collect();
    assert!(
        validations
            .iter()
            .any(|v| v.attribute.as_str() == "title"
                && v.rules.iter().any(|r| matches!(r, ValidationRule::Presence))),
        "presence on title: {validations:?}"
    );
    assert!(
        validations.iter().any(|v| v.attribute.as_str() == "body"
            && v.rules.iter().any(|r| matches!(
                r,
                ValidationRule::Length { min: Some(10), max: None, is: None, message: Some(m), .. }
                    if m == "must be at least 10 characters"
            ))),
        "min-length with custom message on body: {validations:?}"
    );

    let comment = app.models.iter().find(|m| m.name.0.as_str() == "Comment").unwrap();
    let belongs_to = comment.associations().next().expect("many_to_one ingested");
    match belongs_to {
        Association::BelongsTo { name, target, foreign_key, optional, .. } => {
            assert_eq!(name.as_str(), "article");
            assert_eq!(target.0.as_str(), "Article");
            assert_eq!(foreign_key.as_str(), "article_id");
            assert!(!optional, "article_id is NOT NULL → required");
        }
        other => panic!("expected BelongsTo, got {other:?}"),
    }
}

#[test]
fn routing_tree_linearizes_to_rest_routes() {
    let app = ingest();
    let mut flat: Vec<(String, String, String, String)> = Vec::new();
    for entry in &app.routes.entries {
        match entry {
            RouteSpec::Explicit { method, path, controller, action, .. } => flat.push((
                format!("{method:?}"),
                path.clone(),
                controller.0.to_string(),
                action.to_string(),
            )),
            RouteSpec::Root { target, .. } => {
                flat.push(("Get".into(), "/".into(), "root".into(), target.clone()))
            }
            other => panic!("unexpected route spec: {other:?}"),
        }
    }
    let expect = [
        ("Get", "/", "root", "root#index"),
        ("Get", "/articles", "ArticlesController", "index"),
        ("Post", "/articles", "ArticlesController", "create"),
        ("Get", "/articles/new", "ArticlesController", "new"),
        ("Get", "/articles/:id", "ArticlesController", "show"),
        ("Patch", "/articles/:id", "ArticlesController", "update"),
        ("Delete", "/articles/:id", "ArticlesController", "destroy"),
        ("Get", "/articles/:id/edit", "ArticlesController", "edit"),
        ("Post", "/articles/:id/comments", "CommentsController", "create"),
        (
            "Delete",
            "/articles/:id/comments/:comment_id",
            "CommentsController",
            "destroy",
        ),
    ];
    let flat_refs: Vec<(&str, &str, &str, &str)> = flat
        .iter()
        .map(|(a, b, c, d)| (a.as_str(), b.as_str(), c.as_str(), d.as_str()))
        .collect();
    for want in &expect {
        assert!(flat_refs.contains(want), "missing route {want:?} in {flat:?}");
    }
    assert_eq!(flat.len(), expect.len(), "no extra routes: {flat:?}");
}

#[test]
fn integer_matcher_constraint_reaches_the_route_table() {
    // `r.on Integer` only ever matched digit segments; the constraint
    // must survive lowering into `FlatRoute.int_params` so the runtime
    // router can reject `/articles/12abc` the way Roda does (instead
    // of binding `id = "12abc"` and, post-`to_i`, serving article 12 —
    // the #67 review finding).
    let app = ingest();
    let flat = roundhouse::lower::routes::flatten_routes(&app);
    for r in &flat {
        for p in &r.path_params {
            if p == "id" || p == "comment_id" {
                assert!(
                    r.int_params.contains(p),
                    "route {} {} should digit-constrain :{p} — got {:?}",
                    format_args!("{:?}", r.method),
                    r.path,
                    r.int_params,
                );
            }
        }
    }
    // And the root/collection routes carry no spurious constraints.
    let index = flat
        .iter()
        .find(|r| r.path == "/articles")
        .expect("index route");
    assert!(index.int_params.is_empty());
}

#[test]
fn controllers_synthesize_with_prologue_filters() {
    let app = ingest();
    let names: Vec<&str> = app.controllers.iter().map(|c| c.name.0.as_str()).collect();
    assert_eq!(
        names,
        vec!["ApplicationController", "RootController", "ArticlesController", "CommentsController"]
    );

    let application =
        app.controllers.iter().find(|c| c.name.0.as_str() == "ApplicationController").unwrap();
    // The one app-wide layout re-homes onto layouts/application (the
    // convention default), so no explicit declaration is needed.
    assert!(matches!(&application.layout, LayoutDecl::Inherit));

    let articles =
        app.controllers.iter().find(|c| c.name.0.as_str() == "ArticlesController").unwrap();
    let actions: Vec<&str> = articles.actions().map(|a| a.name.as_str()).collect();
    // Route-leaf actions in source order, plus the synthesized
    // prologue method after the private marker.
    assert_eq!(
        actions,
        vec![
            "index",
            "create",
            "new",
            "show",
            "update",
            "destroy",
            "edit",
            "set_article",
            "article_params"
        ]
    );
    let filter = articles.filters().next().expect("prologue filter");
    assert_eq!(filter.kind, FilterKind::Before);
    assert_eq!(filter.target.as_str(), "set_article");
    let mut only: Vec<&str> = filter.only.iter().map(|s| s.as_str()).collect();
    only.sort_unstable();
    assert_eq!(only, vec!["destroy", "edit", "show", "update"]);
    assert!(
        articles
            .body
            .iter()
            .any(|i| matches!(i, ControllerBodyItem::PrivateMarker { .. })),
        "private marker before the synthesized filter method"
    );

    let comments =
        app.controllers.iter().find(|c| c.name.0.as_str() == "CommentsController").unwrap();
    let actions: Vec<&str> = comments.actions().map(|a| a.name.as_str()).collect();
    assert_eq!(actions, vec!["create", "destroy", "set_article", "comment_params"]);
    let filter = comments.filters().next().expect("comments share the article prologue");
    assert_eq!(filter.target.as_str(), "set_article");
    assert!(filter.only.is_empty(), "guards every comments action");
}

#[test]
fn bodies_normalize_to_ar_vocabulary() {
    let app = ingest();
    // No Sequel/Roda surface survives in any controller body: the
    // rewrites in ingest normalize to the AR + Rails-controller
    // vocabulary the rest of the pipeline speaks.
    let banned = ["set_fields", "with_pk", "eager", "comments_dataset", "view"];
    let mut sends: Vec<String> = Vec::new();
    for controller in &app.controllers {
        for action in controller.actions() {
            collect_sends(&action.body, &mut sends);
        }
    }
    for method in banned {
        assert!(
            !sends.iter().any(|s| s == method),
            "{method} survived normalization; sends: {sends:?}"
        );
    }
    // And the normalized spellings are present.
    for expected in ["find_by", "includes", "expect", "update", "redirect_to", "render"] {
        assert!(
            sends.iter().any(|s| s == expected),
            "expected a {expected} call after normalization; sends: {sends:?}"
        );
    }

    // The Integer matcher's block param reads as params[:id], so the
    // show/update/destroy prologue is Rails-shaped.
    let articles =
        app.controllers.iter().find(|c| c.name.0.as_str() == "ArticlesController").unwrap();
    let set_article = articles.actions().find(|a| a.name.as_str() == "set_article").unwrap();
    let mut prologue_sends = Vec::new();
    collect_sends(&set_article.body, &mut prologue_sends);
    assert!(prologue_sends.iter().any(|s| s == "params"), "prologue reads params");
    assert!(prologue_sends.iter().any(|s| s == "find_by"), "Article[id] → find_by");
    assert!(prologue_sends.iter().any(|s| s == "render"), "missing record renders 404");
}

#[test]
fn views_ingest_with_roda_dialect_normalized() {
    let app = ingest();
    let names: Vec<&str> = app.views.iter().map(|v| v.name.as_str()).collect();
    for expected in [
        "layouts/application",
        "not_found",
        "articles/index",
        "articles/show",
        "articles/new",
        "articles/edit",
        "articles/_form",
        "articles/_article",
        "comments/_comment",
    ] {
        assert!(names.contains(&expected), "missing view {expected} in {names:?}");
    }

    // `part(...)` normalized to `render(...)` everywhere.
    let mut sends = Vec::new();
    for view in &app.views {
        collect_sends(&view.body, &mut sends);
    }
    assert!(!sends.iter().any(|s| s == "part"), "part survived in views");
    assert!(sends.iter().any(|s| s == "render"), "partials render");
}

#[test]
fn helpers_register_in_the_bare_call_index() {
    let app = ingest();
    for helper in ["truncate", "pluralize"] {
        let owner = app
            .helper_method_index
            .get(&roundhouse::Symbol::from(helper))
            .unwrap_or_else(|| panic!("{helper} not registered"));
        assert_eq!(owner.0.as_str(), "ApplicationHelper");
    }
    assert!(
        app.library_classes.iter().any(|c| c.name.0.as_str() == "ApplicationHelper"),
        "helper module registered as a library class"
    );
}

#[test]
fn seeds_ingest_normalized() {
    let app = ingest();
    let seeds = app.seeds.as_ref().expect("seeds.rb ingested");
    let mut sends = Vec::new();
    collect_sends(seeds, &mut sends);
    assert!(!sends.iter().any(|s| s == "require_relative"), "boot requires stripped");
    assert!(!sends.iter().any(|s| s == "dataset"), "X.dataset.delete → delete_all");
    assert!(sends.iter().any(|s| s == "delete_all"));
    assert!(
        !sends.iter().any(|s| s == "add_comment"),
        "add_comment → comments.create"
    );
    assert!(sends.iter().any(|s| s == "create"));
}

/// The in-browser playground ingests through `ingest_app_from_tree`
/// (an in-memory path→bytes map — the shape `bundle-src.mjs` ships).
/// Guard that the roda front-end dispatch works on that path with the
/// bundle's file subset: analyzable sources only, no Gemfile, no
/// test/, no README.
#[test]
fn ingests_from_in_memory_tree() {
    use std::collections::HashMap;
    use std::path::PathBuf;

    let fixture = Path::new("fixtures/roda-blog");
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    for rel in [
        "app.rb",
        "config.ru",
        "db.rb",
        "seeds.rb",
        "db/migrate/001_create_articles.rb",
        "db/migrate/002_create_comments.rb",
        "models/article.rb",
        "models/comment.rb",
        "views/articles/_article.erb",
        "views/articles/_form.erb",
        "views/articles/edit.erb",
        "views/articles/index.erb",
        "views/articles/new.erb",
        "views/articles/show.erb",
        "views/comments/_comment.erb",
        "views/layout.erb",
        "views/not_found.erb",
    ] {
        tree.insert(
            PathBuf::from(rel),
            std::fs::read(fixture.join(rel)).unwrap_or_else(|_| panic!("read {rel}")),
        );
    }
    let app = roundhouse::ingest::ingest_app_from_tree(tree).expect("tree ingest");
    assert_eq!(app.routes.entries.len(), 10, "routes linearized from the tree");
    assert!(
        app.controllers.iter().any(|c| c.name.0.as_str() == "ArticlesController"),
        "controllers synthesized from the tree"
    );
    assert_eq!(app.schema.tables.len(), 2, "migrations folded from the tree");
}

fn collect_sends(expr: &roundhouse::expr::Expr, out: &mut Vec<String>) {
    if let roundhouse::expr::ExprNode::Send { method, .. } = &*expr.node {
        out.push(method.to_string());
    }
    expr.node.for_each_child(&mut |c| collect_sends(c, out));
}
