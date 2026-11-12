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
