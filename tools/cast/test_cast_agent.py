"""The watch's reaction to TV state changes, in both one-shot and serving mode, and a
seek the TV answers late.

Run: python -m unittest discover -s tools/cast
"""

import os
import sys
import threading
import time
import unittest
from types import SimpleNamespace

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import cast_agent  # noqa: E402


class FakeMedia:
    def __init__(self) -> None:
        self.status = SimpleNamespace(player_state="BUFFERING")
        self._status_listeners: list = []

    def register_status_listener(self, listener) -> None:
        self._status_listeners.append(listener)

    def tell(self, state: str) -> None:
        self.status = SimpleNamespace(player_state=state)
        for listener in list(self._status_listeners):
            listener.new_media_status(self.status)


class WatchTest(unittest.TestCase):
    def setUp(self) -> None:
        self.cast = SimpleNamespace(media_controller=FakeMedia(), app_id=None, socket_client=None)
        self.lines: list[tuple[float, str]] = []
        self.old_line = cast_agent.line
        cast_agent.line = lambda row: self.lines.append((time.monotonic(), row.get("state")))

    def tearDown(self) -> None:
        cast_agent.line = self.old_line

    def watch(self, cancel: threading.Event | None, lifetime: float) -> threading.Thread:
        def run() -> None:
            cast_agent._CTX.cancel = cancel
            try:
                cast_agent.do_watch(self.cast, 30.0, lifetime)
            except SystemExit:
                pass

        thread = threading.Thread(target=run, daemon=True)
        thread.start()
        return thread

    def wait_lines(self, count: int) -> None:
        deadline = time.monotonic() + 5
        while len(self.lines) < count and time.monotonic() < deadline:
            time.sleep(0.02)

    def check_reacts(self, cancel: threading.Event | None) -> threading.Thread:
        thread = self.watch(cancel, 0)
        self.wait_lines(1)
        start = time.monotonic()
        self.cast.media_controller.tell("PLAYING")
        self.wait_lines(2)
        self.assertEqual(self.lines[1][1], "PLAYING")
        # Well before the 30 second interval, after the one second spacing.
        self.assertLess(self.lines[1][0] - start, 3)
        self.assertGreaterEqual(self.lines[1][0] - self.lines[0][0], 0.95)
        # A burst of changes still gives at most one line a second.
        for state in ("PAUSED", "PLAYING", "PAUSED", "PLAYING"):
            self.cast.media_controller.tell(state)
            time.sleep(0.1)
        time.sleep(1.5)
        gaps = [b[0] - a[0] for a, b in zip(self.lines, self.lines[1:])]
        self.assertTrue(all(gap >= 0.95 for gap in gaps), gaps)
        return thread

    def test_one_shot_watch_reacts_at_once(self) -> None:
        self.check_reacts(None)

    def test_serving_watch_reacts_at_once_and_cancel_ends_it(self) -> None:
        cancel = threading.Event()
        thread = self.check_reacts(cancel)
        cancel.set()
        thread.join(3)
        self.assertFalse(thread.is_alive())
        # The kept connection carries no listener from the finished watch.
        self.assertEqual(self.cast.media_controller._status_listeners, [])


class RequestTimeout(Exception):
    """Stands in for pychromecast.error.RequestTimeout (matched by name)."""


class SlowSeekMedia:
    def __init__(self, state: str) -> None:
        self.status = SimpleNamespace(player_state=state, media_session_id=1, content_id="http://x/cast.mp4", duration=2500.0)
        self.calls: list = []

    def seek(self, position, timeout=10.0) -> None:
        self.calls.append(("seek", position, timeout))
        raise RequestTimeout(f"seek {position}")

    def pause(self, timeout=10.0) -> None:
        self.calls.append(("pause", timeout))


class SeekTest(unittest.TestCase):
    def run_seek(self, state: str):
        cast = SimpleNamespace(media_controller=SlowSeekMedia(state))
        rows: list = []
        old = cast_agent.write_line
        cast_agent.write_line = rows.append
        try:
            with self.assertRaises(SystemExit) as end:
                cast_agent.do_seek(cast, 2450.0, 20.0, 30.0)
        finally:
            cast_agent.write_line = old
        return end.exception.code, rows, cast.media_controller.calls

    def test_a_seek_the_tv_answers_late_was_still_sent(self) -> None:
        code, rows, calls = self.run_seek("PLAYING")
        self.assertEqual(code, 0)
        self.assertEqual(rows, [{"ok": True, "answered": False}])
        self.assertEqual(calls, [("seek", 2450.0, 30.0)])

    def test_slow_answers_keep_the_connection(self) -> None:
        self.assertTrue(cast_agent.timed_out(RequestTimeout("x")))
        self.assertFalse(cast_agent.timed_out(OSError("x")))


if __name__ == "__main__":
    unittest.main()
