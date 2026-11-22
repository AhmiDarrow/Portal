"""Visual parity: the old UI at /#/<route> beside the new one at /next/#/<route>.

For every route (from web/src/main.ts) and window size it writes old.png, new.png, diff.png
and side.png (old | new | diff) and scores the pair: the percentage of pixels whose largest
channel difference is above --tolerance, split into text and non-text regions. Text is
rasterised by the engine on the new side and by Firefox on the old, so its differences are
reported on their own rather than hidden; the non-text score is the strict one.

  python tools/parity/shots.py                       # / vs /next/, every route, 3 sizes
  python tools/parity/shots.py --new http://127.0.0.1:8490/   # self-test: / vs /
  python tools/parity/shots.py --routes home,history --sizes 390x844 --theme light

Exit code 1 when any pair scores above --fail-above (non-text %), else 0.
"""

from __future__ import annotations

import argparse
import io
import json
import sys
import time
from pathlib import Path

import numpy as np
from PIL import Image

sys.path.insert(0, str(Path(__file__).resolve().parent))
import wd  # noqa: E402
from proxy import Recorder, make_proxy  # noqa: E402

# Text boxes on the page: the old UI's text nodes and form fields, and the new UI's ARIA
# mirror leaves (each laid over the node's drawn bounds by remedy-ui-bridge-web aria.rs).
TEXT_RECTS = r"""
const out = [], vw = innerWidth, vh = innerHeight;
const push = (r) => { if (r.width > 0 && r.height > 0 && r.right > 0 && r.bottom > 0 && r.left < vw && r.top < vh) out.push([r.left, r.top, r.width, r.height]); };
const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
for (let n; (n = walker.nextNode()); ) {
  if (!n.textContent.trim() || n.parentElement.closest('[data-remedy-a11y],script,style,noscript')) continue;
  const range = document.createRange(); range.selectNodeContents(n);
  for (const r of range.getClientRects()) push(r);
}
for (const el of document.querySelectorAll('input,textarea,select')) push(el.getBoundingClientRect());
for (const el of document.querySelectorAll('[data-remedy-a11y] [data-node]')) {
  if (el.children.length === 0 && el.textContent.trim()) push(el.getBoundingClientRect());
}
return out;
"""

MASK_RECTS = r"""
const out = [];
for (const sel of arguments[0]) for (const el of document.querySelectorAll(sel)) { const r = el.getBoundingClientRect(); out.push([r.left, r.top, r.width, r.height]); }
return out;
"""

WAIT_FONTS = "const done = arguments[arguments.length - 1]; (document.fonts ? document.fonts.ready : Promise.resolve()).then(() => done(true), () => done(false));"


def decode(png: bytes) -> np.ndarray:
    return np.asarray(Image.open(io.BytesIO(png)).convert("RGB"))


def settle(b: wd.Browser, timeout: float, interval: float, frames: int = 2) -> tuple[np.ndarray, bool]:
    """Screenshots until `frames` in a row match exactly (animations, fetches and fonts done)."""
    try:
        b.js_async(WAIT_FONTS)
    except wd.WebDriverError:
        pass
    end = time.monotonic() + timeout
    # The page's text too (the RemedyUI mirror follows each built page), so a frame drawn
    # while a fetch is still out does not pass for the settled page.
    state = lambda: (decode(b.screenshot()), b.js(MIRROR_TEXT))
    last, text = state()
    same = 1
    while time.monotonic() < end:
        time.sleep(interval)
        now, now_text = state()
        same = same + 1 if now.shape == last.shape and np.array_equal(now, last) and now_text == text else 1
        last, text = now, now_text
        if same >= frames:
            return now, True
    return last, False


MIRROR_TEXT = "return Array.from(document.querySelectorAll('[data-remedy-a11y] [data-node]'), e => e.textContent).join('|')"


def rect_mask(shape, rects, pad=0) -> np.ndarray:
    m = np.zeros(shape[:2], bool)
    h, w = shape[:2]
    for x, y, rw, rh in rects:
        x0, y0 = max(0, int(x) - pad), max(0, int(y) - pad)
        x1, y1 = min(w, int(np.ceil(x + rw)) + pad), min(h, int(np.ceil(y + rh)) + pad)
        if x1 > x0 and y1 > y0:
            m[y0:y1, x0:x1] = True
    return m


def click_steps(b, steps, timeout):
    """Clicks each `role:name[:nth]` control (found as trace.py finds it: the ARIA mirror on
    the new side), waiting for it to appear, before the screenshot."""
    from trace import CLICK  # same control lookup as the behaviour runs
    for step in steps:
        role, name, *rest = step.split(":")
        nth = int(rest[0]) if rest else 0
        end = time.monotonic() + timeout
        while True:
            got = b.js(CLICK, role or None, name, nth, False)
            if got == "ok" or time.monotonic() > end:
                break
            time.sleep(0.5)
        if got != "ok":
            print(f"  click {step}: {got}", flush=True)


