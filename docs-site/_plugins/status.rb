# frozen_string_literal: true

# Project status for /progress/ and the home page's numbers, collected at build
# time from evidence the project already publishes. Writes /status.json (the
# current snapshot) and /status-history.json (one point per publishing build,
# carried forward from the deployed site, so trends accumulate).
#
# Each source is a feed that succeeds or fails on its own:
#
#   { "fetch" => "ok" | "missing" | "error" | "offline", "checked_at",
#     "observed_at", "cached", "sources", "data", "error" }
#
# observed_at is when the evidence was produced, never when it was fetched. A
# feed that fails keeps the deployed snapshot's data, marked cached, so a
# network blip shows a dated result instead of nothing. The measures stay apart
# on purpose: model specs, frozen routes, archive checks and benchmarks test
# different things, and none of them is a percentage of Rails.
#
# ROUNDHOUSE_STATUS=off skips the network (offline builds, pull requests);
# ROUNDHOUSE_STATUS_FROM renders a saved snapshot instead (see the generator).
# ROUNDHOUSE_STATUS_STAGED names the site a publishing build is about to deploy:
# the reports it already holds are read from there, so Progress agrees with the
# evidence it links to.

require "digest"
require "json"
require "net/http"
require "time"
require "timeout"
require "uri"

module RoundhouseStatus
  module_function

  SCHEMA = 1
  class NotFound < StandardError; end

  def offline? = ENV["ROUNDHOUSE_STATUS"] == "off"

  # Each request ends within this, however slowly a server answers.
  REQUEST_SECONDS = 45

  def fetch(url, headers = {})
    uri = URI(url)
    Timeout.timeout(REQUEST_SECONDS, Timeout::Error, "#{url}: no answer within #{REQUEST_SECONDS} s") do
      Net::HTTP.start(uri.host, uri.port, use_ssl: uri.scheme == "https", open_timeout: 10, read_timeout: 30) do |http|
        req = Net::HTTP::Get.new(uri)
        headers.each { |k, v| req[k] = v }
        res = http.request(req)
        raise NotFound, url if res.is_a?(Net::HTTPNotFound)
        raise "#{url}: HTTP #{res.code}" unless res.is_a?(Net::HTTPSuccess)
        res.body
      end
    end
  end

  def json(url, headers = {}) = JSON.parse(fetch(url, headers))

  # A report the site publishes: this publication's copy when one is staged,
  # otherwise the deployed one. A report missing from the staged site won't be
  # deployed either, so it counts as missing.
  def report(site, path)
    staged = ENV["ROUNDHOUSE_STATUS_STAGED"].to_s
    return json("#{site}/#{path}") if staged.empty?
    file = File.join(staged, path)
    raise NotFound, "#{path} isn't in this publication" unless File.file?(file)
    JSON.parse(File.read(file))
  end

  def token = ENV["GITHUB_TOKEN"] || ENV["GH_TOKEN"]

  def github_headers
    h = { "Accept" => "application/vnd.github+json", "User-Agent" => "roundhouse-docs-site" }
    h["Authorization"] = "Bearer #{token}" unless token.to_s.empty?
    h
  end

  def graphql(query, variables = {})
    raise "no GITHUB_TOKEN" if token.to_s.empty?
    uri = URI("https://api.github.com/graphql")
    body = Timeout.timeout(REQUEST_SECONDS, Timeout::Error, "GraphQL: no answer within #{REQUEST_SECONDS} s") do
      Net::HTTP.start(uri.host, uri.port, use_ssl: true, open_timeout: 10, read_timeout: 30) do |http|
        req = Net::HTTP::Post.new(uri, github_headers.merge("Content-Type" => "application/json"))
        req.body = JSON.generate(query: query, variables: variables)
        res = http.request(req)
        raise "GraphQL: HTTP #{res.code}" unless res.is_a?(Net::HTTPSuccess)
        res.body
      end
    end
    data = JSON.parse(body)
    raise "GraphQL: #{data["errors"].map { |e| e["message"] }.join("; ")[0, 160]}" if data["errors"]
    data.fetch("data")
  end

  # A short, safe excerpt of a failure message: no ANSI colour, control
  # characters or machine-local path prefixes.
  def excerpt(text, max = 200)
    line = text.to_s.gsub(/\e\[[\d;]*[A-Za-z]/, "").gsub(%r{/(?:home|Users)/[^/\s]+/}, "")
               .lines.map(&:strip).find { |l| !l.empty? }.to_s.gsub(/[[:cntrl:]]/, "")
    line.length > max ? "#{line[0, max - 1]}…" : line
  end

  def time_or_nil(value) = value && Time.parse(value.to_s).utc.iso8601
  def digest(*parts) = Digest::SHA256.hexdigest(JSON.generate(parts))[0, 16]

  # --- Lobsters' own model specs, run against the emitted Ruby -------------

  def specs(site)
    url = "#{site}/bench/lobsters-specs/summary.json"
    s = report(site, "bench/lobsters-specs/summary.json")
    ex = s.fetch("examples")
    causes = s.fetch("causes").map do |c|
      examples = c["examples"].to_a.map do |e|
        e.is_a?(Hash) ? { "name" => e["example"].to_s, "error" => (excerpt(e["error"]) if e["error"]) } : { "name" => e.to_s, "error" => nil }
      end
      c.slice("slug", "title", "owner", "note")
       .merge("failures" => c["failures"].to_i, "stale_rule" => c["stale_rule"] == true, "examples" => examples.first(3), "example_count" => examples.size)
    end
    canary = s["canary"].to_h
    by_file = s["by_file"].to_a.map { |f| f.slice("file", "examples", "passed", "failed") }
    compensations = s["harness_compensations"].to_a.map { |h| h.slice("what", "kind", "why") }
    problems = []
    counts = ex.values_at("total", "passed", "failed", "pending")
    problems << "the example counts are missing" unless counts.all? { |n| n.is_a?(Integer) && n >= 0 } && ex["total"].positive?
    problems << "a failure group has no count" unless s.fetch("causes").all? { |c| c["failures"].is_a?(Integer) && c["failures"] >= 0 }
    problems << "the example counts don't add up" unless ex.values_at("passed", "failed", "pending").sum(&:to_i) == ex["total"].to_i
    problems << "the failure groups don't add up to the failures" unless causes.sum { |c| c["failures"] } == ex["failed"].to_i
    problems << "a rule with no matches has failures" if causes.any? { |c| c["stale_rule"] && c["failures"].positive? }
    {
      "observed_at" => time_or_nil(s["generated_at"]), "sources" => [url, "#{site}/bench/lobsters-specs/"],
      "data" => {
        "lobsters_sha" => s.dig("lobsters", "sha"), "roundhouse_sha" => s.dig("roundhouse", "sha"), "ruby_pinned" => s.dig("ruby", "pinned"),
        "canary_ok" => canary.values_at("entered", "loaded", "valid_from_emit", "ok").all?(true),
        "scope" => s["scope"].to_h.slice("suites", "spec_files_run", "spec_files_in_suite", "excluded"),
        "examples" => ex.slice("total", "passed", "failed", "pending").transform_values(&:to_i)
                        .merge("errors_outside_examples" => s["errors_outside_examples"].to_a.size),
        "consistent" => problems.empty?, "problems" => problems,
        "causes" => causes, "by_file" => by_file, "harness_compensations" => compensations,
        "warnings" => { "total" => s.dig("emit", "warnings"), "by_kind" => s.dig("emit", "by_kind").to_h },
        # Same app revision, Ruby, spec selection and harness: results under one key compare.
        "scope_key" => digest(s.dig("lobsters", "sha"), s.dig("ruby", "pinned"), s["scope"], by_file.map { |f| [f["file"], f["examples"]] }.sort, compensations)
      }
    }
  end

  # --- The frozen Lobsters benchmark: latest replay and the route ledger -----

  def replay(bench)
    url = "#{bench}/matrix.json"
    m = json(url)
    lanes = m.fetch("rows").map do |r|
      lane = r.slice("id", "label", "kind", "runtime", "jit", "state", "stage", "vs_rails", "failed_visits", "partial")
      lane["detail"] = excerpt(r["detail"]) unless r["state"] == "ok" || r["detail"].to_s.empty?
      %w[content_total content_equivalent content_differs content_scope content_deferred].each { |k| lane[k] = r[k] unless r[k].nil? }
      %w[content_gaps content_undeclared].each { |k| lane[k] = r[k].map { |g| g.is_a?(Hash) ? g["route"] : g } if r[k].is_a?(Array) }
      lane["content_routes"] = r["content_routes"].map { |x| x.slice("route", "verdict") } if r["content_routes"].is_a?(Array)
      lane
    end
    { "observed_at" => time_or_nil(m["generated_at"]), "sources" => [url], "data" => { "workload" => m["workload"], "lanes" => lanes } }
  end

  def routes(root, repo)
    path = "bench/lobsters/declared-gaps.json"
    ledger = JSON.parse(File.read(File.join(root, path)))
    entries = ledger.fetch("declared_gaps").map { |g| g.slice("route", "since", "owner", "reason", "detail", "closes_with", "status", "issue_url") }
    committed = `git -C #{root} log -1 --format=%cI -- #{path} 2>/dev/null`.strip
    { "observed_at" => (time_or_nil(committed) unless committed.empty?), "sources" => ["https://github.com/#{repo}/blob/main/#{path}"],
      "data" => { "entries" => entries } }
  end

  # --- Smoke checks on the published archives --------------------------------

  def archives(site, repo)
    url = "#{site}/ci/archive-results.json"
    a = report(site, "ci/archive-results.json")
    rows = a.fetch("archives").select { |r| r["path"].to_s.end_with?(".tgz") }
            .map { |r| r.slice("path", "check", "status", "reason", "harness_outcome", "selected", "published", "validation_attempt") }
    run = begin
      j = json("https://api.github.com/repos/#{repo}/actions/runs/#{a["run_id"]}", github_headers)
      { "url" => j["html_url"], "created_at" => j["created_at"], "attempt" => j["run_attempt"], "conclusion" => j["conclusion"] }
    rescue StandardError
      nil
    end
    {
      "observed_at" => (time_or_nil(run["created_at"]) if run && run["attempt"] == a["attempt"]), "sources" => [url],
      "data" => { "run_id" => a["run_id"].to_s, "attempt" => a["attempt"], "source_sha" => a["source_sha"], "run" => run, "rows" => rows }
    }
  end

  # --- Reference blog benchmarks ---------------------------------------------

  ENV_HARNESS = %w[workers concurrency runs duration_sec warmup_sec threads wrk_threads endpoints targets].freeze

  def bench(site)
    rows = report(site, "bench/summary.json").map do |r|
      r.slice("target", "endpoint", "workers", "runs", "non_2xx")
       .merge(r.slice("req_sec", "p50_ms", "p99_ms", "rss_mb", "rps_per_gb").transform_values { |v| v.is_a?(Numeric) && v.finite? ? v : nil })
    end
    env = begin
      e = report(site, "bench/env.json")
      { "captured_at" => time_or_nil(e["captured_at"]), "git" => e["git"].to_h.slice("sha", "branch", "dirty"),
        "host" => e["host"].to_h.slice("arch", "platform"), "toolchains" => e["toolchains"], "harness" => e["harness"].to_h.slice(*ENV_HARNESS) }
    rescue NotFound
      nil
    end
    { "observed_at" => env&.dig("captured_at"), "sources" => ["#{site}/bench/summary.json", "#{site}/bench/env.json"],
      "data" => { "rows" => rows, "environment" => env } }
  end

  # --- Issues marked for newcomers -------------------------------------------

  def help(repo)
    fields = "nodes { ... on Issue { number title url updatedAt labels(first: 6) { nodes { name } } assignees(first: 3) { nodes { login url } } } }"
    labels = { "good_first_issue" => "good first issue", "help_wanted" => "help wanted" }
    q = labels.map do |key, label|
      %(#{key}: search(type: ISSUE, first: 3, query: "repo:#{repo} is:issue is:open label:\\"#{label}\\" sort:updated-desc") { issueCount #{fields} })
    end
    d = graphql("{ #{q.join("\n")} }")
    data = labels.keys.to_h do |key|
      issues = d.dig(key, "nodes").map do |i|
        { "number" => i["number"], "title" => i["title"], "url" => i["url"], "updated_at" => i["updatedAt"],
          "labels" => i.dig("labels", "nodes").map { |l| l["name"] }, "assignees" => i.dig("assignees", "nodes") }
      end
      [key, { "count" => d.dig(key, "issueCount"), "issues" => issues }]
    end
    { "observed_at" => Time.now.utc.iso8601, "sources" => ["https://github.com/#{repo}/issues"], "data" => data }
  end

  # --- Who is building: merged pull requests, never ranked -------------------

  PR_FIELDS = "number title url mergedAt updatedAt author { __typename login avatarUrl url }"

  def activity(repo, now, weeks:)
    owner, name = repo.split("/")
    monday = now - ((now.wday - 1) % 7) * 86_400
    start = Time.utc(monday.year, monday.month, monday.day)
    windows = (0...weeks).map { |i| start - (weeks - 1 - i) * 7 * 86_400 }
    counts = windows.each_with_index.map do |from, i|
      %(w#{i}: search(type: ISSUE, first: 0, query: "repo:#{repo} is:pr is:merged merged:#{from.strftime('%F')}..#{(from + 6 * 86_400).strftime('%F')}") { issueCount })
    end
    counts << %(merged_all: search(type: ISSUE, first: 0, query: "repo:#{repo} is:pr is:merged") { issueCount })
    counts_query = Thread.new { graphql("{ #{counts.join("\n")} }") }

    # Most recently updated first; mergedAt <= updatedAt, so once updatedAt
    # passes the window nothing older can have merged inside it.
    since = now - 30 * 86_400
    merged, cursor, complete = [], nil, false
    20.times do
      page = graphql(<<~Q, { "owner" => owner, "name" => name, "after" => cursor }).dig("repository", "pullRequests")
        query($owner: String!, $name: String!, $after: String) {
          repository(owner: $owner, name: $name) {
            pullRequests(states: MERGED, first: 100, after: $after, orderBy: { field: UPDATED_AT, direction: DESC }) {
              pageInfo { hasNextPage endCursor }
              nodes { #{PR_FIELDS} }
            }
          }
        }
      Q
      merged.concat(page["nodes"].select { |pr| Time.parse(pr["mergedAt"]) >= since })
      complete = !page.dig("pageInfo", "hasNextPage") || Time.parse(page["nodes"].last["updatedAt"]) < since
      break if complete
      cursor = page.dig("pageInfo", "endCursor")
    end
    raise "more than 2,000 pull requests changed in 30 days; the count would be incomplete" unless complete
    merged = merged.sort_by { |pr| pr["mergedAt"] }.reverse
    humans = merged.reject { |pr| pr["author"].nil? || pr.dig("author", "__typename") == "Bot" }
    people = humans.group_by { |pr| pr.dig("author", "login") }.map do |login, prs|
      { "login" => login, "url" => prs.first.dig("author", "url"), "avatar" => prs.first.dig("author", "avatarUrl"), "last_merged_at" => prs.first["mergedAt"] }
    end

    # A first contribution: nothing of theirs merged before the window, so their
    # earliest merge inside it (all collected above) is their first.
    earlier = people.each_with_index.map do |p, i|
      %(u#{i}: search(type: ISSUE, first: 0, query: "repo:#{repo} is:pr is:merged author:#{p["login"]} merged:<#{since.utc.iso8601}") { issueCount })
    end
    before = earlier.empty? ? {} : graphql("{ #{earlier.join("\n")} }")
    people.each_with_index do |p, i|
      count = before.dig("u#{i}", "issueCount")
      raise "couldn't tell whether #{p["login"]} contributed before" unless count.is_a?(Integer)
      next unless count.zero?
      first = humans.select { |pr| pr.dig("author", "login") == p["login"] }.min_by { |pr| pr["mergedAt"] }
      p["first_pr"] = first.slice("number", "title", "url").merge("merged_at" => first["mergedAt"])
    end
    c = counts_query.value
    weekly = windows.each_with_index.map { |from, i| { "week" => from.strftime("%F"), "merged" => c["w#{i}"]["issueCount"], "partial" => i == weeks - 1 } }
    weekly.shift while weekly.size > 1 && weekly.first["merged"].zero?
    week_ago = now - 7 * 86_400
    this_week = merged.select { |pr| Time.parse(pr["mergedAt"]) >= week_ago }
    humans_this_week = humans.select { |pr| Time.parse(pr["mergedAt"]) >= week_ago }
    {
      "observed_at" => now.iso8601, "sources" => ["https://github.com/#{repo}/pulls?q=is%3Apr+is%3Amerged"],
      "data" => {
        "weekly_merged" => weekly,
        "merged_7d" => this_week.size, "people_7d" => humans_this_week.map { |pr| pr.dig("author", "login") }.uniq.size,
        "bots_7d" => this_week.count { |pr| pr.dig("author", "__typename") == "Bot" },
        "merged_30d" => merged.size, "merged_all" => c.dig("merged_all", "issueCount"),
        "people_30d" => people.sort_by { |p| p["login"].downcase },
        "first_contributions_30d" => people.select { |p| p["first_pr"] && Time.parse(p["first_pr"]["merged_at"]) >= since }
                                           .sort_by { |p| p["first_pr"]["merged_at"] }.reverse,
        "recent" => humans.first(8).map { |pr| { "number" => pr["number"], "title" => pr["title"], "url" => pr["url"], "author" => pr.dig("author", "login"), "merged_at" => pr["mergedAt"] } }
      }
    }
  end

  # Generated Ruby against Rails on the same runtime, one worker; nil unless both
  # rows are clean (no error responses).
  def ratio(rows, generated, rails, endpoint)
    a, b = [generated, rails].map { |t| rows.find { |r| r["target"] == t && r["endpoint"] == endpoint && r["workers"] == 1 } }
    ok = ->(r) { r && r["req_sec"].is_a?(Numeric) && r["req_sec"].positive? && r["non_2xx"].is_a?(Numeric) && r["non_2xx"].zero? }
    a["req_sec"].fdiv(b["req_sec"]).round(1) if ok.(a) && ok.(b)
  end

  # The few numbers the home page shows, each with its own observation time.
  def highlights(status)
    out = {}
    if (d = status.dig("specs", "data")) && d["canary_ok"] && d["consistent"]
      out["specs"] = { "observed_at" => status.dig("specs", "observed_at"), "passed" => d.dig("examples", "passed"), "total" => d.dig("examples", "total"),
                       "unclassified" => d["causes"].find { |c| c["slug"] == "unclassified" }&.dig("failures") }
    end
    if (d = status.dig("activity", "data"))
      out["activity"] = { "observed_at" => status.dig("activity", "observed_at"), "merged_7d" => d["merged_7d"], "people_7d" => d["people_7d"], "bots_7d" => d["bots_7d"].to_i,
                          "first_contributions_30d" => d["first_contributions_30d"].size }
    end
    if (d = status.dig("bench", "data")) && d["environment"]
      endpoints = d["rows"].map { |r| r["endpoint"] }.uniq
      cruby = endpoints.filter_map { |e| ratio(d["rows"], "ruby", "rails", e) }
      out["bench"] = { "observed_at" => d.dig("environment", "captured_at"), "articles" => ratio(d["rows"], "ruby", "rails", "/articles"),
                       "min" => cruby.min, "max" => cruby.max, "jvm_articles" => ratio(d["rows"], "jruby", "rails-jruby", "/articles") }
    end
    out
  end

  def release(root)
    heading = File.read(File.join(root, "RELEASES.md"))[/^## (\S+)/, 1]
    { "observed_at" => nil, "sources" => ["RELEASES.md"], "data" => { "latest" => heading } }
  end

  # --- Collection -------------------------------------------------------------

  def previous_status(site)
    return nil if offline?
    s = json("#{site}/status.json")
    s if s["schema"] == SCHEMA
  rescue StandardError
    nil
  end

  # How long a publishing build may reuse a source the last one collected. Merged pull
  # requests and labelled issues take most of collection's time and change slowly, so
  # they're collected about once a day; the nightly results are collected every time.
  REFRESH_HOURS = { "activity" => 20, "help" => 20 }.freeze

  def reusable?(key, old, now)
    hours = REFRESH_HOURS[key]
    return false unless hours && old.is_a?(Hash) && old["data"] && !old["cached"] && %w[ok reused].include?(old["fetch"])
    (now - Time.parse(old["checked_at"].to_s)) < hours * 3600
  rescue ArgumentError
    false
  end

  # One source: collected, reused from the deployed snapshot, or, when collection
  # fails, the deployed copy marked as saved. Never raises.
  def feed_for(key, source, networked, old, now)
    feed = { "checked_at" => now.iso8601, "cached" => false }
    return feed.merge("fetch" => "offline", "observed_at" => nil, "data" => nil, "error" => "not collected in this build") if offline? && networked
    return old.merge("fetch" => "reused") if networked && reusable?(key, old, now)
    feed.merge("fetch" => "ok").merge(source.call)
  rescue StandardError => e
    failed(old, now, e)
  end

  def failed(old, now, error)
    feed = { "checked_at" => now.iso8601, "cached" => false, "fetch" => error.is_a?(NotFound) ? "missing" : "error",
             "error" => error.message[0, 200], "observed_at" => nil, "data" => nil }
    feed.merge!("data" => old["data"], "observed_at" => old["observed_at"], "sources" => old["sources"], "cached" => true) if old.is_a?(Hash) && old["data"]
    feed
  end

  # A publication waits this long for its sources, at most. One still running
  # then keeps its saved result, so a slow API never holds up a deploy.
  COLLECT_SECONDS = 120

  def collect(config, root)
    cfg = config.fetch("status")
    site, repo = cfg.fetch("site"), cfg.fetch("repo")
    now = Time.now.utc
    sha = ENV["GITHUB_SHA"] || `git -C #{root} rev-parse HEAD 2>/dev/null`.strip
    run = ENV["GITHUB_RUN_ID"]
    status = { "schema" => SCHEMA, "generated_at" => now.iso8601, "site_sha" => sha,
               "build_id" => run ? "#{run}.#{ENV.fetch("GITHUB_RUN_ATTEMPT", "1")}" : "local:#{sha[0, 12]}" }
    previous = previous_status(site)
    local = { "routes" => -> { routes(root, repo) }, "release" => -> { release(root) } }
    network = {
      "specs" => -> { specs(site) }, "replay" => -> { replay(cfg.fetch("lobsters_bench")) },
      "archives" => -> { archives(site, repo) }, "bench" => -> { bench(site) },
      "help" => -> { help(repo) }, "activity" => -> { activity(repo, now, weeks: cfg.fetch("weeks", 12)) }
    }
    # The sources are independent, so they're collected at once: the build waits for the slowest.
    threads = local.merge(network).to_h { |key, source| [key, Thread.new { feed_for(key, source, network.key?(key), previous&.dig(key), now) }] }
    deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + COLLECT_SECONDS
    threads.each do |key, thread|
      left = deadline - Process.clock_gettime(Process::CLOCK_MONOTONIC)
      status[key] = thread.join([left, 0].max)&.value || begin
        thread.kill
        failed(previous&.dig(key), now, RuntimeError.new("still running after #{COLLECT_SECONDS} s"))
      end
    end
    status["highlights"] = begin
      highlights(status)
    rescue StandardError => e # the home page then keeps its written text
      status["highlights_error"] = e.message[0, 160]
      {}
    end
    status
  end

  # --- History: one point per publishing build, for the gap over time --------

  HISTORY_DAYS = 90

  def history(config, status)
    site = config.dig("status", "site")
    previous, continuity = [], "new"
    unless offline?
      body = nil
      4.times do |attempt|
        body = fetch("#{site}/status-history.json")
        break
      rescue NotFound
        break
      rescue StandardError => e
        next sleep(2**attempt) if attempt < 3
        # Publication goes ahead: the new history starts here, and the page says it's incomplete.
        continuity = "interrupted"
        status["history_error"] = "not carried forward: #{e.message[0, 160]}"
      end
      if body
        begin
          h = JSON.parse(body)
          previous = h.fetch("points").select { |p| p.is_a?(Hash) && p["at"] }
          continuity = h["continuity"] == "interrupted" ? "interrupted" : "continuous"
        rescue StandardError => e
          # Unreadable history can't be recovered; start again, and say so.
          continuity = "interrupted"
          status["history_error"] = "the deployed history was unreadable: #{e.message[0, 160]}"
        end
      end
    end
    specs = status.dig("specs", "data")
    act = status.dig("activity", "data")
    point = {
      "build_id" => status["build_id"], "at" => status["generated_at"], "site_sha" => status["site_sha"],
      "specs" => specs && {
        "observed_at" => status.dig("specs", "observed_at"), "cached" => status.dig("specs", "cached"),
        # The harness showed it tested the emitted Ruby, and the totals agree.
        "verified" => specs["canary_ok"] == true && specs["consistent"] == true,
        "scope_key" => specs["scope_key"], "lobsters_sha" => specs["lobsters_sha"], "roundhouse_sha" => specs["roundhouse_sha"],
        "spec_files_run" => specs.dig("scope", "spec_files_run")
      }.merge(specs["examples"]).merge("causes" => specs["causes"].map { |c| c.slice("slug", "failures") }),
      "activity" => act&.slice("merged_7d", "people_7d")
    }
    cutoff = (Time.parse(status["generated_at"]) - HISTORY_DAYS * 86_400).iso8601
    points = previous.reject { |p| p["build_id"] == point["build_id"] || p["at"].to_s < cutoff } + [point]
    { "schema" => SCHEMA, "generated_at" => status["generated_at"], "continuity" => continuity, "points" => points.sort_by { |p| p["at"] } }
  end
end

module RoundhouseStatus
  class Generator < Jekyll::Generator
    safe true
    priority :high # before the Progress page reads site.data.status

    def generate(site)
      return unless site.config["status"]
      root = File.expand_path(site.config.fetch("repo_docs").fetch("root"), site.source)
      # ROUNDHOUSE_STATUS_FROM=path/to/status.json renders a saved snapshot (and the
      # status-history.json beside it, if any) without collecting: fast previews, and
      # a way to try the page with empty, stale or failed data.
      if (from = ENV["ROUNDHOUSE_STATUS_FROM"])
        @status ||= JSON.parse(File.read(from))
        saved = File.join(File.dirname(from), "status-history.json")
        @history ||= File.exist?(saved) ? JSON.parse(File.read(saved)) : { "schema" => 1, "continuity" => "new", "points" => [] }
      end
      @status ||= RoundhouseStatus.collect(site.config, root)
      @history ||= RoundhouseStatus.history(site.config, @status)
      site.data["status"] = @status
      site.data["status_history"] = @history
      { "status.json" => @status, "status-history.json" => @history }.each do |name, data|
        page = Jekyll::PageWithoutAFile.new(site, site.source, "/", name)
        page.content = JSON.pretty_generate(data) + "\n"
        page.data["layout"] = nil
        page.data["render_with_liquid"] = false # titles and excerpts from outside may hold {{ }} or {% %}
        site.pages << page
      end
      return if @logged
      @logged = true
      @status.each do |key, feed|
        next unless feed.is_a?(Hash) && feed["fetch"] && feed["fetch"] != "ok"
        Jekyll.logger.warn "Status:", "#{key}: #{feed["fetch"]}#{" (showing the saved result)" if feed["cached"]}: #{feed["error"]}"
      end
      Jekyll.logger.warn "Status:", "history #{@status["history_error"]}" if @status["history_error"]
      Jekyll.logger.warn "Status:", "home page numbers withheld: #{@status["highlights_error"]}" if @status["highlights_error"]
      Jekyll.logger.info "Status:", "#{@status.count { |_, f| f.is_a?(Hash) && f["fetch"] == "ok" }} feeds collected; history has #{@history["points"].size} point(s)"
    end
  end
end
