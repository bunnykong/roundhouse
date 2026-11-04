// Home page behaviour.
(function () {
  // The primary buttons' shimmer pauses while they are off-screen.
  if ("IntersectionObserver" in window) {
    var watch = new IntersectionObserver(function (entries) {
      entries.forEach(function (entry) { entry.target.classList.toggle("is-offscreen", !entry.isIntersecting); });
    });
    document.querySelectorAll(".button.primary").forEach(function (button) { watch.observe(button); });
  }

  // One-click copy for the install commands. The buttons stay hidden without the Clipboard API.
  if (navigator.clipboard) document.querySelectorAll(".copy-btn[data-copy]").forEach(function (button) {
    var label = button.querySelector("span");
    var icon = button.querySelector("use");
    var timer;
    button.hidden = false;
    function show(state, text, iconId) {
      button.classList.remove("is-copied", "is-failed");
      if (state) button.classList.add(state);
      label.textContent = text;
      icon.setAttribute("href", iconId);
    }
    button.addEventListener("click", function () {
      clearTimeout(timer);
      navigator.clipboard.writeText(button.dataset.copy).then(function () {
        show("is-copied", "Copied", "#icon-check");
      }, function () {
        // The browser refused the write: select the command so a keyboard copy works.
        var range = document.createRange();
        range.selectNodeContents(button.closest(".cmd").querySelector(".cmd-text"));
        var selection = window.getSelection();
        selection.removeAllRanges();
        selection.addRange(range);
        show("is-failed", "Copy failed; the command is selected", "#icon-copy");
      }).then(function () {
        timer = setTimeout(function () { show(null, "Copy", "#icon-copy"); }, 1600);
      });
    });
  });

  // Live numbers from /status.json (built with the site; see /progress/). Without
  // it, or when a result is too old to show as current, the text links stay as written.
  if (!window.fetch) return;
  var HOURS = 3600 * 1000;
  function fresh(part, maxHours) {
    return part && part.observed_at && Date.now() - Date.parse(part.observed_at) < maxHours * HOURS;
  }
  function n(value) { return Number(value).toLocaleString("en-US"); }
  function day(iso) { return new Date(iso).toLocaleDateString("en-GB", { day: "numeric", month: "short", timeZone: "UTC" }); }
  function fill(name, figure, text, note) {
    var link = document.querySelector('[data-morsel="' + name + '"]');
    if (!link) return;
    var b = link.querySelector("b"), small = link.querySelector("small");
    b.textContent = figure;
    b.hidden = false;
    link.querySelector("span").textContent = text;
    if (note) { small.textContent = note; small.hidden = false; }
  }
  fetch("status.json", { cache: "no-cache" }).then(function (response) {
    return response.ok ? response.json() : null;
  }).then(function (status) {
    var h = status && status.schema === 1 && status.highlights;
    if (!h) return;
    var specs = h.specs, act = h.activity, bench = h.bench;
    if (fresh(specs, 36)) {
      fill("specs", n(specs.passed) + " of " + n(specs.total), "Lobsters’ own model specs pass on the compiled app", "Nightly · " + day(specs.observed_at));
      if (specs.unclassified > 0) fill("triage", n(specs.unclassified), "failing specs waiting for a first look. Start here.", "Nightly · " + day(specs.observed_at));
    }
    if (fresh(act, 24)) {
      fill("merged", n(act.merged_7d), "pull requests merged in the last 7 days, by " + n(act.people_7d) + " people", "GitHub · " + day(act.observed_at));
      if (act.first_contributions_30d > 0) {
        fill("firsts", n(act.first_contributions_30d), "people made their first contribution in the last 30 days", "GitHub · " + day(act.observed_at));
        var welcome = document.querySelector('[data-morsel="welcome"]');
        if (welcome) {
          welcome.textContent = n(act.first_contributions_30d) + " people had their first pull request merged in the last 30 days. You could be next.";
          welcome.hidden = false;
        }
      }
    }
    // Benchmarks run less often; a week-old run is still the latest.
    if (fresh(bench, 24 * 14) && bench.articles) {
      var set = function (key, text) { document.querySelectorAll('[data-bench="' + key + '"]').forEach(function (el) { el.textContent = text; }); };
      set("articles", bench.articles.toFixed(1) + "×");
      if (bench.min && bench.max) set("range", bench.min.toFixed(1) + "× to " + bench.max.toFixed(1) + "×");
      if (bench.jvm_articles) set("jvm_articles", bench.jvm_articles.toFixed(1) + "×");
      set("date", bench.observed_at.slice(0, 10));
    }
  }).catch(function () { /* keep the written text */ });
})();
