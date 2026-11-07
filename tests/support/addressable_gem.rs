//! addressable compiled from its installed source, with public_suffix beside
//! it, answering the calls mastodon makes: URI parsing and its error, a URI
//! built field by field, query values, IDNA, a Template and a component
//! normalized by a character class.

use super::emit_and_run;

const GEMS: [(&str, &str); 2] = [("addressable", "2.9.0"), ("public_suffix", "7.0.5")];

fn installed(name: &str, version: &str) -> std::path::PathBuf {
    let out = std::process::Command::new("ruby")
        .args(["-e", &format!("print Gem::Specification.find_by_name('{name}', '{version}').gem_dir")])
        .output()
        .expect("spawn ruby");
    assert!(out.status.success(), "{name} {version} is not installed: {}", String::from_utf8_lossy(&out.stderr));
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

const MODEL: &str = r#"class LinkCheck
  def self.host_of(url)
    Addressable::URI.parse(url).normalized_host
  rescue Addressable::URI::InvalidURIError
    nil
  end

  def self.query_of(url)
    Addressable::URI.parse(url).query_values
  end

  def self.domain_host(domain)
    Addressable::URI.new.tap { |u| u.host = domain.strip }.normalized_host
  end

  def self.unicode_domain(domain)
    Addressable::IDNA.to_unicode(domain)
  end

  def self.ascii_domain(domain)
    Addressable::IDNA.to_ascii(domain)
  end

  def self.interaction_url(uri)
    Addressable::Template.new("https://example.com/authorize_interaction?uri={uri}").expand(uri: uri).to_s
  end

  def self.query_component(text)
    Addressable::URI.normalize_component(text, Addressable::URI::CharacterClasses::QUERY)
  end

  def self.normalized(url)
    Addressable::URI.parse(url).normalize.to_s
  end
end
"#;

// The answers addressable 2.9.0 gives on CRuby.
pub const ASSERTIONS: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect}, not #{want.inspect}" unless got == want
end

check "host", LinkCheck.host_of("HTTP://Example.COM:80/a?b=1"), "example.com"
check "invalid", LinkCheck.host_of("http://exa mple.com:xx"), nil
check "query", LinkCheck.query_of("https://example.com/s?q=ruby&lang=ja"), { "q" => "ruby", "lang" => "ja" }
check "built host", LinkCheck.domain_host(" Example.ORG "), "example.org"
check "to_unicode", LinkCheck.unicode_domain("xn--r8jz45g.xn--zckzah"), "例え.テスト"
check "to_ascii", LinkCheck.ascii_domain("例え.テスト"), "xn--r8jz45g.xn--zckzah"
check "template", LinkCheck.interaction_url("acct:alice@example.com"),
      "https://example.com/authorize_interaction?uri=acct%3Aalice%40example.com"
check "component", LinkCheck.query_component("a b&c"), "a%20b&c"
check "normalize", LinkCheck.normalized("http://example.com/a/../b/./c"), "http://example.com/b/c"
puts "addressable passed"
"##;

pub fn app() -> emit_and_run::Overlay {
    let fixture = roundhouse::fixtures::real_blog();
    let gemfile = std::fs::read_to_string(fixture.join("Gemfile")).unwrap() + "gem \"addressable\"\n";
    // Not the fixture's own pins: it locks addressable only as capybara's dependency, at whatever version its lock resolved.
    let mut lock: String = std::fs::read_to_string(fixture.join("Gemfile.lock"))
        .unwrap()
        .lines()
        .map(|line| match GEMS.iter().find(|(name, _)| line.starts_with(&format!("    {name} ("))) {
            Some((name, version)) => format!("    {name} ({version})\n"),
            None => format!("{line}\n"),
        })
        .collect();
    for (name, version) in GEMS {
        if !lock.contains(&format!("\n    {name} (")) {
            let spec = if name == "addressable" {
                format!("    {name} ({version})\n      public_suffix (>= 2.0.2, < 8.0)\n")
            } else {
                format!("    {name} ({version})\n")
            };
            lock = lock.replacen("\n\nPLATFORMS", &format!("\n{spec}\nPLATFORMS"), 1);
        }
    }
    let lock = lock.replacen("DEPENDENCIES\n", "DEPENDENCIES\n  addressable\n", 1);
    let mut overlay = emit_and_run::real_blog()
        .write("Gemfile", &gemfile)
        .write("Gemfile.lock", &lock)
        .write(".bundle/config", "---\nBUNDLE_PATH: \"vendor/bundle\"\n")
        .write("app/models/link_check.rb", MODEL);
    for (name, version) in GEMS {
        let gem = installed(name, version);
        let mut files = Vec::new();
        rb_files(&gem, &gem.join("lib"), &mut files);
        for file in files {
            let text = std::fs::read_to_string(gem.join(&file)).unwrap();
            overlay = overlay.write(&format!("vendor/bundle/ruby/3.4.0/gems/{name}-{version}/{}", file.display()), &text);
        }
        // public_suffix reads its rule list from data/ at load.
        let data = gem.join("data/list.txt");
        if data.is_file() {
            overlay = overlay.write(
                &format!("vendor/bundle/ruby/3.4.0/gems/{name}-{version}/data/list.txt"),
                &std::fs::read_to_string(data).unwrap(),
            );
        }
    }
    overlay
}
