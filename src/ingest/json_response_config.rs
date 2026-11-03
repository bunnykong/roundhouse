//! JSON response defaults and explicit overrides, replayed with the other
//! application settings. Unknown values are refused rather than guessed.
use super::IngestResult;
use super::public_file_server_config::{Setting, assignment, synthesize_setting, text};
use crate::vfs::Vfs;
use ruby_prism::Node;
use std::path::Path;

fn boolean(value: &Node<'_>, source: &str) -> Result<String, String> {
    if value.as_true_node().is_some() || value.as_false_node().is_some() {
        Ok(text(value, source))
    } else {
        Err(format!(
            "JSON escaping setting `{}` is not a literal true or false",
            text(value, source)
        ))
    }
}

const SETTINGS: [Setting; 3] = [
    Setting {
        section: "action_controller",
        key: "escape_json_responses",
        method: "action_controller_escape_json_responses",
        default: "true",
        read: boolean,
    },
    Setting {
        section: "active_support",
        key: "escape_html_entities_in_json",
        method: "active_support_escape_html_entities_in_json",
        default: "true",
        read: boolean,
    },
    Setting {
        section: "active_support",
        key: "escape_js_separators_in_json",
        method: "active_support_escape_js_separators_in_json",
        default: "true",
        read: boolean,
    },
];

pub(super) fn reads(node: &Node<'_>) -> bool {
    SETTINGS
        .iter()
        .any(|setting| assignment(node, setting).is_some())
}

pub(super) fn synthesize<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> IngestResult<Option<String>> {
    let mut source = String::new();
    let application_file = vfs.read(&dir.join("config/application.rb")).is_ok();
    for setting in &SETTINGS {
        let effective = Setting {
            section: setting.section,
            key: setting.key,
            method: setting.method,
            default: if application_file || setting.key == "escape_html_entities_in_json" { "true" } else { "false" },
            read: setting.read,
        };
        if let Some(method) = synthesize_setting(vfs, dir, &effective)? {
            source.push_str(&method);
        } else if application_file {
            source.push_str(&format!("def {}\n  true\nend\n", setting.method));
        }
    }
    Ok((!source.is_empty()).then_some(source))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::{FileTree, MapVfs};
    use std::path::PathBuf;

    #[test]
    fn computed_escaping_values_are_refused() {
        for source in [
            "module App\n  class Application < Rails::Application\n    config.load_defaults ENV.fetch('DEFAULTS')\n  end\nend\n",
            "module App\n  class Application < Rails::Application\n    config.action_controller.escape_json_responses = ENV['ESCAPE'] == '1'\n  end\nend\n",
            "Rails.application.configure do\n  config.active_support.escape_js_separators_in_json = nil\nend\n",
        ] {
            let tree: FileTree = [(
                PathBuf::from("/app/config/application.rb"),
                source.as_bytes().to_vec(),
            )]
            .into_iter()
            .collect();
            let error = synthesize(&MapVfs::new(tree), Path::new("/app")).expect_err(source);
            assert!(
                error.to_string().contains("unsupported construct"),
                "{error}"
            );
        }
    }

    #[test]
    fn unproven_assignments_are_not_hidden_by_framework_defaults() {
        for assignment in [
            "1.times { config.action_controller.escape_json_responses = true }",
            "settings = config; settings.action_controller.escape_json_responses = true",
            "settings = config.action_controller; settings.escape_json_responses = true",
            "SETTINGS = config.action_controller; SETTINGS.escape_json_responses = true",
            "config.action_controller.instance_eval { self.escape_json_responses = true }",
            "config.action_controller[:escape_json_responses] = true",
            "config.action_controller[\"escape_json_responses\"] = true",
            "if ENV['ESCAPE']; config.action_controller.escape_json_responses = true; end",
        ] {
            let source = format!("module App\nclass Application < Rails::Application\nconfig.load_defaults 8.1\n{assignment}\nend\nend\n");
            let tree: FileTree = [(PathBuf::from("/app/config/application.rb"), source.into_bytes())].into_iter().collect();
            let error = synthesize(&MapVfs::new(tree), Path::new("/app")).unwrap_err().to_string();
            assert!(error.contains("escape_json_responses"), "{error}");
        }
    }

    #[test]
    fn compound_json_setting_assignments_are_refused() {
        let mut accepted = Vec::new();
        for (section, key) in [
            ("action_controller", "escape_json_responses"),
            ("active_support", "escape_js_separators_in_json"),
        ] {
            for operator in ["||=", "&&=", "+="] {
                let initializer = format!("config = Rails.application.config.{section}\nconfig.{key} {operator} true\n");
                let tree: FileTree = [
                    (PathBuf::from("/app/config/application.rb"), b"module App\nclass Application < Rails::Application\nconfig.load_defaults 8.1\nend\nend\n".to_vec()),
                    (PathBuf::from("/app/config/initializers/escaping.rb"), initializer.into_bytes()),
                ].into_iter().collect();
                match synthesize(&MapVfs::new(tree), Path::new("/app")) {
                    Err(error) => assert!(error.to_string().contains(key), "{error}"),
                    Ok(_) => accepted.push(format!("{section}.{key} {operator}")),
                }
            }
        }
        assert!(accepted.is_empty(), "unread compound assignments accepted: {accepted:?}");
    }
}
