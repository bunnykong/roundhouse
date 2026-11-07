require "fileutils"
require "json"
require "open3"
require "tmpdir"

cli = File.expand_path(ARGV.fetch(0))
config = "marker = \"booted\"\nafter_booted { puts marker }\n"
failures = []
Dir.mktmpdir("roundhouse-puma-source-") do |root|
  app = File.join(root, "app")
  {
    "config/puma.rb" => config,
    "config/routes.rb" => "Rails.application.routes.draw do\nend\n",
    "db/schema.rb" => "ActiveRecord::Schema.define do\nend\n"
  }.each do |path, source|
    file = File.join(app, path)
    FileUtils.mkdir_p(File.dirname(file))
    File.write(file, source)
  end

  cases = [
    ["archive-blog", ["--archives", "blog"], true],
    ["archive-source-targets", ["--archives", "blog,futamura"], true],
    ["futamura", ["--target", "futamura"], true],
    ["ruby", ["--target", "ruby"], false],
    ["jruby", ["--target", "jruby"], false],
    ["spinel", ["--target", "spinel"], false],
    ["archive-mixed", ["--archives", "blog,spinel"], false]
  ]
  cases.each do |name, options, preserves|
    out = File.join(root, name)
    command = [cli, *options, app, "-o", out]
    stdout, stderr, status = Open3.capture3({ "ROUNDHOUSE_INGEST_SURVEY" => nil }, *command)
    puts JSON.generate(case: name, command: command, exit: status.exitstatus, stdout: stdout, stderr: stderr)
    if preserves
      unless status.success?
        failures << "#{name} rejected source-preserving output"
        next
      end
      if options.first == "--archives"
        options.last.split(",").each do |target|
          manifest = JSON.parse(File.read(File.join(out, "browse", "#{target}.json")))
          entry = manifest.fetch("files").find { |file| file.fetch("path") == "config/puma.rb" }
          failures << "#{name}/#{target} changed the original Puma config" unless entry && entry.fetch("content") == config
        end
      else
        failures << "#{name} changed the original Puma config" unless File.read(File.join(out, "config/puma.rb")) == config
      end
    elsif status.success? || !stderr.include?("Puma boot hooks require closed bodies and conditions")
      failures << "#{name} did not refuse the captured config local"
    end
  end
end
abort failures.join("\n") unless failures.empty?
puts "Puma source-preserving CLI contract passed"
