# Parity harness

Proves the RemedyUI port (`/next/`) looks and behaves like the old web UI (`/`), both served
by the same Portal server. Plan: `docs/REMEDYUI-PORT.md` section 6.

Needs Python 3.11+ with Pillow and numpy (`pip install pillow numpy`), Firefox and
geckodriver on PATH. chromedriver is used only with `--browser chrome`. Run the commands
from the repo root in PowerShell (Git Bash rewrites a bare `/` argument into a Windows path;
there, prefix the command with `MSYS_NO_PATHCONV=1`). Output goes to `tools/parity/out/`
(ignored by git) unless `--out` says otherwise.

## 1. Settings safety

```powershell
$backup = pwsh tools/parity/backup.ps1 -Root "$env:TEMP\portal-backups" | Select-Object -Last 1
# ... run the tests ...
pwsh tools/parity/restore.ps1 -Backup $backup
```

- `backup.ps1` copies `library.json`, `prefs.json` and `cast.json` from `%LOCALAPPDATA%\Portal`
  into a new timestamped folder and saves `GET /api/history` as `history.json`. The last
  line it prints is the folder. `-Server` (default `http://127.0.0.1:$PORTAL_PORT` or 8490).
- `restore.ps1` writes every history position that differs back with `POST /api/progress`
  (flush), then checks `GET /api/history` against the backup (matched by id, season and
  episode). If it still differs it says so and exits 1; stop Portal
  (`portal-launcher.exe --stop`) and run it again: with Portal stopped it copies
  `library.json` back.

## 2. Visual parity: `shots.py`

```powershell
python tools/parity/shots.py                                     # / vs /next/, every route, 3 sizes
python tools/parity/shots.py --new http://127.0.0.1:8490/        # self-test: / vs /
python tools/parity/shots.py --routes home,history --sizes 390x844 --theme light `
    --fonts '{"text":"remedy-sans","headings":"ahmi-pixel"}' --mask tools/parity/mask.example.json
