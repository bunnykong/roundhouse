//! The shared Spinel builds of the spinel-framework lane (`scripts/ci-spinel-framework.sh`).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use emit_and_run::bundle::merged_listing;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn spinel_bundle_build() {
    let (Some(collected), Some(out)) =
        (std::env::var_os("RH_SPINEL_BUNDLE_COLLECT"), std::env::var_os("RH_SPINEL_BUNDLE_OUT"))
    else {
        return;
    };
    emit_and_run::bundle::build_all(std::path::Path::new(&collected), std::path::Path::new(&out));
}

fn merged(path: &str, a: &str, b: &str) -> Option<String> {
    merged_listing(path, a.as_bytes(), b.as_bytes()).map(|m| String::from_utf8(m).unwrap())
}

#[test]
fn two_schemas_share_a_build_when_their_tables_differ() {
    let a = "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\n  add_index \"widgets\", [\"name\"]\nend\n";
    let b = "ActiveRecord::Schema.define do\n  create_table \"members\", force: :cascade do |t|\n    t.string \"email\"\n  end\nend\n";
    assert_eq!(
        merged("db/schema.rb", a, b).unwrap(),
        "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\n  add_index \"widgets\", [\"name\"]\n  create_table \"members\", force: :cascade do |t|\n    t.string \"email\"\n  end\nend\n"
    );
    assert_eq!(merged("db/schema.rb", a, a).unwrap(), a);
}

#[test]
fn a_table_declared_twice_differently_does_not_share_a_build() {
    let a = "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n";
    let b = "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.integer \"rank\"\n  end\nend\n";
    assert_eq!(merged("db/schema.rb", a, b), None);
    let symbol = "ActiveRecord::Schema.define do\n  create_table :widgets, force: :cascade do |t|\n    t.integer \"rank\"\n  end\nend\n";
    assert_eq!(merged("db/schema.rb", a, symbol), None);
    let versioned = "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table \"members\" do |t|\n  end\nend\n";
    assert_eq!(merged("db/schema.rb", a, versioned), None);
}

#[test]
fn routes_share_a_build_unless_one_request_goes_two_ways() {
    let a = "Rails.application.routes.draw do\n  get \"/search\", to: \"searches#show\"\nend\n";
    let b = "Rails.application.routes.draw do\n  resources :widgets, only: [:show]\nend\n";
    assert_eq!(
        merged("config/routes.rb", a, b).unwrap(),
        "Rails.application.routes.draw do\n  get \"/search\", to: \"searches#show\"\n  resources :widgets, only: [:show]\nend\n"
    );
    let empty = "Rails.application.routes.draw do\nend\n";
    assert_eq!(merged("config/routes.rb", empty, a).unwrap(), a);
    let elsewhere = "Rails.application.routes.draw do\n  get \"/search\", to: \"finds#show\"\nend\n";
    assert_eq!(merged("config/routes.rb", a, elsewhere), None);
    assert_eq!(merged("app/models/widget.rb", a, b), None);
}

#[test]
fn a_shared_case_waits_on_its_own_group_only() {
    let dir = std::env::temp_dir().join(format!("rh-bundle-wait-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("done")).unwrap();
    std::fs::write(dir.join("pending.json"), r#"["built", "refused", "unsettled"]"#).unwrap();
    assert_eq!(emit_and_run::bundle::shared_case(&dir, "alone"), None);
    std::fs::write(dir.join("done/refused.json"), "null").unwrap();
    assert_eq!(emit_and_run::bundle::shared_case(&dir, "refused"), None);
    let writer = {
        let dir = dir.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(700));
            std::fs::write(dir.join("done/built.json"), r#"["g0", 1]"#).unwrap();
        })
    };
    assert_eq!(emit_and_run::bundle::shared_case(&dir, "built"), Some(("g0".to_string(), 1)));
    writer.join().unwrap();
    std::fs::write(dir.join("finished"), "").unwrap();
    assert_eq!(emit_and_run::bundle::shared_case(&dir, "unsettled"), None);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn contract_cases_adding_routes_share_a_build_and_a_rewritten_route_does_not() {
    let dir = std::env::temp_dir().join(format!("rh-bundle-routes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (base, collected) = (dir.join("base"), dir.join("collected"));
    std::fs::create_dir_all(base.join("config")).unwrap();
    std::fs::create_dir_all(&collected).unwrap();
    std::fs::write(base.join("config/routes.rb"), "Rails.application.routes.draw do\n  resources :articles do\n  end\nend\n").unwrap();
    let anchor = "  resources :articles do";
    let case = |key: &str, with: &str, file: &str| {
        let v = serde_json::json!({
            "key": key, "test": key, "base": base.to_str().unwrap(), "flags": [], "script": "",
            "edits": [
                {"write": file, "content": "class Probe\nend\n"},
                {"replace": "config/routes.rb", "find": anchor, "with": with},
            ],
        });
        std::fs::write(collected.join(format!("{key}.json")), v.to_string()).unwrap();
    };
    case("kinds", "  get \"kinds\" => \"kinds#index\"\n  resources :articles do", "app/controllers/kinds_controller.rb");
    case("json", "  get \"article_json/:id\" => \"article_json#show\"\n  resources :articles do", "app/controllers/article_json_controller.rb");
    case("rewritten", "  resources :articles, only: [:index] do", "app/controllers/other_controller.rb");
    let out = dir.join("plan.json");
    emit_and_run::bundle::plan(&collected, &out);
    let units: Vec<Vec<String>> = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(units, vec![vec!["json".to_string(), "kinds".to_string()]]);
    std::fs::remove_dir_all(&dir).unwrap();
}
