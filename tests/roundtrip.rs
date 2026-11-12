//! Round-trip test: IR → JSON → IR preserves semantics.
//!
//! The forcing function for IR completeness. If constructing an App by hand
//! and round-tripping through JSON loses information, the shape is wrong.
//! Extend this test as the IR grows — every new node kind gets exercised here.

use indexmap::IndexMap;
use roundhouse::{
    Action, App, ClassId, Column, ColumnType, Controller, Effect, EffectSet, Expr, ExprNode,
    HttpMethod, Literal, Model, RenderTarget, RouteTable, Row, Schema, Symbol, Table,
    TableRef, Ty,
};
use roundhouse::span::Span;

fn sp() -> Span {
    Span::synthetic()
}

/// Verifies that a representative blog app survives an IR serialization round-trip.
#[test]
fn tiny_blog_round_trips() {
    let mut tables = IndexMap::new();
    tables.insert(
        Symbol::from("posts"),
        Table {
            name: Symbol::from("posts"),
            columns: vec![
                Column {
                    name: Symbol::from("id"),
                    col_type: ColumnType::BigInt,
                    nullable: false,
                    default: None,
                    default_expression: None,
                    legacy_default_persistence: false,
                    primary_key: true,
                    generated: None,
                    generated_text_compatible: None,
                    generated_int4_compatible: None,
                    array: false,
                    array_stored: false,
                },
                Column {
                    name: Symbol::from("title"),
                    col_type: ColumnType::String { limit: None },
                    nullable: false,
                    default: None,
                    default_expression: None,
                    legacy_default_persistence: false,
                    primary_key: false,
                    generated: None,
                    generated_text_compatible: None,
                    generated_int4_compatible: None,
                    array: false,
                    array_stored: false,
                },
            ],
            indexes: vec![],
            foreign_keys: vec![],
            check_constraints: Vec::new(),
            virtual_module: None,
            known_keyless: false,
            primary_key_columns: Vec::new(),
        },
    );
    let schema = Schema { tables, ..Default::default() };

    let mut attrs = IndexMap::new();
    attrs.insert(Symbol::from("id"), Ty::Int);
    attrs.insert(Symbol::from("title"), Ty::Str);

    let post_model = Model {
        name: ClassId(Symbol::from("Post")),
        parent: None,
        parent_span: Default::default(),
        table: TableRef(Symbol::from("posts")),
        primary_key: None,
        attributes: Row { fields: attrs.into(), rest: None },
        body: vec![],
        enums: Default::default(),
        enum_defaults: Default::default(),
        class_attr_defaults: Default::default(),
        lexical_json_shadow: false,
        sti_subclass_names: Vec::new(),
        mixin_sends: Vec::new(),
        select_aliases: Vec::new(),
        i18n: Default::default(),
        span: Span::synthetic(),
    };

    // Action body: `Post.all` — a Send to a class-level method.
    let recv = Expr::new(sp(), ExprNode::Const { path: vec![Symbol::from("Post")] });
    let action_body = Expr::new(
        sp(),
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from("all"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );

    let index_action = Action {
        name_span: roundhouse::span::Span::synthetic(),
        name: Symbol::from("index"),
        params: Row::closed(),
        opt_params: vec![],
        kw_params: vec![],
        kwrest_param: None,
        rest_param: None,
        anonymous_formal: None,
        block_param: None,
        body: action_body,
        renders: RenderTarget::Inferred,
        effects: EffectSet::singleton(Effect::DbRead {
            table: TableRef(Symbol::from("posts")),
        }),
    };

    let posts_controller = Controller {
        name: ClassId(Symbol::from("PostsController")),
        parent: Some(ClassId(Symbol::from("ApplicationController"))),
        parent_span: Default::default(),
        body: vec![roundhouse::ControllerBodyItem::Action {
            action: index_action,
            leading_comments: vec![],
            leading_blank_line: false,
        }],
        layout: Default::default(),
        sibling_classes: Vec::new(),
    };

    let routes = RouteTable {
        entries: vec![roundhouse::RouteSpec::Explicit {
            method: HttpMethod::Get,
            path: "/posts".into(),
            controller: ClassId(Symbol::from("PostsController")),
            action: Symbol::from("index"),
            as_name: Some(Symbol::from("posts")),
            constraints: IndexMap::new(),
            scope: roundhouse::ResourceScope::Nested,
            action_symbol: false,
        }],
        direct_helpers: vec![],
        redirects: vec![],
        diagnostics: vec![],
    };

    let app = App {
        schema_version: App::SCHEMA_VERSION,
        schema,
        // Passthrough-only and `serde(skip)` — it never round-trips, so
        // the roundtrip fixture has nothing to say about it.
        binary_assets: Vec::new(),
        i18n: Default::default(),
        i18n_table: Default::default(),
        i18n_runtime_sites: Default::default(),
        i18n_table_sites: Default::default(),
        inferred_method_params: Default::default(),
        models: vec![post_model],
        library_classes: vec![],
        native_library_keywords: false,
        pins_unresolved_params: false,
        file_programs: vec![],
        boot_gem_entries: vec![],
        current_attribute_classes: vec![],
        initializer_filters: Vec::new(),
        initializer_defs: Vec::new(),
        gem_package_settings: Vec::new(),
        auto_source_gems: Vec::new(),
        gem_load_statements: Vec::new(),
        initializer_response_headers: Vec::new(),
        collection_cache_store: None,
        app_requires: Vec::new(),
        sql_functions: Vec::new(),
        database_pragmas: Vec::new(),
        databases: Vec::new(),
        model_databases: Default::default(),
        database_refusals: Vec::new(),
        ingest_drops: Vec::new(),
        library_class_files: Vec::new(),
        rbs_includes: Default::default(),
        rbs_spellings: Default::default(),
        controllers: vec![posts_controller],
        routes,
        views: vec![],
        test_modules: vec![],
        fixtures: vec![],
        seeds: None,
        boot_program: Vec::new(),
        boot_gem_constants: Vec::new(),
        gem_middlewares: Vec::new(),
        gem_middleware_blocks: Default::default(),
        rack_env_writes: Default::default(),
        gem_middleware_refusals: Vec::new(),
        included_hoist_conflicts: Vec::new(),
        superclass_refusals: Vec::new(),
        gem_route_gaps: Vec::new(),
        generated_route_helpers: Vec::new(),
        importmap: None,
        engine_importmaps: Vec::new(),
        inertia: None,
        vite_manifest: None,
        stylesheets: vec![],
        rbs_signatures: roundhouse::det::HashMap::default(),
        gem_lock: None,
        loaded_frameworks: None,
        assume_ssl_envs: Vec::new(),
        gem_boundary: Default::default(),
        extensions: Default::default(),
        content_helper_allowed_attributes: Vec::new(),
        content_helper_allowed_tags: Vec::new(),
        helper_method_index: roundhouse::det::HashMap::default(),
        view_visible_controller_methods: std::collections::BTreeSet::new(),
        global_id_locate_models: std::collections::BTreeSet::new(),
        global_id_locate_signed_models: std::collections::BTreeSet::new(),
        unreachable_views: std::collections::BTreeSet::new(),
        helper_diagnostics: None,
        unknown_association_targets: Vec::new(),
        helper_modules: Default::default(),
        job_plans: Vec::new(),
        queue_adapter: None,
        sidekiq_redis: Default::default(),
        sidekiq_worker: Default::default(),
        cable_redis: None,
        attachable_unsigned_models: Vec::new(),
        pending_attachment_on_load: Vec::new(),
        load_hook_class_macros: Vec::new(),
        load_hook_extended_macros: Vec::new(),
        load_hook_constants: Vec::new(),
        macro_includes: Vec::new(),
        expanded_macro_calls: Vec::new(),
        spliced_class_methods: Default::default(),
        time_tag_calls: Vec::new(),
        partial_local_types: roundhouse::det::HashMap::default(),
        view_ivar_types: roundhouse::det::HashMap::default(),
        html_safe_methods: std::collections::BTreeSet::new(),
        html_safe_method_owners: Default::default(),
        html_safe_method_defs: Default::default(),
        time_formats: std::collections::BTreeMap::new(),
        generated_helper_methods: std::collections::BTreeMap::new(),
        module_mixins: Vec::new(),
        rake_tasks_source: None,
        rake_task_gaps: Vec::new(),
        rails_application: None,
        concern_filters: roundhouse::det::HashMap::default(),
        concern_filter_refusals: roundhouse::det::HashMap::default(),
        plain_mixins: Default::default(),
        concern_hook_modules: Default::default(),
        modelled_included_hooks: Default::default(),
        unguarded_included_hooks: Default::default(),
        graphql_types: Vec::new(),
        graphql_signatures: Vec::new(),
        concern_spliced_actions: roundhouse::det::HashMap::default(),
        concern_spliced_class_methods: roundhouse::det::HashMap::default(),
        concern_class_side_spans: roundhouse::det::HashSet::default(),
        concern_model_items: roundhouse::det::HashMap::default(),
        concern_guarded_items: roundhouse::det::HashMap::default(),
        concern_included_redefs: roundhouse::det::HashMap::default(),
        validates_with_guards: Vec::new(),
        render_edges: roundhouse::det::HashMap::default(),
        view_feeders: roundhouse::det::HashMap::default(),
        controller_resolutions: roundhouse::det::HashMap::default(),
        sources: vec![],
        wrap_parameters_by_default: false,
        active_job_retry_jitter: 0.0,
        ports_active_model_validator: false,
        each_validator_calls: Vec::new(),
        // Derived from `sources` and `serde(skip)`, like `binary_assets`.
        const_resolver: Default::default(),
        source_index_required: false,
        root: String::new(),
        app_roots: vec!["app".to_string()],
        isolated_namespaces: Vec::new(),
        spinel_int_overflow: None,
    };

    let json = serde_json::to_string_pretty(&app).expect("serialize");
    let back: App = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(app, back, "IR must survive JSON round-trip");
}

#[test]
fn literals_round_trip() {
    let lits = vec![
        Literal::Nil,
        Literal::Bool { value: true },
        Literal::Int { value: 42 },
        Literal::Str { value: "hello".into() },
        Literal::Sym { value: Symbol::from("name") },
    ];
    for lit in lits {
        let json = serde_json::to_string(&lit).unwrap();
        let back: Literal = serde_json::from_str(&json).unwrap();
        assert_eq!(lit, back);
    }
}

// Produced by App::new + ingest_routes on main 8aa7512f6, before scope
// defaults gained spans. Keep the whole schema-v1 envelope as a reader fixture.
const LEGACY_SCOPE_DEFAULTS: &str = include_str!("support/route_scope_defaults_v1.json");

fn scope_defaults(app: &App) -> &IndexMap<Symbol, (String, Span)> {
    let roundhouse::dialect::RouteSpec::Scope { defaults, .. } = &app.routes.entries[0] else {
        panic!("expected scope")
    };
    defaults
}

#[test]
fn legacy_scope_defaults_load_with_unknown_spans() {
    let app: App = serde_json::from_str(LEGACY_SCOPE_DEFAULTS).expect("schema-v1 App loads");
    assert_eq!(app.schema_version, 1);
    assert_eq!(
        scope_defaults(&app)
            .keys()
            .map(Symbol::as_str)
            .collect::<Vec<_>>(),
        ["user_id", "format"]
    );
    assert_eq!(
        scope_defaults(&app)[&Symbol::from("user_id")],
        ("me".into(), sp())
    );
    assert_eq!(
        scope_defaults(&app)[&Symbol::from("format")],
        ("json".into(), sp())
    );
    let roundhouse::dialect::RouteSpec::Scope { entries, .. } = &app.routes.entries[0] else {
        unreachable!()
    };
    let roundhouse::dialect::RouteSpec::Scope { defaults, .. } = &entries[1] else {
        panic!("nested scope")
    };
    assert_eq!(defaults[&Symbol::from("locale")], ("en".into(), sp()));
    let json = serde_json::to_string(&app).unwrap();
    assert_eq!(serde_json::from_str::<App>(&json).unwrap(), app);
}

#[test]
fn scope_defaults_keep_current_spans_and_accept_mixed_shapes() {
    let span = Span {
        file: roundhouse::span::FileId(3),
        start: 17,
        end: 21,
    };
    for mixed in [false, true] {
        let mut json: serde_json::Value = serde_json::from_str(LEGACY_SCOPE_DEFAULTS).unwrap();
        json["routes"]["entries"][0]["entries"][1]["defaults"]["locale"] =
            serde_json::json!(["en", span]);
        let defaults = &mut json["routes"]["entries"][0]["defaults"];
        defaults["user_id"] = serde_json::json!(["me", span]);
        if !mixed {
            defaults["format"] = serde_json::json!(["json", span]);
        }
        let app: App = serde_json::from_value(json).unwrap();
        assert_eq!(
            scope_defaults(&app)[&Symbol::from("user_id")],
            ("me".into(), span)
        );
        assert_eq!(
            scope_defaults(&app)[&Symbol::from("format")],
            ("json".into(), if mixed { sp() } else { span })
        );
        let encoded = serde_json::to_value(&app).unwrap();
        assert_eq!(
            encoded["routes"]["entries"][0]["defaults"]["user_id"],
            serde_json::json!(["me", span])
        );
        assert_eq!(serde_json::from_value::<App>(encoded).unwrap(), app);
    }
}

#[test]
fn scope_defaults_still_allow_absent_and_empty_maps() {
    for absent in [false, true] {
        let mut json: serde_json::Value = serde_json::from_str(LEGACY_SCOPE_DEFAULTS).unwrap();
        json["routes"]["entries"][0]["entries"][1]["defaults"]["locale"] =
            serde_json::json!(["en", sp()]);
        let scope = json["routes"]["entries"][0].as_object_mut().unwrap();
        scope.insert("defaults".into(), serde_json::json!({}));
        if absent {
            scope.remove("defaults");
        }
        let app: App = serde_json::from_value(json).unwrap();
        assert!(scope_defaults(&app).is_empty());
        assert!(
            serde_json::to_value(app).unwrap()["routes"]["entries"][0]
                .get("defaults")
                .is_none()
        );
    }
}

#[test]
fn scope_defaults_reject_malformed_values() {
    for bad in [
        serde_json::json!(null),
        serde_json::json!(42),
        serde_json::json!({}),
        serde_json::json!(["me"]),
        serde_json::json!(["me", null]),
        serde_json::json!(["me", {"file": 1, "start": 2}]),
        serde_json::json!([42, {"file": 1, "start": 2, "end": 4}]),
        serde_json::json!(["me", {"file": 1, "start": 2, "end": 4}, "extra"]),
    ] {
        let mut json: serde_json::Value = serde_json::from_str(LEGACY_SCOPE_DEFAULTS).unwrap();
        json["routes"]["entries"][0]["entries"][1]["defaults"]["locale"] =
            serde_json::json!(["en", sp()]);
        json["routes"]["entries"][0]["defaults"]["format"] = serde_json::json!(["json", sp()]);
        json["routes"]["entries"][0]["defaults"]["user_id"] = bad.clone();
        assert!(
            serde_json::from_value::<App>(json).is_err(),
            "invalid default: {bad}"
        );
    }
}
