use super::emit_and_run;

/// `record.send(record.class.primary_key)` (orm_adapter's `conditions_to_fields`) on a value the analyzer types a
/// record, where every model's key is `id`: `record.id`, which answers what Rails 8.1.4's send does. On a value of
/// no known type the send stays as written, unless the value's own `is_a?(::ActiveRecord::Base)` guards it.
const KEYS: &str = r#"class KeyReader
  def self.of_article(article)
    article.send(article.class.primary_key)
  end

  def self.of_anything(value)
    value.send(value.class.primary_key)
  end

  def self.of_tested(value)
    value.send(value.class.primary_key) if value.is_a?(::ActiveRecord::Base)
  end
end
"#;

#[test]
fn a_records_primary_key_send_reads_its_id() {
    let (tree, errors) = emit_and_run::real_blog()
        .write("app/models/key_reader.rb", KEYS)
        .edit("app/controllers/articles_controller.rb", "  def index\n", "  def index\n    KeyReader.of_article(Article.new)\n")
        .emit(roundhouse::build_target::BuildTarget::Ruby);
    let text = std::fs::read_to_string(tree.join("app/models/key_reader.rb")).unwrap_or_default();
    assert!(text.contains("article.id"), "{text}\n{errors:#?}");
    assert!(text.contains("value.send(value.class.primary_key)\n"), "{text}\n{errors:#?}");
    assert!(text.contains("value.id if value.is_a?"), "{text}\n{errors:#?}");
}

#[test]
fn a_records_primary_key_send_answers_as_rails() {
    let run = emit_and_run::real_blog()
        .write("app/models/key_reader.rb", KEYS)
        .edit("app/controllers/articles_controller.rb", "  def index\n", "  def index\n    KeyReader.of_article(Article.new)\n")
        .run_ruby("a = Article.create!(title: \"t\", body: \"long enough body\")\ngot = KeyReader.of_article(a)\nraise \"key #{got.inspect}\" unless got == a.id\nraise \"tested\" unless KeyReader.of_tested(a) == a.id && KeyReader.of_tested(1).nil?\nputs \"primary key send passed\"\n");
    run.assert_passes();
    assert!(run.stdout.contains("primary key send passed"), "{}", run.stdout);
}
