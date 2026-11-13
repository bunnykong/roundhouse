use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::Analyzer;
use roundhouse::expr::RESOLVED_DATA_FACTORY;
use roundhouse::ident::{ClassId, Symbol};
use roundhouse::ty::Ty;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const SOURCE: &str = r#"module PtyHost
  class Client
    RequestSupport = Data.define(:request, :state, :instance_id)

    def self.build
      RequestSupport.new(request: "spawn", state: :supported, instance_id: nil)
    end

    def self.label
      build.request.upcase
    end
  end
end
"#;

const SIGNATURES: &str = r#"module PtyHost
  class Client
    class RequestSupport
      def request: () -> String
      def state: () -> Symbol
      def instance_id: () -> String?
    end
  end
end
"#;

fn ingest(source: &str, signatures: &str) -> roundhouse::App {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("app/services/client.rb", source),
        ("sig/client.rbs", signatures),
    ]
    .into_iter()
    .map(|(path, text)| (PathBuf::from(path), text.as_bytes().to_vec()))
    .collect();
    roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest Data with sidecar")
}

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/services/client.rb", SOURCE)
        .write("sig/client.rbs", SIGNATURES)
}

#[test]
fn a_sidecar_only_class_gains_data_methods_without_losing_reader_types() {
    let app = ingest(SOURCE, SIGNATURES);
    let analyzer = Analyzer::new(&app);
    let id = ClassId(Symbol::from("PtyHost::Client::RequestSupport"));
    let info = &analyzer.class_registry()[&id];
    let instance = Ty::Class {
        id: id.clone(),
        args: vec![].into(),
    };
    assert_eq!(
        info.class_methods.get(&Symbol::from("new")),
        Some(&instance)
    );
    assert_eq!(
        info.instance_methods.get(&Symbol::from("with")),
        Some(&instance)
    );
    for name in ["request", "state", "instance_id"] {
        assert_eq!(
            info.instance_methods.get(&Symbol::from(name)),
            app.rbs_signatures[&id].get(&Symbol::from(name)),
            "{name} lost its RBS type"
        );
        assert!(
            !info
                .instance_methods
                .contains_key(&Symbol::from(format!("{name}=")))
        );
    }
    assert!(
        matches!(info.instance_methods.get(&Symbol::from("request")), Some(Ty::Fn(fn_ty)) if *fn_ty.ret == Ty::Str)
    );
}

#[test]
fn a_nested_data_factory_with_a_sidecar_executes_after_emission() {
    app().run_ruby(r#"
value = PtyHost::Client.build
raise "factory identity changed" unless value.class == PtyHost::Client::RequestSupport
raise "reader values changed" unless value.request == "spawn" && value.state == :supported && value.instance_id.nil?
raise "typed reader call changed" unless PtyHost::Client.label == "SPAWN"
changed = value.with(instance_id: "daemon-1")
raise "with changed identity or values" unless changed.class == value.class && changed.instance_id == "daemon-1" && value.instance_id.nil?
raise "Data became mutable" unless value.frozen? && !value.respond_to?(:request=)
"#).assert_passes();
}

#[test]
fn a_sidecar_does_not_enable_data_on_targets_without_a_runtime() {
    use roundhouse::diagnostic::DiagnosticKind;
    use roundhouse::project::{BuildTarget, target_files};

    let mut app = ingest(SOURCE, SIGNATURES);
    roundhouse::session::analyze_and_lower(&mut app);
    for &target in BuildTarget::TRANSPILE {
        if matches!(target, BuildTarget::Ruby | BuildTarget::Spinel | BuildTarget::Futamura) {
            continue;
        }
        let (result, diagnostics) = roundhouse::emit::diagnostics::scope(|| {
            target_files(&app, std::path::Path::new("not-a-fixture"), target)
        });
        assert!(result.is_err(), "{target:?} emitted a Data factory");
        assert!(diagnostics.iter().any(|diagnostic| matches!(&diagnostic.kind,
            DiagnosticKind::Unsupported { construct, .. } if construct.as_str() == "Data.define")),
            "{target:?}: {diagnostics:#?}");
    }
}

#[test]
fn a_sidecar_does_not_admit_a_factory_over_a_gem_class() {
    let mut app = ingest(SOURCE, SIGNATURES);
    let id = ClassId(Symbol::from("PtyHost::Client::RequestSupport"));
    app.gem_boundary.classes.insert(id, roundhouse::gem_boundary::GemClass {
        gem: "example".into(),
        ..Default::default()
    });
    Analyzer::new(&app).analyze(&mut app);
    let declaration = app.library_classes.iter()
        .find(|class| class.name.0.as_str() == "PtyHost::Client")
        .and_then(|class| class.constants.first())
        .expect("factory constant retained");
    assert_eq!(declaration.1.decisions & RESOLVED_DATA_FACTORY, 0);
}

#[test]
fn a_sidecar_does_not_admit_a_factory_over_an_existing_class() {
    for (owner, source, signatures) in [
        (
            "PtyHost::Client",
            r#"module PtyHost
  class Client
    class RequestSupport < Object
    end
    RequestSupport = Data.define(:request, :state, :instance_id)
  end
end
"#,
            SIGNATURES,
        ),
        (
            "ActiveSupport",
            "module ActiveSupport\n  TimeZone = Data.define(:request)\nend\n",
            "module ActiveSupport\n  class TimeZone\n    def request: () -> String\n  end\nend\n",
        ),
    ] {
        let mut app = ingest(source, signatures);
        Analyzer::new(&app).analyze(&mut app);
        let declaration = app
            .library_classes
            .iter()
            .find(|class| class.name.0.as_str() == owner)
            .and_then(|class| class.constants.first())
            .expect("factory constant retained");
        assert_eq!(
            declaration.1.decisions & RESOLVED_DATA_FACTORY,
            0,
            "{owner} factory was admitted"
        );
    }
}
