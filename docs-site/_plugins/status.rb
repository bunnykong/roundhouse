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

require "digest"
require "json"
require "net/http"
require "time"
require "uri"

module RoundhouseStatus
  module_function

  SCHEMA = 1
  class NotFound < StandardError; end

  def offline? = ENV["ROUNDHOUSE_STATUS"] == "off"

  def fetch(url, headers = {})
    uri = URI(url)
    Net::HTTP.start(uri.host, uri.port, use_ssl: uri.scheme == "https", open_timeout: 10, read_timeout: 30) do |http|
      req = Net::HTTP::Get.new(uri)
      headers.each { |k, v| req[k] = v }
      res = http.request(req)
      raise NotFound, url if res.is_a?(Net::HTTPNotFound)
      raise "#{url}: HTTP #{res.code}" unless res.is_a?(Net::HTTPSuccess)
      res.body
    end
  end

  def json(url, headers = {}) = JSON.parse(fetch(url, headers))

  def token = ENV["GITHUB_TOKEN"] || ENV["GH_TOKEN"]

  def github_headers
    h = { "Accept" => "application/vnd.github+json", "User-Agent" => "roundhouse-docs-site" }
    h["Authorization"] = "Bearer #{token}" unless token.to_s.empty?
    h
  end

  def graphql(query, variables = {})
    raise "no GITHUB_TOKEN" if token.to_s.empty?
    uri = URI("https://api.github.com/graphql")
    body = Net::HTTP.start(uri.host, uri.port, use_ssl: true, open_timeout: 10, read_timeout: 30) do |http|
      req = Net::HTTP::Post.new(uri, github_headers.merge("Content-Type" => "application/json"))
      req.body = JSON.generate(query: query, variables: variables)
      res = http.request(req)
      raise "GraphQL: HTTP #{res.code}" unless res.is_a?(Net::HTTPSuccess)
      res.body
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
    s = json(url)
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
    a = json(url)
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
    rows = json("#{site}/bench/summary.json").map do |r|
      r.slice("target", "endpoint", "workers", "runs", "non_2xx")
       .merge(r.slice("req_sec", "p50_ms", "p99_ms", "rss_mb", "rps_per_gb").transform_values { |v| v.is_a?(Numeric) && v.finite? ? v : nil })
    end
    env = begin
      e = json("#{site}/bench/env.json")
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
    merged, cursor = [], nil
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
      break if !page.dig("pageInfo", "hasNextPage") || Time.parse(page["nodes"].last["updatedAt"]) < since
      cursor = page.dig("pageInfo", "endCursor")
    end
    merged = merged.reject { |pr| pr["author"].nil? || pr.dig("author", "__typename") == "Bot" }.sort_by { |pr| pr["mergedAt"] }.reverse
    people = merged.group_by { |pr| pr.dig("author", "login") }.map do |login, prs|
      { "login" => login, "url" => prs.first.dig("author", "url"), "avatar" => prs.first.dig("author", "avatarUrl"), "last_merged_at" => prs.first["mergedAt"] }
    end

    # Each person's first merged pull request: the earliest merge among their
    # earliest-opened ones. A first merge inside the window is a first contribution.
    firsts = people.each_with_index.map do |p, i|
      %(u#{i}: search(type: ISSUE, first: 5, query: "repo:#{repo} is:pr is:merged author:#{p["login"]} sort:created-asc") { nodes { ... on PullRequest { number title url mergedAt } } })
    end
    first_prs = firsts.empty? ? {} : graphql("{ #{firsts.join("\n")} }")
    people.each_with_index do |p, i|
      first = first_prs.dig("u#{i}", "nodes").to_a.min_by { |pr| pr["mergedAt"] }
      p["first_pr"] = first && first.slice("number", "title", "url").merge("merged_at" => first["mergedAt"])
    end
    c = counts_query.value
    weekly = windows.each_with_index.map { |from, i| { "week" => from.strftime("%F"), "merged" => c["w#{i}"]["issueCount"], "partial" => i == weeks - 1 } }
    weekly.shift while weekly.size > 1 && weekly.first["merged"].zero?
    week_ago = now - 7 * 86_400
    this_week = merged.select { |pr| Time.parse(pr["mergedAt"]) >= week_ago }
    {
      "observed_at" => now.iso8601, "sources" => ["https://github.com/#{repo}/pulls?q=is%3Apr+is%3Amerged"],
      "data" => {
        "weekly_merged" => weekly,
        "merged_7d" => this_week.size, "people_7d" => this_week.map { |pr| pr.dig("author", "login") }.uniq.size,
        "merged_30d" => merged.size, "merged_all" => c.dig("merged_all", "issueCount"),
        "people_30d" => people.sort_by { |p| p["login"].downcase },
        "first_contributions_30d" => people.select { |p| p["first_pr"] && Time.parse(p["first_pr"]["merged_at"]) >= since }
                                           .sort_by { |p| p["first_pr"]["merged_at"] }.reverse,
        "recent" => merged.first(8).map { |pr| { "number" => pr["number"], "title" => pr["title"], "url" => pr["url"], "author" => pr.dig("author", "login"), "merged_at" => pr["mergedAt"] } }
      }
    }
  end

# Generated Ruby against Rails on the same runtime, one worker; nil unless both
# rows are clean (no error responses).
def ratio(rows, generated, rails, endpoint)
  a, b = [generated, rails].map { |t| rows.find { |r| r["target"] == t && r["endpoint"] == endpoint && r["workers"] == 1 } }
  ok = ->(r) { r && r["req_sec"].to_f.positive? && r["non_2xx"].to_i.zero? }
  (a["req_sec"] / b["req_sec"]).round(1) if ok.(a) && ok.(b)
end

# The few numbers the home page shows, each with its own observation time.
def highlights(status)
  out = {}
  if (d = status.dig("specs", "data")) && d["canary_ok"] && d["consistent"]
    out["specs"] = { "observed_at" => status.dig("specs", "observed_at"), "passed" => d.dig("examples", "passed"), "total" => d.dig("examples", "total"),
                     "unclassified" => d["causes"].find { |c| c["slug"] == "unclassified" }&.dig("failures") }
  end
  if (d = status.dig("activity", "data"))
    out["activity"] = { "observed_at" => status.dig("activity", "observed_at"), "merged_7d" => d["merged_7d"], "people_7d" => d["people_7d"],
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
    local.merge(network).each do |key, source|
      feed = { "checked_at" => now.iso8601, "cached" => false }
      if offline? && network.key?(key)
        feed.merge!("fetch" => "offline", "observed_at" => nil, "data" => nil, "error" => "not collected in this build")
      else
        begin
          feed.merge!("fetch" => "ok").merge!(source.call)
        rescue StandardError => e
          feed.merge!("fetch" => e.is_a?(NotFound) ? "missing" : "error", "error" => e.message[0, 200], "observed_at" => nil, "data" => nil)
          if (old = previous&.dig(key)) && old["data"]
            feed.merge!("data" => old["data"], "observed_at" => old["observed_at"], "sources" => old["sources"], "cached" => true)
          end
        end
      end
      status[key] = feed
    end
    status["highlights"] = highlights(status)
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
        # A publishing build that can't read the deployed history would replace it with one
        # point. Fail instead: the deployed history stays, and the next build tries again.
        raise "status-history.json could not be carried forward: #{e.message[0, 160]}" if ENV["ROUNDHOUSE_STATUS"] == "on"
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
        site.pages << page
      end
      return if @logged
      @logged = true
      @status.each do |key, feed|
        next unless feed.is_a?(Hash) && feed["fetch"] && feed["fetch"] != "ok"
        Jekyll.logger.warn "Status:", "#{key}: #{feed["fetch"]}#{" (showing the saved result)" if feed["cached"]}: #{feed["error"]}"
      end
      Jekyll.logger.warn "Status:", "history #{@status["history_error"]}" if @status["history_error"]
      Jekyll.logger.info "Status:", "#{@status.count { |_, f| f.is_a?(Hash) && f["fetch"] == "ok" }} feeds collected; history has #{@history["points"].size} point(s)"
    end
  end
end
