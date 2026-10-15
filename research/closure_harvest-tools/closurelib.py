"""Shared validation for research condition registered protocol receipts. Absent is never zero; malformed is unavailable."""
import json
import re

CONSUMERS = ("receiver", "block", "argument", "local")
COUNTS = ("walks", "walks_changed_vs_legacy", "walks_changed_vs_entrances", "slots_changed_new_write",
          "slots_changed_bound_only", "slots_changed_unexplained", "slots_changed_vs_entrances_unexplained",
          "sites_entrance", "sites_applied")
UNEXPLAINED = ("slots_changed_unexplained", "slots_changed_vs_entrances_unexplained")
SCAN_CONSUMER = {"A1": "receiver", "A2": "block", "A3": "argument", "A4": "local"}


FIXTURE_DIGESTS = {"block_values", "constants", "ir", "params", "returns"}


def digest_problems(digests):
    if not isinstance(digests, dict) or set(digests) != FIXTURE_DIGESTS:
        return ["fixture digests absent or keys malformed"]
    if not all(isinstance(v, str) and re.fullmatch(r"[0-9a-f]{16}", v) for v in digests.values()):
        return ["fixture digest value malformed"]
    return []


def census_problems(census, mode, sites=None):
    """Return a list of schema problems for an `closure_harvest` census object; empty means well formed."""
    if not isinstance(census, dict):
        return ["census absent"]
    out = []
    if census.get("mode") != mode:
        out.append(f"census mode {census.get('mode')!r} != {mode!r}")
    for k in COUNTS:
        v = census.get(k)
        if not (isinstance(v, int) and not isinstance(v, bool) and v >= 0):
            out.append(f"census {k} missing or not a non-negative integer")
    by = census.get("sites_applied_by_consumer")
    if not isinstance(by, dict) or sorted(by) != sorted(CONSUMERS) or not all(
            isinstance(by[c], int) and not isinstance(by[c], bool) and by[c] >= 0 for c in CONSUMERS):
        out.append("census sites_applied_by_consumer malformed")
    elif isinstance(census.get("sites_applied"), int) and sum(by.values()) != census["sites_applied"]:
        out.append("census sites_applied_by_consumer does not sum to sites_applied")
    if not out:
        for k in ("walks_changed_vs_legacy", "walks_changed_vs_entrances"):
            if census[k] > census["walks"]:
                out.append(f"census {k} exceeds walks")
        changed = sum(census[k] for k in ("slots_changed_new_write", "slots_changed_bound_only",
                                         "slots_changed_unexplained"))
        walks = census["walks_changed_vs_legacy"]
        if bool(walks) != bool(changed) or walks > changed:
            out.append("census legacy changed walks contradict changed slot categories")
        if census["slots_changed_vs_entrances_unexplained"] and not census["walks_changed_vs_entrances"]:
            out.append("census unexplained entrance changes without changed walks")
        if census["walks"] == 0 and (census["sites_entrance"] or census["sites_applied"]):
            out.append("census sites without any harvest walk")
        if sites is not None:
            distinct = set(tuple(s) for s in sites)
            if census["sites_entrance"] != sum(s[2] == "E" for s in distinct):
                out.append("census entrance total contradicts printed sites")
            if census["sites_applied"] != sum(s[2] == "I" for s in distinct):
                out.append("census applied total contradicts printed sites")
            if by != {c: sum(s[2] == "I" and s[3] == c for s in distinct) for c in CONSUMERS}:
                out.append("census consumer histogram contradicts printed sites")
    if not out and mode == "entrances" and (census["walks_changed_vs_entrances"] or census["sites_applied"]
                                            or census["slots_changed_vs_entrances_unexplained"]):
        out.append("entrances-mode census reports applied-closure activity")
    return out


def parse_site(line, root=None):
    """Parse one `rh-closure_harvest-site: {...}` line into (file, line, tag, consumer); raise ValueError if malformed.

    `file` should be relative to the app root; an absolute path under `root` is made relative."""
    x = json.loads(line[len("rh-closure_harvest-site: "):])
    if root and isinstance(x.get("file"), str) and x["file"].startswith(root.rstrip("/") + "/"):
        x["file"] = x["file"][len(root.rstrip("/")) + 1:]
    if isinstance(x.get("file"), str) and x["file"].startswith("./"):
        x["file"] = x["file"][2:]
    tag, consumer = x.get("tag"), x.get("consumer")
    if tag not in ("E", "I") or (tag == "E" and consumer is not None) or (tag == "I" and consumer not in CONSUMERS):
        raise ValueError("bad tag/consumer")
    if not isinstance(x.get("file"), str) or not isinstance(x.get("line"), int) or x["line"] < 1 \
            or not isinstance(x.get("slot"), str):
        raise ValueError("bad file/line/slot")
    return (x["file"], x["line"], tag, consumer)


def scan_sites(scan, name):
    """Predicted (file, line, tag, consumer) set for one scanned app name."""
    out = set()
    for s in scan["sites"]:
        if s["path"].startswith(name + "/"):
            consumer = None if s["tag"] == "E" else SCAN_CONSUMER[s["consumer"].split(" ")[0]]
            out.add((s["path"].split("/", 1)[1], s["line"], s["tag"], consumer))
    return out
