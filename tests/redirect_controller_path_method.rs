#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const CONTROLLER: &str = r#"class SchedulesController < ApplicationController
  def update
    @article = Article.first
    redirect_to schedule_return_path(@article)
  end

  def listing
    redirect_to articles_path
  end

  def identity
    redirect_to controller_path
  end
end
"#;

const HELPER: &str = r#"
  private

  def schedule_return_path(article)
    "/custom/" + article.id.to_s
  end
"#;

fn app(inherited: bool) -> emit_and_run::Overlay {
    let (base, controller) = if inherited {
        (format!("class ApplicationController < ActionController::Base\n{HELPER}end\n"), CONTROLLER.to_string())
    } else {
        ("class ApplicationController < ActionController::Base\nend\n".to_string(), CONTROLLER.strip_suffix("end\n").unwrap().to_string() + HELPER + "end\n")
    };
    emit_and_run::real_blog()
        .write("app/controllers/application_controller.rb", &base)
        .write("app/controllers/schedules_controller.rb", &controller)
        .edit("config/routes.rb", "Rails.application.routes.draw do\n", "Rails.application.routes.draw do\n  get '/update', to: 'schedules#update'\n  get '/listing', to: 'schedules#listing'\n  get '/identity', to: 'schedules#identity'\n  get '/generated/:id', to: 'schedules#listing', as: :schedule_return\n")
}

#[test]
fn redirect_keeps_controller_path_methods_and_their_record_arguments() {
    for inherited in [false, true] {
        for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
            let (emitted, errors) = app(inherited).emit(target);
            assert!(errors.is_empty(), "{errors:?}");
            let controller = std::fs::read_to_string(emitted.join("app/controllers/schedules_controller.rb")).unwrap();
            assert!(!controller.contains("RouteHelpers.schedule_return_path"), "{controller}");
            assert!(!controller.contains("schedule_return_path(@article.id)"), "{controller}");
            assert!(controller.contains("schedule_return_path(@article)"), "{controller}");
            assert!(!controller.contains("RouteHelpers.controller_path"), "{controller}");
            assert!(controller.contains("RouteHelpers.articles_path"), "{controller}");
        }
    }
}

#[test]
fn redirect_to_an_own_or_inherited_path_method_runs_in_ruby() {
    for inherited in [false, true] {
        app(inherited).run_ruby(r#"
require_relative "app/controllers/schedules_controller"
ActionController::Base.allow_forgery_protection = false
article = Article.create!(title: "Redirect", body: "A body that is long enough.")
controller = SchedulesController.new
controller.request = ActionDispatch::TestRequest.create("HTTP_HOST" => "example.org")
controller.request_method = "GET"
controller.process_action(:update)
expected = "http://example.org/custom/" + article.id.to_s
raise controller.location.inspect unless controller.location == expected
puts "controller path redirect passed"
"#).assert_passes();
    }
}
