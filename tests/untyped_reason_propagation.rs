//! Reason metadata must survive the same shared inference paths as values.

use std::collections::HashMap;
use std::path::PathBuf;

fn declarations(source: &str) -> String {
    let tree = HashMap::from([
        (
            PathBuf::from("db/schema.rb"),
            b"ActiveRecord::Schema.define do\nend\n".to_vec(),
        ),
        (PathBuf::from("lib/probe.rb"), source.as_bytes().to_vec()),
    ]);
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    roundhouse::emit::ruby::emit_library(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rbs"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn gradual_reasons_reach_callers_through_class_and_instance_forwarding_chains() {
    for prefix in ["self.", ""] {
        for depth in [1, 2, 4, 6] {
            for alias_local in [false, true] {
                let mut source = format!(
                    "class Probe\n def {prefix}locale_for(x)\n x&.dig(\"lang\") || :en\n end\n"
                );
                for i in 0..depth {
                    let body = if i + 1 == depth {
                        "JSON.parse(raw)".to_owned()
                    } else {
                        format!("forward{}(raw)", i + 1)
                    };
                    source.push_str(&format!("def {prefix}forward{i}(raw)\n {body}\n end\n"));
                }
                let body = if alias_local {
                    "value = forward0(raw)\n locale_for(value)"
                } else {
                    "locale_for(forward0(raw))"
                };
                source.push_str(&format!("def {prefix}resolve(raw)\n {body}\n end\n end\n"));
                let rbs = declarations(&source);
                for method in ["locale_for", "resolve"] {
                    assert!(
                        rbs.contains(&format!("def {prefix}{method}: (untyped"))
                            && rbs.lines().any(|line| {
                                line.contains(&format!("def {prefix}{method}:"))
                                    && line.ends_with("-> untyped")
                            }),
                        "prefix={prefix:?}, depth={depth}, alias={alias_local}: {rbs}"
                    );
                }
            }
        }
    }
}

#[test]
fn gradual_reasons_cross_distinct_owners_on_the_dirty_frontier() {
    for instance in [false, true] {
        for depth in [4, 6] {
            let call = |i| {
                if instance {
                    format!("Forward{i}.new.call(raw)")
                } else {
                    format!("Forward{i}.call(raw)")
                }
            };
            let mut source = format!(
                "module Probe\n def self.locale_for(x)\n x&.dig(\"lang\") || :en\n end\n\
                 def self.resolve(raw)\n value = {}\n locale_for(value)\n end\n end\n",
                call(0)
            );
            for i in 0..depth {
                let prefix = if instance { "" } else { "self." };
                let body = if i + 1 == depth {
                    "JSON.parse(raw)".to_owned()
                } else {
                    call(i + 1)
                };
                source.push_str(&format!(
                    "class Forward{i}\n def {prefix}call(raw)\n {body}\n end\n end\n"
                ));
            }
            // Keep the frontier below its whole-app fallback budget.
            for i in 0..16 {
                source.push_str(&format!("class Padding{i}; end\n"));
            }
            let rbs = declarations(&source);
            assert!(
                rbs.contains("def self.resolve: (untyped raw) -> untyped"),
                "instance={instance}, depth={depth}: {rbs}"
            );
        }
    }
}

#[test]
fn equal_container_branches_keep_gradual_reasons_in_both_orders() {
    for (pending, gradual, access) in [
        ("[cycle]", "[parse(raw)]", ".first"),
        (
            "{\"lang\" => cycle}",
            "{\"lang\" => parse(raw)}",
            "[\"lang\"]",
        ),
        ("{cycle => 1}", "{parse(raw) => 1}", ".keys.first"),
        ("[[cycle]]", "[[parse(raw)]]", ".first.first"),
        (
            "[{\"lang\" => cycle}]",
            "[{\"lang\" => parse(raw)}]",
            ".first[\"lang\"]",
        ),
    ] {
        for reversed in [false, true] {
            let (left, right) = if reversed {
                (gradual, pending)
            } else {
                (pending, gradual)
            };
            let source = format!(
                "module Probe\n\
                 def self.cycle\n cycle\n end\n\
                 def self.resolve(raw, flag)\n\
                 values = flag ? {left} : {right}\n values{access} || :en\n end\n\
                 def self.parse(raw)\n JSON.parse(raw)\n end\n end\n"
            );
            let rbs = declarations(&source);
            assert!(
                rbs.lines()
                    .any(|line| line.contains("def self.resolve:") && line.ends_with("-> untyped")),
                "pending={pending}, reversed={reversed}: {rbs}"
            );
        }
    }
}

#[test]
fn pending_conversion_chains_settle_on_concrete_elements() {
    for prefix in ["self.", ""] {
        for depth in [2, 4] {
            for (conversion, value, access) in [
                ("to_a", "[\"ok\"]", ".first"),
                ("to_ary", "[\"ok\"]", ".first"),
                ("to_h", "{\"lang\" => \"ok\"}", "[\"lang\"]"),
                ("to_a.to_a", "[\"ok\"]", ".first"),
                ("to_h.to_h", "{\"lang\" => \"ok\"}", "[\"lang\"]"),
            ] {
                let mut source = format!(
                    "class Probe\n def {prefix}value\n\
                     receiver = forward0\n receiver.{conversion}{access} || \"default\"\n end\n"
                );
                for i in 0..depth {
                    let body = if i + 1 == depth {
                        value.to_owned()
                    } else {
                        format!("forward{}", i + 1)
                    };
                    source.push_str(&format!("def {prefix}forward{i}\n {body}\n end\n"));
                }
                source.push_str("end\n");
                let rbs = declarations(&source);
                assert!(
                    rbs.contains(&format!("def {prefix}value: () -> String")),
                    "prefix={prefix:?}, depth={depth}, conversion={conversion}: {rbs}"
                );
            }
        }
    }
}

#[test]
fn pending_nilable_conversion_receivers_do_not_invent_gradual_elements() {
    for (conversion, value, access) in [
        ("to_a", "[\"ok\"]", ".first"),
        ("to_h", "{\"lang\" => \"ok\"}", "[\"lang\"]"),
    ] {
        let source = format!(
            "module Probe\n def self.value\n\
             receiver = [forward0, nil].first\n receiver.{conversion}{access} || \"default\"\n end\n\
             def self.forward0\n forward1\n end\n\
             def self.forward1\n {value}\n end\n end\n"
        );
        let rbs = declarations(&source);
        assert!(
            rbs.contains("def self.value: () -> String"),
            "{conversion}: {rbs}"
        );
    }
}