def capture(b, url, args, mask_cfg, route, size_key, remedy_page=False):
    b.open(url, "/".join(url.split("/")[:3]))
    frames = 2
    if remedy_page:
        # The wasm loads after the document: the canvas comes later, then its first frames
        # (fetches, images, fonts). Blank or half-drawn screens must not count as settled.
        if not b.wait_drawn(args.load_timeout):
            print(f"  {url}: no [data-remedy-window] after {args.load_timeout:.0f} s", flush=True)
        frames = args.stable_frames
        # Its first fetches answer after the first frame; give them a moment before frames count.
        time.sleep(args.remedy_delay)
    if args.click:
        settle(b, args.settle_timeout, args.settle_interval, frames)
        click_steps(b, args.click, args.settle_timeout)
    img, settled = settle(b, args.settle_timeout, args.settle_interval, frames)
    text = b.js(TEXT_RECTS) or []
    sels = mask_cfg.get("selectors", {})
    selectors = sels.get("*", []) + sels.get(route, []) + sels.get(f"{route}@{size_key}", [])
    masked = b.js(MASK_RECTS, selectors) if selectors else []
    remedy = bool(b.js("return !!document.querySelector('[data-remedy-window]')"))
    return img, settled, text, masked, remedy


def pad_to(a: np.ndarray, shape) -> np.ndarray:
    out = np.zeros(shape, a.dtype)
    out[: a.shape[0], : a.shape[1]] = a
    return out


def score(old, new, tolerance, text_mask, ignore):
    shape = (max(old.shape[0], new.shape[0]), max(old.shape[1], new.shape[1]), 3)
    old, new = pad_to(old, shape), pad_to(new, shape)
    text_mask, ignore = pad_to(text_mask, shape[:2]), pad_to(ignore, shape[:2])
    delta = np.abs(old.astype(np.int16) - new.astype(np.int16)).max(axis=2)
    diff = delta > tolerance
    live = ~ignore
    text = text_mask & live
    rest = ~text_mask & live
    pct = lambda d, region: round(100.0 * float((d & region).sum()) / max(1, int(region.sum())), 4)
    stats = {
        "all_pct": pct(diff, live),
        "nontext_pct": pct(diff, rest),
        "text_pct": pct(diff, text),
        "text_area_pct": round(100.0 * float(text.sum()) / max(1, int(live.sum())), 2),
        "masked_pct": round(100.0 * float(ignore.sum()) / diff.size, 2),
        "max_delta": int(delta[live].max()) if live.any() else 0,
        "within_tolerance_nonzero_pct": pct((delta > 0) & ~diff, live),
    }
    # Dimmed old page; red = differs outside text, yellow = differs in text, blue = masked.
    vis = (old.astype(np.float32) * 0.3).astype(np.uint8)
    vis[diff & rest] = (255, 40, 40)
    vis[diff & text] = (255, 210, 0)
    vis[ignore] = (vis[ignore] * 0.5 + np.array([0, 60, 160]) * 0.5).astype(np.uint8)
    return stats, old, new, vis


def side_by_side(*imgs, gap=8):
    h = max(i.shape[0] for i in imgs)
    w = sum(i.shape[1] for i in imgs) + gap * (len(imgs) - 1)
    out = np.full((h, w, 3), 128, np.uint8)
    x = 0
    for i in imgs:
        out[: i.shape[0], x : x + i.shape[1]] = i
        x += i.shape[1] + gap
    return out


