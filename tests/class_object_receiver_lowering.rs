//! Metadata consumers must inspect the represented class without changing
//! the receiver expression or turning stored class values into instances.
use std::collections::HashMap;
use std::path::PathBuf;
use roundhouse::{App, Expr, ExprNode};
use roundhouse::analyze::Analyzer;
use roundhouse::ty::Ty;

const SCHEMA: &str = "ActiveRecord::Schema.define do\n  create_table :books do |t|\n    t.string :name\n    t.integer :rank\n  end\nend\n";

fn analyzed(source: &str) -> App {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", SCHEMA),
        ("app/models/book.rb", "class Book < ApplicationRecord\n  scope :named, -> { where(name: \"x\") }\nend\n"),
        ("app/models/edition.rb", "class Edition < Book\nend\n"),
        ("app/models/catalog/book.rb", "class Catalog::Book < ApplicationRecord\n  self.table_name = \"books\"\nend\n"),
        ("lib/calls.rb", source),
    ].into_iter().map(|(p, s)| (PathBuf::from(p), s.as_bytes().to_vec())).collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).unwrap();
    Analyzer::new(&app).analyze(&mut app);
    app
}

fn is_class_value(ty: &Ty) -> bool {
    ty.is_class_object() || matches!(ty, Ty::Union { variants } if variants.iter().any(Ty::is_class_object)
        && variants.iter().all(|v| matches!(v, Ty::Nil) || v.is_class_object()))
}

fn body<'a>(app: &'a App, name: &str) -> &'a Expr {
    &app.library_classes.iter().find(|c| c.name.0.as_str() == "Calls").unwrap()
        .methods.iter().find(|m| m.name.as_str() == name).unwrap().body
}

fn visit(e: &Expr, f: &mut impl FnMut(&Expr)) {
    f(e);
    e.node.for_each_child(&mut |c| visit(c, f));
}

#[test]
fn range_splitting_follows_class_values_through_every_receiver_form() {
    let receivers = [
        ("constant", "", "", "Book"),
        ("qualified", "", "", "::Book"),
        ("nested", "", "", "Catalog::Book"),
        ("inherited", "", "", "Edition"),
        ("alias", "", "", "ALIAS"),
        ("local", "", "klass = Book", "klass"),
        ("default", "klass = Book", "", "klass"),
        ("ivar", "", "@klass = Book", "@klass"),
        ("return", "", "", "book_class"),
        ("identity", "", "", "Book.itself"),
        ("array", "", "classes = [Book]", "classes[0]"),
        ("hash", "", "classes = { entry: Book }", "classes[:entry]"),
        ("relation", "", "", "Book.all"),
    ];
    for (label, params, setup, recv) in receivers {
        for terminal in ["where", "find_by", "find_by!"] {
            for (conditions, ranges) in [
                ("id: 1..3, name: \"x\"", 1),
                ("name: \"x\", id: 1...3", 1),
                ("id: ..3, name: \"x\"", 1),
                ("name: \"x\", id: 1..", 1),
                ("id: 1..3, rank: 2...8", 2),
                ("id: 1..3, name: \"x\", rank: 2..8", 2),
            ] {
                let source = format!("class Calls\n  ALIAS = Book\n  def self.book_class = Book\n  def self.probe({params})\n    {setup}\n    [{recv}.{terminal}({conditions})]\n  end\nend\n");
                let mut app = analyzed(&source);
                let mut typed_receivers = 0;
                visit(body(&app, "probe"), &mut |e| {
                    if let ExprNode::Send { recv: Some(r), method, .. } = &*e.node {
                        if method.as_str() == terminal && (r.ty.as_ref().is_some_and(|ty| is_class_value(ty)) || label == "relation") {
                            typed_receivers += 1;
                        }
                    }
                });
                assert_eq!(typed_receivers, 1, "{label}: receiver was not a class value: {source}");
                let diags = roundhouse::lower::where_range_split::apply_where_range_split(&mut app);
                assert!(diags.is_empty(), "{label}: {diags:?}");
                let mut split_ranges = 0;
                visit(body(&app, "probe"), &mut |e| {
                    if let ExprNode::Send { method, args, .. } = &*e.node {
                        if matches!(method.as_str(), "where" | "find_by" | "find_by!") {
                            if let [arg] = args.as_slice() {
                                if let ExprNode::Hash { entries, .. } = &*arg.node {
                                    let n = entries.iter().filter(|(_, v)| matches!(&*v.node, ExprNode::Range { .. })).count();
                                    assert!(n == 0 || entries.len() == 1, "{label}: mixed range survived: {e:?}");
                                    split_ranges += n;
                                }
                            }
                        }
                    }
                    if let Some(Ty::Relation { of }) = &e.ty {
                        assert_ne!(of.0.as_str(), "Class", "{label}: relation used the wrapper as its model");
                        // Synthesized hops carry the receiver's span. Existing
                        // inherited-call result types can name the base model.
                        if matches!(&*e.node, ExprNode::Send { recv: Some(r), method, .. }
                            if method.as_str() == "where" && e.span == r.span) {
                            let expected = match label { "nested" => "Catalog::Book", "inherited" => "Edition", _ => "Book" };
                            assert_eq!(of.0.as_str(), expected, "{label}: split hop lost its model");
                        }
                    }
                });
                assert_eq!(split_ranges, ranges, "{label}: {terminal}({conditions})");
                let once = body(&app, "probe").clone();
                roundhouse::lower::where_range_split::apply_where_range_split(&mut app);
                assert_eq!(body(&app, "probe"), &once, "split must be idempotent");
            }
        }
    }
}

