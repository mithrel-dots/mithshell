#!/usr/bin/env python3
"""Run the focused integrated-launcher GTK tests under private Broadway."""

import json
import os
import pathlib
import socket
import subprocess
import tempfile
import time


root = pathlib.Path(__file__).resolve().parents[1]
target = root / "target"
target.mkdir(mode=0o700, exist_ok=True)


def free_port():
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def stop_process(process):
    if process is None or process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()


def test_binary(build):
    for line in build.stdout.splitlines():
        item = json.loads(line)
        if (item.get("reason") == "compiler-artifact" and item.get("executable")
                and "lib" in item["target"]["kind"]):
            return item["executable"]
    raise RuntimeError("cargo did not report a library test executable")


tests = [
    "ui::island::tests::integrated_search_return_uses_real_finish_and_scheduler_path",
    "ui::island::search::tests::integrated_search_host_is_idempotent_on_real_widgets",
    "ui::island::search::tests::integrated_return_focus_guard_handles_reparent_and_stale_close",
    "ui::island::view::tests::finished_integrated_search_is_visible_and_targetable",
]

with tempfile.TemporaryDirectory(prefix="island-presentation-", dir=target) as name:
    runtime = pathlib.Path(name)
    for directory in ("tmp", "cache", "config", "data", "chromium"):
        (runtime / directory).mkdir()
    env = os.environ.copy()
    env.update(
        # Keep rustup's toolchain selection, while all test-owned paths stay
        # below this invocation's disposable directory.
        HOME=os.environ["HOME"],
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
        CHROME_CONFIG_HOME=str(runtime / "config"),
    )
    env.pop("GSK_RENDERER", None)

    build = subprocess.run(
        ["cargo", "test", "--offline", "--no-run", "--message-format=json"],
        cwd=root, env=env, stdout=subprocess.PIPE, text=True, check=True,
    )
    binary = test_binary(build)
    server = None
    try:
        log_path = runtime / "broadway.log"
        for _ in range(5):
            port = free_port()
            with log_path.open("w") as log:
                server = subprocess.Popen(
                    ["gtk4-broadwayd", "-a", "127.0.0.1", "-p", str(port), env["BROADWAY_DISPLAY"]],
                    cwd=root, env=env, stdout=log, stderr=log,
                )
            time.sleep(0.3)
            if server.poll() is None:
                break
            stop_process(server)
            server = None
        else:
            raise RuntimeError(f"gtk4-broadwayd could not bind: {log_path.read_text()}")
        result = 0
        for test in tests:
            result |= subprocess.run(
                [binary, test, "--ignored", "--exact", "--test-threads=1", "--nocapture"],
                cwd=root, env=env,
            ).returncode
    finally:
        stop_process(server)

raise SystemExit(result)
