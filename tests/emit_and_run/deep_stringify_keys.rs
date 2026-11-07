use super::emit_and_run;

use roundhouse::project::BuildTarget;
use std::path::Path;
use std::process::Command;

const SOURCE: &str = r#"require "json"
class KeyCodec
  def self.parse(value)
    case value
    when String then JSON.parse(value)
    when Hash then value.deep_stringify_keys
    else raise "not a document"
    end
  end

  def self.literal
    { "outer" => [{ inner: 1 }] }.deep_stringify_keys
  end

  def self.from_hash
    parse({ outer: [{ inner: 1 }, [{ leaf: false }], nil] })
  end

  def self.from_json
    parse('{"outer":[{"inner":1}]}')
  end

  def self.optional_source(flag, counter)
    counter[:calls] = counter.fetch(:calls, 0) + 1
    if flag
      { outer: [{ inner: 1 }] }
    else
      nil
    end
  end

  def self.optional(flag, counter)
    optional_source(flag, counter).deep_stringify_keys
  end

  def self.from_optional
    counter = { calls: 0 }
    converted = optional(true, counter)
    raise "receiver evaluated more than once" unless counter[:calls] == 1
    converted
  end
end

module LocalKeys
  class Hash
    def deep_stringify_keys
      "custom"
    end
  end

  class Codec
    def self.parse(value)
      case value
      when Hash then value.deep_stringify_keys
      else "other"
      end
    end

    def self.read
      parse(Hash.new)
    end
  end
end
"#;

const SCRIPT: &str = r#"
raise "nested keys" unless JSON.generate(KeyCodec.from_hash) == '{"outer":[{"inner":1},[{"leaf":false}],null]}'
raise "string keys" unless JSON.generate(KeyCodec.literal) == '{"outer":[{"inner":1}]}'
raise "JSON branch" unless JSON.generate(KeyCodec.from_json) == '{"outer":[{"inner":1}]}'
raise "custom Hash method" unless LocalKeys::Codec.read == "custom"
raise "optional Hash" unless JSON.generate(KeyCodec.from_optional) == '{"outer":[{"inner":1}]}'
nil_counter = { calls: 0 }
begin
  KeyCodec.optional(false, nil_counter)
  raise "nil receiver did not raise"
rescue NoMethodError
end
raise "nil receiver evaluated more than once" unless nil_counter[:calls] == 1
mixed = ActiveSupport.deep_stringify_keys({ nil => { true => [false, 1] }, 3 => { false => false } })
raise "mixed keys" unless JSON.generate(mixed) == '{"":{"true":[false,1]},"3":{"false":false}}'
source = { "outer" => [{ nested: 1 }] }
converted = KeyCodec.parse(source)
converted["outer"][0]["nested"] = 2
converted["outer"] << { "new" => true }
raise "input changed" unless JSON.generate(source) == '{"outer":[{"nested":1}]}'
puts "deep stringify passed"
"#;

fn codec() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\nend\n")
        .write("app/models/key_codec.rb", SOURCE)
}

#[test]
fn typed_and_class_guarded_hashes_use_the_deep_runtime_helper() {
    use roundhouse::expr::{Expr, ExprNode};
    use roundhouse::ident::{ClassId, Symbol};
    use roundhouse::span::Span;
    use roundhouse::ty::Ty;
    use std::sync::Arc;

    let (tree, errors) = codec().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    let source = std::fs::read_to_string(tree.join("app/models/key_codec.rb")).unwrap();
    assert!(source.contains("ActiveSupport.deep_stringify_keys(value)"), "{source}");
    assert!(source.contains("ActiveSupport.deep_stringify_keys({"), "{source}");
    assert!(source.matches("ActiveSupport.deep_stringify_keys(").count() >= 3, "{source}");
    let custom = std::fs::read_to_string(tree.join("app/models/local_keys/codec.rb")).unwrap();
    assert!(!custom.contains("ActiveSupport.deep_stringify_keys"), "{custom}");
    assert!(custom.contains("value.deep_stringify_keys"), "{custom}");

    fn lower(receiver_ty: Ty) -> (Expr, Expr) {
        let span = Span::synthetic();
        let mut receiver = Expr::new(span, ExprNode::Send {
            recv: None, method: Symbol::from("next_value"), args: vec![], block: None, parenthesized: true,
        });
        receiver.ty = Some(receiver_ty);
        let original = Expr::new(span, ExprNode::Send {
            recv: Some(receiver), method: Symbol::from("deep_stringify_keys"), args: vec![], block: None, parenthesized: true,
        });
        let mut app = roundhouse::App::default();
        app.seeds = Some(original.clone());
        roundhouse::lower::symbolize_keys::apply_symbolize_keys_grounding(&mut app);
        (original, app.seeds.unwrap())
    }

    let hash = Ty::Hash { key: Arc::new(Ty::Sym), value: Arc::new(Ty::Int) };
    for receiver in [
        Ty::Union { variants: vec![hash.clone(), Ty::Nil].into() },
        Ty::Union { variants: vec![hash.clone(), Ty::Hash { key: Arc::new(Ty::Str), value: Arc::new(Ty::Str) }, Ty::Nil].into() },
    ] {
        let (original, lowered) = lower(receiver);
        assert_eq!(lowered.ty, Some(Ty::Hash { key: Arc::new(Ty::Str), value: Arc::new(Ty::Untyped) }));
        let ExprNode::Send { recv: Some(owner), method, args, .. } = &*lowered.node else { panic!("{lowered:?}") };
        assert!(matches!(&*owner.node, ExprNode::Const { path } if path.len() == 1 && path[0].as_str() == "ActiveSupport"));
        assert_eq!(method.as_str(), "deep_stringify_keys");
        let ExprNode::Send { recv: Some(original_receiver), .. } = &*original.node else { unreachable!() };
        assert_eq!(args.len(), 1);
        assert_eq!(&args[0], original_receiver);
    }
    for receiver in [
        Ty::Nil,
        Ty::Union { variants: vec![Ty::Nil].into() },
        Ty::Union { variants: vec![hash.clone(), Ty::Str, Ty::Nil].into() },
        Ty::Union { variants: vec![hash, Ty::Class { id: ClassId(Symbol::from("LocalKeys::Hash")), args: vec![].into() }, Ty::Nil].into() },
    ] {
        let (original, lowered) = lower(receiver);
        assert_eq!(lowered, original);
    }
}

