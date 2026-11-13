//! #2087: a namespaced controller rendering through
//! active_model_serializers, on TypeScript and Elixir. These pin the
//! emitted shape; `tsc` runs on the TypeScript tree in
//! `typescript_toolchain::real_blog_namespaced_serializer_render_tsc_passes`,
//! and `mix` on the Elixir tree in
//! `elixir_toolchain::real_blog_namespaced_serializer_render_runs`.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;
#[path = "support/namespaced_serializer.rs"]
mod namespaced_serializer;

use roundhouse::project::BuildTarget;

fn read(emitted: &std::path::Path, path: &str) -> String {
    std::fs::read_to_string(emitted.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// The entry files import the namespaced controller's exported class
/// under an alias, and register it under the route table's key: they
/// wrote `import { Api::ArticlesController }` and `api::articles:`.
#[test]
fn typescript_registers_a_namespaced_controller_under_the_route_key() {
    let (emitted, errors) = namespaced_serializer::overlay().emit(BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    for (path, prefix) in [("main.ts", "./app/controllers/"), ("test/_runtime/setup.ts", "../../app/controllers/")] {
        let entry = read(&emitted, path);
        let import = format!("import {{ ArticlesController as ApiArticlesController }} from \"{prefix}api::articles_controller.js\";");
        assert!(entry.contains(&import), "{path}:\n{entry}");
        assert!(entry.contains("    api__articles: ApiArticlesController,\n"), "{path}:\n{entry}");
        assert!(entry.contains("    articles: ArticlesController,\n"), "{path}:\n{entry}");
        assert!(!entry.contains("Api::"), "{path}:\n{entry}");
    }
    let routes = read(&emitted, "app/routes.ts");
    assert!(routes.contains("\"api__articles\""), "{routes}");
}

/// The serializer's `options = {}` keeps its default, so a render's
/// one-argument `new ArticleSerializer(record)` type-checks.
#[test]
fn typescript_serializer_options_keep_their_default() {
    let (emitted, errors) = namespaced_serializer::overlay().emit(BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let serializer = read(&emitted, "app/models/article_serializer.ts");
    assert!(serializer.contains("constructor(object: any, options: any = {}) {"), "{serializer}");
    let controller = read(&emitted, "app/controllers/api::articles_controller.ts");
    assert!(controller.contains("new ArticleSerializer(__ams_item)"), "{controller}");
}

/// The namespaced serializer is declared in its namespace, imported by
/// its root, and constructed with `new`: the render called
/// `Api.ArticleSerializer.new(...)` with nothing named `Api` in scope.
#[test]
fn typescript_constructs_a_namespaced_serializer() {
    let (emitted, errors) = namespaced_serializer::overlay().emit(BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let api = read(&emitted, "app/models/api.ts");
    assert!(api.contains("export namespace Api {"), "{api}");
    assert!(api.contains("export class ArticleSerializer {"), "{api}");
    assert!(!emitted.join("app/models/api::article_serializer.ts").exists());
    let controller = read(&emitted, "app/controllers/api::articles_controller.ts");
    assert!(controller.contains("import { Api } from \"../../app/models/api.js\";"), "{controller}");
    assert!(controller.contains("new Api.ArticleSerializer("), "{controller}");
    assert!(!controller.contains(".new("), "{controller}");
}

/// Elixir emits a module per serializer, the namespaced one under its
/// full name: the controller called `ArticleSerializer.new` and
/// `Api.ArticleSerializer.new`, and no file defined either.
#[test]
fn elixir_emits_the_serializer_modules_the_renders_call() {
    let (emitted, errors) = namespaced_serializer::overlay().emit(BuildTarget::Elixir);
    assert!(errors.is_empty(), "{errors:?}");
    let top = read(&emitted, "lib/articleserializer.ex");
    assert!(top.contains("defmodule ArticleSerializer do"), "{top}");
    assert!(top.contains("def new(object, options \\\\ %{}) do"), "{top}");
    assert!(top.contains("def as_json_str(record) do"), "{top}");
    let nested = read(&emitted, "lib/api_articleserializer.ex");
    assert!(nested.contains("defmodule Api.ArticleSerializer do"), "{nested}");
    assert!(nested.contains("def as_json_str(record) do"), "{nested}");
    let controller = read(&emitted, "lib/api_articlescontroller.ex");
    assert!(controller.contains("Api.ArticleSerializer.new(Article.find("), "{controller}");
    assert!(controller.contains("ArticleSerializer.new(__ams_item)"), "{controller}");
}
