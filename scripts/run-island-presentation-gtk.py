#!/usr/bin/env python3
"""Run the focused integrated-launcher GTK tests under project-local Broadway."""

import json
import os
import pathlib
import subprocess
import time

root = pathlib.Path(__file__).resolve().parents[1]
runtime = root / "target/island-presentation-test-runtime"
tmp = root / "target/island-presentation-test-tmp"
runtime.mkdir(mode=0o700, parents=True, exist_ok=True)
tmp.mkdir(exist_ok=True)

env = os.environ.copy()
env.update(
    TMPDIR=str(tmp),
    XDG_RUNTIME_DIR=str(runtime),
    GDK_BACKEND="broadway",
    BROADWAY_DISPLAY=":76",
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

socket = runtime / "broadway"
if socket.exists():
    socket.unlink()
with (runtime / "broadway.log").open("w") as log:
    server = subprocess.Popen(
        ["gtk4-broadwayd", "-a", "127.0.0.1", "-p", "18776", ":76"],
        cwd=root,
        env=env,
        stdout=log,
        stderr=log,
    )
    try:
        time.sleep(0.3)
        tests = [
            "ui::island::tests::integrated_search_return_uses_real_finish_and_scheduler_path",
            "ui::island::search::tests::integrated_search_host_is_idempotent_on_real_widgets",
            "ui::island::search::tests::integrated_return_focus_guard_handles_reparent_and_stale_close",
            "ui::island::view::tests::finished_integrated_search_is_visible_and_targetable",
        ]
        result = 0
        for test in tests:
            result |= subprocess.run(
                [binary, test, "--ignored", "--exact", "--test-threads=1", "--nocapture"],
                cwd=root,
                env=env,
            ).returncode
    finally:
        server.terminate()
        server.wait(timeout=5)
raise SystemExit(result)
