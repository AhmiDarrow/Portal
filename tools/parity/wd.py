"""A small W3C WebDriver client (stdlib only) and the bits both parity tools share.

Starts geckodriver (headless Firefox) or, with --browser chrome, chromedriver when it is on
PATH. Theme and fonts are set in localStorage, which / and /next/ share because they are the
same origin: the new UI reads the same keys, so one setting covers both sides.
"""

from __future__ import annotations

import base64
import json
import re
import shutil
import socket
import subprocess
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SIZES = {"1280x800": (1280, 800), "1920x1080": (1920, 1080), "390x844": (390, 844)}
# Pages the old UI dispatches but the harness leaves out by default: `cast` is the TV
# receiver page (it polls the cast agent as a receiver), not part of the port.
SKIP_ROUTES = {"cast"}


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def http_json(url: str, method: str = "GET", body=None, timeout: float = 60):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(url, data=data, method=method, headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return json.loads(r.read() or b"null")
    except urllib.error.HTTPError as e:
        return json.loads(e.read() or b"null")


class WebDriverError(RuntimeError):
    pass


class Browser:
    """One headless browser session."""

    def __init__(self, browser: str = "firefox", scheme: str = "dark", reduced_motion: bool = False,
                 load_timeout: float = 300):
        exe = {"firefox": "geckodriver", "chrome": "chromedriver"}[browser]
        path = shutil.which(exe)
        if not path:
            raise SystemExit(f"{exe} is not on PATH")
        port = free_port()
        args = [path, f"--port={port}"]
        self.proc = subprocess.Popen(args, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.base = f"http://127.0.0.1:{port}"
        for _ in range(100):
            try:
                urllib.request.urlopen(self.base + "/status", timeout=1)
                break
            except OSError:
                time.sleep(0.1)
        if browser == "firefox":
            prefs = {
                # 0 dark, 1 light: what `prefers-color-scheme` answers, for the System theme.
                "layout.css.prefers-color-scheme.content-override": 0 if scheme == "dark" else 1,
                "ui.prefersReducedMotion": 1 if reduced_motion else 0,
                "layout.css.devPixelsPerPx": "1.0",
                "browser.cache.disk.enable": False,
                "webgl.force-enabled": True,
            }
            caps = {"browserName": "firefox", "moz:firefoxOptions": {"args": ["-headless"], "prefs": prefs}}
        else:
            chrome_args = ["--headless=new", "--force-device-scale-factor=1", "--hide-scrollbars",
                           f"--force-prefers-color-scheme={scheme}"]
            if reduced_motion:
                chrome_args.append("--force-prefers-reduced-motion")
            caps = {"browserName": "chrome", "goog:chromeOptions": {"args": chrome_args}}
        r = http_json(self.base + "/session", "POST", {"capabilities": {"alwaysMatch": caps}})
        value = r.get("value", {})
        if "sessionId" not in value:
            self.quit()
            raise WebDriverError(f"no session: {value}")
        self.sid = value["sessionId"]
        self.size = None
        # The new UI's wasm is tens of MB in a debug build: navigation may take minutes.
        self.load_timeout = load_timeout
        self.cmd("POST", "/timeouts", {"pageLoad": int(load_timeout * 1000), "script": int(load_timeout * 1000)})

    def cmd(self, method: str, path: str, body=None):
        # Longer than the browser's own page-load timeout, so WebDriver reports it, not us.
        r = http_json(f"{self.base}/session/{self.sid}{path}", method, body, getattr(self, "load_timeout", 60) + 30)
        value = r.get("value") if isinstance(r, dict) else None
        if isinstance(value, dict) and "error" in value:
            raise WebDriverError(f"{value['error']}: {value.get('message', '')[:300]}")
        return value

    def js(self, script: str, *args):
        return self.cmd("POST", "/execute/sync", {"script": script, "args": list(args)})

    def js_async(self, script: str, *args):
        return self.cmd("POST", "/execute/async", {"script": script, "args": list(args)})

    def go(self, url: str):
        self.cmd("POST", "/url", {"url": url})

    def resize(self, width: int, height: int):
        """Makes the viewport exactly width x height CSS pixels.

        Firefox will not make a window narrower than 500 CSS px, even headless. Below that
        the page is opened in a same-origin iframe of exactly the size at (0, 0) and the
        screenshot is cropped to it: the page's viewport, media queries and layout are
        those of a width-px window (devicePixelRatio stays 1).
        """
        self.go("about:blank")
        w, h = width, height
        for _ in range(4):
            self.cmd("POST", "/window/rect", {"width": w, "height": h})
            iw, ih = self.js("return [window.innerWidth, window.innerHeight]")
            if (iw, ih) == (width, height):
                break
            w, h = w + width - iw, h + height - ih
        if iw < width or ih < height:
            raise WebDriverError(f"viewport {iw}x{ih} is smaller than {width}x{height}")
        self.framed = (iw, ih) != (width, height)
        self.size = (width, height)

    def open(self, url: str, origin: str):
        """Loads url as a fresh page (via about:blank, so a hash-only change still reloads)."""
        self.go("about:blank")
        if not getattr(self, "framed", False):
            self.go(url)
            return
        self.go(origin + "/parity-frame")  # any same-origin page: the frame shares localStorage
        frame = self.js_async(
            """const [url, w, h, done] = arguments;
            document.documentElement.style.cssText = 'margin:0;padding:0;overflow:hidden';
            document.body.innerHTML = ''; document.body.style.cssText = 'margin:0;padding:0';
            const f = document.createElement('iframe');
            f.style.cssText = `position:fixed;left:0;top:0;width:${w}px;height:${h}px;border:0;margin:0;padding:0`;
            f.onload = () => done(f);
            f.src = url;
            document.body.appendChild(f);""",
            url, *self.size,
        )
        self.cmd("POST", "/frame", {"id": frame})
        if self.js("return document.documentURI").startswith("about:"):
            raise WebDriverError(f"{url} did not load in the frame (framing refused?)")

    def wait_drawn(self, timeout: float) -> bool:
        """Waits until a RemedyUI page has made its window (`[data-remedy-window]`) and its
        accessibility mirror has content (the app built its first page, not just a canvas)."""
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            if self.js("return !!document.querySelector('[data-remedy-window]') && "
                       "document.querySelectorAll('[data-remedy-a11y] [data-node]').length > 0"):
                return True
            time.sleep(0.25)
        return False

    def screenshot(self) -> bytes:
        png = base64.b64decode(self.cmd("GET", "/screenshot"))
        if not getattr(self, "framed", False):
            return png
        from PIL import Image  # only the framed (phone) size needs Pillow here
        import io
        out = io.BytesIO()
        Image.open(io.BytesIO(png)).crop((0, 0, *self.size)).save(out, "PNG")
        return out.getvalue()

    def quit(self):
        try:
            if getattr(self, "sid", None):
                http_json(f"{self.base}/session/{self.sid}", "DELETE")
        finally:
            self.proc.terminate()


def set_look(b: Browser, origin: str, theme: str | None, fonts: str | None):
    """Writes the theme and fonts both UIs read, before either page loads."""
    b.go(origin + "/parity-blank")  # any same-origin page; a 404 still has localStorage
    b.js(
        """const [t, f] = arguments;
        if (t === null) localStorage.removeItem('portal-theme'); else localStorage.setItem('portal-theme', t);
        if (f === null) localStorage.removeItem('portal-fonts'); else localStorage.setItem('portal-fonts', f);""",
        theme,
        fonts,
    )


def main_ts_pages() -> list[str]:
    """The pages web/src/main.ts dispatches on (`here.page === "..."`), in its order."""
    src = (ROOT / "web" / "src" / "main.ts").read_text(encoding="utf-8")
    seen: list[str] = []
    for page in re.findall(r'here\.page === "([a-z]*)"', src):
        if page and page not in seen:
            seen.append(page)
    return seen


def routes(server: str, include=None) -> list[tuple[str, str]]:
    """(name, hash) for every page, the ones that take an id filled from the real library.

    Ids come from GET /api/history and /api/source only, as the real-content rules ask.
    """
    history = http_json(server + "/api/history").get("items", [])
    source = http_json(server + "/api/source") or {}
    movie = next((e for e in history if e["kind"] == "movie"), None)
    show = next((e for e in history if e["kind"] == "tv"), None)
    q = lambda s: urllib.parse.quote(s, safe="")  # encodeURIComponent
    out = []
    for page in main_ts_pages():
        if page in SKIP_ROUTES and not (include and page in include):
            continue
        if page == "home":
            out.append(("home", "#/"))
        elif page == "browse":
            if source.get("path"):
                out.append(("browse", "#/browse/" + q(source["path"])))
        elif page == "title":
            if movie:
                out.append(("title", "#/title/" + q(movie["id"])))
        elif page == "show":
            if show:
                out.append(("show", "#/show/" + q(show["id"]) + (f"/{show['season']}" if show.get("season") is not None else "")))
        elif page == "settings":
            if source.get("addon_id"):
                out.append(("settings", "#/settings/" + q(source["addon_id"])))
        else:
            out.append((page, "#/" + page))
    out.append(("not-found", "#/parity-no-such-page"))
    if include:
        out = [r for r in out if r[0] in include]
    return out
