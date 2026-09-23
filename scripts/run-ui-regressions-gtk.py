#!/usr/bin/env python3
"""Run every ignored GTK UI regression against one private Broadway display.

The runner deliberately builds into a project-local target directory and gives
GTK, Chromium, and Broadway a disposable runtime.  GtkWindow layer warnings
are expected from ``new_for_test``; fatal GTK criticals are not.
"""

import json
import os
import pathlib
import re
import socket
import subprocess
import sys
import sys
import tempfile
import time


ROOT = pathlib.Path(__file__).resolve().parents[1]
TARGET = ROOT / "target" / "ui-regressions-cargo"
TARGET.mkdir(mode=0o700, parents=True, exist_ok=True)
TESTS = [
    "ui::island::compact::tests::mapped_scale_1p9_hover_content_tracks_animation_and_picking",
    "ui::island::notification_circle::tests::gtk_notification_circle_integration",
    "ui::island::tests::circle_integration_real_widgets_and_callbacks",
    "ui::island::tests::real_circle_allocations_and_gtk_picking_survive_scale_and_rebuilds",
    "ui::island::tests::integrated_search_return_uses_real_finish_and_scheduler_path",
    "ui::island::search::tests::integrated_search_host_is_idempotent_on_real_widgets",
    "ui::island::search::tests::integrated_return_focus_guard_handles_reparent_and_stale_close",
    "ui::island::view::tests::finished_integrated_search_is_visible_and_targetable",
    "ui::island::tray::tracker_tests::broadway_popovers_share_global_lifetime_and_close_on_invalidation",
    "ui::island::media_circle::tests::gtk_update_selection_and_timer_lifecycle",
    "ui::island::media_circle::tests::compact_art_and_ring_fit_mapped_circle_at_runtime_scales",
    "ui::island::battery_wave::tests::playing_media_keeps_a_live_full_width_battery_background",
]
if len(sys.argv) > 1:
    requested = sys.argv[1:]
    unknown = set(requested) - set(TESTS)
    if unknown:
        raise SystemExit("unknown GTK regression filter: " + ", ".join(sorted(unknown)))
    TESTS = requested
if len(sys.argv) > 1:
    requested = set(sys.argv[1:])
    unknown = requested.difference(TESTS)
    if unknown:
        raise SystemExit("unknown GTK regression filter(s): " + ", ".join(sorted(unknown)))
    TESTS = [test for test in TESTS if test in requested]


def free_port():
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def stop(process):
    if process is None or process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()


with tempfile.TemporaryDirectory(prefix="ui-", dir=ROOT / "target") as name:
    runtime = pathlib.Path(name)
    for directory in ("tmp", "cache", "config", "data", "chromium"):
        (runtime / directory).mkdir(mode=0o700)
    env = os.environ.copy()
    env.update(
        CARGO_TARGET_DIR=str(TARGET),
        TMPDIR=str(runtime / "tmp"),
        XDG_CACHE_HOME=str(runtime / "cache"),
        XDG_CONFIG_HOME=str(runtime / "config"),
        XDG_DATA_HOME=str(runtime / "data"),
        XDG_RUNTIME_DIR=str(runtime),
        GDK_BACKEND="broadway",
        BROADWAY_DISPLAY=f":{os.getpid()}",
        GTK_A11Y="none",
        GSETTINGS_BACKEND="memory",
        GTK_USE_PORTAL="0",
        G_DEBUG="fatal-criticals",
    )
    env.pop("GSK_RENDERER", None)
    build = subprocess.run(
        ["cargo", "test", "--offline", "--no-run", "--message-format=json"],
        cwd=ROOT,
        env=env,
        stdout=subprocess.PIPE,
        text=True,
        check=True,
    )
    binary = next(
        item["executable"]
        for line in build.stdout.splitlines()
        if (item := json.loads(line)).get("reason") == "compiler-artifact"
        and item.get("executable")
        and "lib" in item["target"]["kind"]
    )
    listed = subprocess.run(
        [binary, "--list"],
        cwd=ROOT,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        check=True,
    )
    available = {}
    for line in listed.stdout.splitlines():
        match = re.fullmatch(r"(.+): test", line)
        if match:
            available[match.group(1)] = available.get(match.group(1), 0) + 1
    invalid = [
        test
        for test in TESTS
        if available.get(test, 0) != 1
    ]
    if invalid:
        details = ", ".join(
            f"{test} (listed {available.get(test, 0)} times)" for test in invalid
        )
        raise RuntimeError(
            "expected GTK regression test filter did not match exactly one listed test: "
            + details
        )

    server = None
    try:
        port = free_port()
        with (runtime / "broadway.log").open("w") as log:
            server = subprocess.Popen(
                ["gtk4-broadwayd", "-a", "127.0.0.1", "-p", str(port), env["BROADWAY_DISPLAY"]],
                cwd=ROOT,
                env=env,
                stdout=log,
                stderr=log,
            )
            time.sleep(0.3)
            if server.poll() is not None:
                raise RuntimeError((runtime / "broadway.log").read_text())
            result = 0
            for test in TESTS:
                completed = subprocess.run(
                    [binary, test, "--ignored", "--exact", "--test-threads=1", "--nocapture"],
                    cwd=ROOT,
                    env=env,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.STDOUT,
                    text=True,
                )
                print(f"\n== {test} ==")
                print(completed.stdout, end="")
                if completed.returncode != 0:
                    raise SystemExit(completed.returncode)
    finally:
        stop(server)

raise SystemExit(result)
