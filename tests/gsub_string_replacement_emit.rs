//! `s.gsub(pattern, "literal")` on Kotlin, C# and Rust. The three
//! emitters knew only the replacement-TABLE form (`html_escape`'s
//! `gsub(PATTERN, TABLE)`). Kotlin and C# spelled a String replacement as
//! a table lookup — `("_")[it.value]` and `"_".GetValueOrDefault(…)` —
//! and Rust left `name.gsub(…)`, none of which compiles. The runtime's
//! `xml_name_escape` is a `gsub(CONSTANT, "_")` on every attribute name.
//! (`$` in a replacement: the unit tests beside the Kotlin and C# arms;
//! Rust passes it through `regex::NoExpand`.) The compile is the kotlin,
//! C# and rust toolchain lanes.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

use roundhouse::project::BuildTarget;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
}

fn grep_tree(root: &std::path::Path, ext: &str) -> String {
    let mut out = String::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == ext) {
                out.push_str(&std::fs::read_to_string(&path).unwrap());
            }
        }
    }
    out
}

#[test]
fn kotlin_gsub_with_a_string_replaces_with_it() {
    let (emitted, errors) = app().emit(BuildTarget::Kotlin);
    assert!(errors.is_empty(), "{errors:?}");
    let all = grep_tree(&emitted, "kt");
    assert!(all.contains("XML_NAME_INVALID_CHAR.replace(name, \"_\")"), "xml_name_escape not emitted as a plain replace");
    assert!(!all.contains("(\"_\")[it.value]"), "a String replacement still emitted as a table lookup");
}

#[test]
fn csharp_gsub_with_a_string_replaces_with_it() {
    let (emitted, errors) = app().emit(BuildTarget::CSharp);
    assert!(errors.is_empty(), "{errors:?}");
    let all = grep_tree(&emitted, "cs");
    assert!(all.contains("XML_NAME_INVALID_CHAR.Replace(name, \"_\")"), "xml_name_escape not emitted as a plain Replace");
    assert!(!all.contains("\"_\".GetValueOrDefault"), "a String replacement still emitted as a table lookup");
}

#[test]
fn rust_gsub_with_a_string_replaces_with_it() {
    let (emitted, errors) = app().emit(BuildTarget::Rust);
    assert!(errors.is_empty(), "{errors:?}");
    let all = grep_tree(&emitted, "rs");
    assert!(
        all.contains("XML_NAME_INVALID_CHAR.replace_all(&name.clone(), regex::NoExpand(&*\"_\")).into_owned()"),
        "xml_name_escape not emitted as replace_all"
    );
    assert!(!all.contains("name.gsub("), "a String replacement left as a gsub call");
}
