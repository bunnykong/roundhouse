//! A key only a `.merge(k: v)` after the permit adds is typed by what the
//! merge writes, and is never read from the request (Rails' permit drops
//! it). lobsters' `tag_params` merges `edit_user_id: @user.id`; the
//! synthesized params class declared it a String, so the Integer the
//! merge assigns was a TypeError on the Spinel tree (POST /mod/tags).

use super::emit_and_run;
use roundhouse::project::BuildTarget;

fn overlay() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  attr_accessor :editor_id\n",
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "      params.expect(article: [ :title, :body ])\n",
            "      params.expect(article: [ :title, :body ]).merge(editor_id: 7)\n",
        )
}

#[test]
fn a_merged_only_key_takes_the_merged_type_on_spinel() {
    let (emitted, errors) = overlay().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let rbs = std::fs::read_to_string(emitted.join("app/models/article_params.rbs")).expect("rbs");
    assert!(rbs.contains("attr_reader editor_id: Integer?"), "{rbs}");
    let rb = std::fs::read_to_string(emitted.join("app/models/article_params.rb")).expect("rb");
    assert!(!rb.contains("\"editor_id\" =>"), "the String-valued to_h leaves it out:\n{rb}");
}

#[test]
fn a_merged_only_key_is_assigned_as_written_and_not_read_from_the_request() {
    let (emitted, errors) = overlay().emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let controller = std::fs::read_to_string(emitted.join("app/controllers/articles_controller.rb")).expect("controller");
    assert!(controller.contains("_p.editor_id = 7\n"), "the Integer stays an Integer:\n{controller}");
    let rb = std::fs::read_to_string(emitted.join("app/models/article_params.rb")).expect("rb");
    assert!(rb.contains("sub, \"title\")"), "{rb}");
    assert!(!rb.contains("sub, \"editor_id\")"), "the request's editor_id is not permitted:\n{rb}");
}

fn untyped_array_overlay() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  attr_accessor :tag_names\n",
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "      params.expect(article: [ :title, :body ])\n",
            "      raw = params[:article]\n      tag_names = raw.respond_to?(:permit) && raw.include?(:tag_names) ? raw[:tag_names] : []\n      tag_names = nil unless tag_names.is_a?(Array) && tag_names.all?(String)\n      params.expect(article: [ :title, :body ]).merge(tag_names:)\n",
        )
}

#[test]
fn a_merged_only_key_takes_an_array_type_with_untyped_elements_on_spinel() {
    let (emitted, errors) = untyped_array_overlay().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let rbs = std::fs::read_to_string(emitted.join("app/models/article_params.rbs")).expect("rbs");
    assert!(rbs.contains("attr_writer tag_names: Array[untyped]?"), "{rbs}");
    let rb = std::fs::read_to_string(emitted.join("app/models/article_params.rb")).expect("rb");
    assert!(!rb.contains("sub, \"tag_names\")"), "the request's tag_names is not permitted:\n{rb}");
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_merged_array_with_untyped_elements_goes_through_the_spinel_writer() {
    let run = untyped_array_overlay().run_spinel_with_rbs(
        r#"params = ArticleParams.new
params.tag_names = ["ruby"]
raise "array" unless params.tag_names == ["ruby"]
params.tag_names = nil
raise "nil" unless params.tag_names.nil?
puts "merged array writer passed"
"#,
    );
    run.assert_passes();
    assert!(run.stdout.contains("merged array writer passed"), "{}", run.stdout);
}

const ITEMS_SCHEMA: &str = r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "items", force: :cascade do |t|
    t.string "title"
    t.string "label"
    t.integer "count"
    t.float "price"
    t.boolean "active"
    t.date "due"
  end
end
"#;

fn items_app(controller: &str) -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", ITEMS_SCHEMA)
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/models/item.rb", "class Item < ApplicationRecord\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n")
        .write("app/controllers/items_controller.rb", controller)
        .write("config/routes.rb", "Rails.application.routes.draw do\n  resources :items, only: %i[create]\nend\n")
}

const POST_ITEM: &str = r##"
body = %({"item":{"title":"t","label":"x","count":"8"}})
env = { "REQUEST_METHOD" => "POST", "PATH_INFO" => "/items", "QUERY_STRING" => "", "CONTENT_TYPE" => "application/json", "CONTENT_LENGTH" => body.bytesize.to_s, "rack.input" => StringIO.new(body) }
status, _headers, out = Main.run_rack(env)
text = +""
out.each { |part| text << part }
puts "#{status} #{text}"
"##;

