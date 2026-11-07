//! rack-attack compiled from its installed source: the initializer's
//! settings run at boot, and its Railtie's `app.middleware.use(Rack::Attack)`
//! puts it on the stack where Rails does, inside RequestId and before the routes.

use roundhouse::build_target::BuildTarget;

use super::emit_and_run;

const VERSION: &str = "6.7.0";

fn installed_rack_attack() -> std::path::PathBuf {
    let out = std::process::Command::new("ruby")
        .args(["-e", &format!("print Gem::Specification.find_by_name('rack-attack', '{VERSION}').gem_dir")])
        .output()
        .expect("spawn ruby");
    assert!(out.status.success(), "rack-attack {VERSION} is not installed: {}", String::from_utf8_lossy(&out.stderr));
    std::path::PathBuf::from(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn rb_files(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            rb_files(root, &path, out);
        } else if path.extension().is_some_and(|e| e == "rb") {
            out.push(path.strip_prefix(root).unwrap().to_path_buf());
        }
    }
}

const INITIALIZER: &str = r#"# Not Rails.cache: the test environment's is a NullStore, which counts nothing.
Rack::Attack.cache.store = ActiveSupport::Cache::MemoryStore.new

Rack::Attack.safelist("home") do |req|
  req.path == "/"
end

Rack::Attack.throttle("articles", limit: 2, period: 60) do |req|
  req.ip if req.path == "/articles"
end

Rack::Attack.throttle("home", limit: 1, period: 60) do |req|
  req.ip if req.path == "/"
end

Rack::Attack.throttled_responder = lambda do |request|
  match_data = request.env["rack.attack.match_data"]
  [429, { "RateLimit-Limit" => match_data[:limit].to_s }, ["Throttled\n"]]
end
"#;

const ASSERTIONS: &str = r#"
def hit(path, ip)
  env = { "REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => "", "REMOTE_ADDR" => ip,
          "HTTP_HOST" => "localhost", "rack.input" => StringIO.new("") }
  Main.run_rack(env)
end

2.times do
  status, _headers, _body = hit("/articles", "203.0.113.7")
  raise "under the limit: #{status}" unless status == 200
end
# PathNormalizer strips the trailing slash before the throttle reads the path, as in Rails.
status, headers, body = hit("/articles/", "203.0.113.7")
raise "past the limit: #{status}" unless status == 429
raise "RateLimit-Limit #{headers.inspect}" unless headers["RateLimit-Limit"] == "2"
raise "body #{body.inspect}" unless body == ["Throttled\n"]
# Rails' RequestId sits outside the gem's middleware: its own answer carries the id too.
raise "X-Request-Id #{headers.inspect}" if headers["x-request-id"].to_s.empty?
# Another client has its own count.
status, _headers, _body = hit("/articles", "198.51.100.2")
raise "another client: #{status}" unless status == 200
# A safelisted request is never throttled, whatever else matches it.
3.times do
  status, _headers, _body = hit("/", "203.0.113.7")
  raise "safelisted: #{status}" unless status == 200
end
puts "rack-attack passed"
"#;

fn app() -> emit_and_run::Overlay {
    let gem = installed_rack_attack();
    let fixture = roundhouse::fixtures::real_blog();
    let lock = std::fs::read_to_string(fixture.join("Gemfile.lock")).unwrap();
    let lock = lock
        // Not a bare "    rack (": a dependency line ("      rack (>= 2.2.4)") holds it too.
        .replacen("\n    rack (", &format!("\n    rack-attack ({VERSION})\n      rack (>= 1.0, < 4)\n    rack ("), 1)
        .replacen("DEPENDENCIES\n", "DEPENDENCIES\n  rack-attack\n", 1);
    let gemfile = std::fs::read_to_string(fixture.join("Gemfile")).unwrap() + "gem \"rack-attack\"\n";
    let mut overlay = emit_and_run::real_blog()
        .write("Gemfile", &gemfile)
        .write("Gemfile.lock", &lock)
        .write(".bundle/config", "---\nBUNDLE_PATH: \"vendor/bundle\"\n")
        .write("config/initializers/rack_attack.rb", INITIALIZER);
    let mut files = Vec::new();
    rb_files(&gem, &gem.join("lib"), &mut files);
    for file in files {
        let text = std::fs::read_to_string(gem.join(&file)).unwrap();
        overlay = overlay.write(&format!("vendor/bundle/ruby/3.4.0/gems/rack-attack-{VERSION}/{}", file.display()), &text);
    }
    overlay
}

// Not an integration test: the emitted harness's `get` calls the router in process, past the middleware.
#[test]
fn rack_attack_throttles_and_safelists_as_in_rails() {
    let run = app().run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("rack-attack passed"), "{}", run.stdout);
}

// Emitted only: the Spinel toolchain lane, which builds and serves a tree, installs no rack-attack.
#[test]
fn the_spinel_tree_stacks_the_railtie_middleware() {
    let (tree, errors) = app().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
    let stack = std::fs::read_to_string(tree.join("runtime/gem_middleware.rb")).expect("gem_middleware.rb");
    assert!(stack.contains("@stack ||= Rack::Attack.new(App.new)"), "{stack}");
    let attack = std::fs::read_to_string(tree.join("app/models/rack/attack.rb")).expect("the compiled gem");
    // Each nested class under its own namespace.
    assert!(attack.contains("class Configuration\n"), "{attack}");
    assert!(attack.contains("module StoreProxy\n    class RedisProxy < Rack::Attack::BaseProxy"), "{attack}");
}

