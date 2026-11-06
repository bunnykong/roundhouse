//! The Spinel side of rubys/roundhouse#810: a private `*_params` helper
//! with no recognized permit list used to be pinned to
//! `Hash[Symbol, untyped]` in its `.rbs` sidecar by its name alone, and
//! `spin build` refused the mismatch between the guard filter's real
//! (`bool`-shaped) return and that pinned Hash pointer. The CRuby-level
//! sidecar check lives in `tests/params_helper_no_permit_untyped.rs`.
//!
//! This is the Spinel compile+run half, on (close to) the issue's own
//! fixture. It has to go through `spin build` (`native_http::build`),
//! not a lone `spinel contract.rb -o contract` compile: a single-file
//! compile treeshakes differently and did not reproduce the project-
//! wide `-Wint-conversion` hard error `spin build` hits — confirmed by
//! hand against the issue's exact repro before writing this test.

use super::{emit_and_run, native_http};

fn widgets() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        // `touch_request` lives on the ANCESTOR (ApplicationController),
        // not on WidgetsController itself. A second `before_action`
        // targeting it is needed so the inliner keeps `check_kind_params`
        // as a method of its own instead of inlining it into `show` (see
        // `own_filter_inlining_is_ordered`), which is how the sidecar's
        // wrong pin on it becomes observable at all.
        .edit(
            "app/controllers/application_controller.rb",
            "  stale_when_importmap_changes\nend\n",
            "  stale_when_importmap_changes\n\n  private\n\n  def touch_request\n    @touched = true\n  end\nend\n",
        )
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  resources :widgets, only: [:index, :show]\n",
        )
        .write(
            "app/controllers/widgets_controller.rb",
            r#"class WidgetsController < ApplicationController
  KINDS = %w[small large].freeze

  before_action :check_kind_params, only: :show
  before_action :touch_request

  def index
    head :ok
  end

  def show
    head :ok
  end

  private

  def check_kind_params
    @kind = params.require(:kind)
    head(:not_found) unless KINDS.include?(@kind)
  end
end
"#,
        )
}

/// Before the fix, `native_http::build` panics at `spin build blog`
/// with the issue's own C error: "returning 'sp_bool' {aka 'bool'} from
/// a function with return type 'sp_SymPolyHash *' makes pointer from
/// integer without a cast". After the fix, the helper's sidecar is
/// `untyped` and the project builds; a `kind` in the allow list reaches
/// `show` (200) and one outside it is refused by the guard (404).
#[test]
#[ignore = "requires Spinel; run in its CI lane"]
fn a_no_permit_guard_filter_builds_and_runs_on_spinel() {
    // Not a build of its own: the bare-permit controller (`emit_and_run/bare_permit.rs`) rides this one.
    let tree = super::bare_permit_contract::apply(widgets()).native_http_tree();
    let server = native_http::Server::start(&tree);

    let allowed = server.get("/widgets/1?kind=small");
    assert_eq!(allowed.status, 200, "allowed kind: {} {}", allowed.status, allowed.body);

    let refused = server.get("/widgets/1?kind=bogus");
    assert_eq!(refused.status, 404, "disallowed kind: {} {}", refused.status, refused.body);

    let full = server.get(super::bare_permit_contract::QUERY);
    assert_eq!(full.status, 200, "{}", full.body);
    assert!(full.body.contains("<p id=\"q\">&lt;rails&gt;</p>"), "{}", full.body);
    assert!(full.body.contains("<p id=\"other\">one</p>"), "{}", full.body);
    assert!(full.body.contains("<p id=\"log\">Alice|1||2|0</p>"), "{}", full.body);
    assert!(full.body.contains("href=\"/finds/page/2?q=rails\""), "{}", full.body);
    assert!(full.body.contains("href=\"/finds/page/3?q=rails\""), "{}", full.body);
    assert!(full.body.contains("href=\"/finds/page/4?q=rails\""), "{}", full.body);
    let pager = server.get("/finds/pager?q=rails");
    assert_eq!(pager.status, 200, "{}", pager.body);
    assert!(pager.body.contains("href=\"/finds/pager?page=3&amp;q=rails\""), "{}", pager.body);
    let helper = server.get("/finds/helper_pager?q=rails");
    assert_eq!(helper.status, 200, "{}", helper.body);
    assert!(helper.body.contains("href=\"/finds/helper_pager?page=3&amp;q=rails\""), "{}", helper.body);
    let bare = server.get("/finds");
    assert_eq!(bare.status, 200, "{}", bare.body);
    assert!(bare.body.contains("<p id=\"log\">(All)|||1|0</p>"), "{}", bare.body);
}
