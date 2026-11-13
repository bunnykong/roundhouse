//! Doorkeeper compiled from its installed source: `use_doorkeeper` draws the engine's routes, the host's
//! `Doorkeeper.configure` runs at boot, and a confidential application gets a `client_credentials` token, reads it
//! back at `/oauth/token/info`, and an unknown token is refused, as Doorkeeper 5.8.2 answers on Rails.

#[path = "../support/prerequisite.rs"]
mod prerequisite;
use super::emit_and_run;

const VERSION: &str = "5.8.2";

fn installed_doorkeeper() -> Option<std::path::PathBuf> {
    let out = std::process::Command::new("ruby")
        .args(["-e", &format!("print Gem::Specification.find_by_name('doorkeeper', '{VERSION}').gem_dir")])
        .output();
    match out {
        Ok(out) if out.status.success() => Some(std::path::PathBuf::from(String::from_utf8_lossy(&out.stdout).into_owned())),
        _ => {
            prerequisite::missing(&format!("ruby with doorkeeper {VERSION} installed"));
            None
        }
    }
}

fn files(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            files(root, &path, out);
        } else {
            out.push(path.strip_prefix(root).unwrap().to_path_buf());
        }
    }
}

const TABLES: &str = r#"  create_table "oauth_access_grants", force: :cascade do |t|
    t.integer "resource_owner_id", null: false
    t.integer "application_id", null: false
    t.string "token", null: false
    t.integer "expires_in", null: false
    t.text "redirect_uri", null: false
    t.string "scopes", default: "", null: false
    t.datetime "created_at", null: false
    t.datetime "revoked_at"
  end

  create_table "oauth_access_tokens", force: :cascade do |t|
    t.integer "resource_owner_id"
    t.integer "application_id", null: false
    t.string "token", null: false
    t.string "refresh_token"
    t.integer "expires_in"
    t.string "scopes"
    t.datetime "created_at", null: false
    t.datetime "revoked_at"
    t.string "previous_refresh_token", default: "", null: false
  end

  create_table "oauth_applications", force: :cascade do |t|
    t.string "name", null: false
    t.string "uid", null: false
    t.string "secret", null: false
    t.text "redirect_uri", null: false
    t.string "scopes", default: "", null: false
    t.boolean "confidential", default: true, null: false
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end

"#;

const INITIALIZER: &str = "Doorkeeper.configure do\n  orm :active_record\n  resource_owner_authenticator do\n    session[:user_id] || redirect_to(\"/\")\n  end\n  access_token_expires_in 1.year\n  use_refresh_token\n  default_scopes :read\n  optional_scopes :write\nend\n";

const SCRIPT: &str = r##"def call(verb, path, body = "", headers = {})
  env = { "REQUEST_METHOD" => verb, "PATH_INFO" => path, "QUERY_STRING" => "", "SERVER_NAME" => "localhost", "SERVER_PORT" => "80",
          "HTTP_HOST" => "localhost", "rack.url_scheme" => "http", "rack.input" => StringIO.new(body) }.merge(headers)
  unless body.empty?
    env["CONTENT_TYPE"] = "application/x-www-form-urlencoded"
    env["CONTENT_LENGTH"] = body.bytesize.to_s
  end
  status, _headers, chunks = Main.run_rack(env)
  [status, JSON.parse(chunks.join)]
end

# Not `== 1.year`: the script runs as written, and Integer carries no duration readers there (Rails: 365.2425 days).
raise Doorkeeper.config.access_token_expires_in.inspect unless Doorkeeper.config.access_token_expires_in.to_i == 31_556_952
app = Doorkeeper::Application.create!(name: "probe", redirect_uri: "https://example.com/cb", confidential: true)
status, token = call("POST", "/oauth/token", "grant_type=client_credentials&client_id=#{app.uid}&client_secret=#{app.secret}")
raise "token #{status} #{token}" unless status == 200 && token["token_type"] == "Bearer" && token["expires_in"] == 31556952 && token["scope"] == "read"
status, info = call("GET", "/oauth/token/info", "", "HTTP_AUTHORIZATION" => "Bearer #{token["access_token"]}")
raise "info #{status} #{info}" unless status == 200 && info["application"] == { "uid" => app.uid } && info["scope"] == "read" && info["resource_owner_id"].nil?
status, refused = call("GET", "/oauth/token/info", "", "HTTP_AUTHORIZATION" => "Bearer nope")
raise "refused #{status} #{refused}" unless status == 401 && refused["error"] == "invalid_token"
puts "doorkeeper passed"
"##;