#[test]
fn deep_stringify_keys_matches_rails_without_mutating_nested_containers() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("ruby")
        .arg("--enable-frozen-string-literal")
        .arg(root.join("tests/deep_stringify_keys_runtime.rb"))
        .arg(root.join("runtime/ruby/active_support_ext.rb"))
        .output().expect("ruby");
    assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}

#[test]
fn a_custom_case_matcher_does_not_replace_the_input_type() {
    let source = r#"class AnyPattern
  def self.===(value)
    true
  end
end
class PredicateProbe
  def self.convert(value)
    case value
    when AnyPattern then value.upcase
    else value
    end
  end
  def self.call
    convert("works")
  end
end
"#;
    let (tree, errors) = codec().write("app/models/predicate_probe.rb", source).emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    let rbs = std::fs::read_to_string(tree.join("app/models/predicate_probe.rbs")).unwrap();
    assert!(rbs.contains("def self.convert: (String value) -> String"), "{rbs}");
}

#[test]
fn emitted_deep_stringify_keys_runs_under_cruby() {
    codec().run_ruby(SCRIPT).assert_passes();
}

#[test]
fn a_string_case_arm_keeps_an_untyped_receivers_gradual_send() {
    use roundhouse::expr::{Expr, ExprNode};
    use roundhouse::ty::Ty;

    let mut classes = roundhouse::det::HashMap::default();
    roundhouse::analyze::register_stdlib_classes(&mut classes);
    let methods = roundhouse::runtime_src::parse_methods_with_rbs_in_ctx(
        "module GradualProbe\n  def self.read(value)\n    case value\n    when String then value.encoding\n    else value\n    end\n  end\nend\n",
        "module GradualProbe\n  def self.read: (untyped value) -> untyped\nend\n",
        &classes,
    ).unwrap();

    fn inspect(expr: &Expr, count: &mut usize) {
        if let ExprNode::Send { recv: Some(recv), method, .. } = &*expr.node
            && method.as_str() == "encoding"
        {
            assert!(matches!(recv.ty.as_ref(), Some(Ty::Untyped)), "{recv:?}");
            assert!(matches!(expr.ty.as_ref(), Some(Ty::Untyped)), "{expr:?}");
            *count += 1;
        }
        expr.node.for_each_child(&mut |child| inspect(child, count));
    }

    let mut count = 0;
    inspect(&methods[0].body, &mut count);
    assert_eq!(count, 1);
}

#[test]
fn strict_targets_report_the_missing_deep_stringify_runtime() {
    for target in [BuildTarget::Rust, BuildTarget::Typescript] {
        let (_tree, errors) = codec().emit(target);
        assert!(errors.iter().any(|error| error.contains(
            "ActiveSupport.deep_stringify_keys is only available on the ruby family and spinel"
        )), "{target:?}: {errors:?}");
    }
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn native_deep_stringify_keys_runs_with_emitted_rbs() {
    let (tree, errors) = codec().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    let requires = concat!(
        "require \"json\"\n",
        "require_relative \"runtime/active_support_ext\"\n",
        "require_relative \"app/models/key_codec\"\n",
        "require_relative \"app/models/local_keys/hash\"\n",
        "require_relative \"app/models/local_keys/codec\"\n",
    );
    std::fs::write(tree.join("contract.rb"), format!("{requires}{SCRIPT}")).unwrap();
    let compiler = std::env::var("SPINEL").unwrap_or_else(|_| "spinel".into());
    let compiled = Command::new(compiler)
        .args(["--jobs=1", "-O", "0", "--strict-require", "-I", ".", "--rbs", ".", "contract.rb", "-o", "contract"])
        .current_dir(&tree)
        .output().expect("spinel");
    std::fs::write(tree.join("compile.stdout"), &compiled.stdout).unwrap();
    std::fs::write(tree.join("compile.stderr"), &compiled.stderr).unwrap();
    assert!(compiled.status.success(), "{}\n{}", String::from_utf8_lossy(&compiled.stdout), String::from_utf8_lossy(&compiled.stderr));
    let output = Command::new(tree.join("contract")).current_dir(&tree).output().expect("native contract");
    assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "deep stringify passed\n");
}
