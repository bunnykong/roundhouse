# frozen_string_literal: true

# The Progress page's view: turns site.data.status and site.data.status_history
# (see status.rb) into the rows, copy and inline SVG charts that
# progress/index.html lays out. Text from outside the repository is escaped
# here (for SVG) or by Liquid's `escape` (everywhere else).

require "cgi"
require "time"

module RoundhouseProgress
  module_function

  # How old a result may be before the page says so. Editorial, not a diagnosis.
  STALE_HOURS = { "specs" => 36, "replay" => 36, "archives" => 12, "bench" => 24 * 7, "activity" => 24, "help" => 24 }.freeze

  AREAS = { "compiler" => "Roundhouse compiler", "roundhouse" => "Roundhouse", "app" => "App behaviour", "gem" => "Gem support",
            "spinel" => "Spinel", "harness" => "Test harness" }.freeze
  STEPS = {
    "unclassified" => "Reproduce one example, compare it with Rails, then give the failure a name.",
    "expectation-mismatch" => "Reduce one wrong answer to a few lines.",
    "symbol-into-integer" => "Find the first value whose type differs from Rails."
  }.freeze
  CLOSES = "Reduce a named failure, add a regression test, and check the answer against Rails."

  TARGETS = { "ruby" => "Ruby", "rust" => "Rust", "go" => "Go", "typescript" => "TypeScript", "elixir" => "Elixir", "crystal" => "Crystal",
              "python" => "Python", "csharp" => "C#", "kotlin" => "Kotlin", "swift" => "Swift", "spinel" => "Spinel", "jruby" => "JRuby",
              "docker" => "Docker image", "typescript-worker" => "TypeScript, in a worker", "futamura" => "Futamura", "blog" => "Rails source" }.freeze

  BENCH_PAIRS = [%w[ruby rails CRuby\ +\ YJIT], %w[ruby-int rails-int CRuby,\ no\ JIT], %w[jruby rails-jruby JRuby], %w[truffleruby rails-truffleruby TruffleRuby]].freeze

  def h(text) = CGI.escapeHTML(text.to_s)
  def date(iso) = iso && Time.parse(iso).utc.strftime("%-d %b %Y")
  def datetime(iso) = iso && Time.parse(iso).utc.strftime("%-d %b %Y, %H:%M UTC")
  def short(sha) = sha.to_s[0, 7]
  def num(n) = n.is_a?(Integer) ? n.to_s.gsub(/\B(?=(\d{3})+\z)/, ",") : n.to_s
  def plural(n, one, many = "#{one}s") = "#{num(n)} #{n == 1 ? one : many}"

  def state(key, feed, now)
    return { "available" => false, "note" => feed&.dig("fetch") == "offline" ? "Not collected in this build." : "Couldn't load this source." } unless feed&.dig("data")
    observed = feed["observed_at"]
    stale = observed && STALE_HOURS[key] && (now - Time.parse(observed)) / 3600 > STALE_HOURS[key]
    note = if feed["cached"] then "Couldn't refresh this source. Showing the saved result from #{datetime(observed) || "an unknown date"}."
           elsif stale then "Last result #{datetime(observed)}; no newer result is available."
           end
    { "available" => true, "observed" => datetime(observed), "observed_date" => date(observed), "stale" => !!stale, "cached" => feed["cached"], "note" => note }
  end

  # --- Charts (inline SVG; colours are brand tokens, set in progress.css) -----

  # A round axis maximum, and how many gridlines divide it evenly.
  def nice_max(value)
    return [4, 4] if value <= 4
    step = 10**Math.log10(value).floor
    m = [1, 2, 3, 4, 5, 6, 8, 10].find { |x| x * step >= value }
    [m * step, [3, 6].include?(m) ? 3 : 4]
  end

  # A bar with its data end rounded (4px) and its baseline end square.
  def bar_path(x, y, w, hgt, horizontal: false, r: 4)
    r = [r, (horizontal ? hgt : w) / 2.0, horizontal ? w : hgt].min
    if horizontal # baseline on the left
      "M#{x},#{y}h#{w - r}a#{r},#{r} 0 0 1 #{r},#{r}v#{hgt - 2 * r}a#{r},#{r} 0 0 1 -#{r},#{r}h-#{w - r}z"
    else # baseline at the bottom
      "M#{x},#{y + hgt}v-#{hgt - r}a#{r},#{r} 0 0 1 #{r},-#{r}h#{w - 2 * r}a#{r},#{r} 0 0 1 #{r},#{r}v#{hgt - r}z"
    end
  end

  def svg(width, height, title, desc, body)
    id = "c#{title.hash.abs.to_s(36)}"
    %(<svg class="chart" viewBox="0 0 #{width} #{height}" role="group" aria-labelledby="#{id}-t #{id}-d"><title id="#{id}-t">#{h(title)}</title>) +
      %(<desc id="#{id}-d">#{h(desc)}</desc>#{body}</svg>)
  end

  def grid(x0, x1, y_of, max, ticks: 4)
    (0..ticks).map do |i|
      v = max * i / ticks.to_f
      y = y_of.(v).round(1)
      label = v == v.round ? v.round : v.round(1)
      %(<line class="grid" x1="#{x0}" x2="#{x1}" y1="#{y}" y2="#{y}"/><text class="tick" x="#{x0 - 8}" y="#{y + 4}" text-anchor="end">#{num(label)}</text>)
    end.join
  end

  def weekly_chart(weekly)
    w, hgt, left, right, top, bottom = 720, 220, 44, 8, 16, 34
    max, ticks = nice_max(weekly.map { |p| p["merged"] }.max.to_i)
    y_of = ->(v) { top + (hgt - top - bottom) * (1 - v / max.to_f) }
    slot = (w - left - right) / weekly.size.to_f
    bw = [slot - 2, 44].min.floor
    bars = weekly.each_with_index.map do |p, i|
      x = (left + i * slot + (slot - bw) / 2).round(1)
      y = y_of.(p["merged"]).round(1)
      tip = "Week of #{date(p["week"])}: #{plural(p["merged"], "pull request")} merged#{" so far" if p["partial"]}"
      label = Time.parse(p["week"]).strftime("%-d %b")
      %(<g class="mark#{" partial" if p["partial"]}" role="img" tabindex="0" data-tip="#{h(tip)}" aria-label="#{h(tip)}">) +
        %(<rect class="hit" x="#{(left + i * slot).round(1)}" y="#{top}" width="#{slot.round(1)}" height="#{hgt - top - bottom}"/>) +
        (p["merged"].positive? ? %(<path class="bar" d="#{bar_path(x, y, bw, (hgt - bottom - y).round(1))}"/>) : "") +
        %(<text class="tick" x="#{(x + bw / 2.0).round(1)}" y="#{hgt - bottom + 18}" text-anchor="middle">#{h(label)}#{"*" if p["partial"]}</text></g>)
    end.join
    last = weekly[-2] || weekly[-1]
    svg(w, hgt, "Pull requests merged per week", "#{weekly.size} weeks; the last week is still in progress. Most recent full week: #{num(last["merged"])}.",
        grid(left, w - right, y_of, max, ticks: ticks) + %(<line class="axis" x1="#{left}" x2="#{w - right}" y1="#{y_of.(0)}" y2="#{y_of.(0)}"/>) + bars)
  end

  def bench_chart(pair, endpoint)
    w, left, right, row = 720, 150, 96, 36
    hgt = row * pair.size + 8
    max = pair.map { |r| r["req_sec"] }.max
    bars = pair.each_with_index.map do |r, i|
      y = 4 + i * row + 10
      bw = ((w - left - right) * r["req_sec"] / max).round(1)
      tip = "#{r["label"]}: #{num(r["req_sec"].round)} requests per second on #{endpoint}"
      %(<g class="mark" role="img" tabindex="0" data-tip="#{h(tip)}" aria-label="#{h(tip)}"><rect class="hit" x="0" y="#{y - 8}" width="#{w}" height="#{row}"/>) +
        %(<text class="bar-label" x="#{left - 12}" y="#{y + 12}" text-anchor="end">#{h(r["label"])}</text>) +
        %(<path class="bar" d="#{bar_path(left, y, bw, 16, horizontal: true)}"/>) +
        %(<text class="value" x="#{left + bw + 8}" y="#{y + 12}">#{num(r["req_sec"].round)} req/s</text></g>)
    end.join
    svg(w, hgt, "Requests per second on #{endpoint}", pair.map { |r| "#{r["label"]} #{num(r["req_sec"].round)}" }.join("; "),
        %(<line class="axis" x1="#{left}" x2="#{left}" y1="0" y2="#{hgt}"/>) + bars)
  end

  def gap_chart(points)
    w, hgt, left, right, top, bottom = 720, 220, 44, 64, 16, 34
    max, ticks = nice_max(points.map { |p| p["failed"] }.max.to_i)
    y_of = ->(v) { top + (hgt - top - bottom) * (1 - v / max.to_f) }
    times = points.map { |p| Time.parse(p["observed_at"]).to_f }
    span = times.max - times.min
    x_of = ->(t) { span.zero? ? (left + (w - left - right) / 2.0) : left + (w - left - right) * (t - times.min) / span }
    lines = points.each_with_index.slice_when { |(a, _), (b, _)| a["break"] || b["new_segment"] }.map do |seg|
      next "" if seg.size < 2
      %(<polyline class="line" points="#{seg.map { |p, i| "#{x_of.(times[i]).round(1)},#{y_of.(p["failed"]).round(1)}" }.join(" ")}"/>)
    end.join
    marks = points.each_with_index.map do |p, i|
      x, y = x_of.(times[i]).round(1), y_of.(p["failed"]).round(1)
      tip = "#{datetime(p["observed_at"])}: #{num(p["failed"])} failing of #{num(p["total"])} run (#{p["spec_files_run"]} spec files)"
      %(<g class="mark" role="img" tabindex="0" data-tip="#{h(tip)}" aria-label="#{h(tip)}"><circle class="hit" cx="#{x}" cy="#{y}" r="14"/><circle class="dot" cx="#{x}" cy="#{y}" r="4.5"/></g>)
    end.join
    last = points.last
    lx, ly = x_of.(times.last).round(1), y_of.(last["failed"]).round(1)
    first_date = Time.parse(points.first["observed_at"]).strftime("%-d %b")
    last_date = Time.parse(last["observed_at"]).strftime("%-d %b")
    xticks = %(<text class="tick" x="#{x_of.(times.first).round(1)}" y="#{hgt - bottom + 18}" text-anchor="#{span.zero? ? "middle" : "start"}">#{h(first_date)}</text>) +
             (span.zero? ? "" : %(<text class="tick" x="#{lx}" y="#{hgt - bottom + 18}" text-anchor="end">#{h(last_date)}</text>))
    svg(w, hgt, "Failing Lobsters model examples over time", "#{points.size} observation(s); latest #{num(last["failed"])} failing on #{last_date}.",
        grid(left, w - right, y_of, max, ticks: ticks) + %(<line class="axis" x1="#{left}" x2="#{w - right}" y1="#{y_of.(0)}" y2="#{y_of.(0)}"/>) +
        lines + marks + %(<text class="value" x="#{lx + 10}" y="#{ly + 4}">#{num(last["failed"])}</text>) + xticks)
  end

  # --- Modules ----------------------------------------------------------------

  def gap(feed, repo)
    d = feed["data"]
    group = lambda do |c|
      { "slug" => c["slug"], "anchor" => "cause-#{c["slug"]}", "title" => c["title"], "failures" => c["failures"],
        "area" => c["slug"] == "unclassified" ? "Needs triage" : AREAS.fetch(c["owner"].to_s, "Needs triage"),
        "step" => c["stale_rule"] ? "Recheck its original reproducer before retiring the rule." : STEPS.fetch(c["slug"], "Reproduce a named example."),
        "note" => c["note"], "examples" => c["examples"], "more" => [c["example_count"].to_i - c["examples"].size, 0].max,
        "related_url" => "https://github.com/#{repo}/issues?q=#{CGI.escape(c["slug"])}" }
    end
    active = d["causes"].reject { |c| c["stale_rule"] || c["failures"].zero? }
                        .sort_by { |c| [c["slug"] == "unclassified" ? 0 : 1, -c["failures"], c["slug"]] }
    ex = d["examples"]
    d.slice("scope", "canary_ok", "consistent", "problems", "harness_compensations", "warnings").merge(
      "failed" => num(ex["failed"]), "passed" => num(ex["passed"]), "total" => num(ex["total"]), "pending" => ex["pending"],
      "outside" => ex["errors_outside_examples"], "headline" => d["canary_ok"] && d["consistent"],
      "ruby" => d["ruby_pinned"], "lobsters_sha" => short(d["lobsters_sha"]), "lobsters_url" => "https://github.com/lobsters/lobsters/commit/#{d["lobsters_sha"]}",
      "roundhouse_sha" => short(d["roundhouse_sha"]), "roundhouse_url" => "https://github.com/#{repo}/commit/#{d["roundhouse_sha"]}",
      "report_url" => feed["sources"].last, "groups" => active.map(&group), "group_count" => active.count { |c| c["slug"] != "unclassified" },
      "unclassified" => d["causes"].find { |c| c["slug"] == "unclassified" }&.dig("failures").to_i,
      "stale_rules" => d["causes"].select { |c| c["stale_rule"] }.sort_by { |c| c["slug"] }.map(&group),
      "warning_kinds" => d.dig("warnings", "by_kind").sort_by { |_, n| -n }.map { |k, n| { "kind" => k.tr("_", " "), "count" => num(n) } },
      "warning_total" => num(d.dig("warnings", "total").to_i)
    )
  end

  def over_time(history)
    seen = {}
    points = history["points"].filter_map { |p| p["specs"]&.dup }.reject { |s| s["cached"] || s["observed_at"].nil? }
                              .select { |s| seen[s["observed_at"]] ? false : (seen[s["observed_at"]] = true) }.sort_by { |s| s["observed_at"] }
    points.each_cons(2) do |a, b|
      b["new_segment"] = a["scope_key"] != b["scope_key"]
      a["break"] = (Time.parse(b["observed_at"]) - Time.parse(a["observed_at"])) > 36 * 3600
    end
    comparable = points.last(2).size == 2 && !points.last["new_segment"]
    change = if comparable
      diff = points[-1]["failed"] - points[-2]["failed"]
      since = date(points[-2]["observed_at"])
      if diff.zero? then "No change in failing examples since #{since}, with the same app revision and scope."
      else "#{num(diff.abs)} #{diff.negative? ? "fewer" : "more"} failing #{diff.abs == 1 ? "example" : "examples"} since #{since}, with the same app revision and scope."
      end
    end
    {
      "count" => points.size, "since" => date(history["points"].first&.dig("at")), "interrupted" => history["continuity"] == "interrupted",
      "chart" => points.empty? ? nil : gap_chart(points), "change" => change,
      "rows" => points.reverse.map do |p|
        { "date" => datetime(p["observed_at"]), "failed" => num(p["failed"]), "total" => num(p["total"]), "files" => p["spec_files_run"],
          "outside" => p["errors_outside_examples"], "lobsters_sha" => short(p["lobsters_sha"]), "roundhouse_sha" => short(p["roundhouse_sha"]),
          "note" => p["new_segment"] ? "Scope changed" : nil }
      end
    }
  end

  def replay(feed, routes_feed)
    lanes = feed&.dig("data", "lanes").to_a.map do |l|
      ok = l["state"] == "ok"
      comparison = if l["kind"] == "rails" then "Reference"
                   elsif ok && l["content_total"] then "#{l["content_equivalent"]} of #{l["content_total"]} routes match after normalization#{" (partial run)" if l["partial"]}"
                   elsif ok then "Not compared"
                   else "Unavailable: the replay didn't complete"
                   end
      { "label" => [l["label"], l["jit"]].compact.join(" · "), "runtime" => l["runtime"], "ok" => ok,
        "outcome" => ok ? "Completed" : "Stopped at #{l["stage"] || "an unknown stage"}", "detail" => l["detail"], "comparison" => comparison }
    end
    failed = lanes.reject { |l| l["ok"] || l["comparison"] == "Reference" }
    entries = routes_feed&.dig("data", "entries").to_a.sort_by { |e| e["route"] }.map do |e|
      slug = e["route"].delete_prefix("/").gsub(/[^a-z0-9]+/i, "-").gsub(/^-|-$/, "").downcase
      e.merge("anchor" => "route-#{slug.empty? ? "root" : slug}", "area" => AREAS.fetch(e["owner"].to_s, e["owner"]))
    end
    { "lanes" => lanes, "failed_count" => failed.size, "all_failed" => !lanes.empty? && failed.size == lanes.count { |l| l["comparison"] != "Reference" },
      "workload" => feed&.dig("data", "workload"), "source" => feed&.dig("sources", 0), "entries" => entries }
  end

  def targets(feed)
    d = feed["data"]
    rows = d["rows"].map do |r|
      group, name = r["path"].split("/", 2)
      name = File.basename(name, ".tgz")
      verified = r["selected"] && r["status"] == "passed" && r["harness_outcome"] == "success" && r["validation_attempt"] == d["attempt"]
      result, css = if r["check"].nil? then ["No named smoke check", "none"]
                     elsif verified then ["Passed", "ok"]
                     elsif r["selected"] && r["status"] == "failed" then ["Failed", "fail"]
                     elsif r["selected"] then [r["status"].to_s.capitalize, "unknown"]
                     else ["Not selected", "none"]
                     end
      detail = if !r["selected"] && r["harness_outcome"] then "Its check reported #{r["harness_outcome"]}."
               elsif !r["reason"].to_s.empty? then r["reason"]
               end
      { "app" => group == "campfire" ? "Campfire" : "Blog", "name" => TARGETS.fetch(name, name), "check" => r["check"], "result" => result, "class" => css, "detail" => detail }
    end
    checked = rows.select { |r| r["check"] }
    selected = d["rows"].count { |r| r["check"] && r["selected"] }
    passed = checked.count { |r| r["class"] == "ok" }
    not_selected = checked.count { |r| r["result"] == "Not selected" }
    {
      "headline" => "#{passed} of #{selected} selected checks passed", "not_selected" => not_selected,
      "apps" => checked.group_by { |r| r["app"] }.map { |app, rs| { "app" => app, "rows" => rs.sort_by { |r| r["name"].downcase } } },
      "unchecked" => rows.reject { |r| r["check"] }.map { |r| "#{r["app"]}: #{r["name"]}" },
      "run_url" => d.dig("run", "url") || "https://github.com/roundhouse-rb/roundhouse/actions/runs/#{d["run_id"]}",
      "run_conclusion" => d.dig("run", "conclusion"), "source_sha" => short(d["source_sha"]), "source_sha_full" => d["source_sha"]
    }
  end

  def bench(feed)
    d = feed["data"]
    rows = d["rows"].select { |r| r["workers"] == 1 }
    at = ->(target, endpoint) { rows.find { |r| r["target"] == target && r["endpoint"] == endpoint } }
    endpoints = rows.map { |r| r["endpoint"] }.uniq
    pairs = BENCH_PAIRS.filter_map do |gen, rails, label|
      ratios = endpoints.map { |e| (r = RoundhouseStatus.ratio(rows, gen, rails, e)) ? "#{r}×" : "—" }
      { "label" => label, "ratios" => ratios } unless ratios.all?("—")
    end
    ruby, rails = at.("ruby", "/articles"), at.("rails", "/articles")
    headline = RoundhouseStatus.ratio(rows, "ruby", "rails", "/articles")
    chart = headline && bench_chart([ruby.merge("label" => "Generated Ruby"), rails.merge("label" => "Rails")], "/articles")
    table = rows.select { |r| r["endpoint"] == "/articles" }.sort_by { |r| r["target"] }.map do |r|
      { "target" => r["target"], "req_sec" => r["req_sec"] && num(r["req_sec"].round), "p50" => r["p50_ms"]&.round(1), "p99" => r["p99_ms"]&.round(1),
        "rss" => r["rss_mb"]&.round, "per_gib" => r["rps_per_gb"] && num(r["rps_per_gb"].round), "runs" => r["runs"], "errors" => r["non_2xx"] }
    end
    env = d["environment"]
    { "headline" => headline && "#{headline}×", "chart" => chart, "runs" => ruby&.dig("runs"), "endpoints" => endpoints, "pairs" => pairs, "table" => table,
      "env_date" => env && date(env["captured_at"]), "env_sha" => env && short(env.dig("git", "sha")), "toolchains" => env&.dig("toolchains").to_h.map { |k, v| "#{k} #{v}" } }
  end

  def activity(feed)
    d = feed["data"]
    weekly = d["weekly_merged"]
    d.slice("people_30d", "recent").merge(
      "merged_7d" => num(d["merged_7d"]), "people_7d" => d["people_7d"], "merged_all" => num(d["merged_all"].to_i),
      "contributors_30d" => d["people_30d"].size, "chart" => weekly.empty? ? nil : weekly_chart(weekly),
      "weeks" => weekly.reverse.map { |w| { "week" => date(w["week"]), "merged" => num(w["merged"]), "partial" => w["partial"] } },
      "firsts" => d["first_contributions_30d"].map { |p| p.merge("date" => date(p.dig("first_pr", "merged_at"))) },
      "recent" => d["recent"].map { |r| r.merge("date" => date(r["merged_at"])) }
    )
  end

  SOURCE_NAMES = { "specs" => "Lobsters model specs", "replay" => "Frozen Lobsters replay", "routes" => "Frozen route ledger",
                   "archives" => "Archive smoke checks", "bench" => "Reference blog benchmarks", "activity" => "Merged pull requests",
                   "help" => "Labelled issues" }.freeze

  def build(site)
    status = site.data["status"] or return nil
    now = Time.parse(status["generated_at"])
    repo = site.config.dig("status", "repo")
    states = SOURCE_NAMES.keys.to_h { |k| [k, state(k, status[k], now)] }
    view = { "built" => datetime(status["generated_at"]), "site_sha" => short(status["site_sha"]), "states" => states, "repo" => repo }
    view["gap"] = gap(status["specs"], repo) if states.dig("specs", "available")
    view["over_time"] = over_time(site.data["status_history"])
    view["replay"] = replay(status["replay"], status["routes"])
    view["targets"] = targets(status["archives"]) if states.dig("archives", "available")
    view["bench"] = bench(status["bench"]) if states.dig("bench", "available")
    view["activity"] = activity(status["activity"]) if states.dig("activity", "available")
    view["help"] = status.dig("help", "data")
    view["sources"] = SOURCE_NAMES.map do |key, name|
      feed = status[key].to_h
      { "name" => name, "url" => feed["sources"]&.first, "observed" => datetime(feed["observed_at"]) || "Not published",
        "fetch" => { "ok" => feed["cached"] ? "Saved result" : "Collected", "missing" => "Not found", "error" => "Couldn't load", "offline" => "Not collected" }[feed["fetch"]] || "Not collected",
        "stale" => states.dig(key, "stale") }
    end
    view
  end

  class Generator < Jekyll::Generator
    safe true
    priority :normal # after RoundhouseStatus::Generator

    def generate(site)
      site.data["progress"] = RoundhouseProgress.build(site)
    end
  end
end