const MERGED_COLUMNS: &str = r#"class ItemsController < ApplicationController
  def create
    created = Item.create!(item_params)
    built = Item.new(item_params)
    built.save!
    updated = Item.create!(title: "old", label: "old", count: 9, price: 9.5, active: true, due: Date.new(2000, 1, 1))
    updated.update(item_params)
    assigned = Item.new
    assigned.attributes = item_params
    assigned.save!
    render json: [created, built, updated, assigned].map { |item| [item.label, item.count, item.price, item.active, item.due.to_s] }
  end

  private

  def item_params
    params.expect(item: [:title]).merge(label: 7, count: 2.7, price: 3, active: 0, due: Date.new(2026, 1, 2))
  end
end
"#;

#[test]
fn merged_values_of_other_types_reach_the_columns_as_rails_casts_them() {
    let run = items_app(MERGED_COLUMNS).run_ruby(POST_ITEM);
    run.assert_passes();
    let row = r#"["7",2,3.0,false,"2026-01-02"]"#;
    assert!(run.stdout.contains(&format!("200 [{row},{row},{row},{row}]")), "{}", run.stdout);
}

const MIXED_COUNT: &str = r#"class ItemsController < ApplicationController
  def create
    render json: [Item.create!(int_params).count, Item.create!(str_params).count]
  end

  private

  def int_params
    params.expect(item: [:title]).merge(count: 1)
  end

  def str_params
    params.expect(item: [:title]).merge(count: "5")
  end
end
"#;

#[test]
fn a_merged_key_written_as_an_integer_and_a_string_casts_each_value() {
    let (emitted, errors) = items_app(MIXED_COUNT).emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let rbs = std::fs::read_to_string(emitted.join("app/models/item_params.rbs")).expect("rbs");
    assert!(rbs.contains("attr_writer count: (Integer | String | nil)"), "{rbs}");
    let run = items_app(MIXED_COUNT).run_ruby(POST_ITEM);
    run.assert_passes();
    assert!(run.stdout.contains("200 [1,5]"), "{}", run.stdout);
}

const MERGED_HELPER: &str = r#"class ItemsController < ApplicationController
  def create
    Item.create!(item_params)
    head :created
  end

  private

  def item_params
    params.expect(item: [:title]).merge(label: 7, count: 2.7, price: 3, active: 0, due: Date.new(2026, 1, 2))
  end
end
"#;

const TYPED_STORE_USERS: &str = r#"class UsersController < ApplicationController
  def create
    user = User.new(user_params)
    user.save!
    render json: { avatar_source: user.avatar_source, notify: user.notify }
  end

  private

  def user_params
    params.require(:user).permit(:username).merge(avatar_source: 2.7, notify: 0)
  end
end
"#;

#[test]
fn merged_values_for_typed_store_attributes_keep_their_casts() {
    let app = || emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table \"users\", force: :cascade do |t|\n    t.string \"username\"\n    t.text \"settings\"\n  end\nend\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/models/user.rb", "class User < ApplicationRecord\n  typed_store :settings do |s|\n    s.integer :avatar_source\n    s.boolean :notify\n  end\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n")
        .write("app/controllers/users_controller.rb", TYPED_STORE_USERS)
        .write("config/routes.rb", "Rails.application.routes.draw do\n  resources :users, only: %i[create]\nend\n");
    let (emitted, errors) = app().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let model = std::fs::read_to_string(emitted.join("app/models/user.rb")).expect("model");
    assert!(model.contains("instance.avatar_source = p.avatar_source.to_i if"), "{model}");
    assert!(model.contains(r#"instance.notify = !["0", "", "false"].include?((p.notify).to_s) if"#), "{model}");
    let run = app().run_ruby(
        r##"
body = %({"user":{"username":"u"}})
env = { "REQUEST_METHOD" => "POST", "PATH_INFO" => "/users", "QUERY_STRING" => "", "CONTENT_TYPE" => "application/json", "CONTENT_LENGTH" => body.bytesize.to_s, "rack.input" => StringIO.new(body) }
status, _headers, out = Main.run_rack(env)
text = +""
out.each { |part| text << part }
puts "#{status} #{text}"
"##,
    );
    run.assert_passes();
    assert!(run.stdout.contains(r#"200 {"avatar_source":2,"notify":false}"#), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn merged_values_of_other_types_go_through_the_spinel_factory() {
    let run = items_app(MERGED_HELPER).run_spinel_with_rbs(
        r#"params = ItemParams.new
params.title = "t"
params.title_provided = true
params.label = 7
params.label_provided = true
params.count = 2.7
params.count_provided = true
params.price = 3
params.price_provided = true
params.active = 0
params.active_provided = true
params.due = Date.new(2026, 1, 2)
params.due_provided = true
item = Item.from_params(params)
raise "label" unless item.label == "7"
raise "count" unless item.count == 2
raise "price" unless item.price == 3.0
raise "active" unless item.active == false
raise "due" unless item.due == Date.new(2026, 1, 2)
puts "merged factory passed"
"#,
    );
    run.assert_passes();
    assert!(run.stdout.contains("merged factory passed"), "{}", run.stdout);
}
