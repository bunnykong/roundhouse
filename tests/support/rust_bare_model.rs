//! #1711: real-blog plus an `Account` model with no validations, no
//! timestamp columns, or neither. The Rust model's `save` called
//! `self.validate()` and `_insert_row` called `self.fill_timestamps(true)`
//! whether or not the model defined them.

use super::emit_and_run;

/// real-blog with an `accounts` table (a string `name`, and the two
/// timestamp columns when `timestamped`), an `Account` model (with
/// `validates :name, presence: true` when `validated`), and a model test
/// that saves one.
pub fn app(validated: bool, timestamped: bool) -> emit_and_run::Overlay {
    let schema = std::fs::read_to_string("fixtures/real-blog/db/schema.rb").unwrap();
    let at = schema.find("  create_table").unwrap();
    let timestamps = if timestamped {
        "    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n"
    } else {
        ""
    };
    let table = format!("  create_table \"accounts\", force: :cascade do |t|\n    t.string \"name\"\n{timestamps}  end\n\n");
    let rule = if validated { "  validates :name, presence: true\n" } else { "" };
    emit_and_run::real_blog()
        .write("db/schema.rb", &format!("{}{table}{}", &schema[..at], &schema[at..]))
        .write("app/models/account.rb", &format!("class Account < ApplicationRecord\n{rule}end\n"))
        .write("test/models/account_test.rb", TEST)
}

const TEST: &str = "require \"test_helper\"\n\nclass AccountTest < ActiveSupport::TestCase\n  test \"an account saves\" do\n    account = Account.new(name: \"Ada\")\n    assert account.save\n    assert_equal 1, Account.count\n  end\nend\n";

/// The three variants #1711 names: (validated, timestamped).
pub const VARIANTS: [(bool, bool); 3] = [(false, false), (true, false), (false, true)];
