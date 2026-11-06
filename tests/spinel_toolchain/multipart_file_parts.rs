use super::{emit_and_run, native_http};

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn repeated_multipart_files_reach_the_spinel_entrypoint() {
    let tree = emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table :widgets do |t|\n    t.string :name\n  end\nend\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\nend\n",
        )
        .write(
            "app/models/widget.rb",
            "class Widget < ApplicationRecord\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::API\nend\n",
        )
        .write(
            "app/controllers/uploads_controller.rb",
            "class UploadsController < ApplicationController\n  def create\n    render plain: \"accepted\"\n  end\nend\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  post \"/uploads\", to: \"uploads#create\"\nend\n",
        )
        .native_http_tree();
    let server = native_http::Server::start(&tree);
    let body = concat!(
        "--entrypoint-parts\r\nContent-Disposition: form-data; name=\"upload[files][]\"; filename=\"first.bin\"\r\n",
        "Content-Type: application/octet-stream\r\n\r\nfirst\r\n",
        "--entrypoint-parts\r\nContent-Disposition: form-data; name=\"upload[files][]\"; filename=\"second.bin\"\r\n",
        "Content-Type: application/octet-stream\r\n\r\nsecond\r\n",
        "--entrypoint-parts--\r\n",
    );
    let response = server.post_without_session(
        "/uploads",
        "multipart/form-data; boundary=entrypoint-parts",
        body,
    );
    assert_eq!(response.status, 200, "{}\n{}", response.body, server.log());
    assert_eq!(response.body, "accepted", "{}", server.log());
}
