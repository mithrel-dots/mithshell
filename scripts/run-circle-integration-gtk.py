#!/usr/bin/env python3
"""Run the real circle/launcher GTK integration scenario on isolated Broadway.

All runtime state is below this checkout's target directory.  The display and
HTTP port are process-specific so two invocations can run back-to-back.
"""

import json
import os
import pathlib
import random
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

# The runtime directory is private to this invocation, so the conventional
# display zero is collision-free and avoids stale global Broadway socket names.
env["BROADWAY_DISPLAY"] = ":0"
port = random.SystemRandom().randint(20000, 45000)

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
    with log_path.open("w") as log:
        server_env = env.copy()
        # Passing the display both through the environment and argv makes
        # this GTK build attempt to bind its Unix socket twice.
        server_env.pop("BROADWAY_DISPLAY", None)
        server_env.pop("GDK_BACKEND", None)
        server = subprocess.Popen(
            ["gtk4-broadwayd", "-a", "127.0.0.1", "-p", str(port), env["BROADWAY_DISPLAY"]],
            cwd=root,
            env=server_env,
            stdout=log,
            stderr=log,
        )
        # Debian/GTK4 builds expose Broadway through the XDG runtime socket;
        # some builds also expose the optional HTTP port, so checking TCP
        # alone makes the runner report a false startup failure.
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            if server.poll() is not None:
                raise RuntimeError(f"gtk4-broadwayd exited; see {log_path}: {log_path.read_text()}")
            if list(runtime.glob("broadway*.socket")):
                break
            time.sleep(0.05)
        else:
            raise RuntimeError(f"Broadway did not create a runtime socket; see {log_path}")

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
