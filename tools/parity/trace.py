"""Behaviour parity: the same scripted steps in both UIs, their /api/ calls compared.

A recording reverse proxy sits on a second port in front of Portal. It forwards the
browser's Host unchanged and adds no X-Forwarded-*/Forwarded header, and it connects from
loopback, so Portal still sees "this PC" (crates/http/src/pairing.rs `this_pc`). For each
/api/ call it records method, path, normalised query, normalised body and status.

The scenario runs in headless Firefox through WebDriver: open every route (from
web/src/main.ts), then click controls by role and accessible name. In the new UI those are
the ARIA mirror elements remedy-ui-bridge-web lays over the canvas ([data-remedy-a11y]
[data-node], role and aria-label or text); a DOM click on one becomes an AccessKit Click.
Every step also records the visible controls (role + name), so missing or renamed
controls show up too.

  python tools/parity/trace.py                              # / vs /next/
  python tools/parity/trace.py --new /                      # self-test: / vs /
  python tools/parity/trace.py proxy --port 8491            # just the recording proxy
  python tools/parity/trace.py compare old.jsonl new.jsonl  # compare two saved traces

Exit code 1 when the call sets or control lists differ. Call counts are timing-dependent
(the header chip polls every 4 s) and are printed but only fail with --strict-counts.
Clicks whose name matches DENY are refused, so the scenario stays read-only.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import time
from collections import Counter
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import wd  # noqa: E402
from proxy import Recorder, make_proxy  # noqa: E402

# Controls the runner never clicks: they change the library, add-ons, playback or the server.
DENY = re.compile(r"\b(remove|delete|clear|forget|unpair|rename|restart|stop|install|update|updates|uninstall|"
                  r"resume|play|cast|add to|save|mark|reset|upload|sign in|sign out|pair|check)\b", re.I)

CONTROLS = r"""
const ROLES = new Set(['button','link','tab','checkbox','switch','menuitem','menuitemcheckbox','menuitemradio','option','textbox','searchbox','combobox','slider','radio','spinbutton']);
function roleOf(el) {
  const r = el.getAttribute('role'); if (r) return r.split(' ')[0];
  const t = el.tagName;
  if (t === 'A' && el.hasAttribute('href')) return 'link';
  if (t === 'BUTTON' || t === 'SUMMARY') return 'button';
  if (t === 'SELECT') return 'combobox';
  if (t === 'TEXTAREA') return 'textbox';
  if (t === 'INPUT') return ({button:'button',submit:'button',reset:'button',checkbox:'checkbox',radio:'radio',range:'slider',search:'searchbox',number:'spinbutton'})[el.type] || (el.type === 'hidden' ? null : 'textbox');
  return null;
}
function nameOf(el, mirror) {
  const label = el.getAttribute('aria-label'); if (label) return label.replace(/\s+/g, ' ').trim();
  const by = el.getAttribute('aria-labelledby');
  if (by) return by.split(/\s+/).map((id) => document.getElementById(id)?.textContent || '').join(' ').replace(/\s+/g, ' ').trim();
  if (!mirror && el.labels && el.labels.length) return el.labels[0].innerText.replace(/\s+/g, ' ').trim();
  return ((mirror ? el.textContent : el.innerText) || el.getAttribute('title') || el.getAttribute('placeholder') || '').replace(/\s+/g, ' ').trim();
}
function controls() {
  const out = [];
  for (const el of document.querySelectorAll('a[href],button,summary,input,select,textarea,[role]')) {
    const mirror = !!el.closest('[data-remedy-a11y]');
    if (el.closest('[aria-hidden="true"],[hidden],[inert]')) continue;
    if (!mirror && el.getClientRects().length === 0) continue;
    if (!mirror && el.closest('[data-remedy-window]')) continue;
    const role = roleOf(el);
    if (!ROLES.has(role)) continue;
    out.push({el, role, name: nameOf(el, mirror), disabled: el.disabled === true || el.getAttribute('aria-disabled') === 'true'});
  }
  return out;
}
"""

LIST = CONTROLS + "return controls().map(c => [c.role, c.name, c.disabled]);"
CLICK = CONTROLS + r"""
const [role, name, nth, prefix] = arguments;
const hits = controls().filter(c => (prefix ? c.name.startsWith(name) : c.name === name) && (!role || c.role === role) && !c.disabled);
if (hits.length <= nth) return 'not found (' + hits.length + ' matches)';
hits[nth].el.click();
return 'ok';
"""


def wait_idle(rec: Recorder, quiet=1.0, least=1.5, most=20.0):
    start = time.monotonic()
    time.sleep(least)
    while time.monotonic() - start < most:
        with rec.lock:
            if rec.inflight == 0 and time.monotonic() - rec.last >= quiet:
                return
        time.sleep(0.1)


def default_scenario(routes) -> list[dict]:
    steps = [{"open": name} for name, _ in routes]
    steps.append({"open": "home"})
    for name in ["Watchlist", "Bookmarks", "History", "Source", "Settings", "Home"]:
        steps.append({"click": name, "role": "link"})
    # The theme button's name carries the current theme ("Theme, Portal"): open and close it.
    steps += [{"click": "Theme", "role": "button", "prefix": True}, {"click": "Theme", "role": "button", "prefix": True},
              {"click": "Details"}, {"open": "home"}, {"click": "Episodes"}]
    return steps


def run_side(b, rec, base, side, routes, steps, remedy_page=False, load_timeout=300.0) -> None:
    hashes = dict(routes)
    for i, step in enumerate(steps):
        label = f"{i:02} " + (f"open {step['open']}" if "open" in step else f"click {step.get('role') or '*'} '{step['click']}'")
        rec.tag = f"{side}|{label}"
        if "open" in step:
            if step["open"] not in hashes:
                rec.add({"event": "skipped", "why": "no such route"})
                continue
            b.open(base + hashes[step["open"]], "/".join(base.split("/")[:3]))
            result = "ok" if not remedy_page or b.wait_drawn(load_timeout) else "no RemedyUI window"
        else:
            if DENY.search(step["click"]):
                raise SystemExit(f"refusing to click '{step['click']}': it matches the read-only deny list")
            result = b.js(CLICK, step.get("role"), step["click"], step.get("nth", 0), step.get("prefix", False))
        wait_idle(rec)
        rec.add({"event": "step", "result": result, "href": b.js("return location.href"),
                 "controls": sorted(map(list, b.js(LIST) or []))})
        print(f"  {side:<4} {label}: {result}", flush=True)


def by_step(entries):
    steps: dict[str, dict] = {}
    for e in entries:
        side, _, label = e["tag"].partition("|")
        s = steps.setdefault(label, {"calls": Counter(), "controls": None, "result": None})
        if "event" in e:
            if e["event"] == "step":
                s["controls"] = Counter(tuple(c) for c in e["controls"])
                s["result"] = e["result"]
        else:
            s["calls"][(e["method"], e["path"], e["query"], e["body"], e["status"])] += 1
    return steps


def fmt(call):
    m, p, q, body, status = call
    return f"{m} {p}{'?' + q if q else ''}{' ' + body[:200] if body else ''} -> {status}"


def compare(old_entries, new_entries, strict_counts=False) -> int:
    old, new = by_step(old_entries), by_step(new_entries)
    failures = counts = 0
    for label in sorted(set(old) | set(new)):
        o, n = old.get(label), new.get(label)
        lines = []
        if not o or not n:
            lines.append(f"    only in {'old' if o else 'new'}")
        else:
            if o["result"] != n["result"]:
                lines.append(f"    result: old {o['result']!r}, new {n['result']!r}")
            for call in sorted(set(o["calls"]) - set(n["calls"])):
                lines.append(f"    - old only: {fmt(call)}")
            for call in sorted(set(n["calls"]) - set(o["calls"])):
                lines.append(f"    + new only: {fmt(call)}")
            oc, nc = o["controls"] or Counter(), n["controls"] or Counter()
            for c in sorted((oc - nc).elements()):
                lines.append(f"    - old control: {c[0]} '{c[1]}'{' (disabled)' if c[2] else ''}")
            for c in sorted((nc - oc).elements()):
                lines.append(f"    + new control: {c[0]} '{c[1]}'{' (disabled)' if c[2] else ''}")
        count_lines = [f"    ~ count {fmt(c)}: old {o['calls'][c]}, new {n['calls'][c]}"
                       for c in sorted(set(o["calls"]) & set(n["calls"])) if o["calls"][c] != n["calls"][c]] if o and n else []
        if lines or count_lines:
            print(f"  {label}")
            for line in lines + count_lines:
                print(line)
        failures += len(lines)
        counts += len(count_lines)
    print(f"{failures} differences, {counts} call-count differences (timing-dependent)")
    return 1 if failures or (strict_counts and counts) else 0


def load(path):
    return [json.loads(line) for line in Path(path).read_text(encoding="utf-8").splitlines() if line.strip()]


def main():
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    if len(sys.argv) > 1 and sys.argv[1] == "proxy":
        ap = argparse.ArgumentParser(prog="trace.py proxy")
        ap.add_argument("--target", default="http://127.0.0.1:8490")
        ap.add_argument("--port", type=int, default=8491)
        ap.add_argument("--log", default="trace.jsonl")
        a = ap.parse_args(sys.argv[2:])
        rec = Recorder()
        rec.file = open(a.log, "a", encoding="utf-8")
        make_proxy(a.target, rec, a.port)
        print(f"recording /api/ calls on http://127.0.0.1:{a.port}/ -> {a.target}, log {a.log}; Ctrl+C stops")
        try:
            while True:
                time.sleep(3600)
        except KeyboardInterrupt:
            return
    if len(sys.argv) > 1 and sys.argv[1] == "compare":
        ap = argparse.ArgumentParser(prog="trace.py compare")
        ap.add_argument("old")
        ap.add_argument("new")
        ap.add_argument("--strict-counts", action="store_true")
        a = ap.parse_args(sys.argv[2:])
        sys.exit(compare(load(a.old), load(a.new), a.strict_counts))

    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--target", default="http://127.0.0.1:8490", help="Portal (default %(default)s)")
    ap.add_argument("--port", type=int, default=8491, help="proxy port (default %(default)s)")
    ap.add_argument("--old", default="/", help="old UI path (default %(default)s)")
    ap.add_argument("--new", default="/next/", help="new UI path (default %(default)s)")
    ap.add_argument("--routes", help="comma list of route names (default: every page in main.ts)")
    ap.add_argument("--scenario", help="JSON list of steps: {\"open\": route} or {\"click\": name, \"role\": r, \"nth\": n, \"prefix\": bool}")
    ap.add_argument("--size", default="1280x800")
    ap.add_argument("--theme")
    ap.add_argument("--fonts")
    ap.add_argument("--out", default=str(Path(__file__).resolve().parent / "out" / "trace"))
    ap.add_argument("--browser", default="firefox", choices=["firefox", "chrome"])
    ap.add_argument("--strict-counts", action="store_true")
    ap.add_argument("--load-timeout", type=float, default=300,
                    help="seconds a page may take to load, and the RemedyUI page to make its window (default %(default)s)")
    args = ap.parse_args()

    routes = wd.routes(args.target, set(args.routes.split(",")) if args.routes else None)
    steps = json.loads(Path(args.scenario).read_text()) if args.scenario else default_scenario(routes)
    rec = Recorder()
    # frameable: a phone size is shown in an iframe (wd.Browser.resize); API replies are untouched.
    server = make_proxy(args.target, rec, args.port, frameable=True)
    origin = f"http://127.0.0.1:{args.port}"
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    sides = {}
    try:
        for side, path in (("old", args.old), ("new", args.new)):
            # A fresh profile per side: a warm image cache would hide the second side's fetches.
            b = wd.Browser(args.browser, load_timeout=args.load_timeout)
            try:
                wd.set_look(b, origin, args.theme, args.fonts)
                b.resize(*map(int, args.size.split("x")))
                start = len(rec.entries)
                run_side(b, rec, origin + "/" + path.strip("/") + ("/" if path.strip("/") else ""), side, routes, steps,
                         remedy_page=side == "new" and args.new != args.old, load_timeout=args.load_timeout)
                rec.tag = ""
                sides[side] = rec.entries[start:]
            finally:
                b.quit()
            (out / f"{side}.jsonl").write_text("".join(json.dumps(e) + "\n" for e in sides[side]), encoding="utf-8")
    finally:
        server.shutdown()
    print(f"traces: {out / 'old.jsonl'} {out / 'new.jsonl'}")
    sys.exit(compare(sides["old"], sides["new"], args.strict_counts))


if __name__ == "__main__":
    main()
