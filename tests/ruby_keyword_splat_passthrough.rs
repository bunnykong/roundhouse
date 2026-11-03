//! A `**kwargs` the shared keyword lowering makes positional (legacy
//! policy: a dynamic `send`, an ordinary callee) stays a keyword splat in
//! emitted Ruby. In Ruby 3 a positional Hash does not bind keyword
//! parameters, so `@connection.send(meth, *args, kwargs)` raised where the
//! source's `**kwargs` worked.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::process::Command;

const LIBRARY: &str = r##"class KeywordConnection
  def query(sql, limit:, offset: 0)
    "#{sql} #{limit} #{offset}"
  end
end

class KeywordConnectionProxy
  def initialize(connection)
    @connection = connection
  end

  def method_missing(meth, *args, **kwargs, &block)
    @connection.send(meth, *args, **kwargs, &block)
  end

  def respond_to_missing?(meth, include_private = false)
    @connection.respond_to?(meth, include_private) || super
  end

  def relay(options)
    @connection.query("relay", **options)
  end
end
"##;

const ASSERTIONS: &str = r##"
proxy = KeywordConnectionProxy.new(KeywordConnection.new)
got = proxy.query("q", limit: 5, offset: 2)
raise "method_missing: #{got.inspect}" unless got == "q 5 2"
got = proxy.relay({ limit: 3 })
raise "relay: #{got.inspect}" unless got == "relay 3 0"
puts "keyword splat passed"
"##;

#[test]
fn native_and_emitted_ruby_keep_a_forwarded_keyword_splat() {
    let native = Command::new("ruby")
        .arg("-e")
        .arg(format!("{LIBRARY}\n{ASSERTIONS}"))
        .output()
        .expect("native Ruby control");
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    assert_eq!(String::from_utf8_lossy(&native.stdout), "keyword splat passed\n");

    let run = emit_and_run::real_blog()
        .write("app/lib/keyword_connection_proxy.rb", LIBRARY)
        .run_ruby(ASSERTIONS);
    run.assert_passes();
    assert_eq!(run.stdout, "keyword splat passed\n");
    let emitted =
        std::fs::read_to_string(run.emitted.join("app/models/keyword_connection_proxy.rb")).unwrap();
    assert!(emitted.contains("@connection.send(meth, *args, **kwargs, &block)"), "{emitted}");
    assert!(emitted.contains("**options)"), "{emitted}");
}

#[test]
fn typed_stdlib_receiver_keeps_keyword_splats() {
    let source = r#"require "psych"
class KeywordYaml
  def self.read
    options = { symbolize_names: true }
    Psych.safe_load("---\nx: 1\n", **options).keys.first.class.to_s
  end
end
"#;
    let run = emit_and_run::real_blog()
        .write("app/lib/keyword_yaml.rb", source)
        .run_ruby("puts KeywordYaml.read");
    run.assert_passes();
    assert_eq!(run.stdout, "Symbol\n");
}

#[test]
fn empty_keyword_splat_omits_an_optional_positional_hash() {
    let source = r#"class KeywordOptions
  def options(opts = {value: 5})
    opts.fetch(:value, 9) * 10
  end
  def self.options(opts = {value: 5})
    opts.fetch(:value, 9) * 10
  end
  def self.read
    empty = {}
    present = {value: 3}
    [options(**empty), options(**present), options({}), new.options(**empty)].join(",")
  end
end
"#;
    let run = emit_and_run::real_blog()
        .write("app/lib/keyword_options.rb", source)
        .run_ruby("puts KeywordOptions.read");
    run.assert_passes();
    assert_eq!(run.stdout, "50,30,90,50\n");
}


#[test]
fn positional_options_receive_a_keyword_copy() {
    let source = r#"class KeywordCopy
  def mutate(options = {})
    options[:value] = 9
  end
  def self.mutate(options = {})
    options[:value] = 9
  end
  def self.read
    h = {value: 3}
    mutate(**h)
    self.mutate(**h)
    new.mutate(**h)
    h[:value]
  end
end
"#;
    let run = emit_and_run::real_blog()
        .write("app/lib/keyword_copy.rb", source)
        .run_ruby("puts KeywordCopy.read");
    run.assert_passes();
    assert_eq!(run.stdout, "3\n");
}

#[test]
fn operator_splat_keeps_explicit_keyword_call_syntax() {
    let run = emit_and_run::real_blog()
        .write("app/lib/probe.rb", r#"class Options
  def +(h)
    h.fetch(:value, 9)
  end
end
class Probe
  def self.run
    h = {value: 3}
    Options.new.+(**h)
  end
end
"#)
        .run_ruby("puts Probe.run");
    run.assert_passes();
    assert_eq!(run.stdout, "3\n");
}

#[test]
fn index_writer_splat_keeps_explicit_keyword_call_syntax() {
    let run = emit_and_run::real_blog()
        .write("app/lib/probe.rb", r#"class Options
  def []=(key, h)
    h.fetch(:value, 9)
  end
end
class Probe
  def self.run
    h = {value: 3}
    Options.new.[]=(:key, **h)
  end
end
"#)
        .run_ruby("puts Probe.run");
    run.assert_passes();
    assert_eq!(run.stdout, "3\n");
}

#[test]
fn marked_and_structural_packets_keep_the_same_call_syntax() {
    use roundhouse::expr::{Expr, ExprNode, Literal, ERASED_KEYWORD_SPLAT};
    use roundhouse::{ident::Symbol, span::Span};
    let span = Span::synthetic();
    for method in ["+", "[]=", "[]", "value=", "+@", "-@", "!", "~", "`"] {
        let value = Expr::new(span, ExprNode::Lit { value: Literal::Str { value: "packet".to_string() } });
        let mut marked = value.clone();
        marked.decisions |= ERASED_KEYWORD_SPLAT;
        let structural = Expr::new(span, ExprNode::KeywordSplat { value });
        let call = |packet, block| {
            let mut args = vec![];
            if method == "[]=" {
                args.push(Expr::new(span, ExprNode::Lit { value: Literal::Sym { value: Symbol::from("key") } }));
            }
            args.push(packet);
            Expr::new(span, ExprNode::Send {
                recv: Some(Expr::new(span, ExprNode::SelfRef)), method: Symbol::from(method),
                args, block, parenthesized: true,
            })
        };
        for block in [None, Some(Expr::new(span, ExprNode::Var {
            id: roundhouse::ident::VarId(0), name: Symbol::from("fallback"),
        }))] {
            let has_block = block.is_some();
            let emitted = roundhouse::emit::ruby::emit_expr(&call(marked.clone(), block.clone()));
            assert_eq!(emitted,
                       roundhouse::emit::ruby::emit_expr(&call(structural.clone(), block)), "{method}");
            if has_block { assert!(emitted.contains("&fallback"), "{emitted}"); }
        }
    }
}
