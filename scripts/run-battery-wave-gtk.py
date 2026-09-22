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


def scoped_runtime(project_root):
    """Create a disposable runtime below target, including fresh checkouts."""
    target = project_root / "target"
    target.mkdir(parents=True, exist_ok=True)
    runtime = tempfile.TemporaryDirectory(prefix="battery-wave-", dir=target)
    runtime_path = pathlib.Path(runtime.name)
    assert runtime_path.parent == target
    (runtime_path / "tmp").mkdir()
    return runtime


with scoped_runtime(root) as runtime_name:
    runtime = pathlib.Path(runtime_name)
    for name in ("home", "cache", "config", "data"):
        (runtime / name).mkdir()
    env = os.environ.copy()
    env.update(
        # Every temporary path is scoped to this disposable runtime.
        TMPDIR=str(runtime / "tmp"),
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
                # A short relative TMPDIR keeps Chromium's singleton socket
                # under the scoped runtime without exceeding UNIX limits.
                cwd=runtime,
                env={**env, "TMPDIR": "tmp"},
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