#[test]
fn derived_query_hops_keep_the_represented_model() {
    for terminal in ["exists?", "destroy_by", "delete_by", "find_sole_by"] {
        let mut app = analyzed(&format!("class Calls\n  def self.probe\n    Book.{terminal}(id: 1..3, name: \"x\")\n  end\nend\n"));
        roundhouse::lower::apply_exists_conditions_lowering(&mut app);
        roundhouse::lower::apply_destroy_by_lowering(&mut app);
        roundhouse::lower::where_range_split::apply_where_range_split(&mut app);
        let mut hops = 0;
        visit(body(&app, "probe"), &mut |e| {
            if let ExprNode::Send { method, .. } = &*e.node {
                if method.as_str() == "where" {
                    hops += 1;
                    assert!(matches!(&e.ty, Some(Ty::Relation { of }) if of.0.as_str() == "Book"), "{terminal}: {e:?}");
                }
            }
        });
        assert_eq!(hops, 2, "{terminal}: expected a range hop and a sibling-condition hop");
    }
}

#[test]
fn range_splitting_keeps_receiver_and_bound_evaluation_once_in_nested_calls() {
    let mut app = analyzed(r#"class Calls
  def self.book_class = Book
  def self.lo = 1
  def self.hi = 3
  def self.name = "x"
  def self.probe
    [book_class.where(id: lo..hi, name: name), Book.where(id: 1..3, name: Book.find_by(id: 1..3, name: "x").name)]
  end
end
"#);
    roundhouse::lower::where_range_split::apply_where_range_split(&mut app);
    let mut names = Vec::new();
    visit(body(&app, "probe"), &mut |e| {
        if let ExprNode::Send { method, args, .. } = &*e.node {
            if ["book_class", "lo", "hi"].contains(&method.as_str()) { names.push(method.as_str().to_string()); }
            if let [arg] = args.as_slice() {
                if let ExprNode::Hash { entries, .. } = &*arg.node {
                    assert!(entries.len() == 1, "nested range conditions were not split: {e:?}");
                }
            }
        }
    });
    assert_eq!(names, ["book_class", "lo", "hi"]);
}

#[test]
fn literal_query_reflection_uses_class_metadata_for_aliases_and_respects_overrides() {
    for receiver in ["Book", "Book.itself", "klass", "@klass", "ALIAS"] {
        for reflect in ["send", "__send__", "public_send"] {
            let source = format!("class Calls\n  ALIAS = Book\n  def self.probe\n    klass = Book\n    @klass = Book\n    {receiver}.{reflect}(:named)\n  end\nend\n");
            let app = analyzed(&source);
            let assocs = roundhouse::lower::scope_chain::build_assoc_registry(&app.models);
            let mut e = body(&app, "probe").clone();
            roundhouse::lower::scope_chain::ground_literal_model_dispatch(&mut e, &app, &assocs);
            let mut reflected = 0;
            visit(&e, &mut |e| {
                if matches!(&*e.node, ExprNode::Send { method, .. } if method.as_str() == reflect) { reflected += 1; }
            });
            assert_eq!(reflected, 0, "{receiver}.{reflect}: {e:?}");
        }
    }
    let tree = [
        (PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec()),
        (PathBuf::from("app/models/book.rb"), b"class Book < ApplicationRecord\n  def self.named = 42\n  def self.probe = Book.public_send(:named)\nend\n".to_vec()),
    ].into_iter().collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).unwrap();
    Analyzer::new(&app).analyze(&mut app);
    let mut e = app.models[0].methods().find(|m| m.name.as_str() == "probe").unwrap().body.clone();
    let assocs = roundhouse::lower::scope_chain::build_assoc_registry(&app.models);
    roundhouse::lower::scope_chain::ground_literal_model_dispatch(&mut e, &app, &assocs);
    assert!(matches!(&*e.node, ExprNode::Send { method, .. } if method.as_str() == "public_send"));
}

#[test]
fn explicit_and_implicit_class_self_ranges_keep_the_model() {
    for recv in ["", "self."] {
        let tree = [
            (PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec()),
            (PathBuf::from("app/models/book.rb"), format!("class Book < ApplicationRecord\n  def self.probe\n    {recv}where(id: 1..3, name: \"x\")\n  end\nend\n").into_bytes()),
        ].into_iter().collect();
        let mut app = roundhouse::ingest::ingest_app_from_tree(tree).unwrap();
        roundhouse::session::analyze_and_lower(&mut app);
        let emitted = roundhouse::emit::ruby::emit_lowered_models(&app).into_iter()
            .map(|f| f.content).collect::<Vec<_>>().join("\n");
        assert!(emitted.contains("books.id >= ? AND books.id <= ?"), "{recv}: {emitted}");
        assert!(emitted.contains(".where(name: \"x\")"), "{recv}: {emitted}");
    }
}
