#!/usr/bin/env python3
"""Run the tray popover lifecycle regression on a private GTK Broadway display."""
import json
import os
import pathlib
import subprocess
import time

root = pathlib.Path(__file__).resolve().parents[1]
runtime = root / "target/tray-circle-test-runtime"
tmp = root / "target/tray-circle-tmp"
runtime.mkdir(mode=0o700, parents=True, exist_ok=True)
tmp.mkdir(parents=True, exist_ok=True)
build_env = os.environ.copy()
build_env["TMPDIR"] = str(tmp)
build = subprocess.run(
    ["cargo", "test", "--offline", "--no-run", "--message-format=json"],
    cwd=root,
    env=build_env,
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
env = os.environ.copy()
for key, name in [("HOME", "home"), ("XDG_CONFIG_HOME", "config"),
                  ("XDG_CACHE_HOME", "cache"), ("XDG_DATA_HOME", "data")]:
    path = runtime / name
    path.mkdir(exist_ok=True)
    env[key] = str(path)
env.update(
    TMPDIR=str(tmp), XDG_RUNTIME_DIR=str(runtime), GDK_BACKEND="broadway",
    BROADWAY_DISPLAY=":74", GTK_A11Y="none", GSETTINGS_BACKEND="memory",
    GTK_USE_PORTAL="0",
)
env.pop("GSK_RENDERER", None)
with (runtime / "broadway.log").open("w") as log:
    server = subprocess.Popen(
        ["gtk4-broadwayd", "-a", "127.0.0.1", "-p", "18774", ":74"],
        cwd=root, env=env, stdout=log, stderr=log,
    )
    try:
        time.sleep(0.2)
        result = subprocess.run(
            [binary, "broadway_popovers_share_global_lifetime_and_close_on_invalidation",
             "--ignored", "--test-threads=1", "--nocapture"],
            cwd=root, env=env,
        )
    finally:
        server.terminate()
        server.wait(timeout=5)
raise SystemExit(result.returncode)
