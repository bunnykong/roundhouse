# frozen_string_literal: true

# The Progress page's view: turns site.data.status and site.data.status_history
# (see status.rb) into the rows, copy and inline SVG charts that
# progress/index.html lays out. Text from outside the repository is escaped
# here (for SVG) or by Liquid's `escape` (everywhere else).

require "cgi"
require "date"
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
  # The same first steps, as the few words a link can carry.
  ACTIONS = { "unclassified" => "Reproduce and classify", "expectation-mismatch" => "Reduce one wrong answer",
              "symbol-into-integer" => "Find the first mismatch" }.freeze
  CLOSES = "Reduce a named failure, add a regression test, and check the answer against Rails."

  TARGETS = { "ruby" => "Ruby", "rust" => "Rust", "go" => "Go", "typescript" => "TypeScript", "elixir" => "Elixir", "crystal" => "Crystal",
              "python" => "Python", "csharp" => "C#", "kotlin" => "Kotlin", "swift" => "Swift", "spinel" => "Spinel", "jruby" => "JRuby",
              "docker" => "Docker image", "typescript-worker" => "TypeScript, in a worker", "futamura" => "Futamura", "blog" => "Rails source" }.freeze

  BENCH_PAIRS = [%w[ruby rails CRuby\ +\ YJIT], %w[ruby-int rails-int CRuby,\ no\ JIT], %w[jruby rails-jruby JRuby], %w[truffleruby rails-truffleruby TruffleRuby]].freeze

  def h(text) = CGI.escapeHTML(text.to_s)
  # A calendar date ("2026-09-14") stays that day in any build time zone.
  def day(iso) = iso.to_s.match?(/\A\d{4}-\d{2}-\d{2}\z/) ? Date.parse(iso) : Time.parse(iso).utc
  def date(iso) = iso && day(iso).strftime("%-d %b %Y")
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
    { "available" => true, "observed" => datetime(observed), "observed_date" => date(observed), "observed_short" => observed && Time.parse(observed).utc.strftime("%-d %b"), "stale" => !!stale, "cached" => feed["cached"], "note" => note }
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

  def svg(width, height, title, desc, body, cls: nil)
    id = "c#{"#{title}#{width}".hash.abs.to_s(36)}"
    %(<svg class="chart#{" #{cls}" if cls}" viewBox="0 0 #{width} #{height}" role="group" aria-labelledby="#{id}-t #{id}-d"><title id="#{id}-t">#{h(title)}</title>) +
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

  # Merged pull requests per week, as data for HTML bars. Heights are relative
  # to the busiest week, which carries the only direct label.
  def weekly_bars(weekly)
    max = weekly.map { |p| p["merged"] }.max.to_i
    weekly.each_with_index.map do |p, i|
      tip = "Week of #{date(p["week"])}: #{plural(p["merged"], "pull request")} merged#{" so far" if p["partial"]}"
      { "label" => day(p["week"]).strftime("%-d %b"), "merged" => num(p["merged"]), "partial" => p["partial"], "tip" => tip,
        "pct" => max.zero? ? 0 : (p["merged"] * 100.0 / max).round(1), "peak" => p["merged"] == max && max.positive?,
        "show_label" => i.zero? || i == weekly.size - 1 }
    end
  end

  # Drawn twice, wide and narrow, so its text stays legible; CSS shows one.
  def gap_chart(points, w: 480, cls: "wide")
    hgt, left, right, top, bottom = 150, 40, 44, 12, 26
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
    xticks = %(<text class="tick" x="#{x_of.(times.first).round(1)}" y="#{hgt - bottom + 17}" text-anchor="#{span.zero? ? "middle" : "start"}">#{h(first_date)}</text>) +
             (span.zero? ? "" : %(<text class="tick" x="#{lx}" y="#{hgt - bottom + 17}" text-anchor="end">#{h(last_date)}</text>))
    svg(w, hgt, "Failing Lobsters model examples over time", "#{points.size} observation(s); latest #{num(last["failed"])} failing on #{last_date}.",
        grid(left, w - right, y_of, max, ticks: ticks) + %(<line class="axis" x1="#{left}" x2="#{w - right}" y1="#{y_of.(0)}" y2="#{y_of.(0)}"/>) +
        lines + marks + %(<text class="value" x="#{lx + 10}" y="#{ly + 4}">#{num(last["failed"])}</text>) + xticks, cls: cls)
  end

  # --- Figures beside the results (180 x 24; the table row carries the numbers) --

  def fig(label, body, w: 180, hgt: 24)
    %(<svg class="fig" width="#{w}" height="#{hgt}" viewBox="0 0 #{w} #{hgt}" role="img" aria-label="#{h(label)}">#{body}</svg>)
  end

  def mark(title, shape) = %(<g><title>#{h(title)}</title>#{shape}</g>)

  # Passed, then the gap: the failing part is drawn as an empty, dashed track.
  def gap_meter(passed, total)
    w = total.positive? ? (178.0 * passed / total).round(1) : 0
    fig("#{num(passed)} of #{num(total)} examples pass; the dashed gap is the #{num(total - passed)} that fail",
        mark("#{num(passed)} pass", %(<rect class="f-pass" x="0" y="7" width="#{w}" height="10" rx="3"/>)) +
        mark("#{num(total - passed)} fail: the gap", %(<rect class="f-gap" x="#{(w + 2.5).round(1)}" y="7.5" width="#{(177.5 - w).clamp(0, 178).round(1)}" height="9" rx="3"/>)))
  end

  # One square per item: green with a tick when it passed, red with a cross when
  # it failed, a dashed outline with a cross when it didn't finish. A wider space
  # separates groups.
  def unit_marks(items, label)
    x = 0
    body = items.each_with_index.map do |item, i|
      x += 10 if i.positive? && item["group"] != items[i - 1]["group"]
      shape = if item["ok"]
        %(<rect class="f-ok" x="#{x}" y="4" width="16" height="16" rx="3"/><text class="f-tick" x="#{x + 8}" y="15.5" text-anchor="middle">✓</text>)
      elsif item["failed"]
        %(<rect class="f-bad" x="#{x}" y="4" width="16" height="16" rx="3"/><text class="f-tick" x="#{x + 8}" y="15.5" text-anchor="middle">✕</text>)
      else
        %(<rect class="f-open" x="#{x + 0.5}" y="4.5" width="15" height="15" rx="3"/><text class="f-cross" x="#{x + 8}" y="15.5" text-anchor="middle">✕</text>)
      end
      x += 20
      mark(item["title"], shape)
    end.join
    fig(label, body, w: [x, 180].max)
  end

  # Throughput against Rails on one scale: Rails is 1.
  def ratio_bars(ratio)
    w = 124.0
    rails, gen = ratio >= 1 ? [(w / ratio).round(1), w] : [w, (w * ratio).round(1)]
    fig("Requests per second on /articles: Rails 1, generated Ruby #{ratio}",
        mark("Rails: 1×", %(<rect class="f-ref" x="0" y="3" width="#{rails}" height="7" rx="2"/>)) +
        %(<text class="f-label" x="#{(rails + 6).round(1)}" y="10">Rails</text>) +
        mark("Generated Ruby: #{ratio}×", %(<rect class="f-pass" x="0" y="14" width="#{gen}" height="7" rx="2"/>)) +
        %(<text class="f-label" x="#{(gen + 6).round(1)}" y="21">Roundhouse</text>), w: 200)
  end

  # Merged per week, small, on a baseline: a week with none draws nothing, and the
  # current week is an outline because it isn't over.
  def weeks_spark(weekly)
    max = weekly.map { |p| p["merged"] }.max.to_i
    w = [weekly.size * 18 - 6, 12].max
    body = %(<rect class="f-base" x="0" y="27" width="#{w}" height="1"/>) + weekly.each_with_index.map do |p, i|
      hgt = max.zero? ? 0 : (26.0 * p["merged"] / max).round(2)
      next "" if hgt.zero?
      x = i * 18
      tip = "Week of #{date(p["week"])}: #{num(p["merged"])} merged#{" so far" if p["partial"]}"
      s = [1.0, hgt / 2].min
      mark(tip, p["partial"] ? %(<rect class="f-week partial" x="#{x + s / 2}" y="#{(27 - hgt + s / 2).round(2)}" width="#{12 - s}" height="#{(hgt - s).round(2)}" rx="2" stroke-width="#{s}"/>)
                             : %(<rect class="f-week" x="#{x}" y="#{(27 - hgt).round(2)}" width="12" height="#{hgt}" rx="2"/>))
    end.join
    label = "Merged per week, #{date(weekly.first["week"])} to #{date(weekly.last["week"])}: " + weekly.map { |p| "#{num(p["merged"])}#{" so far" if p["partial"]}" }.join(", ")
    fig(label, body, w: w, hgt: 28)
  end

  # --- Modules ----------------------------------------------------------------

  def gap(feed, repo)
    d = feed["data"]
    failing = d.dig("examples", "failed").to_i
    group = lambda do |c|
      { "slug" => c["slug"], "share" => failing.positive? ? (c["failures"] * 100.0 / failing).round(1) : 0, "anchor" => "cause-#{c["slug"]}", "title" => c["title"], "failures" => c["failures"],
        "area" => c["slug"] == "unclassified" ? "Needs triage" : AREAS.fetch(c["owner"].to_s, "Needs triage"),
        "step" => c["stale_rule"] ? "Recheck its original reproducer before retiring the rule." : STEPS.fetch(c["slug"], "Reproduce a named example."),
        "action" => c["stale_rule"] ? "Recheck the reproducer" : ACTIONS.fetch(c["slug"], "Reproduce a named example"),
        "note" => c["note"], "examples" => c["examples"], "more" => [c["example_count"].to_i - c["examples"].size, 0].max,
        "related_url" => "https://github.com/#{repo}/issues?q=#{CGI.escape(c["slug"])}" }
    end
    active = d["causes"].reject { |c| c["stale_rule"] || c["failures"].zero? }
                        .sort_by { |c| [c["slug"] == "unclassified" ? 0 : 1, -c["failures"], c["slug"]] }
    ex = d["examples"]
    d.slice("scope", "canary_ok", "consistent", "problems", "harness_compensations", "warnings").merge(
      "failed" => num(ex["failed"]), "passed" => num(ex["passed"]), "total" => num(ex["total"]), "pending" => ex["pending"],
      "outside" => ex["errors_outside_examples"], "headline" => d["canary_ok"] && d["consistent"],
      "pass_pct" => ex["total"].to_i.positive? ? (ex["passed"] * 100.0 / ex["total"]).round(1) : 0,
      "ruby" => d["ruby_pinned"], "lobsters_sha" => short(d["lobsters_sha"]), "lobsters_url" => "https://github.com/lobsters/lobsters/commit/#{d["lobsters_sha"]}",
      "roundhouse_sha" => short(d["roundhouse_sha"]), "roundhouse_url" => "https://github.com/#{repo}/commit/#{d["roundhouse_sha"]}",
      "report_url" => feed["sources"].last, "groups" => active.map(&group), "group_count" => active.count { |c| c["slug"] != "unclassified" },
      "smaller_count" => [active.size - 3, 0].max, "smaller_examples" => num(active.drop(3).sum { |c| c["failures"] }),
      # Two parts only: with pending examples, passed + failed isn't the whole run.
      "figure" => d["canary_ok"] && d["consistent"] && ex["pending"].to_i.zero? ? gap_meter(ex["passed"].to_i, ex["total"].to_i) : nil,
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
      "chart" => points.empty? ? nil : gap_chart(points) + gap_chart(points, w: 330, cls: "narrow"), "change" => change,
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
        "reference" => l["kind"] == "rails", "outcome" => ok ? "Completed" : "Stopped at #{l["stage"] || "an unknown stage"}", "detail" => l["detail"], "comparison" => comparison }
    end
    failed = lanes.reject { |l| l["ok"] || l["comparison"] == "Reference" }
    entries = routes_feed&.dig("data", "entries").to_a.sort_by { |e| e["route"] }.map do |e|
      slug = e["route"].delete_prefix("/").gsub(/[^a-z0-9]+/i, "-").gsub(/^-|-$/, "").downcase
      e.merge("anchor" => "route-#{slug.empty? ? "root" : slug}", "area" => AREAS.fetch(e["owner"].to_s, e["owner"]))
    end
    compiled = lanes.reject { |l| l["comparison"] == "Reference" }
    lead = if compiled.empty? then nil
           elsif compiled.none? { |l| l["ok"] } then "No compiled configuration completed the latest run, so route agreement is unknown."
           elsif compiled.all? { |l| l["ok"] } then "Every compiled configuration completed the latest run."
           else "#{compiled.count { |l| l["ok"] }} of #{compiled.size} compiled configurations completed the latest run."
           end
    figure = compiled.empty? ? nil : unit_marks(compiled.map { |l| { "ok" => l["ok"], "title" => "#{l["label"]}: #{l["ok"] ? "completed" : l["outcome"].downcase}", "group" => 0 } },
                                                 "#{compiled.count { |l| l["ok"] }} of #{compiled.size} compiled configurations completed")
    { "lanes" => lanes, "lead" => lead, "figure" => figure, "failed_count" => failed.size, "compiled" => compiled.size, "completed" => compiled.count { |l| l["ok"] }, "all_failed" => !lanes.empty? && failed.size == lanes.count { |l| l["comparison"] != "Reference" },
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
    apps = checked.group_by { |r| r["app"] }.map { |app, rs| { "app" => app, "rows" => rs.sort_by { |r| r["name"].downcase }.reject { |r| r["result"] == "Not selected" } } }.reject { |x| x["rows"].empty? }
    failed = checked.select { |r| r["class"] == "fail" }.map { |r| "#{r["app"]} on #{r["name"]}" }
    marks = apps.flat_map { |x| x["rows"].map { |r| { "ok" => r["class"] == "ok", "failed" => r["class"] == "fail", "title" => "#{r["app"]} · #{r["name"]}: #{r["result"].downcase}", "group" => x["app"] } } }
    {
      "headline" => "#{passed} of #{selected} selected checks passed", "passed" => passed, "selected" => selected, "not_selected" => not_selected,
      "failed" => checked.select { |r| r["class"] == "fail" }.map { |r| "#{r["app"]}: #{r["name"]}" },
      "apps" => apps, "apps_label" => apps.map { |x| x["app"] }.join(" + "),
      "failed_text" => failed.empty? ? nil : "#{failed.size > 1 ? "#{failed[0..-2].join(", ")} and #{failed[-1]}" : failed[0]} failed",
      "figure" => marks.empty? ? nil : unit_marks(marks, "#{passed} of #{selected} selected checks passed#{"; #{failed.join(", ")} failed" unless failed.empty?}"),
      "others" => checked.select { |r| r["result"] == "Not selected" }.sort_by { |r| [r["app"], r["name"].downcase] },
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
    by_endpoint = endpoints.each_with_index.map { |e, i| { "endpoint" => e, "ratios" => pairs.map { |pr| pr["ratios"][i] } } }
    ruby = at.("ruby", "/articles")
    headline = RoundhouseStatus.ratio(rows, "ruby", "rails", "/articles")
    table = rows.select { |r| r["endpoint"] == "/articles" }.sort_by { |r| r["target"] }.map do |r|
      { "target" => r["target"], "req_sec" => r["req_sec"] && num(r["req_sec"].round), "p50" => r["p50_ms"]&.round(1), "p99" => r["p99_ms"]&.round(1),
        "rss" => r["rss_mb"]&.round, "per_gib" => r["rps_per_gb"] && num(r["rps_per_gb"].round), "runs" => r["runs"], "errors" => r["non_2xx"] }
    end
    env = d["environment"]
    { "headline" => headline && "#{headline}×", "figure" => headline && ratio_bars(headline.to_f), "runs" => ruby&.dig("runs"), "endpoints" => endpoints, "pairs" => pairs, "by_endpoint" => by_endpoint, "table" => table,
      "env_date" => env && date(env["captured_at"]), "env_sha" => env && short(env.dig("git", "sha")), "toolchains" => env&.dig("toolchains").to_h.map { |k, v| "#{k} #{v}" } }
  end

  def activity(feed)
    d = feed["data"]
    weekly = d["weekly_merged"]
    d.slice("people_30d", "recent").merge(
      "merged_7d" => num(d["merged_7d"]), "people_7d" => d["people_7d"], "merged_all" => num(d["merged_all"].to_i),
      "contributors_30d" => d["people_30d"].size, "bars" => weekly_bars(weekly), "spark" => weekly.empty? ? nil : weeks_spark(weekly),
      "weeks" => weekly.reverse.map { |w| { "week" => date(w["week"]), "merged" => num(w["merged"]), "partial" => w["partial"] } },
      "firsts" => d["first_contributions_30d"].map { |p| p.merge("date" => date(p.dig("first_pr", "merged_at"))) },
      "firsts_alpha" => d["first_contributions_30d"].sort_by { |p| p["login"].downcase },
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
    # The results table shows one date when its rows share it; a row that differs,
    # or isn't current, carries its own.
    rows = %w[specs replay archives bench].select { |k| states.dig(k, "available") }
    dates = rows.map { |k| states.dig(k, "observed_date") }.uniq
    common = dates.size == 1 ? dates.first : nil
    rows.each do |k|
      st = states[k]
      own = st["observed_short"] || "Date not published"
      st["when"] = if st["cached"] then "Saved result, #{own}"
                   elsif st["stale"] then "#{own}, not current"
                   elsif common.nil? then own
                   end
    end
    if (act = states["activity"])["available"]
      own = act["observed_short"] || "Date not published"
      act["when"] = if act["cached"] then "Saved result, #{own}" elsif act["stale"] then "#{own}, not current" end
    end
    view = { "built" => datetime(status["generated_at"]), "site_sha" => short(status["site_sha"]), "states" => states, "repo" => repo, "results_date" => common }
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
        "fetch" => { "ok" => feed["cached"] ? "Saved result" : "Collected", "reused" => "Collected #{datetime(feed["checked_at"])}; refreshed daily",
                     "missing" => "Not found", "error" => "Couldn't load", "offline" => "Not collected" }[feed["fetch"]] || "Not collected",
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
