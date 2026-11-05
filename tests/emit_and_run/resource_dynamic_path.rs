//! `resources :notes, path: ":status/notes"`: the segment is a param of
//! every route the resource draws, and the first argument of each helper,
//! as Rails 8.1 builds it (`notes` requires `[:status]`, `note`
//! `[:status, :id]`). mission_control-jobs' engine draws its jobs this way.

use super::emit_and_run;

const CONTROLLER: &str = r##"class NotesController < ApplicationController
  def index
    render plain: "index #{params[:status]} #{notes_path("done")}"
  end

  def show
    render plain: "show #{params[:status]} #{params[:id]} #{note_path("open", 7)}"
  end
end
"##;

const TEST: &str = r##"require "test_helper"

class NotesControllerTest < ActionDispatch::IntegrationTest
  test "a dynamic resources path binds its segment" do
    get "/failed/notes"
    assert_equal "index failed /done/notes", response.body
    get "/failed/notes/3"
    assert_equal "show failed 3 /open/notes/7", response.body
  end
end
"##;

#[test]
fn a_dynamic_resources_path_binds_its_segment() {
    let run = emit_and_run::real_blog()
        .edit("config/routes.rb", "  root \"articles#index\"\n", "  root \"articles#index\"\n  resources :notes, only: [:index, :show], path: \":status/notes\"\n")
        .write("app/controllers/notes_controller.rb", CONTROLLER)
        .write("test/controllers/notes_controller_test.rb", TEST)
        .run_test("test/controllers/notes_controller_test.rb");
    run.assert_passes();
    assert!(run.stdout.contains("1 tests passed"), "{}", run.stdout);
}
