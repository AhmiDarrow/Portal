# Portal on RemedyUI: port plan

Scope: everything Portal's web UI (`web/src`, `web/index.html`) and the launcher's tray UI (`crates/launcher/src/tray.rs`, `ui.rs`) do today, rebuilt on RemedyUI (`origin/main` b0ddf177) for the Windows desktop, the web (served by Portal's server) and Android. The server crates stay.

**Parity rule.** Portal must work and look identically to today. Any difference must be an improvement, and each one is listed in section 5 with why it is better. Every phase in section 6 ends with a visual parity check (web Portal and the port side by side, same theme and window size) and a behaviour parity check (the same API calls with the same results). Where exact parity can't be had, that is an engine gap (section 4) to fix in RemedyUI, never an accepted difference.

## Decisions (these override the plan below where they differ)

- Portal stays a **web UI**: the port is the RemedyUI *web build* (wasm), served by Portal's server and opened in the browser. There is no native window app; the tray's "Open Portal" opens the browser.
- Targets: the Windows and Linux servers, with browsers on the PC and on LAN devices (phones). Android native is dropped. Old TV browsers without WebGL2 are not supported.
- The TV-browser receiver (`#/cast`, `web/src/cast.ts` and its server routes) is decided later; it keeps working exactly as today, so the old UI keeps serving it.
- Confirm dialogs use Portal's theme, not native message boxes.
- Parity: it must work and look identically to Portal today. Any difference is a deliberate improvement listed in section 5, or proposed explicitly.
- No hard caps: every limit gets a default and a public setter.
- An engine shortcoming is fixed in RemedyUI, never worked around in Portal.
- The web app lives in `crates/web-app` (portal-web), not `crates/app`.

---

## 1. Architecture

### 1.1 What stays and what is replaced

| Part | Fate |
|---|---|
| `crates/api` (portal-api, serde only) | **Stays and grows**: it becomes the typed wire contract for the app. Wire types now built inline in `crates/http` (19 `json!` sites: cast, pairing, prefs, metadata, subtitles, update, app) move here so client and server can't drift. Builds for wasm32 and Android (serde only). |
| `crates/library`, `crates/engine`, `crates/http` (portal.exe) | **Stay**, plus the small server changes S1–S3 below. |
| `tools/cast/cast_agent.py`, Kodi + Umbrella, ffmpeg HLS, installer | **Stay.** Casting stays server → Cast TV (`/api/cast/devices/{id}/*` driving `cast_agent.py`); the client only sends commands. |
| `web/src/*.ts`, `styles.css`, `index.html`, `web/vendor/hls.min.js` | **Replaced** by the RemedyUI app; deleted only at cutover (P15), after both run side by side. |
| `crates/launcher/src/tray.rs` (Win32 tray), `ui.rs` (MessageBox, ShellExecute) | **Replaced** by the engine tray (`App::tray_dyn`, `TrayItem::Check/Submenu`, default item), `App::notify` with `Urgency`, `autostart::Entry::windows_run_key`, `desktop::reveal`. |
| `crates/launcher/src/model.rs` (menu + watchdog state machine), `service.rs`, `autostart.rs` | **Stay as Rust logic**; `model.rs` has no platform code, so the new tray is drawn from `Model`. The exe keeps its name `portal-launcher.exe` and flags (`--stop`, `--quiet`, `--restart-server`, `--autostart`, `--no-browser`) used by `installer/portal.iss:84,321,477`. |

New crate `crates/app` (portal-app): a library with every screen, depending on `remedy-ui` (features `widgets`, `net`, playback, `media-session`, `keep-awake`, `extra`, `desktop`, `web`) and `portal-api`. Entry points: `portal-launcher` (Windows: tray, window and watchdog in one process), a web `cdylib` (`cargo remedy package --target web`), and an Android package.

### 1.2 The native app does not embed the server

- The Windows app on the Portal PC talks to `http://127.0.0.1:<PORTAL_PORT>`. The server treats it as this PC (`pairing::this_pc`: loopback, no forwarded headers), so it has full rights with no pairing, as the browser on the PC does today.
- Android, or Windows on another PC, talks over the LAN with `Http::new(base).policy(HttpPolicy::PrivateNetworks)` and a `CookieJar`.
- Why not embed: the server owns Kodi, ffmpeg, cast_agent and the port; the watchdog (3 restarts in 10 minutes) and "Restart Portal server" need a separate process; closing the window must not stop casting or autoplay on the TV; the installer and updater already manage `portal.exe`.

### 1.3 Talking to the server

- **Same HTTP API**, one route per call as `web/src/api.ts` sends it (api.ts:440-690), through services-net `Http::send_json::<T>` with portal-api types. Errors arrive as `HttpError{status, detail}`; `detail` is the string the page shows today.
- **Guard** (`crates/http/src/guard.rs`): a native client sends no `Sec-Fetch-Site`/`Origin`, which the guard accepts; its Host is an IP or local name, which passes. In a page, `fetch` is same-origin, so nothing changes.
- **Pairing** (`crates/http/src/pairing.rs`): `GET /api/pair/me` → `{paired, this_pc}` (`this_pc` replaces `isRemote()`, main.ts:3782); a 401 with `{"pair":true}` opens the pairing screen; `POST /api/pair/start {name}`, poll `GET /api/pair/status` every 2 s, retry after 30 s (pair.ts:6-7). The `portal_device` cookie (HttpOnly, SameSite=Strict, pairing.rs:451) is kept by the browser on the web and by `CookieJar` natively, which must persist across restarts (**gap G2**).
- **Keepalive**: the last progress save on close uses `Request::keepalive()` (today `fetch(..., {keepalive})`, api.ts:525).
- **Media**: `/media/{session}/{name}` is outside `/api/`, so it needs no cookie (pairing.rs:369).

### 1.4 Serving the web build

- `cargo remedy package --target web` writes the wasm, glue, `index.html`, manifest and service worker; the server already serves whatever `PORTAL_WEB_DIST` points at (`ServeDir` fallback, main.rs ~466).
- **S2**: add `application/wasm` to `compressible()` (main.rs:195).
- **S3 (transition only)**: serve the new build at `/next/` beside the old one at `/`, so parity tests run both against one server. Routes stay hash routes (`#/browse/<path>`, `#/title/<id>`, `#/settings/<id>`), so old bookmarks keep working. Cutover swaps them (P15).
- **Caching**: `/assets/*` immutable, everything else `no-cache` (main.rs:170-191); wasm-bindgen names aren't content-hashed, so `no-cache` plus validators is right. Make sure the service worker can't serve a stale wasm after an update (R5).
- **Secure context**: on a phone or TV the page is `http://<lan-ip>:8490`, not a secure context, so WebCodecs, Screen Wake Lock, the Gamepad API, service workers and WebGPU are unavailable. `localhost` on the PC is fine. This makes **gap G1** a blocker for the web build on LAN devices.

### 1.5 Images and the proxy route (S1)

- Kodi art (`image://…`, `special://…`) keeps going to `GET /api/image?path=` (main.rs:2366, SSRF-checked in `process.rs:396`); same-origin, no proxy needed.
- Direct `http(s)` art (`artUrl`, api.ts:691; `artFromId`, main.ts:467) loads cross-site today through `<img>`. The engine fetches with CORS and falls back to `media_image::set_web_proxy(prefix)`.
- **S1 adds `GET /api/image/web?url=<encoded>`**: http/https only; the `public_web_host`/`resolves_public` checks from `process.rs:79-249` on every redirect hop, connecting to the address it resolved (no DNS rebinding); only `image/*` replies; size cap and timeout configurable (`PORTAL_IMAGE_PROXY_MAX_BYTES`, no hard cap); the same in-memory LRU as `cached_image`; `Cache-Control: max-age` passed through; under `/api/`, so pairing and same-site checks apply; fetched directly (reqwest), so posters show while Kodi is down.
- Web build: `set_web_proxy(Some("/api/image/web?url=".into()))`.
- Native on a paired remote device, `/api/image` needs the pairing cookie and LAN http, but the engine's default image fetcher is `Http::new("")` (loopback only, no cookies, `remote.rs:630-642`): **gap G3**.

### 1.6 Per-device state (today's localStorage)

| Key | What | Port |
|---|---|---|
| `portal-theme` (themes.ts:35) | theme | `Signal::persist("theme")` |
| `portal-fonts` (fonts.ts:31) | text and heading fonts | persist |
| `portal-playback` (prefs.ts:27) | local copy of `/api/prefs` plus volume | persist; volume stays per device |
| `portal-seasons` (api.ts:751) | season memory per show | persist |
| `portal-cast-receiver` (cast.ts:123) | TV-browser receiver id | not ported (Q1) |

At web cutover, the old keys are imported once with `services-webstorage`, so web users keep their theme and fonts.

### 1.7 Linux

The client is cheap (the engine supports Linux and the app is pure HTTP: a CI build and a smoke test). The server is not (DPAPI keys, Kodi registry lookup, `windows-sys` in the launcher, the Inno installer). Recommendation: a Linux client only, after P14.

---

## 2. Screen-by-screen map

Theme: `remedy_ui_themes::portal_theme(id)` plus `portal_sheet(t)` (`remedy-ui-themes/src/portal.rs`), token for token from themes.ts including Azure, Skyline, Lagoon and the colorblind palettes. Page background: `portal_page_layers()` / `PORTAL_PAGE_CSS` (styles.css:109-110, matched to Chrome within 5 levels by `devtools/tests/portal_gradients.rs`). Fonts: Remedy Sans, Remedy Sans UI, Ahmi Pixel (headings by default, fonts.ts:29) and AhmiPrint from `remedy-ui-fonts`.

| Screen | Portal refs | RemedyUI | Look-parity notes |
|---|---|---|---|
| **Header**: skip link, brand, nav, theme picker, update pill, status chip | index.html header; main.ts:172-205, 313 `refreshChip` (every 4 s, paused while hidden, 3799), 331 `markNav`, 3532 `paintUpdatePill`; themes.ts:182 | `Router::for_window`, `Router::link_to` (real `<a href>`, middle-click new tab), `Role::Navigation` + `Current::Page`; header `El::sticky(StickyInsets::top(0))` + `backdrop_blur(16)` (styles.css:185-189); theme picker `menu_button` with swatches (System swatch via `Gradient::parse`); `chip`; pill `link` | nav lowercase (styles.css:245) via `TextTransform::Lower`; theme order exactly as themes.ts:81-86; frosted header over scrolling content |
| **Activity panel** | index.html `#activity`; activity.ts:32, 54, 87-226 | `progress`, Cancel `button`, `El::animate` for `activity-slide` (styles.css:321) | wording word for word; no slide under reduced motion (styles.css:775) |
| **Notices / errors** | activity.ts:67; main.ts:257, 309, 274 | `text` in a polite live region; error variant | the 350/400 ms delayed show kept, so fast loads don't flash |
| **Home**: source link, Continue watching, Watchlist, Bookmarks | main.ts:397 `renderHome`, 420 `shelf`, 427 `cardHtml`, 485, 493; styles.css:368-376, 422, 425 | horizontal `scroll` per shelf with `El::scroll_snap(SnapAxis::X, Proximity)` + `snap_align(Start)`, `El::focus_group()`; card `button` with `image_ext(Source::Url, ImageOptions{placeholder, fallback: letter tile, on_state})`; progress bar; `link_to` for Episodes/Details | 2:3 poster; wide/square framing from `on_state` real size (styles.css:440-442); letter tile (main.ts:286); hover lift only when `pointer_caps()` hover && fine (styles.css:391,437) |
| **Shelf rename / remove** | main.ts:1493-1616 | inline `text_input` + `button`s; focus kept on the row (main.ts:1601) | quiet buttons |
| **Browse**: folder grid, in-page search, run items, settings links | main.ts:552 `renderBrowse` (stale-while-revalidate cache, limit 60, main.ts:120), 593, 1181, 1224, 1238, 1250, 1268; settings-link.ts | `grid_view_with(.., GridOptions{own_scroll:false, interactive_tiles:true})` + `Tiles::fluid(180, 2/3, ..)` (= `repeat(auto-fill,minmax(180px,1fr))`, styles.css:376); `search` field (`InputPurpose::Search`, `EnterKey::Search`) | cache limit configurable (no hard cap) |
| **Search results** | main.ts:647 `runSearch` (sequence guard) | `Http::resource` keyed by query (a new key aborts the old request) | — |
| **Show page**: hero over backdrop, season tabs, episodes with stills and progress, watched seasons, resume | main.ts:887-1123; styles.css:1951-2150 | hero = `stack` of `image_ext(Fit::Cover)` + `background_layers` scrims (styles.css:1973-1974, `show_hero` gradient test); `tabs` for seasons; 16:9 still rows; `max_lines(3)` plot (styles.css:2126) | aired dates via `CalendarSymbols::medium_date` in the page's English locale (html.ts:7) |
| **Title details**: Play / Resume / From start / Episodes, episode rows | main.ts:1350-1468 | `column`/`row`, pill buttons (styles.css:1937) | small 16:9 stills (styles.css:2135-2138) |
| **Watchlist / Bookmarks pages** | main.ts:1638; empty texts 140-141 | grid as Browse | — |
| **History**: rows, rename, remove (slot-checked), play / from start | main.ts:1665-1732 | `virtual_list`, 48 px square art (styles.css:574) | dates via `CalendarSymbols::date_time` (check against today's format) |
| **Watchlist / Bookmark / Info toggles** | saved.ts:18-168; index.html dock-save SVGs | `toggle_button` (`aria-pressed`) with Portal's SVG glyphs via media-svg | glyph paths copied from index.html |
| **Info dialog**: hero, poster, facts, rating, actions, tagline, plot, people, sources, online refresh | info.ts:35-470 | `modal` (focus scope traps Tab; player keys stay quiet); close bar `El::sticky`; fanart scrim (`info_backdrop` test); `NumberSymbols::compact` for votes; `max_lines` where clamped | keeps focus and scroll on refresh (info.ts:303); a broken poster is removed (info.ts:322) |
| **Source page**: add-ons, choose source, install repo by URL or zip, repo browser + filter, install/remove, updates, Check now | main.ts:1760-2364 | `card` sections; `text_input` (`InputPurpose::Url`, `EnterKey::Go`); filter `search`; `dialog` confirms | on remote devices the "only on the PC" text (main.ts:366) |
| **Add-on settings page**: categories; header, bool, select, number, text, secret and action fields; save per field | main.ts:2377-2661; styles.css:684-760 | widgets-forms `form_field`, `switch`, `select`, `number_input`, `text_input`, `secret_input`; a `section` per category | grid `minmax(0,1fr) minmax(0,18rem)`, one column under 720 px (styles.css:690-724) via `Ui::window_size` |
| **Add-on prompts**: text (with file picker), select, yes/no, ok, progress | player.ts:2438-2685; api.ts:234, 541 | `modal`; `secret_input` for secrets; `App::file_dialog` + multipart POST `/api/incoming`; listbox for select | leaves fullscreen and closes Info first (player.ts:2580-2582); Enter submits; focus returns (player.ts:2459) |
| **Settings ("Server") page**: jump bar, Appearance, Playback, Portal (PC), Cast, Devices (PC), Subtitles, Media info, Kodi engine, Updates; remote variant | main.ts:2744-3709 (sections 3000-3008; remote 3038) | `section`s with ids; jump bar `link`s with `El::scroll_rect_into_view`; `switch`/`select` rows; 6-digit code `number_input` (`InputPurpose::Digits`, `EnterKey::Done`); API keys in `secret_input`; `language_name` labels | subtitle language limit (4, main.ts:3282) configurable |
| **Pairing screen** | pair.ts:11-25 | full-page `column`, `heading`, live `text` | code shown as "123 456"; wording kept |
| **Portal app menu** (brand on the PC) | appmenu.ts:15-218 | `menu_button` on the brand, sharing `model.rs`'s command set with the tray | one source of entries |
| **Player dock**: stage, chrome, side panel (titles, save/info, minimize, close, status, Next, Cast, Choose source, Quality, Audio, Autoplay) | index.html dock; player.ts:416-4199; stage.ts:138, 412 | media-playback `Player` + `player_with(.., PlayerParts::default()...)`, `skip_buttons(true)`, `speed_button(false)`; `Timeline` + `on_seek_outside` → `replace_source`; `hold_for_source` for quality/audio; `set_status(StageStatus)`, `offer_cancel`, `on_retry`, `fail`, `Player::empty` | metrics already matched in `media-playback/tests/portal_look.rs`; `max_lines(2)` titles (styles.css:1484-1496) |
| **Up next / autoplay** | player.ts:1955, 1965, 2705, 1263 | `NextUp::later` + `on_next_needed` → `start_next`; `next_up_lead(20 s)`; `set_next_up_delay` follows Autoplay | "Up next" upper case (styles.css:1131-1132) |
| **Theater + episodes panel** | player.ts:508-962; styles.css:1189-1244 | `Player::on_theater`; wide picture + side `drawer` with `tabs` and a list; `focus_group` | arrows on rows move the list and don't seek (player.ts:3955-3965) |
| **Mini player**: minimize pauses local playback, a cast keeps playing; expand restores theater | player.ts:573-595 | `Player::set_compact(true)` + **gap G4** (pause rule) | corner size and placement from `.dock.mini` |
| **Fullscreen overlay** | stage.ts:404 (2.6 s), styles.css:1147-1180 | built into the `player` widget | differences are improvements I2 |
| **Subtitles control** | player.ts:1336-1863; subtitles.ts:66-360 | `set_caption_choices`, `on_caption_pick`, `set_captions_loading`, `set_caption_note`, `add_subtitles_stream`, timing menu with `MenuItem::stays_open`; `preferred_locales()` replaces `browserLanguages` | **redesigned CC** (improvement I1) |
| **Cast picker and controls** | player.ts:2110-3817; api.ts:647-688 | `Player::on_cast`, `set_remote` + `RemoteStatus` via `remote_status_sender` from polling `/api/cast/device`, `set_remote_poster`, `RemoteStatus::starting_next` | "Playing on <name>" over the art scrim (`stage_cast` test); TV volume semantics |
| **OS notifications** | launcher NIF_INFO balloons | `App::notify(Notification::new(..).urgency(..))` | — |
| **Launcher tray menu** | README "Using Portal"; `model.rs:30` `Cmd`; tray.rs menu; confirms tray.rs:531-567 | `App::tray_dyn` from `Model`: Open Portal (bold default), Install update, Server ▸ (status, Allow TVs ✓, Encode on GPU ✓, Restart Kodi engine, Restart/Start server, Check for updates, Open settings, Open log folder), Start with Windows ✓, Quit Portal; confirms need **gap G6** | labels word for word from `model.rs` |
| **First run** | README; main.ts:402-406 | same texts; Android adds a "Portal address" step | — |
| **Not found / errors** | main.ts:371 | `Router` fallback | wording kept |
| **TV-browser receiver** `#/cast` | cast.ts, main.ts:353-357, `/api/cast/session*`, `/receivers`, `/presence` | **not ported** under "never cast to a TV browser"; a TV browser can still open Portal itself (spatial navigation) | Q1 |

---

## 3. Feature checklist

**P#** = phase in section 6, **G#** = engine gap in section 4, ✓ = engine already covers it.

**Launcher and server**
- [ ] Start from the shortcut: start the server, open the dashboard, add the tray icon; if running, just open the dashboard. (P13)
- [ ] Closing the dashboard doesn't stop Portal; the tray click reopens it. (P13)
- [ ] Tray menu entries as in section 2; Quit asks first, stops playing and casting, closes Kodi if Portal started it. (P13, G6)
- [ ] Watchdog: restart a stopped or unresponsive server (about a minute; three while starting or updating), at most 3 times in 10 minutes. (P13)
- [ ] `--stop`, `--quiet`, `--restart-server`, `--autostart`, `--no-browser` keep their meaning. (P13, G8)
- [ ] Update notice once in the tray; Install update downloads, checks SHA-256, closes, updates, comes back. (P11, P13)
- [ ] GPU encoding automatic with fallback; the toggle applies to the next video. (P11, P13)
- [ ] Conversion pause/resume, 20-minute abandon, 4 videos at once, disk-space limits, the player saying why. (server; P7)
- [ ] The brand opens the tray-equivalent menu on the PC and goes Home elsewhere. (P13)

**Library and browsing**
- [ ] Continue watching, Watchlist, Bookmarks shelves with progress and resume labels. (P3)
- [ ] Rename and Remove on watchlist, bookmark and history entries. (P4)
- [ ] History: play, from start, rename, remove (slot-checked), clear. (P4)
- [ ] Watchlist, Bookmark and Info on cards, details, the Info dialog and the player; an episode saves its show. (P4, P8)
- [ ] Info: add-on data first, then online lookups (TVmaze, Wikipedia, TMDB with a key); lookups can be switched off. (P4, P11)
- [ ] Browse folders, browse cache, in-folder search, run items, add-on settings links. (P5)
- [ ] Show page: seasons, episodes, aired dates, watched marks, "where you are"; season memory per device. (P6)
- [ ] Details page for movies and shows. (P6)
- [ ] Posters with letter fallback; wide and square art framed right; backdrops. (P3, P6)

**Player**
- [ ] HLS playback, resolve phases on the picture, Cancel when allowed, retry. (P7)
- [ ] Resume; progress saved every 5 s and on close. (P7)
- [ ] Seeking past what is converted: "Skipping to …", ±10 presses add up. (P7 ✓)
- [ ] Quality (Auto, Source, 4K, 1080p, 720p, 480p) and audio track switching. (P7 ✓)
- [ ] Choose source. (P7)
- [ ] Next episode, Autoplay next, Up next in the last 20 s, early next-season lookup. (P8 ✓)
- [ ] Theater with episodes panel; Minimize pauses local playback, a cast keeps playing; expand restores theater. (P8, G4)
- [ ] Fullscreen (button and double-click); Picture in picture. (P8, G5)
- [ ] Keys: Space/K, J/L and ←/→ (±10 s), ↑/↓ volume, M, C, F, T, N, Esc; working while minimized, F/T expand first. (P8 ✓)
- [ ] Volume per device; quality, subtitles, audio language and autoplay shared via `/api/prefs`. (P7, P9)
- [ ] Keep the screen awake while playing. (P8 ✓)

**Subtitles**
- [ ] CC on/off remembered; auto pick in your languages (embedded first, else best online match). (P9)
- [ ] Gestdown, TVsubtitles, Subtitle Cat (machine translations marked and ranked lower), OpenSubtitles, SubDL, SubSource with keys. (server; P9, P11)
- [ ] Menu of every subtitle with 0.5 s timing steps. (P9 ✓)
- [ ] Settings: up to 4 languages, hearing-impaired preference, keys and sign-in. (P11)

**Casting**
- [ ] Cast to a Chromecast / Cast TV; whole timeline skippable; Portal converts ahead of the TV. (P10)
- [ ] Autoplay next on the TV with the dashboard closed. (P10)
- [ ] Subtitles follow to the TV; CC changes them while casting; the next episode keeps the language. (P10)
- [ ] TV volume and mute, pause, seek, stop; "Playing on <TV>"; a lost cast reported with its position; a running cast picked up when the player opens. (P10 ✓)
- [ ] Allow TVs on this network in Settings and the tray. (P11, P13)

**Settings, devices, source**
- [ ] Theme and fonts per device, all themes. (P1, P11)
- [ ] Devices: approve a 6-digit code, list, remove; PC-only rights enforced by the server. (P2, P11)
- [ ] Remote devices see Settings without PC-only sections; Source says "only on the PC". (P11, P12)
- [ ] Kodi engine facts and Show/Hide, Restart, Close. (P11)
- [ ] Portal updates: check, install, progress, release link. (P11)
- [ ] Source: pick an add-on, install from repo URL or zip, repo browser, remove, add-on updates (hourly, Check now). (P12)
- [ ] Add-on settings pages and actions with their questions in the page. (P12)
- [ ] Add-on prompts while playing and in settings. (P7, P12)

**Everywhere**
- [ ] Landmarks, live regions, focus kept on repaint (main.ts:380-395), focus returned after dialogs. (✓)
- [ ] Reduced motion (styles.css:775, 1531, 1655). (✓)
- [ ] Hover-only affordances for fine pointers. (✓)
- [ ] Layouts at 720 px and 1600 px (styles.css:720, 1590, 1628, 1822, 2142). (`Ui::window_size`)

---

## 4. Remaining engine gaps

These get fixed in RemedyUI before (or alongside) the phase that needs them.

- **G1 (blocks the web build on LAN devices): web playback without WebCodecs.** media-playback decodes on the web only through WebCodecs (`os/web.rs`, `web_hls.rs`), which browsers allow only in secure contexts; Portal on a phone or TV is plain `http://<lan-ip>`. Older TV browsers lack WebCodecs too. Proposal: a Media Source Extensions backend (remux TS to fMP4 with the crate's own demuxer, feed a `SourceBuffer` on an offscreen `<video>`, upload frames as a texture each `requestVideoFrameCallback`, sound and clock from the element), chosen automatically when `!isSecureContext` or no `VideoDecoder`, plus `PlayerOptions::web_backend(Auto | WebCodecs | MediaSource)`. Also check the web bridge with no `navigator.gpu` (WebGL2 path). Test: headless browser at a non-localhost `http://` address playing Portal's test HLS.
- **G2 (blocks native remote devices): a persistent, protected cookie jar.** `services-net` `CookieJar` can't save or load, so the pairing key would be lost on every restart. Proposal: `CookieJar::persist(SecretStore)` plus `SecretStore` (DPAPI on Windows, Android Keystore, Secret Service on Linux); unused in a page.
- **G3: images through the app's HTTP client.** media-image's default fetcher is loopback-only with no cookies (`remote.rs:630-642`), so `/api/image` over the LAN fails natively. Proposal: `media_image::fetch_through(http: Http)` reusing a services-net client's policy, cookies and timeouts (or a process-wide default client).
- **G4: the mini player pauses local playback.** `Player::set_compact` keeps playing; Portal pauses unless casting (player.ts:594-595). Proposal: `PlayerOptions::compact_pauses(true)`; expanding doesn't auto-resume.
- **G5: picture in picture.** Portal has a PiP button (player.ts:2172); the engine has none. Proposal: `Player::request_pip()` / `pip_available()`: Document PiP or `<video>` PiP on the web, `enterPictureInPictureMode` on Android, an always-on-top borderless window on desktop.
- **G6: a confirm dialog from a tray-only app.** The launcher asks via `MessageBoxW` with no window open (ui.rs:35-62). Proposal: `App::confirm(ConfirmSpec{title, text, ok, cancel, danger}) -> async bool` (Windows task dialog, topmost; engine-drawn elsewhere).
- **G7 (optional): LAN service discovery** so a phone finds Portal without typing an address. Proposal: `net::discover("_portal._tcp")` (NsdManager on Android, DnsServiceBrowse on Windows) plus the server advertising it. Not lost functionality, so an improvement.
- **G8: a second launch that waits for the first.** The installer runs `--stop --quiet` and waits for Portal to be gone; the engine's single instance hands over and exits at once. Proposal: `on_second_instance` returns a reply code; `AppConfig::single_instance_reply(timeout)` waits for it (and optionally for the first process to exit) and exits with it.

Checked and covered by existing APIs: breakpoints (`Ui::window_size`), hover/fine pointers (`pointer_caps`), `:has()` (signals), `tabular-nums`, `line-clamp`, `text-transform`, sticky blurred header, scroll snap, fluid grids, deep links, gamepads and spatial navigation, notification urgency, Run-key autostart, HLS on Windows and Android, subtitles, next-up, `Timeline`, remote mode, keep-awake, media keys, file pickers. To verify: `color-mix` in sRGB (the engine has `mix_oklab`; a `Color::mix_srgb` would be a tiny gap).

---

## 5. Improvements (deliberate differences)

| # | Change | Why it is better |
|---|---|---|
| I1 | CC is one button that states its state in words ("Off", "Auto · English", "Loading…"), details in words instead of "HI"; pick and timing carry to the next episode | Fixes the CC flaws: a toggle beside a tiny arrow, state shown only by a pulse, timing forgotten every episode |
| I2 | Faded fullscreen controls stay in the accessibility tree and tab order; a press on faded controls only wakes them | Today a tap presses things that can't be seen |
| I3 | Typed client from `portal-api` | Client and server can't drift; JSON mistakes become compile errors |
| I4 | `/api/image/web` proxy with caching | Cross-site posters load even without CORS headers; SSRF-checked |
| I5 | Native disk image cache (256 MiB, configurable) | Shelves appear instantly after a restart |
| I6 | Windows media controls / browser `mediaSession` | Media keys and system overlay controls, which Portal never had |
| I7 | Themed confirms (optional, Q3) | Match the app instead of a grey MessageBox |
| I8 | QR code of the server address on the pairing screen | Faster setup of another phone |
| I9 | Server-pushed status (SSE) instead of 1–4 s polling (optional) | Less traffic and battery on phones; needs a server route |
| I10 | Playback speed, off by default | New feature without changing today's look |
| I11 | Android: discovered server (G7), back key through the Router, TV d-pad | First run without typing an IP |

---

## 6. Phases

Each phase is one PR (or a few) and leaves master shippable; the old UI keeps serving `/` until P15.

**Parity harness (built in P0, used in every phase)**
- **Visual**: the old UI in a WebDriver browser at `http://localhost:8490/#/<route>` (theme and fonts set in localStorage) and the port via `Window::snapshot_png`, at 1280×800, 1920×1080 and 390×844; all themes for the shell, Portal/Light/Azure/Skyline/Lagoon for content pages; pixel diff (start at 5 levels, text regions masked and reviewed by eye); side-by-side PNGs for review.
- **Behaviour**: a recording reverse proxy on a second port (keeps Host, adds no forwarded headers so `this_pc` holds) logs method, path, normalised body and status; the same scripted scenario runs on both UIs and the traces must match except for differences listed in section 5.

**Real-content test protocol (any phase touching the library or playback)**
1. Back up `%LOCALAPPDATA%\Portal\library.json` (and `prefs.json`) before anything runs.
2. `GET /api/history`; use only items from it; record each touched item's id, kind, name, season, episode, file, position and duration.
3. Never call `DELETE /api/history`; toggle watchlist/bookmarks only on history items and toggle them back.
4. Restore positions with `POST /api/progress` for every touched item.
5. Check `GET /api/history` against the backup; if anything differs, quit Portal and copy `library.json` back.
6. Cast tests only to the device whose name is exactly "The  Ris" (two spaces) **and** host is 192.168.0.9; abort if none matches; stop the cast at the end.

| Phase | Slice | Tests |
|---|---|---|
| **P0** | Wire types into `portal-api`; `crates/app` skeleton building for Windows, wasm32 and Android; S1 image proxy, S2 wasm compression, S3 `/next/`; parity harness; backup/restore script | serde round trip of recorded JSON for every route; proxy SSRF tests (private IP, redirect to private, non-image, oversize); harness smoke run |
| **P1** | Shell: themes, fonts, background, sticky header, nav, theme picker, chip, update pill, notices, activity panel, Router with every route, skip link, reduced motion | visual: header in all themes × 3 sizes; chip polling trace; deep links open their routes |
| **P2** | Connection and pairing: PC or remote mode, pairing screen, native server address; **G2** | pair-flow trace; restart and still paired; removing the device brings pairing back |
| **P3** | Home shelves and cards, image pipeline (letter fallback, framing, **G3**, web proxy), snap and focus groups | visual of Home with real history; `/api/home`, `/api/source`, `/api/saved` traces equal |
| **P4** | Watchlist, Bookmarks, History pages; rename/remove; saved toggles; Info dialog | toggles and renames restored afterwards; Info screenshots (movie, show, episode); focus trap and Esc |
| **P5** | Browse, search, browse cache, run items, settings links | grid columns match CSS at 3 widths; search abort trace; cache revalidate |
| **P6** | Show page and Details page | hero and season tabs visual; season memory persists |
| **P7** | Player core: start, resolve phases, prompts during play, Choose source, HLS, Timeline seeks, progress saves, heartbeat, quality/audio, retry | play 2 history items, positions restored; `/api/play*` and `/api/progress` traces equal; server killed mid-play shows retry |
| **P8** | Up next and autoplay, theater + episodes panel, mini player (**G4**), fullscreen, keys, keep-awake, PiP (**G5**) | key table test; minimize pauses local playback; theater and mini visuals |
| **P9** | Subtitles: stream, online search, auto pick, timing, prefs sync | `/api/subtitles/*` traces; pick carries to the next episode; ranking re-checked against subtitles.ts:225-315 |
| **P10** | Casting: picker, play, stop, pause, seek, volume, subtitles, remote mode, lost cast, resume a running cast | only "The  Ris"; `/api/cast/devices/{id}/*` traces equal; app closed mid-cast, the TV keeps playing and autoplays |
| **P11** | Settings page, every section, and the remote variant | trace per toggle, each restored; visuals at 3 sizes |
| **P12** | Source page, repositories, add-on updates, add-on settings, actions, prompts, file upload | a throwaway test repo/add-on only; never remove the user's add-ons |
| **P13** | Desktop app: tray from `model.rs`, watchdog, notifications, Run-key autostart, single instance + CLI flags (**G6**, **G8**), brand menu; installer ships the new exe | installer `--stop --quiet` exit-code test; tray menu read headless; watchdog restart test |
| **P14** | Android: package, server address (G7), pairing, Android TV navigation and back key, keep-awake, HLS | emulator and a real phone on the LAN: P3–P10 smoke with the real-content protocol |
| **P15** | Web cutover with **G1** in place: swap `/` and `/next/`, import old localStorage, then delete the old web UI (and the `#/cast` routes per Q1) | phone and TV browser on `http://<lan-ip>` play HLS; full parity suite on all three targets |

---

## 7. Open questions and risks

**Questions**
- **Q1**: the TV-browser receiver (`#/cast`, `cast.ts`, `/api/cast/session*`, `/receivers`, `/presence`) is casting to a TV browser, which the rules exclude, but dropping it removes a feature. Recommendation: don't port it; remove its routes after cutover; a TV browser can still open Portal itself.
- **Q2**: "Open Portal" in the tray: the native window (recommended), the browser, or both ("Open in browser" as an extra entry)?
- **Q3**: confirms (G6): native MessageBox look (exact parity) or themed dialogs (I7)?
- **Q4**: Android distribution and updates: GitHub release APK plus `install_apk`? Signed with which key?
- **Q5**: pixel-parity tolerance: text is rasterised by the engine, not Chrome, so glyph edges differ; agree a threshold and review text by eye.
- **Q6**: web UI on old TV browsers needs WebGL2 or the CPU renderer; speed unknown until measured in P15.
- **Q7**: link RemedyUI as a git dependency pinned to a revision, or a path dependency during the port?

**Risks**
- **R1**: G1 is the largest engine job and gates the web build on LAN devices; start it alongside P1.
- **R2**: wasm size on phones (engine, fonts, about 3 MB of language names). Turn `language-names` off for the web build if Portal's languages are covered by the built-in set (check).
- **R3**: copied AAC can be HE-AAC or 5.1 (playback.rs:1499); test on Media Foundation, MediaCodec and WebCodecs/MSE.
- **R4**: the pairing cookie is host-only; if the PC's IP changes, native devices pair again (as browsers do today). Consider pairing against a hostname.
- **R5**: a service worker could keep a stale wasm after an update; tie it to the server version or turn it off for LAN origins.
- **R6**: restoring via `POST /api/progress` can change history order, `at` and `finished`; the `library.json` backup is the real safety net.
- **R7**: moving `json!` responses into typed structs in P0 could change field order or nulls; the serde fixtures catch it first.

Not yet read line by line: `stage.ts`, most of `cast.ts`, `subtitles.ts` ranking and the server's subtitle and metadata modules; P9 and P10 re-check them.
