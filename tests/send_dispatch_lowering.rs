//! Dynamic `send` → static `case` dispatch (the shared
//! `lower::apply_send_static_dispatch` pass, run on the post-analyze
//! hook).
//!
//! Three source shapes from lobsters, plus the bail case:
//!  A. `as_json` spec-array walk — a local array literal (grown by
//!     `push`) iterated with `send(k)` / `send(k.values.first)`.
//!  B. literal symbol array mapped straight into `send(p)`.
//!  C. `dur.send(intv.downcase)` where the string set flows out of a
//!     hash-literal-returning helper and a frozen const table — in
//!     both call forms: qualified (`IntervalHelper.time_interval`) and
//!     the bare mixed-in call lobsters actually writes (the emit-time
//!     ancestor of this pass only saw the qualified form, post
//!     helper-lowering).
//! Unprovable name sets must leave the `send` untouched and go on the
//! residue ledger.

#[path = "support/prerequisite.rs"]
mod prerequisite;

use roundhouse::App;
use roundhouse::analyze::Analyzer;
use roundhouse::diagnostic::Diagnostic;
use roundhouse::emit::ruby::emit_library;
use roundhouse::ingest::ingest_library_classes;
use roundhouse::lower::{apply_duration_lowering, apply_send_static_dispatch};

