#!/usr/bin/env python3
"""Run the integrated battery-wave GTK regression test under private Broadway."""

import json
import os
import pathlib
import signal
import subprocess
import time

root = pathlib.Path(__file__).resolve().parents[1]
runtime = root / "target/battery-wave-test-runtime"
# Chromium puts its singleton socket in TMPDIR; keep this path short enough
# for Chromium's UNIX-socket limit even when the checkout path is long.
tmp = pathlib.Path("/tmp/opencode/mithshell-battery-wave")
runtime.mkdir(mode=0o700, parents=True, exist_ok=True)
tmp.mkdir(exist_ok=True)

env = os.environ.copy()
env.update(
    TMPDIR=str(tmp),
    XDG_RUNTIME_DIR=str(runtime),
    GDK_BACKEND="broadway",
    BROADWAY_DISPLAY=":193",
    GTK_A11Y="none",
    GSETTINGS_BACKEND="memory",
    GTK_USE_PORTAL="0",
)
env.pop("GSK_RENDERER", None)

build = subprocess.run(
    ["cargo", "test", "--offline", "--no-run", "--message-format=json"],
    cwd=root,
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

with (runtime / "broadway.log").open("w") as log, (runtime / "chromium.log").open("w") as browser_log:
    server = subprocess.Popen(
        ["gtk4-broadwayd", "-a", "127.0.0.1", "-p", "28793", ":193"],
        cwd=root,
        env=env,
        stdout=log,
        stderr=log,
    )
    browser = None
    try:
        time.sleep(0.3)
        if server.poll() is not None:
            raise RuntimeError("gtk4-broadwayd exited; see target/battery-wave-test-runtime/broadway.log")
        browser = subprocess.Popen(
            [
                "chromium",
                "--headless",
                "--disable-gpu",
                "--no-sandbox",
                "--no-first-run",
                "--no-proxy-server",
                "--disable-background-networking",
                "--disable-dev-shm-usage",
                "--user-data-dir=" + str(runtime / "chromium"),
                "http://127.0.0.1:28793",
            ],
            cwd=root,
            env=env,
            stdout=browser_log,
            stderr=browser_log,
            start_new_session=True,
        )
        time.sleep(1)
        if browser.poll() is not None:
            raise RuntimeError("headless Chromium exited; see target/battery-wave-test-runtime/chromium.log")
        result = subprocess.run(
            [
                binary,
                "ui::island::battery_wave::tests::playing_media_keeps_a_live_full_width_battery_background",
                "--ignored",
                "--exact",
                "--test-threads=1",
                "--nocapture",
            ],
            cwd=root,
            env=env,
        )
    finally:
        if browser is not None and browser.poll() is None:
            os.killpg(browser.pid, signal.SIGTERM)
            browser.wait(timeout=5)
        server.terminate()
        server.wait(timeout=5)

raise SystemExit(result.returncode)