// Not through an emitted app: the ruby family loads rack's own Rack::Request, so only Spinel runs the port.
#[test]
fn the_spinel_rack_request_port_finds_the_client_rack_finds() {
    let rack = std::process::Command::new("ruby").args(["-rrack", "-e", "1"]).status();
    if !rack.is_ok_and(|s| s.success()) {
        eprintln!("skipping: ruby with the rack gem not available");
        return;
    }
    let envs = r#"[
  { "REMOTE_ADDR" => "127.0.0.1", "HTTP_X_FORWARDED_FOR" => "198.51.100.2" },
  { "REMOTE_ADDR" => "127.0.0.1", "HTTP_X_FORWARDED_FOR" => "192.0.2.9, 10.0.0.1" },
  { "REMOTE_ADDR" => "10.1.2.3", "HTTP_X_FORWARDED_FOR" => "192.0.2.9, 172.16.0.4, 192.168.1.1" },
  { "REMOTE_ADDR" => "127.0.0.1", "HTTP_FORWARDED" => "for=\"[2001:db8::1]:4711\"" },
  { "REMOTE_ADDR" => "127.0.0.1", "HTTP_X_FORWARDED_FOR" => "unknown, 127.0.0.2" },
  { "REMOTE_ADDR" => "127.0.0.1" },
  { "REMOTE_ADDR" => "203.0.113.5", "HTTP_X_FORWARDED_FOR" => "198.51.100.2" },
  { "REMOTE_ADDR" => "172.32.0.1" },
].each { |env| p Rack::Request.new(env).ip }
"#;
    let run = |prelude: String| {
        let out = std::process::Command::new("ruby").args(["-e", &(prelude + envs)]).output().expect("spawn ruby");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let port = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/spinel/rack_request.rb");
    let ported = run(format!("load {:?}\n", port.display().to_string()));
    let rack = run("require \"rack\"\n".to_string());
    assert_eq!(ported, rack);
}

// Not served over HTTP: no CI lane serves a Spinel tree with rack-attack, so the emitted adapter runs on CRuby over a stand-in server.
#[test]
fn the_spinel_adapter_sends_the_response_the_middleware_returns() {
    let (tree, errors) = app().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
    let adapter = std::fs::read_to_string(tree.join("runtime/gem_middleware.rb"))
        .expect("gem_middleware.rb")
        .replace("Rack::Attack.new(App.new)", "Probe.new(App.new)");
    let script = format!(
        r#"{adapter}
module Tep
  def self.str_hash = {{}}
end
Res = Struct.new(:status, :headers, :body, :file_path, :streaming, :set_cookies, :header_fields)
Req = Struct.new(:verb, :path, :raw_query, :remote_host, :http_version, :req_headers, :raw_body)
ReqObj = Struct.new(:request_id)
module Main
  def self.dispatch_request(req, res, _obj)
    res.status = 200
    res.headers["x-routes"] = req.path
    res.body = +"routes body"
    res.file_path = +"/tmp/sent.pdf" if req.path == "/file"
    res.streaming = true if req.path == "/stream"
  end
end
class Probe
  class << self
    attr_accessor :built
  end
  self.built = 0
  def initialize(app)
    Probe.built += 1
    # Wide enough for a second first request to arrive while this one builds.
    sleep 0.05
    @app = app
  end
  def call(env)
    env["PATH_INFO"] = env["PATH_INFO"].sub("/old", "/new")
    status, headers, body = response = @app.call(env)
    case env["PATH_INFO"]
    when "/file", "/stream" then [429, {{ "x-own" => "1" }}, ["replaced"]]
    when "/element" then (body[0] = "element"; response)
    else [status, headers, body]
    end
  end
end
def run(path)
  res = Res.new(0, {{}}, +"", +"", false, [], [])
  GemMiddleware.dispatch(Req.new("GET", path, "", "127.0.0.1", "HTTP/1.1", {{}}, ""), res, ReqObj.new("id"))
  res
end
# Two first requests at once still build one stack.
2.times.map {{ Thread.new {{ run("/old/first") }} }}.each(&:join)
res = run("/file")
raise "file: #{{res.to_h}}" unless res.status == 429 && res.body == "replaced" && res.file_path == "" && res.headers == {{ "x-own" => "1" }}
res = run("/stream")
raise "stream: #{{res.to_h}}" unless res.status == 429 && res.body == "replaced" && res.streaming == false
res = run("/element")
raise "element: #{{res.to_h}}" unless res.body == "element" && res.headers["x-routes"] == "/element"
res = run("/old/page")
raise "path: #{{res.to_h}}" unless res.body == "routes body" && res.headers["x-routes"] == "/new/page"
# Built once, as Rails builds its stack, not per request.
raise "built #{{Probe.built}} times" unless Probe.built == 1
puts "adapter passed"
"#
    );
    let out = std::process::Command::new("ruby").args(["-e", &script]).output().expect("spawn ruby");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("adapter passed"), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
}
