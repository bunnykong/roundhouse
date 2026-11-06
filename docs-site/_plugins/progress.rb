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
  STALE_HOURS = { "specs" => 36, "replay" => 36, "archives" => 12, "bench" => 24 * 7, "activity" => 24, "help" => 24, "ci" => 48 }.freeze

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
  # Text from a report, made safe for an id: "Symbol into Integer" -> "symbol-into-integer".
  def id_part(text) = text.to_s.downcase.gsub(/[^a-z0-9]+/, "-").gsub(/\A-|-\z/, "")
  # A commit link, only for a commit hash.
  def commit_url(base, sha) = sha.to_s.match?(/\A\h{7,64}\z/) ? "#{base}/commit/#{sha}" : nil

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

  # max: never drawn wider than its own width, so text keeps its size.
  def svg(width, height, title, desc, body, cls: nil, max: false)
    id = "c#{"#{title}#{width}".hash.abs.to_s(36)}"
    %(<svg class="chart#{" #{cls}" if cls}" viewBox="0 0 #{width} #{height}"#{%( style="max-width: #{width}px") if max} role="group" aria-labelledby="#{id}-t #{id}-d"><title id="#{id}-t">#{h(title)}</title>) +
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
    taken = Hash.new(0)
    anchor = lambda do |slug| # two slugs can reduce to one id; the first keeps it
      base = id_part(slug)
      taken[base] += 1
      "cause-#{base}#{"-#{taken[base]}" if taken[base] > 1}"
    end
    group = lambda do |c|
      { "slug" => c["slug"], "share" => failing.positive? ? (c["failures"] * 100.0 / failing).round(1) : 0, "anchor" => anchor.(c["slug"]), "title" => c["title"], "failures" => c["failures"],
        "area" => c["slug"] == "unclassified" ? "Needs triage" : AREAS.fetch(c["owner"].to_s, "Needs triage"),
        "step" => c["stale_rule"] ? "Recheck its original reproducer before retiring the rule." : STEPS.fetch(c["slug"], "Reproduce a named example."),
        "action" => c["stale_rule"] ? "Recheck the reproducer" : ACTIONS.fetch(c["slug"], "Reproduce a named example"),
        "note" => c["note"], "examples" => c["examples"], "more" => [c["example_count"].to_i - c["examples"].size, 0].max,
        "related_url" => "https://github.com/#{repo}/issues?q=#{CGI.escape(c["slug"].to_s)}" }
    end
    active = d["causes"].reject { |c| c["stale_rule"] || c["failures"].zero? }
                        .sort_by { |c| [c["slug"] == "unclassified" ? 0 : 1, -c["failures"], c["slug"]] }
    ex = d["examples"]
    d.slice("scope", "canary_ok", "consistent", "problems", "harness_compensations", "warnings").merge(
      "failed" => num(ex["failed"]), "passed" => num(ex["passed"]), "total" => num(ex["total"]), "pending" => ex["pending"],
      "outside" => ex["errors_outside_examples"], "headline" => d["canary_ok"] && d["consistent"],
      "pass_pct" => ex["total"].to_i.positive? ? (ex["passed"] * 100.0 / ex["total"]).round(1) : 0,
      "ruby" => d["ruby_pinned"], "lobsters_sha" => short(d["lobsters_sha"]), "lobsters_url" => commit_url("https://github.com/lobsters/lobsters", d["lobsters_sha"]),
      "roundhouse_sha" => short(d["roundhouse_sha"]), "roundhouse_url" => commit_url("https://github.com/#{repo}", d["roundhouse_sha"]),
      "report_url" => feed["sources"].last, "groups" => active.map(&group), "group_count" => active.count { |c| c["slug"] != "unclassified" },
      "smaller_count" => [active.size - 3, 0].max, "smaller_examples" => plural(active.drop(3).sum { |c| c["failures"] }, "example"),
      # Two parts only: with pending examples, passed + failed isn't the whole run.
      "figure" => d["canary_ok"] && d["consistent"] && ex["pending"].to_i.zero? ? gap_meter(ex["passed"].to_i, ex["total"].to_i) : nil,
      "unclassified" => d["causes"].find { |c| c["slug"] == "unclassified" }&.dig("failures").to_i,
      "stale_rules" => d["causes"].select { |c| c["stale_rule"] }.sort_by { |c| c["slug"] }.map(&group),
      "warning_kinds" => d.dig("warnings", "by_kind").sort_by { |_, n| -n }.map { |k, n| { "kind" => k.tr("_", " "), "count" => num(n) } },
      "warning_total" => plural(d.dig("warnings", "total").to_i, "warning")
    )
  end

  # --- Trends: measures with a direction, drawn as mini series and small multiples ---

  # A tick value without trailing noise: 1,000 / 2.5 / 0.4.
  def tick(v) = v >= 100 ? num(v.round) : (v == v.round ? v.round.to_s : v.round(2).to_s.sub(/0+\z/, ""))

  def scale(pts, ref)
    top = [pts.map { |p| p["hi"] || p["y"] }.max.to_f, ref.to_f].max
    nice_max(top <= 4 ? top : top.ceil).then { |max, n| top <= 4 && top.positive? ? [[(top * 1.25).ceil(1), 0.5].max, 4] : [max, n] }
  end

  # A 12-point sparkline: the neutral line, its latest point in the accent, the
  # reference (Rails = 1×) as a hairline, a hollow point for an unfinished period.
  def spark(series, ref: nil, w: 132, hgt: 32)
    series = series.select { |x| x["primary"] } # a tile draws its own measure; the detail chart carries references
    pts = series.flat_map { |x| x["points"] }.map { |p| p.merge("hi" => nil) }
    return nil if pts.map { |p| p["at"] }.uniq.size < 2
    max, = scale(pts, ref)
    times = pts.map { |p| Time.parse(p["at"]).to_f }
    t0, t1 = times.minmax
    x_of = ->(at) { t1 == t0 ? w / 2.0 : 4 + (w - 8) * (Time.parse(at).to_f - t0) / (t1 - t0) }
    y_of = ->(v) { (hgt - 4) - (hgt - 8) * v / max.to_f }
    body = ref ? %(<line class="ref" x1="0" x2="#{w}" y1="#{y_of.(ref).round(1)}" y2="#{y_of.(ref).round(1)}"/>) : +""
    series.each do |x|
      ps = x["points"].sort_by { |p| p["at"] }
      ps.slice_when { |_, b| b["new_segment"] }.each do |seg|
        body << %(<polyline class="line #{x["cls"]}" points="#{seg.map { |p| "#{x_of.(p["at"]).round(1)},#{y_of.(p["y"]).round(1)}" }.join(" ")}"/>) if seg.size > 1
      end
      last = ps.last
      body << %(<circle class="end#{" open" if last["partial"]}" cx="#{x_of.(last["at"]).round(1)}" cy="#{y_of.(last["y"]).round(1)}" r="3.5"/>) if x["primary"]
    end
    %(<svg class="spark" viewBox="0 0 #{w} #{hgt}" width="#{w}" height="#{hgt}" aria-hidden="true" focusable="false">#{body}</svg>)
  end

  # A detail chart: one axis from zero (or a 1× reference for ratios), a faint
  # band for spread, lines broken where runs stop being comparable, end labels,
  # and one focusable column per date whose readout lists every series there.
  def trend_chart(series, title:, desc:, fmt:, ref: nil, ref_label: nil, w: 480, cls: "wide", hgt: 180)
    pts = series.flat_map { |x| x["points"] }
    longest = series.map { |x| x["points"].empty? ? 0 : "#{x["short"]} #{fmt.(x["points"].max_by { |p| p["at"] }["y"])} so far".size }.max.to_i
    left, right, top, bottom = 40, [48, (longest * 7.2 + 14).ceil].max, 14, 26
    max, ticks = scale(pts, ref)
    y_of = ->(v) { top + (hgt - top - bottom) * (1 - v / max.to_f) }
    stamps = pts.map { |p| p["at"] }.uniq.sort
    t0, t1 = stamps.map { |a| Time.parse(a).to_f }.minmax
    x_of = ->(at) { t1 == t0 ? (left + (w - left - right) / 2.0) : left + (w - left - right) * (Time.parse(at).to_f - t0) / (t1 - t0) }
    body = +""
    body << (0..ticks).map do |i|
      v = max * i / ticks.to_f
      %(<line class="grid" x1="#{left}" x2="#{w - right}" y1="#{y_of.(v).round(1)}" y2="#{y_of.(v).round(1)}"/><text class="tick" x="#{left - 8}" y="#{(y_of.(v) + 4).round(1)}" text-anchor="end">#{tick(v)}</text>)
    end.join
    if ref
      y = y_of.(ref).round(1)
      body << %(<line class="ref" x1="#{left}" x2="#{w - right}" y1="#{y}" y2="#{y}"/><text class="ref-label" x="#{w - right + 8}" y="#{(y + 4).round(1)}">#{h(ref_label)}</text>)
    end
    labels = []
    series.each do |x|
      ps = x["points"].sort_by { |p| p["at"] }
      next if ps.empty?
      ps.slice_when { |_, b| b["new_segment"] }.each do |seg|
        if seg.size > 1 && seg.all? { |p| p["hi"] }
          edge = seg.map { |p| "#{x_of.(p["at"]).round(1)},#{y_of.(p["hi"]).round(1)}" } + seg.reverse.map { |p| "#{x_of.(p["at"]).round(1)},#{y_of.(p["lo"] || p["y"]).round(1)}" }
          body << %(<polygon class="band #{x["cls"]}" points="#{edge.join(" ")}"/>)
        end
        body << %(<polyline class="line #{x["cls"]}" points="#{seg.map { |p| "#{x_of.(p["at"]).round(1)},#{y_of.(p["y"]).round(1)}" }.join(" ")}"/>) if seg.size > 1
      end
      ps.each { |p| body << %(<circle class="dot #{x["cls"]}#{" open" if p["partial"]}" cx="#{x_of.(p["at"]).round(1)}" cy="#{y_of.(p["y"]).round(1)}" r="4"/>) }
      at = y_of.(ps.last["y"])
      labels << [at + 4, "#{x["short"]} #{fmt.(ps.last["y"])}#{" so far" if ps.last["partial"] && ps.last["when"].to_s.include?("so far")}".strip, x_of.(ps.last["at"]), at]
    end
    labels.sort_by!(&:first).each_cons(2) { |a, b| b[0] = a[0] + 14 if b[0] - a[0] < 14 }
    body << labels.map do |y, text, px, py|
      leader = (y - 4 - py).abs > 2 ? %(<polyline class="leader" points="#{(px + 6).round(1)},#{py.round(1)} #{w - right + 2},#{(y - 4).round(1)} #{w - right + 6},#{(y - 4).round(1)}"/>) : ""
      leader + %(<text class="value" x="#{w - right + 8}" y="#{y.round(1)}">#{h(text)}</text>)
    end.join
    # One column per date: the crosshair, and the readout for every series there.
    step = stamps.size > 1 ? (w - left - right) / (stamps.size - 1).to_f : (w - left - right).to_f
    stamps.each do |at|
      x = x_of.(at).round(1)
      tip = series.filter_map { |s| (p = s["points"].find { |q| q["at"] == at }) && "#{s["name"]}: #{p["tip"] || fmt.(p["y"])}" }.join(" · ")
      head = series.flat_map { |s| s["points"] }.find { |q| q["at"] == at }&.dig("when") || date(at)
      label = "#{head}. #{tip}"
      body << %(<g class="mark col" tabindex="0" role="img" data-tip="#{h(label)}" aria-label="#{h(label)}"><rect class="hit" x="#{(x - step / 2).round(1)}" y="#{top}" width="#{[step, 24].max.round(1)}" height="#{hgt - top - bottom}"/><line class="cross" x1="#{x}" x2="#{x}" y1="#{top}" y2="#{hgt - bottom}"/></g>)
    end
    xt = ->(at, anchor) { %(<text class="tick" x="#{x_of.(at).round(1)}" y="#{hgt - bottom + 17}" text-anchor="#{anchor}">#{h(Time.parse(at).utc.strftime("%-d %b"))}</text>) }
    if stamps.size == 1
      body << xt.(stamps[0], "middle")
    else
      n = [[(w - left - right) / 110, 1].max, stamps.size - 1].min
      picks = (0..n).map { |k| stamps[(k * (stamps.size - 1) / n.to_f).round] }.uniq
      body << picks.each_with_index.map { |at, k| xt.(at, k.zero? ? "start" : (k == picks.size - 1 ? "end" : "middle")) }.join
    end
    if stamps.size > 1 && series.any? { |s| s["points"].find { |q| q["at"] == stamps.last }&.dig("when").to_s.include?("so far") }
      x0 = ((x_of.(stamps[-2]) + x_of.(stamps[-1])) / 2).round(1)
      body = %(<rect class="so-far" x="#{x0}" y="#{top}" width="#{(w - right - x0 + 4).round(1)}" height="#{hgt - top - bottom}"/><text class="so-far-label" x="#{(w - right).round(1)}" y="#{top - 3}" text-anchor="end">So far</text>) + body
    end
    svg(w, hgt, title, desc, %(<line class="axis" x1="#{left}" x2="#{w - right}" y1="#{y_of.(0)}" y2="#{y_of.(0)}"/>) + body, cls: cls, max: true)
  end

  def both(series, w: 480, **opts) = trend_chart(series, **opts, w: w) + trend_chart(series, **opts, w: 330, cls: "narrow")

  # The delta a tile states: signed, against a named point, and judged only when
  # the measure has a better direction and the change clears its noise.
  def delta(now, was, better:, since:, fmt:, noise: 0)
    return nil unless now && was
    change = now - was
    return { "text" => "No change since #{since}", "verdict" => "same" } if change.abs <= noise
    good = better && (better == "higher" ? change.positive? : change.negative?)
    { "text" => "#{change.positive? ? "▲" : "▼"} #{fmt.(change.abs)} since #{since}", "verdict" => better ? (good ? "better" : "worse") : "neutral",
      "mark" => better ? (good ? "✓ better" : "✕ worse") : nil }
  end

  def short_date(iso) = iso && day(iso).strftime("%-d %b")
  def ratio(v) = v >= 1 ? "#{v.round(1)}×" : "#{v.round(2)}×"

  CUES = { "lower" => "Lower is better", "higher" => "Higher is better" }.freeze

  # One sentence per family, from the numbers it heads; never a guess.
  def findings(trends, status)
    out = {}
    if (weeks = status.dig("ci", "data", "weeks"))
      done = weeks.reject { |x| x["partial"] }.select { |x| x["median_min"] }
      if done.size >= 8
        recent, before = done.last(4).sum { |x| x["median_min"] } / 4.0, done[-8..-5].sum { |x| x["median_min"] } / 4.0
        change = (recent - before) / before
        out["build"] = if change > 0.15 then "Green runs on main are taking longer."
                       elsif change < -0.15 then "Green runs on main are getting faster."
                       else "Time to a green run on main is steady."
                       end
      end
    end
    rt = trends["runtime"].to_a
    if rt.any?
      t = rt.find { |m| m["id"] == "throughput" }
      out["runtime"] = if rt.all? { |m| m["count"] == 1 } then "A baseline to build on."
                       elsif t && t["delta"] && t["delta"]["verdict"] == "better" then "Throughput is up on the last comparable run."
                       elsif t && t["delta"] && t["delta"]["verdict"] == "worse" then "Throughput is down on the last comparable run."
                       elsif t && t["delta"] then "No measurable change since the last comparable run."
                       else "The latest run isn't comparable with the one before."
                       end
    end
    sp = trends["specs"]
    out["apps"] = sp && sp["value"] ? "#{sp["value"]} model specs pass." : "Model-spec results are missing upstream."
    out
  end

  def measure(id, label:, definition:, better:, series:, value:, sub: nil, delta: nil, fmt:, ref: nil, ref_label: nil, rows: [], columns: [], note: nil, first: nil, legend: nil, facts: nil, conditions: nil, w: 480, pair: nil, headline_value: nil, href: nil)
    pts = series.flat_map { |x| x["points"] }
    stamps = pts.map { |p| p["at"] }.uniq
    {
      "id" => id, "label" => label, "definition" => definition, "better" => better, "cue" => CUES[better], "value" => value, "sub" => sub,
      "value_num" => value&.split(" ", 2)&.first, "value_unit" => value&.split(" ", 2)&.dig(1),
      "delta" => delta || nil, "note" => note, "count" => stamps.size, "first" => first, "legend" => legend, "facts" => facts, "conditions" => conditions,
      "pair" => pair, "headline_value" => headline_value, "href" => href || "#trend-#{id}",
      "spark" => spark(series, ref: ref), "series" => series.map { |x| x.slice("name", "cls") },
      "chart" => stamps.size > 1 ? both(series, title: label, desc: definition, fmt: fmt, ref: ref, ref_label: ref_label, w: w) : nil,
      "rows" => rows, "columns" => columns
    }
  end

  # Time to a green run on main: the weekly median, with the 90th percentile as
  # the band's upper edge. The latest finished week is the one a tile states.
  def ci_measure(feed)
    weeks = feed.dig("data", "weeks").to_a
    timed = weeks.select { |x| x["median_min"] }
    return nil if timed.empty?
    min = ->(v) { "#{v.round} min" }
    pts = timed.map do |x|
      thin = x["timed"].to_i < 10
      { "at" => "#{x["week"]}T00:00:00Z", "y" => x["median_min"], "lo" => x["median_min"], "hi" => x["p90_min"], "partial" => x["partial"] || thin,
        "when" => "Week of #{date(x["week"])}#{", so far" if x["partial"]}",
        "tip" => "#{min.(x["median_min"])}; 90th percentile #{min.(x["p90_min"])}; #{plural(x["timed"], "green run")}#{" (few runs)" if thin}" }
    end
    done = timed.reject { |x| x["partial"] }
    now, was = done.last, timed.first
    recent = weeks.last(4)
    runs = recent.sum { |x| x["completed"] }
    measure("ci", label: "Time to a green run on main", better: "lower", fmt: min, w: 720,
            definition: "Minutes from a green run's start to its last update, for runs after a merge to main. Queueing before the start isn't included.",
            series: [{ "name" => "Median", "short" => "median", "cls" => "s-n", "primary" => true, "points" => pts }],
            value: now && min.(now["median_min"]), sub: now && "Week of #{short_date(now["week"])} · p90 #{min.(now["p90_min"])}",
            delta: now && now != was && delta(now["median_min"], was["median_min"], better: "lower", since: short_date(was["week"]), fmt: min, noise: 1),
            legend: "Line: weekly median. Band: up to the 90th percentile. Hollow: an unfinished week, or under 10 green runs.",
            conditions: "ci.yml on pushes to main · green runs only · start to finish",
            note: nil,
            facts: (cur = weeks.last)["partial"] && cur["median_min"] ? [["This week, so far", "#{min.(cur["median_min"])} · #{plural(cur["timed"], "green run")}"]] : nil,
            columns: ["Week of", "Runs", "Green", "Failed", "Cancelled", "Median, min", "90th percentile, min"],
            rows: weeks.reverse.map { |x| [date(x["week"]) + (x["partial"] ? ", so far" : ""), x["completed"], x["green"], x["failed"], x["superseded"], x["median_min"]&.round, x["p90_min"]&.round] })
  end

  # Merged pull requests per week: activity, with no better direction.
  def merges_measure(feed)
    weekly = feed&.dig("data", "weekly_merged").to_a
    return nil if weekly.size < 2
    pts = weekly.map { |x| { "at" => "#{x["week"]}T00:00:00Z", "y" => x["merged"], "partial" => x["partial"], "when" => "Week of #{date(x["week"])}#{", so far" if x["partial"]}", "tip" => plural(x["merged"], "merge") } }
    done = weekly.reject { |x| x["partial"] }
    now = done.last
    measure("merges", label: "Merged pull requests", better: nil, fmt: ->(v) { num(v.round) },
            definition: "Pull requests merged each week. Merges measure activity, not progress.",
            series: [{ "name" => "Merged", "short" => "", "cls" => "s-n", "primary" => true, "points" => pts }],
            value: now && num(now["merged"]), sub: now && "Week of #{short_date(now["week"])}", legend: "Hollow: this week, so far.", href: "#activity",
            columns: ["Week of", "Merged"], rows: weekly.reverse.map { |x| [date(x["week"]) + (x["partial"] ? ", so far" : ""), x["merged"]] })
  end

  # The blog at /articles, one worker: Roundhouse's generated Ruby on CRuby + YJIT,
  # with Rails measured in the same run as its reference. Each benchmark run in
  # the history is a point; a line breaks where host, toolchains or harness changed.
  RUNTIME = [
    ["throughput", 0, "higher", "Throughput", "Requests per second on the blog's /articles.", ->(v) { "#{num(v.round)} requests/s" }, ->(v) { num(v.round) }, "requests/s"],
    ["memory", 1, "lower", "Resident memory", "Memory per worker on the same runs, in MB.", ->(v) { "#{num(v.round)} MB" }, ->(v) { "#{num(v.round)} MB" }, "MB"],
    ["latency", 2, "lower", "Tail latency", "The slowest 1% of responses on the same runs: p99, in ms.", ->(v) { "#{v.round(1)} ms" }, ->(v) { "#{v.round(1)} ms" }, "ms"]
  ].freeze

  # What a benchmark run was: runtime versions, platform and harness.
  def bench_conditions(feed)
    env = feed&.dig("data", "environment") or return nil
    t, hn = env["toolchains"].to_h, env["harness"].to_h
    ruby = t["ruby"].to_s[/\Aruby (\S+)/, 1]
    spinel = t["spinel"].to_s[/\Aspinel (\d{4}\.\d+\.\d+)/, 1]
    host = env["host"].to_h.values_at("platform", "arch").compact.join(" ")
    parts = ["Blog /articles, one worker", ("Ruby #{ruby} + YJIT" if ruby), ("Spinel #{spinel}" if spinel), (host unless host.empty?),
             (hn["concurrency"] && "#{hn["concurrency"]} connections"), (hn["runs"] && "median of #{hn["runs"]} runs of #{hn["duration_sec"]} s")].compact
    parts.join(" · ")
  end

  def runtime_measures(history, feed)
    blocks = history["points"].filter_map { |p| p["bench"] }.uniq { |b| b["observed_at"] }.sort_by { |b| b["observed_at"] }
    return [] if blocks.empty?
    conditions = bench_conditions(feed)
    RUNTIME.map do |id, i, better, label, definition, long, short_fmt, unit|
      pick = lambda do |target|
        prev = nil
        blocks.filter_map do |b|
          row = b.dig("rows", "#{target} /articles") or next
          v = row[i]
          next unless v.is_a?(Numeric)
          lo, hi = (i.zero? ? [row[3], row[4]] : [nil, nil])
          pt = { "at" => b["observed_at"], "y" => v, "lo" => lo, "hi" => hi, "new_segment" => !prev.nil? && prev != b["key"], "key" => b["key"], "sha" => b["sha"],
                 "when" => datetime(b["observed_at"]), "tip" => "#{long.(v)}#{" (repeats #{num(lo.round)}–#{num(hi.round)})" if lo && hi}#{" at #{short(b["sha"])}" if b["sha"]}" }
          prev = b["key"]
          pt
        end
      end
      mine, rails, native = pick.("ruby"), pick.("rails"), pick.("spinel")
      now, base = mine.last, rails.last
      prev = mine[-2]
      comparable = prev && prev["key"] == now["key"]
      # A change counts only beyond the spread of the two runs' repeats (throughput), or 2% otherwise.
      noise = if i.zero? && now["lo"] && prev&.dig("lo") then ((now["hi"] - now["lo"]) + (prev["hi"] - prev["lo"])) / 2.0 else 0.02 * (prev&.dig("y") || 0) end
      pct = ->(d) { "#{(100.0 * d / prev["y"]).abs.round(1)}%" }
      saved = base && (100 * (1 - now["y"].fdiv(base["y"]))).round
      against = base && if better == "higher" then "#{ratio(now["y"].fdiv(base["y"]))} the throughput in this run"
                        elsif saved >= 0 then "#{saved}% #{id == "memory" ? "less memory" : "lower"} in this run"
                        else "#{-saved}% #{id == "memory" ? "more memory" : "higher"} in this run"
                        end
      measure(id, label: label, definition: definition, better: better, fmt: short_fmt, conditions: conditions, w: 420,
              series: [{ "name" => "Roundhouse, generated Ruby", "short" => "Roundhouse", "cls" => "s-a", "primary" => true, "points" => mine },
                       { "name" => "Rails", "short" => "Rails", "cls" => "s-ref", "points" => rails }],
              value: long.(now["y"]), sub: base && "Rails #{short_fmt.(base["y"])} · #{against}",
              delta: comparable ? delta(now["y"], prev["y"], better: better, since: "the run of #{short_date(prev["at"])}", fmt: pct, noise: noise) : nil,
              facts: native.last ? [["Spinel, native", "#{long.(native.last["y"])}#{" · #{ratio(native.last["y"].fdiv(base["y"]))} Rails" if base && i.zero?}"]] : nil,
              first: short_date(blocks.first["observed_at"]),
              pair: base && [["Roundhouse, generated Ruby", long.(now["y"]), now["y"], "s-a"], ["Rails", long.(base["y"]), base["y"], "s-ref"]].then { |rows| top = rows.map { |r| r[2] }.max.to_f; rows.map { |n, t, v, c| { "name" => n, "text" => t, "pct" => (100.0 * v / top).round(1), "cls" => c } } },
              headline_value: against,
              columns: ["Run", "Roundhouse (#{unit})", "Spinel (#{unit})", "Rails (#{unit})"],
              rows: blocks.reverse.map { |b| r = b["rows"]; [datetime(b["observed_at"]), r.dig("ruby /articles", i)&.round(1), r.dig("spinel /articles", i)&.round(1), r.dig("rails /articles", i)&.round(1)] })
    end
  end

  # CI outcomes on main by week: green, failed and cancelled (mostly superseded by a
  # newer merge) as counts, never as a score.
  def outcomes_measure(feed)
    weeks = feed.dig("data", "weeks").to_a
    return nil if weeks.empty?
    max, ticks = nice_max(weeks.map { |x| x["completed"] }.max.to_i)
    w, hgt, left, top, bottom, right = 330, 180, 36, 14, 26, 8
    y_of = ->(v) { top + (hgt - top - bottom) * (1 - v / max.to_f) }
    slot = (w - left - right) / weeks.size.to_f
    bw = [slot - 4, 18].min
    body = (0..ticks).map { |k| v = max * k / ticks.to_f; %(<line class="grid" x1="#{left}" x2="#{w - right}" y1="#{y_of.(v).round(1)}" y2="#{y_of.(v).round(1)}"/><text class="tick" x="#{left - 6}" y="#{(y_of.(v) + 4).round(1)}" text-anchor="end">#{num(v.round)}</text>) }.join
    weeks.each_with_index do |x, k|
      cx = left + slot * k + (slot - bw) / 2
      base = 0
      [["green", "f-ok"], ["failed", "f-bad"], ["superseded", "f-cancel"], ["other", "f-other"]].each do |key, cls|
        n = x[key].to_i
        next if n.zero?
        y1, y0 = y_of.(base + n), y_of.(base)
        body << %(<rect class="#{cls}#{" open" if x["partial"]}" x="#{cx.round(1)}" y="#{(y1 + 1).round(1)}" width="#{bw.round(1)}" height="#{[(y0 - y1 - 1), 0.5].max.round(1)}"/>)
        base += n
      end
      tip = "Week of #{date(x["week"])}#{", so far" if x["partial"]}: #{x["completed"]} runs, #{x["green"]} green, #{x["failed"]} failed, #{x["superseded"]} cancelled#{", #{x["other"]} other" if x["other"].to_i.positive?}"
      body << %(<g class="mark col" tabindex="0" role="img" data-tip="#{h(tip)}" aria-label="#{h(tip)}"><rect class="hit" x="#{(left + slot * k).round(1)}" y="#{top}" width="#{slot.round(1)}" height="#{hgt - top - bottom}"/></g>)
    end
    body << %(<text class="tick" x="#{left}" y="#{hgt - bottom + 17}">#{h(short_date(weeks.first["week"]))}</text><text class="tick" x="#{w - right}" y="#{hgt - bottom + 17}" text-anchor="end">#{h(short_date(weeks.last["week"]))}</text>)
    chart = svg(w, hgt, "Runs on main by week, by outcome", "Green, failed and cancelled runs each week.", %(<line class="axis" x1="#{left}" x2="#{w - right}" y1="#{y_of.(0)}" y2="#{y_of.(0)}"/>) + body, max: true)
    done = weeks.reject { |x| x["partial"] }.last || weeks.last
    share = ->(n) { done["completed"].positive? ? "#{(100.0 * n / done["completed"]).round}%" : "–" }
    {
      "id" => "outcomes", "label" => "Runs on main, by outcome", "cue" => "Context only", "chart" => chart,
      "definition" => "Each merge starts a run; a newer merge cancels one still running. Counts, not a score.",
      "facts" => [["Week of #{short_date(done["week"])}", "#{num(done["completed"])} runs"], ["✓ Green", "#{done["green"]} · #{share.(done["green"])}"],
                  ["✕ Failed", "#{done["failed"]} · #{share.(done["failed"])}"], ["Cancelled", "#{done["superseded"]} · #{share.(done["superseded"])}"]],
      "legend_keys" => [["f-ok", "✓ Green"], ["f-bad", "✕ Failed"], ["f-cancel", "Cancelled"]],
      "columns" => ["Week of", "Runs", "Green", "Failed", "Cancelled", "Other"],
      "rows" => weeks.reverse.map { |x| [date(x["week"]) + (x["partial"] ? ", so far" : ""), x["completed"], x["green"], x["failed"], x["superseded"], x["other"]] }
    }
  end

  # Campfire's Spinel archive, recorded when its size changes.
  def size_measure(history)
    blocks = history["points"].filter_map { |p| p["sizes"] }.select { |b| b.dig("bytes", "campfire/spinel.tgz") }
    return nil if blocks.empty?
    mb = ->(v) { "#{v.round(1)} MB" }
    pts = blocks.map { |b| v = b.dig("bytes", "campfire/spinel.tgz") / 1e6; { "at" => b["observed_at"], "y" => v.round(2), "when" => datetime(b["observed_at"]), "tip" => "#{mb.(v)}#{" at #{short(b["sha"])}" if b["sha"]}" } }
    measure("size", label: "Campfire's Spinel archive", better: "lower", fmt: mb,
            definition: "The published source archive Spinel compiles Campfire from, in MB, recorded when it changes.",
            series: [{ "name" => "Archive", "short" => "", "cls" => "s-a", "primary" => true, "points" => pts }],
            value: mb.(pts.last["y"]), first: short_date(pts.first["at"]),
            delta: pts.size > 1 && delta(pts.last["y"], pts.first["y"], better: "lower", since: short_date(pts.first["at"]), fmt: mb, noise: 0.05),
            columns: ["Recorded", "MB"], rows: pts.reverse.map { |p| [p["when"], p["y"]] })
  end

  # Lobsters model specs passing: compared only at the same app revision and scope.
  def specs_measure(over_time, gap)
    pts = over_time["points"].to_a
    series = [{ "name" => "Passing", "short" => "", "cls" => "s-a", "primary" => true,
                "points" => pts.map { |p| { "at" => p["observed_at"], "y" => p["passed"].to_i, "new_segment" => p["new_segment"], "when" => datetime(p["observed_at"]), "tip" => "#{num(p["passed"].to_i)} of #{num(p["total"].to_i)} passing" } } }]
    last, prev = pts[-1], pts[-2]
    comparable = last && prev && !last["new_segment"]
    measure("specs", label: "Lobsters model specs passing", better: "higher", fmt: ->(v) { num(v.round) }, series: series,
            definition: "Examples passing in Lobsters' own model specs on the emitted Ruby. Lobsters isn't pinned, so runs compare only at the same app revision and scope.",
            value: gap && gap["headline"] ? "#{gap["passed"]} of #{gap["total"]}" : nil, sub: gap && gap["headline"] ? "Nightly" : nil,
            delta: comparable ? delta(last["passed"].to_i, prev["passed"].to_i, better: "higher", since: date(prev["observed_at"]), fmt: ->(v) { num(v.round) }) : nil,
            first: pts.first && short_date(pts.first["observed_at"]))
  end

  def over_time(history)
    seen = {}
    points = history["points"].filter_map { |p| p["specs"]&.dup }.select { |s| s["verified"] == true && !s["cached"] && s["observed_at"] }
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
      "chart" => points.empty? ? nil : gap_chart(points) + gap_chart(points, w: 330, cls: "narrow"), "change" => change, "points" => points,
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
    seen = Hash.new(0)
    entries = routes_feed&.dig("data", "entries").to_a.sort_by { |e| e["route"].to_s }.map do |e|
      slug = id_part(e["route"])
      slug = "root" if slug.empty?
      seen[slug] += 1
      slug = "#{slug}-#{seen[slug]}" if seen[slug] > 1 # two routes can reduce to one slug
      e.merge("anchor" => "route-#{slug}", "area" => AREAS.fetch(e["owner"].to_s, e["owner"]))
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
      "others_text" => plural(not_selected, "more check") + (not_selected == 1 ? " wasn’t" : " weren’t") + " selected for this run",
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
    { "headline" => headline && "#{headline}×", "figure" => headline && ratio_bars(headline.to_f), "runs" => ruby&.dig("runs"), "runs_text" => ruby&.dig("runs") == 1 ? "one run" : "median of #{ruby&.dig("runs")} runs", "endpoints" => endpoints, "pairs" => pairs, "by_endpoint" => by_endpoint, "table" => table,
      "env_date" => env && date(env["captured_at"]), "env_sha" => env && short(env.dig("git", "sha")), "toolchains" => env&.dig("toolchains").to_h.map { |k, v| "#{k} #{v}" } }
  end

  def activity(feed)
    d = feed["data"]
    weekly = d["weekly_merged"]
    d.slice("people_30d", "recent").merge(
      "merged_7d" => num(d["merged_7d"]), "people_7d" => d["people_7d"], "merged_all" => num(d["merged_all"].to_i),
      "merged_7d_text" => plural(d["merged_7d"], "pull request"), "people_7d_text" => plural(d["people_7d"], "person", "people") + (d["bots_7d"].to_i.positive? ? " and #{plural(d["bots_7d"], "bot")}" : ""),
      "firsts_text" => plural(d["first_contributions_30d"].size, "first-timer"),
      "firsts_people_text" => plural(d["first_contributions_30d"].size, "person", "people"),
      "contributors_30d" => d["people_30d"].size, "bars" => weekly_bars(weekly), "spark" => weekly.empty? ? nil : weeks_spark(weekly), "weeks_text" => plural(weekly.size, "week"),
      "weeks" => weekly.reverse.map { |w| { "week" => date(w["week"]), "merged" => num(w["merged"]), "partial" => w["partial"] } },
      "firsts" => d["first_contributions_30d"].map { |p| p.merge("date" => date(p.dig("first_pr", "merged_at"))) },
      "firsts_alpha" => d["first_contributions_30d"].sort_by { |p| p["login"].downcase },
      "recent" => d["recent"].map { |r| r.merge("date" => date(r["merged_at"])) }
    )
  end

  SOURCE_NAMES = { "specs" => "Lobsters model specs", "replay" => "Frozen Lobsters replay", "routes" => "Frozen route ledger",
                   "archives" => "Archive smoke checks", "bench" => "Reference blog benchmarks", "activity" => "Merged pull requests",
                   "help" => "Labelled issues", "ci" => "CI runs on main" }.freeze

  UNREADABLE = { "available" => false, "note" => "This result couldn’t be read." }.freeze

  # A section whose data has an unexpected shape is left out, and its rows say so:
  # one odd report never stops the docs and the rest of Progress from publishing.
  def guarded(states, *keys)
    yield
  rescue StandardError => e
    Jekyll.logger.warn "Progress:", "#{keys.join(", ")} left out: #{e.class}: #{e.message[0, 160]}"
    keys.each { |k| states[k] = UNREADABLE.dup if states.key?(k) }
    nil
  end

  def build(site)
    status = site.data["status"] or return nil
    now = Time.parse(status["generated_at"])
    repo = site.config.dig("status", "repo")
    states = SOURCE_NAMES.keys.to_h { |k| [k, guarded({}, k) { state(k, status[k], now) } || UNREADABLE.dup] }
    view = { "built" => datetime(status["generated_at"]), "site_sha" => short(status["site_sha"]), "states" => states, "repo" => repo }
    view["gap"] = guarded(states, "specs") { gap(status["specs"], repo) } if states.dig("specs", "available")
    view["over_time"] = guarded(states, "history") { over_time(site.data["status_history"]) } || { "count" => 0 }
    view["replay"] = guarded(states, "replay", "routes") { replay(status["replay"], status["routes"]) }
    view["targets"] = guarded(states, "archives") { targets(status["archives"]) } if states.dig("archives", "available")
    view["bench"] = guarded(states, "bench") { bench(status["bench"]) } if states.dig("bench", "available")
    view["activity"] = guarded(states, "activity") { activity(status["activity"]) } if states.dig("activity", "available")
    view["help"] = status.dig("help", "data")
    history = site.data["status_history"] || { "points" => [] }
    trends = guarded(states, "trends") do
      runtime = runtime_measures(history, status["bench"])
      ci = (ci_measure(status["ci"]) if states.dig("ci", "available"))
      {
        "ci" => ci, "outcomes" => (outcomes_measure(status["ci"]) if states.dig("ci", "available")), "runtime" => runtime,
        "specs" => specs_measure(view["over_time"], view["gap"]), "archive" => size_measure(history), "merges" => merges_measure(status["activity"]),
        "conditions" => bench_conditions(status["bench"])
      }
    end || {}
    trends["findings"] = guarded(states, "trends") { findings(trends, status) } || {}
    view["trends"] = trends
    # The summary strip: build time first, then the runtime trio, then the work merged.
    view["tiles"] = ([trends["ci"]] + trends["runtime"].to_a + [trends["merges"]]).compact
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
    view["results_date"] = common
    view["sources"] = guarded(states, "sources") do
      SOURCE_NAMES.map do |key, name|
        feed = status[key].to_h
        { "name" => name, "url" => feed["sources"]&.first, "observed" => datetime(feed["observed_at"]) || "Not published",
          "fetch" => { "ok" => feed["cached"] ? "Saved result" : "Collected", "reused" => "Collected #{datetime(feed["checked_at"])}; refreshed daily",
                       "missing" => "Not found", "error" => "Couldn't load", "offline" => "Not collected" }[feed["fetch"]] || "Not collected",
          "stale" => states.dig(key, "stale") }
      end
    end || []
    view
  end

  class Generator < Jekyll::Generator
    safe true
    priority :normal # after RoundhouseStatus::Generator

    # Values in progress/index.html are escaped, except the figures progress.rb
    # draws; a new one without `escape` or `web_url` fails the build.
    RAW = /\A[\w.]+\.(\w+_)?(figure|spark|chart)\z|\Asite\.repo_docs\.github\z|\Acontent\z/

    def generate(site)
      check_escaping(site.pages.find { |page| page.url == "/progress/" })
      (Dir[File.join(site.source, "_includes", "trend-*.html")] + [File.join(site.source, "_layouts", "progress.html")]).each do |f|
        check_escaping(Struct.new(:content).new(File.read(f)), f.delete_prefix("#{site.source}/")) if File.file?(f)
      end
      site.data["progress"] = RoundhouseProgress.build(site)
    end

    def check_escaping(page, name = "progress/index.html")
      return unless page
      raw = page.content.scan(/\{\{-?(.*?)-?\}\}/m).flatten.map(&:strip)
                .reject { |expr| expr.match?(/\|\s*(escape|web_url|relative_url|url_encode)\b/) || expr.match?(RAW) }
      return if raw.empty?
      message = "#{name}: escape these values, or add them to RAW if progress.rb builds them: #{raw.uniq.join(", ")}"
      raise Jekyll::Errors::FatalException, message unless ENV["ROUNDHOUSE_DOCS_LENIENT"] == "1"
      Jekyll.logger.warn "Progress:", message
    end
  end

  # Liquid: a link taken from data, escaped for an attribute. Only web and
  # site-relative URLs pass; anything else (javascript:, data:) becomes "#".
  module Filters
    def web_url(input)
      url = input.to_s.strip
      url.match?(%r{\A(?:https?://[^\s]+|/(?![/\\])\S*)\z}i) ? CGI.escapeHTML(url) : "#"
    end
  end
end

Liquid::Template.register_filter(RoundhouseProgress::Filters)