```

- Routes come from the pages `web/src/main.ts` dispatches (`here.page === "..."`), plus a
  not-found address. Pages that take an id are filled from `GET /api/history` (first movie
  for `title`, first show for `show`) and `GET /api/source` (`browse`, `settings`). `cast`,
  the TV receiver page, is left out unless named in `--routes`.
- Sizes: 1280x800, 1920x1080 and 390x844 CSS px at devicePixelRatio 1. Firefox will not make
  a window narrower than 500 px, so 390x844 opens the page in a same-origin iframe of exactly
  that size and crops the screenshot to it. Portal forbids framing (`guard.rs`), so that size
  goes through a local proxy that removes only `X-Frame-Options` and the CSP
  `frame-ancestors` directive.
- Each page loads fresh (via `about:blank`), waits for `document.fonts.ready`, then takes
  screenshots every 0.5 s until two in a row are identical (20 s at most; `UNSETTLED` is
  printed otherwise). Live content that never settles (a clock) belongs in a mask.
- The RemedyUI side first waits for the page to make its window (`[data-remedy-window]`)
  and for its ARIA mirror to have content (`--load-timeout`, default 300 s, is also
  WebDriver's page-load timeout, since a debug wasm is tens of MB), waits `--remedy-delay`
  (default 2 s) for its first fetches, then needs `--stable-frames` (default 3) screenshots
  in a row with the same pixels and the same mirror text, so a blank canvas or a page still
  waiting on the server is never compared. `trace.py` waits for the
  window the same way and takes the same `--load-timeout`.
- Per route and size it writes `old.png`, `new.png`, `diff.png` and `side.png`
  (old | new | diff) under `out/shots/<size>/<route>/`, plus `report.md` and `report.json`.
  In `diff.png` red is a difference outside text, yellow a difference inside text, blue is
  masked.
- **Score**: the percentage of pixels whose largest channel difference is above
  `--tolerance`, reported separately for non-text and text regions. Text regions are the
  boxes of the old UI's text nodes and form fields and of the new UI's ARIA-mirror leaves,
  each padded 2 px. Text is rasterised by the engine on the new side and by Firefox on the
  old, so its differences are reported on their own, not hidden. The non-text score is the
  strict one; `--fail-above` (default 0) sets the exit code on it.
- **Tolerance: 2 of 255 per channel** (default). An exact match (0) would flag the ±1
  rounding that two correct pipelines produce in 8-bit blending and premultiplied alpha;
  2 also covers rounding through sRGB/linear conversion and gradient interpolation. A
  wrong theme token, border, shadow or 1 px shift moves pixels by far more than 2. The plan
  suggested 5; 2 is stricter. `report.json` also lists `within_tolerance_nonzero_pct`, the
  share of pixels that differ by 1 or 2, so even those stay visible.
- `--mask` file (JSON): rectangles in CSS px and CSS selectors (looked up in both pages),
  keyed by `*`, a route name, or `route@WxH`. See `mask.example.json`.
- `PARITY_FIREFOX_PREFS` (JSON) adds Firefox prefs to every session, e.g.
  `{"webgl.disabled": true}` when WebGL hangs on the machine (the new UI then draws with
  RemedyUI's CPU renderer).
- `--click role:name[:nth]` (repeatable) presses a control after the page settles and
  before the screenshot, found as `trace.py` finds controls (the ARIA mirror on the new
  side); `--tag -info` suffixes the output folders. Info dialogs:
  `--routes title --click button:Info --tag -info` (a movie), `--routes show --click
  button:Info --tag -info` (the show) and `--click button:Info:1` (its first episode).
- While `/next/` only has placeholders (or 404s), the tool still runs and reports large
  differences; it notes when the new side is not a RemedyUI page (`[data-remedy-window]`).

### Theme and fonts for both UIs (for the web-app agents)

The old UI keeps its look per browser in localStorage. `/` and `/next/` are the same origin,
so they share it. The new app must read and write the **same keys with the same values**:

| key | value | absent means |
|---|---|---|
| `portal-theme` | a theme id from `web/src/themes.ts` `themeMenu`: `portal`, `system`, `light`, `orange`, `purple`, `deutan`, `green`, `tritan`, `cyan`, `protan`, `dark`, `neutral`, `forest`, `alien` | `portal` |
| `portal-fonts` | JSON `{"text": FontId, "headings": FontId}`, FontId from `web/src/fonts.ts`: `remedy-sans`, `remedy-sans-ui`, `ahmi-pixel`, `ahmi-print`, `system` | `{"text":"remedy-sans","headings":"ahmi-pixel"}` |

The harness writes both keys before any page loads (`--theme`, `--fonts`; omitted means the
key is removed, so both UIs show their default). `system` follows `prefers-color-scheme`,
which `--scheme dark|light` sets in Firefox. This also keeps a user's choice across the
cutover.

### What the new app must do for the harness

- Expose controls through RemedyUI's ARIA mirror (`remedy-ui-bridge-web` `web/aria.rs`)
  with the same role and accessible name as the old UI (the `aria-label`, else the text).
- Stop drawing when idle (no endless animation or blinking caret on first load), or the
  screenshot never settles.

## 3. Behaviour parity: `trace.py`

```powershell
python tools/parity/trace.py                    # / vs /next/ through a proxy on 8491
python tools/parity/trace.py --new /            # self-test: / vs /
python tools/parity/trace.py --scenario my-steps.json --size 390x844 --routes home,history
python tools/parity/trace.py proxy --port 8491 --log trace.jsonl   # recording proxy only
python tools/parity/trace.py compare out/trace/old.jsonl out/trace/new.jsonl
```

- The proxy (`proxy.py`) listens on `--port` (8491) in front of `--target`
  (`http://127.0.0.1:8490`). It forwards the browser's Host and every other header unchanged,
  adds no `Forwarded`/`X-Forwarded-*`/`X-Real-IP`, and connects from loopback, so Portal
  still sees "this PC" (`crates/http/src/pairing.rs` `this_pc`). For every `/api/` call it
  logs method, decoded path, sorted query, body (JSON re-serialised with sorted keys) and
  status.
- The scenario runs once per side, each in a fresh browser profile (a warm image cache would
  hide the second side's fetches): open every route, then click the header links by name,
  open and close the Theme menu, open a card's Details and Episodes. Steps are
  `{"open": route}` or `{"click": name, "role": role, "nth": n, "prefix": bool}`. A click
  is a DOM `click()` on the element with that role and accessible name: a real element in
  the old UI, an ARIA-mirror element (`[data-remedy-a11y] [data-node]`) in the new one,
  which the bridge turns into an AccessKit Click. Clicks whose name matches the deny list
  (Remove, Delete, Play, Resume, Cast, Install, Update, Restart, ...) are refused, so the
  run stays read-only.
- After each step it waits until no request is in flight and none has finished for 1 s
  (20 s at most), and records the visible controls (role, name, disabled).
- The comparison prints, per step, calls made on one side only, controls present on one
  side only and click results that differ. Call counts depend on timing (the header chip
  polls `/api/status` every 4 s), so count differences are printed but fail only with
  `--strict-counts`. Exit code 1 on any other difference.
