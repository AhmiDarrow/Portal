#!/usr/bin/env python3
"""Chromecast / Vizio Cast helper for Portal.

Speaks the Default Media Receiver through pychromecast. No Cast developer
app id. One JSON object on stdout. Portal passes commands as argv.

Commands: discover, play, stop, pause, resume, seek, subtitles, volume,
status, watch.
`status` prints the device's media status once. `watch` keeps one connection
open and prints a status line every --interval seconds until it is stopped,
so Portal can tell when an item has finished without starting Python again
for every look.
PORTAL_CAST_HOSTS is an optional comma-separated list of extra hosts to probe.
"""

from __future__ import annotations

import argparse
import json
import logging
import math
import os
import socket
import sys
import threading
import time
import uuid
from typing import Any, Callable

# Portal waits 45 seconds for a control command (CONTROL_LIMIT). Connecting
# and starting playback share this much of it, so the answer arrives in time.
OVERALL_LIMIT = 38.0
# Looking for a device that moved; it ends early once the device is found.
DISCOVERY_LIMIT = 8.0


def emit(payload: dict[str, Any], code: int = 0) -> None:
    sys.stdout.write(json.dumps(payload, ensure_ascii=False) + "\n")
    sys.stdout.flush()
    raise SystemExit(code)


def line(payload: dict[str, Any]) -> None:
    """One JSON line that does not end the process (for watch)."""
    sys.stdout.write(json.dumps(payload, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def fail(message: str, code: str = "unavailable") -> None:
    text = " ".join(message.split()).strip() or "Cast failed."
    emit({"ok": False, "error": text[:300], "code": code}, 1)


def within(deadline: float, limit: float) -> float:
    """`limit` seconds, or what is left before `deadline` if that is less."""
    wait = min(limit, deadline - time.monotonic())
    if wait < 1:
        fail("The Cast device did not answer in time.", "unavailable")
    return wait


def known_hosts() -> list[str]:
    raw = os.environ.get("PORTAL_CAST_HOSTS", "")
    hosts: list[str] = []
    for part in raw.split(","):
        host = part.strip()
        if host and host not in hosts:
            hosts.append(host)
    return hosts


def content_type_for(url: str) -> str:
    path = url.split("?", 1)[0].split("#", 1)[0].lower()
    if path.endswith(".m3u8"):
        return "application/vnd.apple.mpegurl"
    if path.endswith(".mp4") or path.endswith(".m4v"):
        return "video/mp4"
    if path.endswith(".webm"):
        return "video/webm"
    if path.endswith(".mp3"):
        return "audio/mpeg"
    return "application/vnd.apple.mpegurl"


def stream_type_for(url: str) -> str:
    # BUFFERED even for event HLS that is still growing: the receiver reloads
    # the playlist either way, but LIVE starts at the newest segment.
    return "BUFFERED"


def parse_uuid(value: str | None) -> uuid.UUID | None:
    if not value:
        return None
    try:
        return uuid.UUID(value.strip())
    except ValueError:
        return None


def device_row(info: Any) -> dict[str, Any]:
    name = (getattr(info, "friendly_name", None) or "").strip() or "Cast device"
    model = (getattr(info, "model_name", None) or "").strip()
    host = (getattr(info, "host", None) or "").strip()
    port = int(getattr(info, "port", None) or 8009)
    return {
        "id": str(info.uuid),
        "name": name,
        "host": host,
        "model": model,
        "port": port,
    }


def discover_infos(
    timeout: float,
    hosts: list[str] | None,
    wanted: Callable[[list[Any]], bool] | None = None,
) -> list[Any]:
    """Devices found within `timeout` seconds, or as soon as `wanted` says they will do."""
    import zeroconf
    from pychromecast.discovery import CastBrowser, SimpleCastListener

    done = threading.Event()
    browser: Any = None

    def seen(_uuid: Any, _service: str) -> None:
        if wanted is not None and browser is not None and wanted(list(browser.devices.values())):
            done.set()

    browser = CastBrowser(
        SimpleCastListener(seen, update_callback=seen),
        zeroconf.Zeroconf(),
        hosts or None,
    )
    browser.start_discovery()
    try:
        done.wait(timeout)
        return list(browser.devices.values())
    finally:
        # Stopping joins the host browser, which can spend half a minute on a
        # known host that no longer answers. It is a daemon thread, so stop it
        # without waiting.
        threading.Thread(target=stop_browser, args=(browser,), daemon=True).start()


def stop_browser(browser: Any) -> None:
    try:
        browser.stop_discovery()
    except Exception:
        pass


def discover(timeout: float) -> None:
    try:
        infos = discover_infos(timeout, known_hosts())
    except Exception as exc:
        fail(import_or_runtime(exc))
    rows: list[dict[str, Any]] = []
    seen: set[str] = set()
    for info in infos:
        if getattr(info, "uuid", None) is None:
            continue
        row = device_row(info)
        if not row["id"] or row["id"] in seen:
            continue
        seen.add(row["id"])
        rows.append(row)
    rows.sort(key=lambda row: row["name"].casefold())
    emit({"ok": True, "items": rows})


def pick(infos: list[Any], device_id: str, host: str | None, uid: uuid.UUID | None) -> Any | None:
    wanted = device_id.strip()
    for info in infos:
        if uid is not None and info.uuid == uid:
            return info
    # With a known id, a device at the old address with another id is another TV.
    if host and uid is None:
        for info in infos:
            if (info.host or "") == host:
                return info
    for info in infos:
        if str(info.uuid) == wanted:
            return info
        name = (info.friendly_name or "").strip()
        if name and name == wanted:
            return info
    return None


def optional_text(value: str | None) -> str | None:
    text = (value or "").strip()
    return text or None


def answers(host: str, port: int, timeout: float) -> bool:
    """Whether anything accepts a connection at the address."""
    try:
        with socket.create_connection((host, port or 8009), timeout=timeout):
            return True
    except OSError:
        return False


def same_device(host: str, port: int, uid: uuid.UUID, timeout: float) -> bool:
    """Whether the Cast device answering at `host` is the one with `uid`.

    The router can give a remembered address to another TV. A device that
    does not say its id is trusted, as before.
    """
    if (port or 8009) != 8009:
        # A speaker group answers on its leader's address, which reports the
        # leader's own id.
        return True
    try:
        from pychromecast import dial

        info = dial.get_device_info(host, timeout=timeout)
    except Exception:
        return True
    found = getattr(info, "uuid", None) if info is not None else None
    return found is None or found == uid


def connect_host(
    host: str,
    port: int,
    uid: uuid.UUID,
    timeout: float,
    model: str | None = None,
    friendly: str | None = None,
) -> Any | None:
    import pychromecast

    cast = pychromecast.get_chromecast_from_host(
        (host, port or 8009, uid, optional_text(model), optional_text(friendly)),
        tries=2,
        retry_wait=1,
        timeout=min(timeout, 10),
    )
    try:
        cast.wait(timeout=min(timeout, 10))
        return cast
    except Exception:
        try:
            cast.disconnect(timeout=1)
        except Exception:
            pass
        return None


def open_cast(
    device_id: str,
    host: str | None,
    port: int,
    timeout: float,
    deadline: float,
    model: str | None = None,
    friendly: str | None = None,
) -> Any:
    uid = parse_uuid(device_id)
    # pychromecast asks an address for its device type with a 30 second
    # timeout, so an address the TV no longer has is checked quickly first.
    # Another device answering there sends the search below looking for it.
    if (
        host
        and uid is not None
        and answers(host, port, min(3.0, within(deadline, timeout)))
        and same_device(host, port, uid, min(4.0, within(deadline, timeout)))
    ):
        cast = connect_host(host, port, uid, within(deadline, timeout), model, friendly)
        if cast is not None:
            return cast
    hosts = known_hosts()
    if host and host not in hosts:
        hosts = [host, *hosts]
    try:
        # With a known id, only the id ends the scan early: the TV's old
        # address may belong to another device now.
        infos = discover_infos(
            within(deadline, min(timeout, DISCOVERY_LIMIT)),
            hosts,
            lambda found: (
                any(info.uuid == uid for info in found)
                if uid is not None
                else pick(found, device_id, host, uid) is not None
            ),
        )
    except Exception as exc:
        fail(import_or_runtime(exc), "not_found")
    match = pick(infos, device_id, host, uid)
    if match is None:
        fail("That Cast device is not on the network.", "not_found")
    import pychromecast

    wait = within(deadline, min(timeout, 10))
    cast = pychromecast.get_chromecast_from_host(
        (
            match.host,
            int(match.port or 8009),
            match.uuid,
            match.model_name,
            match.friendly_name,
        ),
        tries=2,
        retry_wait=1,
        timeout=wait,
    )
    try:
        cast.wait(timeout=wait)
    except Exception:
        try:
            cast.disconnect(timeout=1)
        except Exception:
            pass
        fail("The Cast device did not answer in time.", "unavailable")
    return cast


def close_cast(cast: Any) -> None:
    """Drops the connection without saying goodbye to the receiver.

    Some TVs close the receiver, and the video with it, when the last sender
    connected to it says goodbye. One that just goes away leaves it playing.
    """
    client = getattr(cast, "socket_client", None)
    lock = getattr(client, "_socket_lock", None)
    channels = getattr(client, "_open_channels", None)
    if lock is not None and isinstance(channels, list):
        try:
            # The goodbye goes to each channel on this list.
            with lock:
                channels.clear()
        except Exception:
            pass
    try:
        cast.disconnect(timeout=2)
    except Exception:
        pass


def import_or_runtime(exc: BaseException) -> str:
    text = str(exc).strip()
    name = exc.__class__.__name__
    if name in {"ModuleNotFoundError", "ImportError"} or "pychromecast" in text:
        return "Cast support is not installed."
    if not text or "Traceback" in text:
        return "The Cast device did not answer."
    return text


def media_controller(cast: Any, timeout: float) -> Any:
    mc = cast.media_controller
    # Monotonic, like every other wait here: a clock set back would stretch it.
    deadline = time.monotonic() + min(max(timeout, 1), 8)
    while time.monotonic() < deadline:
        status = mc.status
        if status is not None and status.media_session_id is not None:
            return mc
        time.sleep(0.2)
    fail("Nothing is playing on that TV.", "bad_request")
    return mc


def media_path(url: str) -> str:
    return str(url).split("?", 1)[0].split("#", 1)[0]


def same_media(content_id: str | None, url: str) -> bool:
    if not content_id or not url:
        return False
    return media_path(content_id) == media_path(url)


def player_state_of(status: Any) -> str:
    state = getattr(status, "player_state", None)
    if state:
        return str(state).strip().upper()
    if getattr(status, "player_is_playing", False):
        return "PLAYING"
    return ""


def cast_has_started(status: Any, url: str, previous_session: Any) -> bool:
    """PLAYING or BUFFERING for this load. A session id by itself is not enough."""
    if status is None or player_state_of(status) not in {"PLAYING", "BUFFERING"}:
        return False
    # A longer file of the same title has the same path, so only the whole
    # address tells this load from the one before it.
    if str(getattr(status, "content_id", None) or "") == url:
        return True
    session_id = getattr(status, "media_session_id", None)
    return session_id is not None and session_id != previous_session


def cast_rejected(status: Any, url: str) -> bool:
    if status is None or player_state_of(status) != "IDLE":
        return False
    reason = str(getattr(status, "idle_reason", None) or "").strip().upper()
    if reason not in {"ERROR", "CANCELLED"}:
        return False
    return same_media(getattr(status, "content_id", None), url)


def do_play(
    cast: Any,
    url: str,
    title: str | None,
    thumb: str | None,
    start: float | None,
    timeout: float,
    subtitles: tuple[str, str] | None = None,
) -> None:
    mc = cast.media_controller
    status = getattr(mc, "status", None)
    previous = getattr(status, "media_session_id", None) if status is not None else None
    mc.play_media(
        url,
        content_type_for(url),
        title=title or None,
        thumb=thumb or None,
        current_time=start,
        autoplay=True,
        stream_type=stream_type_for(url),
        **subtitle_args(subtitles),
    )
    # block_until_active returns for any existing session, including the previous one.
    deadline = time.monotonic() + max(timeout, 1.0)
    while time.monotonic() < deadline:
        status = getattr(mc, "status", None)
        if cast_rejected(status, url):
            fail("The TV did not start playback.", "unavailable")
        if cast_has_started(status, url, previous):
            emit({"ok": True})
        try:
            mc.update_status()
        except Exception:
            pass
        time.sleep(0.25)
    fail("The TV did not start playback.", "unavailable")


def do_stop(cast: Any) -> None:
    mc = cast.media_controller
    try:
        status = mc.status
        if status is not None and status.media_session_id is not None:
            mc.stop()
    except Exception:
        pass
    try:
        cast.quit_app()
    except Exception:
        pass
    emit({"ok": True})


def do_pause(cast: Any, timeout: float) -> None:
    media_controller(cast, timeout).pause()
    emit({"ok": True})


def do_resume(cast: Any, timeout: float) -> None:
    media_controller(cast, timeout).play()
    emit({"ok": True})


def subtitle_args(subtitles: tuple[str, str] | None) -> dict[str, Any]:
    """play_media's side-loaded WebVTT track, shown at once, or nothing."""
    if not subtitles:
        return {}
    url, language = subtitles
    return {
        "subtitles": url,
        "subtitles_lang": language or "und",
        "subtitles_mime": "text/vtt",
        "subtitle_id": 1,
    }


def active_subtitles(status: Any) -> tuple[str, str] | None:
    """The text track the receiver shows now, as (address, language)."""
    active = set(getattr(status, "current_subtitle_tracks", None) or [])
    for track in getattr(status, "subtitle_tracks", None) or []:
        if not isinstance(track, dict) or track.get("trackId") not in active:
            continue
        address = str(track.get("trackContentId") or "")
        if address:
            return address, str(track.get("language") or "und")
    return None


def reload_at(mc: Any, status: Any, url: str, seconds: float, subtitles: tuple[str, str] | None) -> None:
    """Load what is playing again at `seconds`, with `subtitles` as its text track."""
    images = getattr(status, "images", None) or []
    mc.play_media(
        url,
        content_type_for(url),
        title=getattr(status, "title", None) or None,
        thumb=getattr(images[0], "url", None) if images else None,
        current_time=seconds,
        autoplay=player_state_of(status) != "PAUSED",
        stream_type=stream_type_for(url),
        **subtitle_args(subtitles),
    )


def do_subtitles(cast: Any, subtitles: tuple[str, str] | None, timeout: float) -> None:
    """Shows `subtitles` on what is playing, or none. A text track can only
    be added with a load, so a new one reloads at the current time; turning
    them off just hides the track."""
    mc = media_controller(cast, timeout)
    status = getattr(mc, "status", None)
    url = str(getattr(status, "content_id", None) or "")
    if not url:
        # The first status after connecting can name the session and not
        # what it plays. Asked directly, the TV says.
        refresh_status(cast, 3.0)
        status = getattr(mc, "status", None)
        url = str(getattr(status, "content_id", None) or "")
    if not url:
        fail("Nothing is playing on the TV.", "not_found")
    if subtitles is None:
        if getattr(status, "current_subtitle_tracks", None):
            mc.disable_subtitle()
        emit({"ok": True})
    current = active_subtitles(status)
    if current is not None and media_path(current[0]) == media_path(subtitles[0]):
        emit({"ok": True})
    # A track the receiver already has (switched off earlier) only needs showing.
    for track in getattr(status, "subtitle_tracks", None) or []:
        if isinstance(track, dict) and str(track.get("trackContentId") or "") == subtitles[0]:
            mc.enable_subtitle(track.get("trackId"))
            emit({"ok": True})
    here = finite(getattr(status, "adjusted_current_time", None)) or 0.0
    reload_at(mc, status, url, here, subtitles)
    emit({"ok": True})


def do_seek(cast: Any, seconds: float, timeout: float) -> None:
    mc = media_controller(cast, timeout)
    status = getattr(mc, "status", None)
    url = str(getattr(status, "content_id", None) or "")
    growing = finite(getattr(status, "duration", None)) is None
    if url and growing and media_path(url).lower().endswith(".m3u8"):
        # A playlist loaded while still growing plays as live and ignores
        # seeks, so load it again at the new time.
        reload_at(mc, status, url, seconds, active_subtitles(status))
    else:
        paused = player_state_of(status) == "PAUSED"
        mc.seek(seconds)
        if paused:
            # A seek always asks the receiver to resume, so pause it again.
            mc.pause()
    emit({"ok": True})


def do_volume(cast: Any, level: float | None, muted: bool | None) -> None:
    """Sets the TV's own volume (0 to 1), its mute, or both."""
    if level is None and muted is None:
        fail("A volume or mute setting is required.", "bad_request")
    if level is not None:
        cast.set_volume(min(max(level, 0.0), 1.0))
    if muted is not None:
        cast.set_volume_muted(muted)
    emit({"ok": True})


def subtitles_from(args: Any) -> tuple[str, str] | None:
    url = (args.subtitles or "").strip()
    if not url:
        return None
    if not url.lower().startswith(("http://", "https://")):
        fail("A subtitle address must be a web address.", "bad_request")
    return url, (args.subtitles_lang or "").strip() or "und"


def finite(value: Any) -> float | None:
    try:
        number = float(value)
    except (TypeError, ValueError):
        return None
    if number != number or number in (float("inf"), float("-inf")) or number < 0:
        return None
    return round(number, 3)


def status_row(cast: Any) -> dict[str, Any]:
    """The media status Portal watches: state, why it went idle, time, what
    is loaded, and the TV's own volume."""
    app = getattr(cast, "app_id", None)
    # The receiver's status arrives by itself when the volume changes.
    receiver = getattr(cast, "status", None)
    muted = getattr(receiver, "volume_muted", None)
    row: dict[str, Any] = {
        "ok": True,
        "app": str(app) if app else None,
        "state": "",
        "idle_reason": None,
        "current_time": None,
        "duration": None,
        "content_id": None,
        "volume": finite(getattr(receiver, "volume_level", None)),
        "muted": muted if isinstance(muted, bool) else None,
    }
    status = getattr(cast.media_controller, "status", None)
    if status is None:
        return row
    state = player_state_of(status)
    row["state"] = "" if state == "UNKNOWN" else state
    reason = getattr(status, "idle_reason", None)
    row["idle_reason"] = (str(reason).strip().upper() or None) if reason else None
    current = getattr(status, "adjusted_current_time", None)
    if current is None:
        current = getattr(status, "current_time", None)
    row["current_time"] = finite(current)
    row["duration"] = finite(getattr(status, "duration", None))
    content = getattr(status, "content_id", None)
    row["content_id"] = str(content) if content else None
    return row


def refresh_status(cast: Any, wait: float) -> None:
    """Asks the receiver for its media status and waits briefly for the answer."""
    if not getattr(cast, "app_id", None):
        return
    mc = cast.media_controller
    before = getattr(getattr(mc, "status", None), "last_updated", None)
    try:
        mc.update_status()
    except Exception:
        return
    deadline = time.monotonic() + max(wait, 0.1)
    while time.monotonic() < deadline:
        status = getattr(mc, "status", None)
        stamp = getattr(status, "last_updated", None)
        if status is not None and stamp is not None and stamp != before:
            return
        time.sleep(0.1)


def do_status(cast: Any) -> None:
    refresh_status(cast, 3.0)
    emit(status_row(cast))


def connected(cast: Any) -> bool:
    client = getattr(cast, "socket_client", None)
    if client is None:
        return True
    return bool(getattr(client, "is_connected", True))


class StateChange:
    """Wakes the watch when the TV's player state changes, so Portal hears
    "playing" or "finished" at once instead of at the next interval."""

    def __init__(self) -> None:
        self.event = threading.Event()
        self.state: str | None = None

    def new_media_status(self, status: Any) -> None:
        state = player_state_of(status)
        if state != self.state:
            self.state = state
            self.event.set()

    def load_media_failed(self, _item: Any, _code: Any) -> None:
        self.event.set()


def do_watch(cast: Any, interval: float, lifetime: float) -> None:
    """A status line every `interval` seconds, and one as soon as the TV's
    state changes. Ends when the TV drops off (Portal starts it again)."""
    started = time.monotonic()
    lost_since: float | None = None
    change = StateChange()
    try:
        cast.media_controller.register_status_listener(change)
    except Exception:
        pass
    while True:
        if connected(cast):
            lost_since = None
            refresh_status(cast, min(3.0, interval))
            # A change already in this line does not wake the next one.
            change.event.clear()
            line(status_row(cast))
        else:
            now = time.monotonic()
            lost_since = lost_since or now
            line({"ok": False, "error": "The Cast device is not answering.", "code": "unavailable"})
            if now - lost_since > 30:
                raise SystemExit(1)
        if lifetime and time.monotonic() - started > lifetime:
            raise SystemExit(0)
        # A TV that flips state quickly still gets a line at most once a
        # second, so Portal's line counts keep about their length.
        if change.event.wait(interval):
            time.sleep(min(1.0, interval))


class Parser(argparse.ArgumentParser):
    """Reports bad arguments as the helper's JSON error instead of usage text."""

    def error(self, message: str) -> None:  # type: ignore[override]
        fail(message, "bad_request")


def finite_number(text: str) -> float:
    number = float(text)
    if not math.isfinite(number):
        raise argparse.ArgumentTypeError(f"not a finite number: {text!r}")
    return number


def build_parser() -> argparse.ArgumentParser:
    parser = Parser(description="Portal Chromecast helper")
    parser.add_argument(
        "command",
        choices=(
            "discover",
            "play",
            "stop",
            "pause",
            "resume",
            "seek",
            "subtitles",
            "volume",
            "status",
            "watch",
        ),
    )
    parser.add_argument("--id", default="")
    parser.add_argument("--host", default="")
    parser.add_argument("--port", type=int, default=8009)
    parser.add_argument("--model", default="")
    parser.add_argument("--name", default="")
    parser.add_argument("--url", default="")
    parser.add_argument("--title", default="")
    parser.add_argument("--thumb", default="")
    parser.add_argument("--subtitles", default="")
    parser.add_argument("--subtitles-lang", default="")
    parser.add_argument("--seconds", type=finite_number, default=-1)
    parser.add_argument("--level", type=finite_number, default=None)
    parser.add_argument("--muted", choices=("true", "false"), default=None)
    parser.add_argument("--timeout", type=float, default=0)
    parser.add_argument("--interval", type=float, default=4)
    parser.add_argument("--lifetime", type=float, default=0)
    return parser


def main(argv: list[str] | None = None) -> None:
    logging.disable(logging.CRITICAL)
    if hasattr(sys.stdout, "reconfigure"):
        try:
            sys.stdout.reconfigure(encoding="utf-8")
        except Exception:
            pass
    args = build_parser().parse_args(argv)
    timeout = args.timeout if args.timeout and args.timeout > 0 else 0
    try:
        if args.command == "discover":
            discover(timeout or 8)
        device_id = args.id.strip()
        if not device_id:
            fail("A Cast device is required.", "bad_request")
        host = args.host.strip() or None
        port = args.port if args.port and args.port > 0 else 8009
        limit = timeout or 20
        deadline = time.monotonic() + OVERALL_LIMIT
        cast = open_cast(
            device_id,
            host,
            port,
            limit,
            deadline,
            optional_text(args.model),
            optional_text(args.name),
        )
        try:
            if args.command == "play":
                url = args.url.strip()
                if not url:
                    fail("A video address is required.", "bad_request")
                start = args.seconds if args.seconds >= 0 else None
                wait = within(deadline, limit)
                do_play(
                    cast,
                    url,
                    args.title.strip() or None,
                    args.thumb.strip() or None,
                    start,
                    wait,
                    subtitles_from(args),
                )
            elif args.command == "stop":
                do_stop(cast)
            elif args.command == "pause":
                do_pause(cast, limit)
            elif args.command == "resume":
                do_resume(cast, limit)
            elif args.command == "status":
                do_status(cast)
            elif args.command == "watch":
                interval = min(max(args.interval, 1.0), 60.0)
                lifetime = max(args.lifetime, 0.0)
                do_watch(cast, interval, lifetime)
            elif args.command == "subtitles":
                do_subtitles(cast, subtitles_from(args), limit)
            elif args.command == "volume":
                muted = None if args.muted is None else args.muted == "true"
                do_volume(cast, args.level, muted)
            elif args.command == "seek":
                if args.seconds < 0 or args.seconds != args.seconds:
                    fail("A seek time is required.", "bad_request")
                do_seek(cast, args.seconds, limit)
            else:
                fail("Unknown cast command.", "bad_request")
        finally:
            close_cast(cast)
    except SystemExit:
        raise
    except Exception as exc:
        fail(import_or_runtime(exc))


if __name__ == "__main__":
    main()
