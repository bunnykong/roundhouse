pub fn overlay() -> super::emit_and_run::Overlay {
    super::emit_and_run::real_blog()
        .write(
            "config/puma.rb",
            r#"
after_booted { BootProbe.first }
if ENV.fetch("PUMA_HOOK_SELECTION", "selected") == "selected"
  on_booted { BootProbe.second }
else
  after_booted { BootProbe.wrong }
end
unless ENV.fetch("PUMA_HOOK_SELECTION", "selected") == "selected"
  after_booted { BootProbe.wrong }
end
"#,
        )
        .write(
            "app/models/boot_probe.rb",
            r#"
class BootProbe
  def self.first
    ENV["PUMA_HOOK_SELECTION"] = "changed"
    ENV["PUMA_HOOK_EVENTS"] = ENV.fetch("PUMA_HOOK_EVENTS", "") + "first"
    puts "puma hook first"
  end
  def self.second
    ENV["PUMA_HOOK_EVENTS"] = ENV.fetch("PUMA_HOOK_EVENTS", "") + ",second"
    puts "puma hook second"
    $stdout.flush
  end
  def self.wrong
    raise "unselected Puma callback ran"
  end
end
"#,
        )
}
