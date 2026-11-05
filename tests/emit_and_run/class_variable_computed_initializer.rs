//! A class body's `@@x = <value>` whose value reads constants and calls
//! their methods (`@@search_path = DEFAULT_SEARCH_PATH.dup`,
//! `@@default_mutex = Mutex.new`) runs where the source has it, beside
//! `private_constant` and an `include Comparable` (tzinfo 2.0's
//! DataSource, ZoneinfoDataSource and Timezone).

use super::emit_and_run;

const PROBE: &str = r##"class SearchPaths
  include Comparable

  DEFAULT = ["/usr/share/zoneinfo", "/etc/zoneinfo"].freeze
  private_constant :DEFAULT

  @@paths = DEFAULT.dup
  @@lock = Mutex.new

  def self.paths
    @@lock.synchronize { @@paths }
  end

  def self.add(path)
    @@paths << path
    @@paths.length
  end

  attr_reader :rank

  def initialize(rank)
    @rank = rank
  end

  def <=>(other)
    rank <=> other.rank
  end
end
"##;

const ASSERTIONS: &str = "got = [SearchPaths.add(\"/opt/zoneinfo\"), SearchPaths.paths.last, SearchPaths.new(1) < SearchPaths.new(2)]\nraise got.inspect unless got == [3, \"/opt/zoneinfo\", true]\nputs \"class variable initializer passed\"\n";

#[test]
fn a_computed_class_variable_initializer_runs_where_the_source_has_it() {
    let run = emit_and_run::real_blog().write("app/models/search_paths.rb", PROBE).run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("class variable initializer passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_computed_class_variable_initializer_runs_on_spinel() {
    let run = emit_and_run::real_blog().write("app/models/search_paths.rb", PROBE).run_spinel(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("class variable initializer passed"), "{}", run.stdout);
}
