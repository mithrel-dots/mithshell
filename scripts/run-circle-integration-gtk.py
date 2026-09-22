#!/usr/bin/env python3
"""Run the real circle/launcher GTK integration scenario on isolated Broadway.

All runtime state is below this checkout's target directory.  The display and
HTTP port are process-specific so two invocations can run back-to-back.
"""

import json
import os
import pathlib
import socket
import shutil
import subprocess
import sys
import tempfile
import time

root = pathlib.Path(__file__).resolve().parents[1]
target = root / "target"
target.mkdir(mode=0o700, exist_ok=True)
run_dir_handle = tempfile.TemporaryDirectory(
    prefix=f"circle-integration-{os.getpid()}-", dir=target
)
run_dir = pathlib.Path(run_dir_handle.name)
runtime = run_dir / "runtime"
tmp = run_dir / "tmp"
runtime.mkdir(mode=0o700)
tmp.mkdir(mode=0o700)
(run_dir / "cache").mkdir(mode=0o700)
(run_dir / "config").mkdir(mode=0o700)
(run_dir / "data").mkdir(mode=0o700)

env = os.environ.copy()
env.update(
    TMPDIR=str(tmp),
    XDG_RUNTIME_DIR=str(runtime),
    XDG_CACHE_HOME=str(run_dir / "cache"),
    XDG_CONFIG_HOME=str(run_dir / "config"),
    XDG_DATA_HOME=str(run_dir / "data"),
    GDK_BACKEND="broadway",
    GTK_A11Y="none",
    GSETTINGS_BACKEND="memory",
    GTK_USE_PORTAL="0",
)
env.pop("GSK_RENDERER", None)

# The runtime directory is private to this invocation; use a process-specific
# display so the same runner can be launched concurrently.
env["BROADWAY_DISPLAY"] = f":{os.getpid()}"

server = None
try:
    build = subprocess.run(
        ["cargo", "test", "--offline", "--no-run", "--message-format=json"],
        cwd=root,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=None,
        check=True,
    )
    binary = next(
        item["executable"]
        for line in build.stdout.splitlines()
        if (item := json.loads(line)).get("reason") == "compiler-artifact"
        and item.get("executable")
        and "lib" in item["target"]["kind"]
    )
    log_path = run_dir / "broadway.log"
    for attempt in range(5):
        env["BROADWAY_DISPLAY"] = f":{os.getpid() + attempt + 1}"
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 0))
            port = probe.getsockname()[1]
        with log_path.open("w") as log:
            server = subprocess.Popen(
                ["gtk4-broadwayd", "-a", "127.0.0.1", "-p", str(port), env["BROADWAY_DISPLAY"]],
                cwd=root,
                env=env,
                stdout=log,
                stderr=log,
            )
        time.sleep(0.3)
        if server.poll() is None:
            break
        server.terminate()
        server.wait(timeout=5)
        server = None
    else:
        raise RuntimeError(f"gtk4-broadwayd could not bind: {log_path.read_text()}")

        result = subprocess.run(
            [
                binary,
                "ui::island::tests::circle_integration_real_widgets_and_callbacks",
                "--ignored",
                "--exact",
                "--test-threads=1",
                "--nocapture",
            ],
            cwd=root,
            env=env,
        )
        raise SystemExit(result.returncode)
finally:
    if server is not None and server.poll() is None:
        server.terminate()
        try:
            server.wait(timeout=5)
        except subprocess.TimeoutExpired:
            server.kill()
            server.wait()
    run_dir_handle.cleanup()
