// Docs page behavior: mobile navigation, "On this page" highlighting, copy buttons.
(function () {
  var body = document.body;

  // Mobile navigation drawer.
  var btn = document.querySelector(".menu-btn");
  var scrim = document.querySelector(".scrim");
  function setNav(open) {
    body.classList.toggle("nav-open", open);
    if (btn) btn.setAttribute("aria-expanded", String(open));
    if (scrim) scrim.hidden = !open;
  }
  if (btn) btn.addEventListener("click", function () { setNav(!body.classList.contains("nav-open")); });
  if (scrim) scrim.addEventListener("click", function () { setNav(false); });
  document.addEventListener("keydown", function (e) { if (e.key === "Escape") setNav(false); });

  // Sidebar stability: the layout restores its scroll before paint; save it on the way out.
  // Only when the current page's link is off-screen (first visit, deep link) scroll the sidebar
  // itself, never the page.
  var side = document.getElementById("docs-sidebar");
  var current = side && side.querySelector('a[aria-current="page"]');
  if (current) {
    var r = current.getBoundingClientRect(), s = side.getBoundingClientRect();
    if (r.top < s.top || r.bottom > s.bottom) side.scrollTop += r.top - s.top - (s.height - r.height) / 2;
  }
  if (side) window.addEventListener("pagehide", function () {
    try { sessionStorage.setItem("rh-docs-sidebar", String(side.scrollTop)); } catch (e) {}
  });

  // Highlight the section being read.
  var links = Array.prototype.slice.call(document.querySelectorAll(".docs-toc a"));
  if (links.length && "IntersectionObserver" in window) {
    var byId = {};
    links.forEach(function (a) { byId[decodeURIComponent(a.hash.slice(1))] = a; });
    var headings = Object.keys(byId).map(function (id) { return document.getElementById(id); }).filter(Boolean);
    var visible = new Set();
    var observer = new IntersectionObserver(function (entries) {
      entries.forEach(function (e) { e.isIntersecting ? visible.add(e.target) : visible.delete(e.target); });
      var top = headings.find(function (h) { return visible.has(h); });
      if (!top) return;
      links.forEach(function (a) { a.classList.remove("is-active"); });
      byId[top.id].classList.add("is-active");
    }, { rootMargin: "-64px 0px -70% 0px" });
    headings.forEach(function (h) { observer.observe(h); });
  }

  // Copy buttons on code blocks.
  if (navigator.clipboard) {
    document.querySelectorAll(".prose div.highlighter-rouge").forEach(function (block) {
      var b = document.createElement("button");
      b.type = "button";
      b.className = "copy-btn";
      b.textContent = "Copy";
      b.addEventListener("click", function () {
        var code = block.querySelector("code");
        function say(text) {
          b.textContent = text;
          setTimeout(function () { b.textContent = "Copy"; }, 1400);
        }
        navigator.clipboard.writeText(code ? code.innerText : "").then(function () { say("Copied"); }, function () { say("Couldn’t copy"); });
      });
      block.appendChild(b);
    });
  }
})();
