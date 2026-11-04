// Progress page behaviour: chart readouts on hover and focus, and opening the
// disclosure a link points at. Everything works without it.
(function () {
  // One readout per chart, placed by the mark under the pointer or focus.
  document.querySelectorAll(".chart-frame").forEach(function (frame) {
    var tip = document.createElement("div");
    tip.className = "tip";
    tip.hidden = true;
    tip.setAttribute("aria-hidden", "true"); // each mark already carries its text as aria-label
    frame.appendChild(tip);
    function show(mark) {
      tip.textContent = mark.getAttribute("data-tip");
      tip.hidden = false;
      var f = frame.getBoundingClientRect(), m = mark.getBoundingClientRect();
      var x = m.left + m.width / 2 - f.left - tip.offsetWidth / 2;
      x = Math.max(8, Math.min(x, f.width - tip.offsetWidth - 8));
      var y = m.top - f.top - tip.offsetHeight - 8;
      tip.style.left = x + "px";
      tip.style.top = (y < 4 ? m.bottom - f.top + 8 : y) + "px";
    }
    function hide() { tip.hidden = true; }
    frame.querySelectorAll(".mark[data-tip]").forEach(function (mark) {
      mark.addEventListener("pointerenter", function () { show(mark); });
      mark.addEventListener("pointerleave", hide);
      mark.addEventListener("focus", function () { show(mark); });
      mark.addEventListener("blur", hide);
    });
    // Arrow keys move between a chart's marks.
    frame.addEventListener("keydown", function (e) {
      var marks = Array.prototype.slice.call(frame.querySelectorAll(".mark[data-tip]"));
      var i = marks.indexOf(document.activeElement);
      if (e.key === "Escape") return hide();
      if (i < 0) return;
      var next = { ArrowRight: i + 1, ArrowDown: i + 1, ArrowLeft: i - 1, ArrowUp: i - 1, Home: 0, End: marks.length - 1 }[e.key];
      if (next === undefined) return;
      e.preventDefault();
      marks[Math.max(0, Math.min(next, marks.length - 1))].focus();
    });
  });

  // A link to #cause-… or #route-… opens that row.
  function openTarget() {
    var id = decodeURIComponent(location.hash.slice(1));
    var el = id && document.getElementById(id);
    if (!el) return;
    var details = el.tagName === "DETAILS" ? el : el.closest("details");
    if (details) details.open = true;
  }
  window.addEventListener("hashchange", openTarget);
  openTarget();
})();