/// What the gem still does that no tree answers, none of which this flow reaches: constants Ruby itself could not
/// find there (`MissingConfigurationBuilderClass`, which nothing defines; `Errors::…` from a compact
/// `module Doorkeeper::Orm::ActiveRecord::Mixins`; `Mapper` in a default `Routes` overrides), the applications
/// page's sanitizing `simple_format`, and four calls the analysis cannot type.
const KNOWN_GAPS: [&str; 9] = [
    "constant not supported (all targets): Doorkeeper::MissingConfigurationBuilderClass",
    "constant not supported (all targets): Errors::UnableToGenerateToken",
    "constant not supported (all targets): Errors::TokenGeneratorNotFound",
    "constant not supported (all targets): Mapper",
    "simple_format not supported (all targets)",
    "no known method `missing_param`",
    "no known method `transform_secret` on nil",
    "no known method `constantize` on String?",
    "no known method `columns` on Doorkeeper::Application",
];

fn app() -> Option<emit_and_run::Overlay> {
    let gem = installed_doorkeeper()?;
    let fixture = roundhouse::fixtures::real_blog();
    let lock = std::fs::read_to_string(fixture.join("Gemfile.lock")).unwrap();
    let lock = lock
        // Not a bare "    rack (": a dependency line ("      rack (>= 2.2.4)") holds it too.
        .replacen("\n    rack (", &format!("\n    doorkeeper ({VERSION})\n      railties (>= 5)\n    rack ("), 1)
        .replacen("DEPENDENCIES\n", "DEPENDENCIES\n  doorkeeper\n", 1);
    let gemfile = std::fs::read_to_string(fixture.join("Gemfile")).unwrap() + "gem \"doorkeeper\"\n";
    let mut overlay = emit_and_run::real_blog()
        .write("Gemfile", &gemfile)
        .write("Gemfile.lock", &lock)
        .write(".bundle/config", "---\nBUNDLE_PATH: \"vendor/bundle\"\n")
        .write("config/initializers/doorkeeper.rb", INITIALIZER)
        .edit("db/schema.rb", "  create_table \"articles\"", &format!("{TABLES}  create_table \"articles\""))
        .edit("config/routes.rb", "Rails.application.routes.draw do\n", "Rails.application.routes.draw do\n  use_doorkeeper\n");
    let mut list = Vec::new();
    for dir in ["lib", "app", "config"] {
        files(&gem, &gem.join(dir), &mut list);
    }
    for file in list {
        let Ok(text) = std::fs::read_to_string(gem.join(&file)) else { continue };
        overlay = overlay.write(&format!("vendor/bundle/ruby/3.4.0/gems/doorkeeper-{VERSION}/{}", file.display()), &text);
    }
    Some(overlay)
}

#[test]
fn doorkeeper_issues_a_client_credentials_token_on_ruby() {
    let Some(app) = app() else { return };
    let run = app.run_ruby(SCRIPT);
    let unknown: Vec<_> = run.errors.iter().filter(|e| !KNOWN_GAPS.iter().any(|gap| e.contains(gap))).collect();
    assert!(unknown.is_empty(), "{unknown:#?}");
    for gap in KNOWN_GAPS {
        assert!(run.errors.iter().any(|e| e.contains(gap)), "{gap} is answered now: drop it from KNOWN_GAPS\n{:#?}", run.errors);
    }
    assert!(run.program_succeeded(), "{}\n{}", run.stdout, run.stderr);
    assert!(run.stdout.contains("doorkeeper passed"), "{}", run.stdout);
}
