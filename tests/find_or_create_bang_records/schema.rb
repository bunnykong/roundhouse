ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "users" do |t|
    t.string "name"
  end
  create_table "articles" do |t|
    t.string "title"
  end
  create_table "follows" do |t|
    t.integer "follower_id"
    t.integer "followed_id"
    t.string "initialized_for"
  end
  create_table "favorites" do |t|
    t.integer "article_id"
    t.integer "user_id"
  end
end
