#!/usr/bin/env python3
"""Run the integrated battery-wave GTK regression test under private Broadway."""

import json
import os
import pathlib
import signal
import socket
import subprocess
import tempfile
import time

root = pathlib.Path(__file__).resolve().parents[1]


def free_port():
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


with tempfile.TemporaryDirectory(
    prefix="battery-wave-", dir=root / "target"
) as runtime_name:
    runtime = pathlib.Path(runtime_name)
    # Keep every temporary, cache, profile, socket, and log path below target.
    # TMPDIR is the target itself so Chromium's singleton socket remains short
    # enough even when the checkout path is long.
    for name in ("home", "cache", "config", "data"):
        (runtime / name).mkdir()
    env = os.environ.copy()
    env.update(
        # Chromium uses TMPDIR verbatim for its singleton socket.  A
        # project-relative path keeps that socket below the UNIX limit while
        # still resolving inside this worktree's target directory (all child
        # processes use cwd=root).
        TMPDIR="target",
        # Keep the caller's HOME so rustup can find the selected toolchain;
        # all test-specific XDG/cache/profile paths remain project-local.
        HOME=os.environ.get("HOME", str(runtime / "home")),
        XDG_CACHE_HOME=str(runtime / "cache"),
        XDG_CONFIG_HOME=str(runtime / "config"),
        XDG_DATA_HOME=str(runtime / "data"),
        XDG_RUNTIME_DIR=str(runtime),
        GDK_BACKEND="broadway",
        BROADWAY_DISPLAY=f":{100 + os.getpid() % 9000}",
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

    port = free_port()
    server = browser = None
    result = 1
    with (runtime / "broadway.log").open("w") as log, (runtime / "chromium.log").open("w") as browser_log:
        try:
            server = subprocess.Popen(
                ["gtk4-broadwayd", "-a", "127.0.0.1", "-p", str(port), env["BROADWAY_DISPLAY"]],
                cwd=root,
                env=env,
                stdout=log,
                stderr=log,
            )
            time.sleep(0.3)
            if server.poll() is not None:
                detail = (runtime / "broadway.log").read_text()
                raise RuntimeError(f"gtk4-broadwayd exited: {detail}")
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
                    f"http://127.0.0.1:{port}",
                ],
                cwd=root,
                env=env,
                stdout=browser_log,
                stderr=browser_log,
                start_new_session=True,
            )
            time.sleep(1)
            if browser.poll() is not None:
                browser_log.flush()
                detail = (runtime / "chromium.log").read_text()
                raise RuntimeError(f"headless Chromium exited: {detail}")
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
            ).returncode
        finally:
            if browser is not None and browser.poll() is None:
                os.killpg(browser.pid, signal.SIGTERM)
                try:
                    browser.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(browser.pid, signal.SIGKILL)
                    browser.wait()
            if server is not None and server.poll() is None:
                server.terminate()
                try:
                    server.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    server.kill()
                    server.wait()

raise SystemExit(result)
