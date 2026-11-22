"""Offline checks of the harness's own logic: python tools/parity/test_parity.py"""

import importlib.util
import sys
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from proxy import normalise  # noqa: E402
import shots  # noqa: E402

spec = importlib.util.spec_from_file_location("parity_trace", HERE / "trace.py")
trace = importlib.util.module_from_spec(spec)
spec.loader.exec_module(trace)

# Query order, percent-encoding and JSON key order do not matter; status does.
a = normalise("POST", "/api/title/a%2Fb?y=2&x=1", b'{"b":1,"a":2}', 200)
b = normalise("POST", "/api/title/a/b?x=1&y=2", b'{"a": 2, "b": 1}', 200)
assert a == b, (a, b)
assert normalise("GET", "/api/x", b"", 404) != normalise("GET", "/api/x", b"", 200)

# Tolerance: a 2-level change is equal, 3 is not; text and non-text are scored apart.
old = np.zeros((4, 4, 3), np.uint8)
new = old.copy()
new[0, 0] = 2
new[3, 3] = 3
text = np.zeros((4, 4), bool)
text[3, 3] = True
stats, *_ = shots.score(old, new, 2, text, np.zeros((4, 4), bool))
assert stats["nontext_pct"] == 0 and stats["text_pct"] == 100.0, stats
assert stats["max_delta"] == 3 and stats["within_tolerance_nonzero_pct"] == 6.25, stats
masked = np.zeros((4, 4), bool)
masked[3, 3] = True
stats, *_ = shots.score(old, new, 2, text, masked)
assert stats["all_pct"] == 0, stats

# The read-only deny list stops writes but not the scenario's navigation.
for name in ["Watchlist", "Bookmarks", "History", "Source", "Settings", "Home", "Theme, Portal", "Details", "Episodes"]:
    assert not trace.DENY.search(name), name
for name in ["Resume 4:10", "Remove Tracker from the watchlist", "Play", "Restart Portal", "Check for updates", "Cast"]:
    assert trace.DENY.search(name), name

# Two identical traces compare clean; a missing call does not.
call = {"method": "GET", "path": "/api/home", "query": "", "body": "", "status": 200, "tag": "x|00 open home"}
step = {"event": "step", "result": "ok", "controls": [["link", "Home", False]], "tag": "x|00 open home"}
assert trace.compare([call, step], [call, step]) == 0
assert trace.compare([call, step], [step]) == 1
print("ok")
