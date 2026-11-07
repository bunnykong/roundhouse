pub fn overlay() -> super::emit_and_run::Overlay {
    super::emit_and_run::real_blog()
        .write(
            "app/channels/application_cable/channel.rb",
            "module ApplicationCable\n  class Channel < ActionCable::Channel::Base\n  end\nend\n",
        )
        .write(
            "app/channels/stream_channel.rb",
            r#"class StreamChannel < ApplicationCable::Channel
  AFTER = []

  def subscribed
    stream_from("shared")
    stream_from("second")
  end

  def unsubscribed
    stop_all_streams
    AFTER << identifier
  end
end
"#,
        )
}

pub const FIRST: &str = r#"{"channel":"StreamChannel","id":"first"}"#;
pub const SECOND: &str = r#"{"channel":"StreamChannel","id":"second"}"#;
pub const THIRD: &str = r#"{"channel":"StreamChannel","id":"third"}"#;
