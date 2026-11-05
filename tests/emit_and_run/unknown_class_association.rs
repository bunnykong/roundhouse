//! #1721: an association whose `class_name` names a class nothing
//! defines (a gem's model in a real app, `has_many :applications,
//! class_name: "Doorkeeper::Application"`). Naming the constant is
//! refused (`constant not supported`); reaching it through the
//! association was silent, and the emitted query was built on a class the
//! tree doesn't have.

use super::emit_and_run;

fn app(association: &str) -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy\n",
            &format!("  has_many :comments, dependent: :destroy\n  {association}\n"),
        )
        .edit("config/routes.rb", "Rails.application.routes.draw do\n", "Rails.application.routes.draw do\n  resources :widgets, only: [:show]\n")
        .write(
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  def show\n    widget = Article.first.widgets.find(params[:id])\n    redirect_to widget_path(widget)\n  end\nend\n",
        )
}

#[test]
fn an_association_to_a_class_nothing_defines_is_refused() {
    for association in [
        "has_many :widgets, class_name: \"Vendor::Widget\", as: :owner, dependent: nil",
        "has_many :widgets, class_name: \"Vendor::Widget\"",
    ] {
        let errors = app(association).check_errors();
        assert!(
            errors.iter().any(|e| e.contains("constant not supported") && e.contains("Vendor::Widget")),
            "{association}: {errors:?}"
        );
    }
}

/// A model the app defines, and Active Storage's own records, are left alone.
#[test]
fn an_association_to_a_defined_class_is_not_refused() {
    let errors = app("has_many :widgets, class_name: \"Comment\"").check_errors();
    assert!(!errors.iter().any(|e| e.contains("constant not supported")), "{errors:?}");
}