/// Ingest → analyze → shared send grounding → duration grounding →
/// ruby render, mirroring the post-analyze hook's pass order (duration
/// AFTER dispatch — the all-duration-unit arms below count on it).
/// Returns the emitted source plus the dispatch pass's residue ledger.
fn ground_and_emit(source: &str) -> (String, Vec<Diagnostic>) {
    let classes =
        ingest_library_classes(source.as_bytes(), "test.rb").expect("ingest test source");
    let mut app = App::new();
    for lc in classes {
        app.library_classes.push(lc);
    }
    let mut analyzer = Analyzer::new(&app);
    analyzer.analyze(&mut app);
    let diags = apply_send_static_dispatch(&mut app, analyzer.class_registry());
    apply_duration_lowering(&mut app);
    let out = emit_library(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    (out, diags)
}

#[test]
fn spec_array_walk_rewrites_to_case_dispatch() {
    let (out, diags) = ground_and_emit(
        r##"
class Story
  def as_json(options = {})
    h = [:short_id, :title, { submitter_user: :user }]
    h.push(comments: options[:with_comments]) if options
    js = {}
    h.each do |k|
      if k.is_a?(Symbol)
        js[k] = send(k)
      elsif k.is_a?(Hash) && k.values.first.is_a?(Symbol)
        js[k.keys.first] = send(k.values.first)
      end
    end
    js
  end
end
"##,
    );
    // Direct dispatch: the two symbol elements, first-seen order.
    assert!(out.contains("when :short_id"), "direct arm missing:\n{out}");
    assert!(out.contains("when :title"), "direct arm missing:\n{out}");
    // Projection dispatch: the hash element's first value. The pushed
    // hash's value (`options[:with_comments]`) is not a symbol literal
    // and contributes nothing.
    assert!(out.contains("when :user"), "projection arm missing:\n{out}");
    // The unknown-name fallback keeps send's loud-failure behavior and
    // renders as `else`, never the invalid `when _`.
    assert!(out.contains("raise \"dynamic send"), "fallback arm missing:\n{out}");
    assert!(!out.contains("when _"), "wildcard arm rendered as `when _`:\n{out}");
    assert!(!out.contains("send(k"), "dynamic send survived:\n{out}");
    assert!(diags.is_empty(), "grounded sites must not ledger residue: {diags:?}");
}

#[test]
fn literal_array_map_rewrites_to_case_dispatch() {
    let (out, _) = ground_and_emit(
        r##"
class Search
  attr_reader :q, :what, :order
  def to_url_params
    [:q, :what, :order].map { |p| "#{p}=#{send(p).to_s}" }.join("&")
  end
end
"##,
    );
    for arm in ["when :q", "when :what", "when :order"] {
        assert!(out.contains(arm), "{arm} missing:\n{out}");
    }
    assert!(!out.contains("send(p"), "dynamic send survived:\n{out}");
}

#[test]
fn helper_hash_string_set_rewrites_to_case_dispatch() {
    let (out, _) = ground_and_emit(
        r##"
module IntervalHelper
  TIME_INTERVALS = { "h" => "Hour", "d" => "Day" }.freeze

  def self.time_interval(param)
    if param == "1d"
      { dur: 1, intv: TIME_INTERVALS[param] }
    else
      { dur: 2, intv: "Week" }
    end
  end
end

class FlaggedCommenters
  def initialize(interval)
    length = IntervalHelper.time_interval(interval)
    @period = length[:dur].send(length[:intv].downcase).ago
  end
end
"##,
    );
    // Const-table values + the else-literal, lowercased; all duration
    // units, so the arms call the plural form (which the downstream
    // duration pass grounds to the Duration runtime).
    for arm in ["when \"hour\"", "when \"day\"", "when \"week\""] {
        assert!(out.contains(arm), "{arm} missing:\n{out}");
    }
    assert!(
        out.contains("ActiveSupport::Duration.weeks(length[:dur])"),
        "duration grounding missing:\n{out}"
    );
    assert!(!out.contains(".send("), "dynamic send survived:\n{out}");
}

#[test]
fn bare_mixed_in_helper_call_matches_provider() {
    // The lobsters source shape: `include IntervalHelper` + a BARE
    // `time_interval(...)` call. At hook time helper-qualification
    // hasn't happened, so the provider scan attributes the bare call by
    // app-wide-unique method name.
    let (out, diags) = ground_and_emit(
        r##"
module IntervalHelper
  TIME_INTERVALS = { "h" => "Hour", "d" => "Day" }.freeze

  def time_interval(param)
    if param == "1d"
      { dur: 1, intv: TIME_INTERVALS[param] }
    else
      { dur: 2, intv: "Week" }
    end
  end
end

class FlaggedCommenters
  include IntervalHelper

  def initialize(interval)
    length = time_interval(interval)
    @period = length[:dur].send(length[:intv].downcase).ago
  end
end
"##,
    );
    for arm in ["when \"hour\"", "when \"day\"", "when \"week\""] {
        assert!(out.contains(arm), "{arm} missing:\n{out}");
    }
    assert!(!out.contains(".send("), "dynamic send survived:\n{out}");
    assert!(diags.is_empty(), "grounded sites must not ledger residue: {diags:?}");
}

#[test]
fn unprovable_name_set_is_left_alone() {
    let (out, diags) = ground_and_emit(
        r##"
class Foo
  def dyn(name)
    send(name)
  end
end
"##,
    );
    assert!(
        out.contains("send(name)"),
        "send with an unprovable name set must survive untouched:\n{out}"
    );
    // The site joins the residue ledger — strict targets report it as
    // an honest per-target gap instead of failing silently at compile.
    assert_eq!(diags.len(), 1, "expected one residue entry: {diags:?}");
    assert!(
        diags[0].to_string().contains("not statically enumerable"),
        "unexpected residue reason: {}",
        diags[0]
    );
}

#[test]
fn provider_returning_a_constant_and_a_multi_assigned_local() {
    // Current lobsters' `time_interval`: bad input returns the
    // PLACEHOLDER constant, and `intv:` is a local bound by a multiple
    // assignment from an `if` of array literals.
    let (out, diags) = ground_and_emit(
        r##"
module IntervalHelper
  PLACEHOLDER = {param: "1w", dur: 7, intv: "Day", placeholder: true}
  TIME_INTERVALS = {"h" => "Hour", "w" => "Week", "m" => "Month"}.freeze

  def self.time_interval(param)
    if (m = param.to_s.match(/\A(\d+)([hwm])\z/))
      dur = m[1].to_i
      return PLACEHOLDER unless dur > 0
      intv = TIME_INTERVALS[m[2]]
      sqlite_dur, sqlite_intv =
        if intv == "Week"
          [dur * 7, "Day"]
        else
          [dur, intv]
        end
      {param: param, dur: sqlite_dur, intv: sqlite_intv}
    else
      PLACEHOLDER
    end
  end
end

class FlaggedCommenters
  def initialize(interval)
    length = IntervalHelper.time_interval(interval)
    @period = length[:dur].send(length[:intv].downcase).ago
  end
end
"##,
    );
    for arm in ["when \"day\"", "when \"hour\"", "when \"week\"", "when \"month\""] {
        assert!(out.contains(arm), "{arm} missing:\n{out}");
    }
    assert!(!out.contains(".send("), "dynamic send survived:\n{out}");
    assert!(diags.is_empty(), "grounded sites must not ledger residue: {diags:?}");
}

#[test]
fn a_helper_spliced_into_several_includers_is_still_one_definition() {
    // `include IntervalHelper` in a controller splices a copy of
    // `time_interval` into it; the copies share the one `def`'s span and
    // must not make the bare call in the model look ambiguous.
    let files: Vec<(&str, &str)> = vec![
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        (
            "app/helpers/interval_helper.rb",
            "module IntervalHelper\n  TIME_INTERVALS = {\"h\" => \"Hour\", \"d\" => \"Day\"}.freeze\n\n  def time_interval(param)\n    {dur: 1, intv: TIME_INTERVALS[param]}\n  end\nend\n",
        ),
        (
            "app/controllers/home_controller.rb",
            "class HomeController < ActionController::Base\n  include IntervalHelper\n\n  def index\n    @i = time_interval(params[:t])\n  end\nend\n",
        ),
        (
            "app/models/flagged_commenters.rb",
            "class FlaggedCommenters\n  include IntervalHelper\n\n  def initialize(interval)\n    length = time_interval(interval)\n    @period = length[:dur].send(length[:intv].downcase).ago\n  end\nend\n",
        ),
        ("config/routes.rb", "Rails.application.routes.draw do\n  get \"/\" => \"home#index\"\nend\n"),
    ];
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let out = emit_library(&app).into_iter().map(|f| f.content).collect::<Vec<_>>().join("\n");
    assert!(out.contains("when \"hour\""), "got:\n{out}");
    assert!(!out.contains(".send("), "dynamic send survived:\n{out}");
}

/// Shape D counts a constant-receiver call site only when the constant
/// names exactly one class: by its full name, or by a trailing part of
/// it (`Pager` for `Paging::Pager`) that no other class's name also ends
/// with.
const PARAM_SELECTOR_SOURCE: &str = r##"
module Paging
  class Pager
    def self.load(relation, direction, size)
      relation.public_send(direction, size)
    end
  end
end

class Report
  def oldest(relation)
    Pager.load(relation, :first, 2)
  end

  def newest(relation)
    Paging::Pager.load(relation, :last, 1)
  end
end
"##;

#[test]
fn a_selector_parameter_collects_calls_through_a_unique_trailing_name() {
    let (out, _) = ground_and_emit(PARAM_SELECTOR_SOURCE);
    assert!(out.contains("when :first"), "trailing-name call site not counted:\n{out}");
    assert!(out.contains("when :last"), "full-name call site not counted:\n{out}");
    assert!(!out.contains("public_send(direction"), "dynamic send survived:\n{out}");
}

#[test]
fn a_trailing_name_two_classes_share_counts_no_call_site() {
    let source = format!(
        "{PARAM_SELECTOR_SOURCE}\nmodule Archive\n  class Pager\n    def self.load(relation, direction, size)\n      relation\n    end\n  end\nend\n"
    );
    let (out, _) = ground_and_emit(&source);
    assert!(out.contains("when :last"), "full-name call site not counted:\n{out}");
    assert!(
        !out.contains("when :first"),
        "an ambiguous trailing name must not resolve to either class:\n{out}"
    );
}

/// solid_queue 1.7.0's `Job::Executable#execution`: the name is built
/// around the element parameter of a `reduce` over a literal list, so it
/// is one of the names the list spells. The module defines none of them
/// (its includer's `has_one`s do), so the receiver's methods prove nothing
/// here; the list does.
const REDUCE_SOURCE: &str = r##"
module Executable
  def execution
    %w[ ready claimed failed ].reduce(nil) { |acc, status| acc || public_send("#{status}_execution") }
  end
end
"##;

/// A host for the module: the readers record that they ran, and one is
/// private, which `public_send` refuses as Ruby does.
const REDUCE_HOST: &str = r##"
class Host
  include Executable
  CALLS = []

  def initialize(ready, claimed)
    @ready = ready
    @claimed = claimed
  end

  def ready_execution
    CALLS << :ready
    @ready
  end

  def claimed_execution
    CALLS << :claimed
    @claimed
  end

  private

  def failed_execution
    CALLS << :failed
    :failed
  end
end

[[:r, :c], [nil, :c], [nil, nil]].each do |ready, claimed|
  Host::CALLS.clear
  result = begin
    Host.new(ready, claimed).execution.inspect
  rescue NoMethodError => e
    "NoMethodError #{e.name}"
  end
  puts "#{result} #{Host::CALLS.inspect}"
end
"##;

/// The program's output on CRuby, or None (reported as a missing
/// prerequisite) when no `ruby` is on PATH. A run that fails still fails.
fn cruby(source: &str) -> Option<String> {
    let Ok(out) = std::process::Command::new("ruby").args(["-e", source]).output() else {
        prerequisite::missing("ruby is not on PATH");
        return None;
    };
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[test]
fn a_name_built_from_a_reduce_blocks_literal_list_rewrites_to_case_dispatch() {
    // `inject` is `reduce`; `:"…"` is the String's Symbol, and its arms are Symbols.
    for (iterator, symbol) in [("reduce", false), ("inject", false), ("reduce", true)] {
        let mut source = REDUCE_SOURCE.replace("reduce", iterator);
        if symbol {
            source = source.replace("public_send(\"#{status}_execution\")", "public_send(:\"#{status}_execution\")");
        }
        let (out, diags) = ground_and_emit(&source);
        for name in ["ready_execution", "claimed_execution", "failed_execution"] {
            let literal = if symbol { format!(":{name}") } else { format!("\"{name}\"") };
            assert!(out.contains(&format!("when {literal}")), "{iterator}: arm for {name} missing:\n{out}");
            // Each arm is the same send by the literal name, so visibility and a missing method answer as before.
            assert!(out.contains(&format!("public_send({literal})")), "{iterator}: send of {name} missing:\n{out}");
        }
        assert!(!out.contains("public_send(\"#{") && !out.contains("public_send(:\"#{"), "{iterator}: the built name survived:\n{out}");
        assert!(diags.is_empty(), "{iterator}: grounded sites must not ledger residue: {diags:?}");
        // The emitted module answers as the source does, in the same order, stopping at the first reader that answers.
        let Some(want) = cruby(&format!("{source}{REDUCE_HOST}")) else { return };
        assert_eq!(want, ":r [:ready]\n:c [:ready, :claimed]\nNoMethodError failed_execution [:ready, :claimed]\n");
        let Some(got) = cruby(&format!("{out}\n{REDUCE_HOST}")) else { return };
        assert_eq!(got, want, "{iterator}:\n{out}");
    }
}

/// The `define_method` loop solid_queue writes beside it: unrolled, each
/// body's name is the literal the loop's element spells.
#[test]
fn a_name_built_in_an_unrolled_define_method_block_is_the_literal_it_spells() {
    let (out, diags) = ground_and_emit(
        r##"
module Executable
  %w[ ready claimed ].each do |status|
    define_method("#{status}?") { public_send("#{status}_execution").nil? }
    define_method("#{status}_name") { public_send(:"#{status}_label") }
  end

  def finished?
    false
  end
end
"##,
    );
    for call in [
        "public_send(\"ready_execution\")",
        "public_send(\"claimed_execution\")",
        "public_send(:ready_label)",
        "public_send(:claimed_label)",
    ] {
        assert!(out.contains(call), "{call} missing:\n{out}");
    }
    assert!(!out.contains("#{"), "a built name survived:\n{out}");
    assert!(diags.is_empty(), "{diags:?}");
}

/// The walk a list-local source ends with, and the host it runs in on
/// CRuby: each `*_execution` answers its status.
const LIST_WALK: &str = "statuses.map { |status| public_send(\"#{status}_execution\") }";
const LIST_HOST: &str = "\nclass Host\n  include Executable\n\n  def ready_execution = :ready\n  def claimed_execution = :claimed\n  def failed_execution = :failed\nend\n";

/// A list held in a local is a proof only while nothing else can reach
/// the list, or a String element to change in place: otherwise the walk
/// over the local keeps its built name. Each source calls
/// `failed_execution` on CRuby, a name its literal never spells.
#[test]
fn a_list_local_that_escapes_stays_dynamic() {
    let run = "p Host.new.execution";
    let later = "host = Host.new\nhost.execution << :failed\np host.instance_variable_get(:@later).call";
    for (label, method, driver) in [
        ("another local", "def execution\n    statuses = %w[ ready claimed ]\n    other = statuses\n    other << \"failed\"\n    WALK\n  end", run),
        ("an instance variable", "def execution\n    statuses = %w[ ready claimed ]\n    @kept = statuses\n    @kept.first.replace(\"failed\")\n    WALK\n  end", run),
        ("an Array element", "def execution\n    statuses = %w[ ready claimed ]\n    box = [statuses]\n    box.first << \"failed\"\n    WALK\n  end", run),
        ("the list `each` returns", "def execution\n    statuses = %i[ ready claimed ]\n    other = statuses.each { |status| status }\n    other << :failed\n    WALK\n  end", run),
        ("the list `push` returns", "def execution\n    statuses = %i[ ready ]\n    other = statuses.push(:claimed)\n    other << :failed\n    WALK\n  end", run),
        ("the list the method returns, walked later by a block", "def execution\n    statuses = %i[ ready claimed ]\n    @later = -> { WALK }\n    statuses.each { |status| status }\n  end", later),
        ("a walk without a block", "def execution\n    statuses = %w[ ready claimed ]\n    walker = statuses.map\n    walker.each { |status| status.replace(\"failed\") }\n    WALK\n  end", run),
        ("an element another walk stores", "def execution\n    statuses = %w[ ready claimed ]\n    statuses.each { |status| @kept = status }\n    @kept.replace(\"failed\")\n    WALK\n  end", run),
        ("an element a `map` result shares", "def execution\n    statuses = %w[ ready claimed ]\n    kept = statuses.map { |status| status }\n    kept.last.replace(\"failed\")\n    WALK\n  end", run),
        ("the only element, changed by another walk", "def execution\n    statuses = %w[ ready ]\n    statuses.each { |status| status.replace(\"failed\") }\n    WALK\n  end", run),
        ("a list bound again by `+=`", "def execution\n    statuses = %w[ ready claimed ]\n    statuses += %w[ failed ]\n    WALK\n  end", run),
        ("a block parameter of the same name", "def execution\n    statuses = %w[ ready claimed ]\n    [%w[ failed ]].flat_map { |statuses| WALK }\n  end", run),
        ("a parameter the body binds on one path only", "def execution(statuses = %w[ failed ], reset: false)\n    statuses = %w[ ready claimed ] if reset\n    WALK\n  end", run),
    ] {
        let source = format!("module Executable\n  {}\nend\n", method.replace("WALK", LIST_WALK));
        let (out, diags) = ground_and_emit(&source);
        assert!(out.contains("public_send(\"#{status}_execution\")"), "{label}: the send must survive untouched:\n{out}");
        assert!(!out.contains("when ") && !out.contains("public_send(\"ready_execution\")"), "{label}: rewritten without a proof:\n{out}");
        assert_eq!(diags.len(), 1, "{label}: expected one residue entry: {diags:?}");
        if let Some(ran) = cruby(&format!("{source}{LIST_HOST}{driver}\n")) {
            assert!(ran.contains(":failed"), "{label}: the source calls failed_execution:\n{ran}");
        }
    }
}

/// A block inside a walk keeps the String element it reads and runs after
/// the walk. When the method hands the list back, its caller can change
/// that element, so the kept block's name is not one the literal spells.
/// That holds for a list local that `each` returns, with a lambda or a
/// `proc`, for one another walk keeps from and `<<` returns, and for a
/// literal's `each`. Each source calls `failed_execution` on CRuby.
#[test]
fn a_string_element_a_block_keeps_stays_dynamic_when_the_list_is_handed_back() {
    let later = "host = Host.new\nhost.prepare.first.replace(\"failed\")\np host.instance_variable_get(:@run).call";
    let send = "public_send(\"#{status}_execution\")";
    for (label, method) in [
        ("a lambda, and `each` returns the list", "statuses = %w[ ready ]\n    statuses.each { |status| @run = -> { SEND } }"),
        ("a `proc`, and `each` returns the list", "statuses = %w[ ready ]\n    statuses.each { |status| @run = proc { SEND } }"),
        ("another walk keeps it, and `<<` returns the list", "statuses = %w[ ready ]\n    statuses.map { |status| @run = -> { SEND } }\n    statuses << \"claimed\""),
        ("a literal's `each`", "%w[ ready ].each { |status| @run = -> { SEND } }"),
    ] {
        let source = format!("module Executable\n  def prepare\n    {}\n  end\nend\n", method.replace("SEND", send));
        let (out, diags) = ground_and_emit(&source);
        assert!(out.contains(send), "{label}: the send must survive untouched:\n{out}");
        assert!(!out.contains("when ") && !out.contains("public_send(\"ready_execution\")"), "{label}: rewritten without a proof:\n{out}");
        assert_eq!(diags.len(), 1, "{label}: expected one residue entry: {diags:?}");
        if let Some(ran) = cruby(&format!("{source}{LIST_HOST}{later}\n")) {
            assert_eq!(ran, ":failed\n", "{label}: the source calls failed_execution");
        }
    }
}

/// A list nothing else can change still proves its names: a Symbol list
/// whose element another walk stores, as a Symbol cannot change; a String
/// list another walk reads only as an interpolated piece; a walk the method
/// ends with, whose list no block reads; a Symbol element a block keeps in
/// a list handed back; and a String element a block keeps in a list the
/// method does not hand back. The emitted module answers as the source does.
#[test]
fn a_list_local_nothing_else_can_change_still_proves_its_names() {
    let run = "p Host.new.execution";
    let keep = "statuses.each { |status| @run = -> { public_send(\"#{status}_execution\") } }";
    for (label, method, driver, expected) in [
        ("a stored Symbol element", "statuses = %i[ ready claimed ]\n    statuses.each { |status| @kept = status }\n    WALK", run, "[:ready, :claimed]\n"),
        ("a String element read only in interpolation", "statuses = %w[ ready claimed ]\n    statuses.each { |status| @seen = \"#{status} seen\" }\n    WALK", run, "[:ready, :claimed]\n"),
        (
            "a walk the method ends with",
            "statuses = %w[ ready claimed ]\n    @calls = []\n    statuses.each { |status| @calls << public_send(\"#{status}_execution\") }",
            "host = Host.new\np host.execution\np host.instance_variable_get(:@calls)",
            "[\"ready\", \"claimed\"]\n[:ready, :claimed]\n",
        ),
        (
            "a Symbol element a block keeps, in a list handed back",
            "statuses = %i[ ready claimed ]\n    KEEP",
            "host = Host.new\nlist = host.execution\nlist << :failed\np [list, host.instance_variable_get(:@run).call]",
            "[[:ready, :claimed, :failed], :claimed]\n",
        ),
        ("a String element a block keeps, in a list not handed back", "statuses = %w[ ready claimed ]\n    KEEP\n    @run.call", run, ":claimed\n"),
    ] {
        let source = format!("module Executable\n  def execution\n    {}\n  end\nend\n", method.replace("WALK", LIST_WALK).replace("KEEP", keep));
        let (out, diags) = ground_and_emit(&source);
        for name in ["ready_execution", "claimed_execution"] {
            assert!(out.contains(&format!("public_send(\"{name}\")")), "{label}: send of {name} missing:\n{out}");
        }
        assert!(!out.contains("public_send(\"#{"), "{label}: the built name survived:\n{out}");
        assert!(diags.is_empty(), "{label}: {diags:?}");
        let Some(want) = cruby(&format!("{source}{LIST_HOST}{driver}\n")) else { return };
        assert_eq!(want, expected, "{label}");
        let Some(got) = cruby(&format!("{out}\n{LIST_HOST}{driver}\n")) else { return };
        assert_eq!(got, want, "{label}:\n{out}");
    }
}

/// Without a finite proof the name stays built, and the site joins the
/// residue ledger: a list that is not a literal, an element parameter
/// the block rebinds or changes in place, or one an inner block's own
/// parameter shadows.
#[test]
fn a_built_name_without_a_finite_proof_stays_dynamic() {
    for (label, body, call) in [
        (
            "a list that is not a literal",
            "def execution(statuses)\n    statuses.reduce(nil) { |acc, status| acc || public_send(\"#{status}_execution\") }\n  end",
            "public_send(\"#{status}_execution\")",
        ),
        (
            "a reassigned element parameter",
            "def execution\n    %w[ ready claimed ].reduce(nil) do |acc, status|\n      status = status.upcase\n      acc || public_send(\"#{status}_execution\")\n    end\n  end",
            "public_send(\"#{status}_execution\")",
        ),
        (
            "an element parameter assigned to in a nested block",
            "def execution\n    %w[ ready claimed ].reduce(nil) do |acc, status|\n      [1].each { status += \"x\" }\n      acc || public_send(\"#{status}_execution\")\n    end\n  end",
            "public_send(\"#{status}_execution\")",
        ),
        (
            "an element String changed in place",
            "def execution\n    %w[ ready claimed ].reduce(nil) do |acc, status|\n      status << \"x\"\n      acc || public_send(\"#{status}_execution\")\n    end\n  end",
            "public_send(\"#{status}_execution\")",
        ),
        (
            "a rest parameter of the same name in an inner element block",
            "def execution\n    %w[ ready claimed ].each do |status|\n      [[1]].each { |value, *status| public_send(\"#{status}_execution\") }\n    end\n  end",
            "public_send(\"#{status}_execution\")",
        ),
        (
            "a rest parameter of the same name in an inner block that binds no element",
            "def execution\n    %w[ ready claimed ].each do |status|\n      tap { |value, *status| public_send(\"#{status}_execution\") }\n    end\n  end",
            "public_send(\"#{status}_execution\")",
        ),
        (
            "a reassigned each parameter",
            "def execution\n    [:ready, :claimed].each do |status|\n      status = :failed\n      send(status)\n    end\n  end",
            "send(status)",
        ),
    ] {
        let (out, diags) = ground_and_emit(&format!("module Executable\n  {body}\nend\n"));
        assert!(out.contains(call), "{label}: the send must survive untouched:\n{out}");
        assert!(!out.contains("when "), "{label}: rewritten without a proof:\n{out}");
        assert_eq!(diags.len(), 1, "{label}: expected one residue entry: {diags:?}");
        assert!(diags[0].to_string().contains("not statically enumerable"), "{label}: {}", diags[0]);
    }
}
