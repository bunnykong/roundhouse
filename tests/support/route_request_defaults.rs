//! The app behind the literal route defaults contract: real-blog with the
//! routes and controller in `route_request_defaults/`. The Ruby family and
//! native checks (`tests/route_request_defaults.rs`) and the native HTTP
//! server check (`tests/spinel_toolchain/route_request_defaults.rs`) share it.

use super::emit_and_run;

const ROUTES: &str = include_str!("route_request_defaults/routes.rb");
const CONTROLLER: &str = include_str!("route_request_defaults/controller.rb");

pub fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("config/routes.rb", "  root \"articles#index\"\n", ROUTES)
        .write("app/controllers/defaults_controller.rb", CONTROLLER)
}
