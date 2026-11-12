//! A block argument whose expression is a conditional with a `nil`
//! branch, `&(c ? x : nil)`: the block is the proc when `c` holds and no
//! block otherwise.
#![allow(dead_code)]

pub const SOURCE: &str = r#"class Shouter
  SHOUT = proc { |s| s.upcase }

  def self.apply(value, &block)
    block ? block.call(value) : value
  end

  def self.render(value, loud)
    apply(value, &(loud ? SHOUT : nil))
  end
end
"#;

pub const ASSERTIONS: &str = r#"
loud = Shouter.render("hi", true)
raise "loud: #{loud.inspect}" unless loud == "HI"
quiet = Shouter.render("hi", false)
raise "quiet: #{quiet.inspect}" unless quiet == "hi"
puts "block arg conditional passed"
"#;

pub fn overlay() -> super::emit_and_run::Overlay {
    super::emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\nend\n")
        .write("app/services/shouter.rb", SOURCE)
}
