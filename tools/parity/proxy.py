"""The recording reverse proxy trace.py and shots.py share.

It forwards the browser's Host and every other header unchanged and adds none (no
X-Forwarded-*, no Forwarded), and it connects from loopback, so Portal still treats the
request as "this PC" (crates/http/src/pairing.rs `this_pc`). Each /api/ call is recorded
with method, path, normalised query and body, and status.
"""

from __future__ import annotations

import http.client
import json
import threading
import time
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HOP = {"connection", "keep-alive", "proxy-connection", "proxy-authorization", "te", "trailer", "transfer-encoding", "upgrade"}


class Recorder:
    def __init__(self):
        self.entries: list[dict] = []
        self.tag = ""
        self.inflight = 0
        self.last = time.monotonic()
        self.lock = threading.Lock()
        self.file = None

    def add(self, entry):
        with self.lock:
            entry["tag"] = self.tag
            self.entries.append(entry)
            if self.file:
                self.file.write(json.dumps(entry) + "\n")
                self.file.flush()


def normalise(method, raw_path, body: bytes, status) -> dict:
    parts = urllib.parse.urlsplit(raw_path)
    query = sorted(urllib.parse.parse_qsl(parts.query, keep_blank_values=True))
    text = body.decode("utf-8", "replace") if body else ""
    try:
        text = json.dumps(json.loads(text), sort_keys=True, separators=(",", ":")) if text else ""
    except ValueError:
        pass
    return {"method": method, "path": urllib.parse.unquote(parts.path), "query": urllib.parse.urlencode(query),
            "body": text, "status": status}


def make_proxy(target: str, rec: Recorder, port: int, frameable: bool = False) -> ThreadingHTTPServer:
    """Starts the proxy on 127.0.0.1:port in a thread.

    frameable drops X-Frame-Options and CSP frame-ancestors from replies, for the narrow
    sizes the harness can only show in an iframe (see wd.Browser.resize).
    """
    t = urllib.parse.urlsplit(target)

    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.0"

        def log_message(self, *a):
            pass

        def forward(self):
            length = int(self.headers.get("Content-Length") or 0)
            body = self.rfile.read(length) if length else b""
            # Host and every other header as the browser sent them; nothing added.
            headers = {k: v for k, v in self.headers.items() if k.lower() not in HOP}
            headers["Connection"] = "close"
            with rec.lock:
                rec.inflight += 1
            status = 502
            try:
                conn = http.client.HTTPConnection(t.hostname, t.port or 80, timeout=300)
                conn.request(self.command, self.path, body=body or None, headers=headers)
                resp = conn.getresponse()
                status = resp.status
                self.send_response_only(resp.status, resp.reason)
                for k, v in resp.getheaders():
                    if k.lower() in HOP or (frameable and k.lower() == "x-frame-options"):
                        continue
                    if frameable and k.lower() == "content-security-policy":
                        # Only frame-ancestors goes; the rest of the policy still applies.
                        v = "; ".join(d for d in v.split(";") if d.strip() and not d.strip().startswith("frame-ancestors"))
                        if not v:
                            continue
                    self.send_header(k, v)
                self.send_header("Connection", "close")
                self.end_headers()
                if self.command != "HEAD":
                    while chunk := resp.read1(65536):
                        self.wfile.write(chunk)
                conn.close()
            except (ConnectionError, OSError) as e:
                if status == 502:
                    self.send_error(502, f"parity proxy: {e}")
            finally:
                with rec.lock:
                    rec.inflight -= 1
                    rec.last = time.monotonic()
                if self.path.startswith("/api/"):
                    rec.add(normalise(self.command, self.path, body, status))

        do_GET = do_POST = do_PUT = do_DELETE = do_PATCH = do_HEAD = do_OPTIONS = forward

    server = ThreadingHTTPServer(("127.0.0.1", port), Handler)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server