def main():
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--old", default="http://127.0.0.1:8490/", help="old UI base URL (default %(default)s)")
    ap.add_argument("--new", default="http://127.0.0.1:8490/next/", help="new UI base URL (default %(default)s)")
    ap.add_argument("--routes", help="comma list of route names (default: every page in main.ts)")
    ap.add_argument("--sizes", default=",".join(wd.SIZES), help="comma list of WxH (default %(default)s)")
    ap.add_argument("--theme", help="portal-theme id (portal, system, light, deutan, ...); default: none stored")
    ap.add_argument("--fonts", help='portal-fonts JSON, e.g. \'{"text":"remedy-sans","headings":"ahmi-pixel"}\'')
    ap.add_argument("--scheme", default="dark", choices=["dark", "light"], help="prefers-color-scheme, for the System theme")
    ap.add_argument("--reduced-motion", action="store_true")
    ap.add_argument("--tolerance", type=int, default=2, help="largest per-channel difference (0-255) still counted equal (default %(default)s)")
    ap.add_argument("--mask", help="JSON mask file: {\"rects\": {route|route@WxH|*: [[x,y,w,h]]}, \"selectors\": {...: [css]}}")
    ap.add_argument("--out", default=str(Path(__file__).resolve().parent / "out" / "shots"))
    ap.add_argument("--browser", default="firefox", choices=["firefox", "chrome"])
    ap.add_argument("--settle-timeout", type=float, default=20)
    ap.add_argument("--settle-interval", type=float, default=0.5)
    ap.add_argument("--stable-frames", type=int, default=3,
                    help="identical screenshots in a row that count as settled on the RemedyUI side (default %(default)s)")
    ap.add_argument("--remedy-delay", type=float, default=2.0,
                    help="seconds to wait after the RemedyUI page first draws, before counting stable frames (default %(default)s)")
    ap.add_argument("--load-timeout", type=float, default=300,
                    help="seconds a page may take to load, and the RemedyUI page to make its window (default %(default)s)")
    ap.add_argument("--fail-above", type=float, default=0.0, help="exit 1 when a non-text score is above this %% (default %(default)s)")
    ap.add_argument("--click", action="append", default=[],
                    help="after load, click the control `role:name[:nth]` (repeatable), e.g. button:Info:1")
    ap.add_argument("--extra", action="append", default=[],
                    help="another page to shoot, `name=#/hash` (repeatable), e.g. a folder opened directly")
    ap.add_argument("--tag", default="", help="suffix for the output folder names, e.g. -info")
    args = ap.parse_args()

    old_base, new_base = args.old.rstrip("/") + "/", args.new.rstrip("/") + "/"
    server = "/".join(old_base.split("/")[:3])
    mask_cfg = json.loads(Path(args.mask).read_text()) if args.mask else {}
    include = set(args.routes.split(",")) if args.routes else None
    routes = wd.routes(server, include)
    routes += [tuple(e.split("=", 1)) for e in args.extra]
    out = Path(args.out)
    results = []

    origin_of = lambda url: "/".join(url.split("/")[:3])
    framing: dict[str, str] = {}  # Portal origin -> frameable proxy origin, for narrow sizes
    proxies = []

    def via(base: str) -> str:
        if not b.framed:
            return base
        o = origin_of(base)
        if o not in framing:
            port = wd.free_port()
            proxies.append(make_proxy(o, Recorder(), port, frameable=True))
            framing[o] = f"http://127.0.0.1:{port}"
            wd.set_look(b, framing[o], args.theme, args.fonts)
        return framing[o] + base[len(o):]

    b = wd.Browser(args.browser, args.scheme, args.reduced_motion, args.load_timeout)
    try:
        for o in {origin_of(old_base), origin_of(new_base)}:
            wd.set_look(b, o, args.theme, args.fonts)
        for size_key in args.sizes.split(","):
            w, h = map(int, size_key.split("x"))
            b.resize(w, h)
            for name, hash_ in routes:
                o = capture(b, via(old_base) + hash_, args, mask_cfg, name, size_key)
                n = capture(b, via(new_base) + hash_, args, mask_cfg, name, size_key, remedy_page=new_base != old_base)
                rects = mask_cfg.get("rects", {})
                fixed = rects.get("*", []) + rects.get(name, []) + rects.get(f"{name}@{size_key}", [])
                shape = (max(o[0].shape[0], n[0].shape[0]), max(o[0].shape[1], n[0].shape[1]))
                # Text on either side, padded for antialiasing halos.
                text_mask = rect_mask(shape, o[2] + n[2], pad=2)
                ignore = rect_mask(shape, fixed + o[3] + n[3])
                stats, old, new, vis = score(o[0], n[0], args.tolerance, text_mask, ignore)
                d = out / size_key / (name + args.tag)
                d.mkdir(parents=True, exist_ok=True)
                Image.fromarray(o[0]).save(d / "old.png")
                Image.fromarray(n[0]).save(d / "new.png")
                Image.fromarray(vis).save(d / "diff.png")
                Image.fromarray(side_by_side(old, new, vis)).save(d / "side.png")
                row = {"route": name, "size": size_key, "hash": hash_, **stats,
                       "old_settled": o[1], "new_settled": n[1], "new_is_remedy": n[4], "dir": str(d)}
                results.append(row)
                print(f"{size_key:>9} {name:<10} non-text {stats['nontext_pct']:8.4f}%  text {stats['text_pct']:8.4f}%  "
                      f"max delta {stats['max_delta']:3}  {'' if o[1] and n[1] else 'UNSETTLED '}{'' if n[4] else '(new side is not a RemedyUI page)'}",
                      flush=True)
    finally:
        b.quit()
        for p in proxies:
            p.shutdown()

    out.mkdir(parents=True, exist_ok=True)
    meta = {"old": old_base, "new": new_base, "tolerance": args.tolerance, "theme": args.theme,
            "fonts": args.fonts, "scheme": args.scheme, "browser": args.browser, "results": results}
    (out / "report.json").write_text(json.dumps(meta, indent=2))
    lines = [f"# Visual parity: {old_base} vs {new_base}", "",
             f"Tolerance {args.tolerance}/255 per channel; theme {args.theme or '(default)'}; fonts {args.fonts or '(default)'}.", "",
             "| size | route | non-text % | text % | text area % | max delta | settled | side by side |",
             "|---|---|---|---|---|---|---|---|"]
    for r in results:
        lines.append(f"| {r['size']} | {r['route']} | {r['nontext_pct']} | {r['text_pct']} | {r['text_area_pct']} | {r['max_delta']} | "
                     f"{'yes' if r['old_settled'] and r['new_settled'] else 'NO'} | {r['size']}/{r['route']}/side.png |")
    (out / "report.md").write_text("\n".join(lines) + "\n")
    print(f"report: {out / 'report.md'}")
    sys.exit(1 if any(r["nontext_pct"] > args.fail_above for r in results) else 0)


if __name__ == "__main__":
    main()
