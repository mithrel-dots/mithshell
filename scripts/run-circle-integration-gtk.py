#!/usr/bin/env python3
"""Run the production circle GTK integration test under private Broadway."""

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


def stop(process):
    if process is None or process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()


with tempfile.TemporaryDirectory(
    # Keep the Broadway Unix socket below the 108-byte sockaddr limit even in
    # this deliberately long worktree path. The runtime directory itself is
    # the disposable directory; do not add another nested component.
    prefix="c-",
    dir=target,
) as name:
    runtime = pathlib.Path(name)
    for directory in ("tmp", "cache", "config", "data", "chromium"):
        (runtime / directory).mkdir(mode=0o700)
    env = os.environ.copy()
    env.update(
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

    server = None
    log_path = runtime / "broadway.log"
    try:
        server_env = env.copy()
        server_env.pop("BROADWAY_DISPLAY", None)
        server_env.pop("GDK_BACKEND", None)
        for _ in range(5):
            with socket.socket() as probe:
                probe.bind(("127.0.0.1", 0))
                port = probe.getsockname()[1]
            with log_path.open("w") as log:
                server = subprocess.Popen(
                    [
                        "gtk4-broadwayd",
                        "-a",
                        "127.0.0.1",
                        "-p",
                        str(port),
                        env["BROADWAY_DISPLAY"],
                    ],
                    cwd=root,
                    env=server_env,
                    stdout=log,
                    stderr=log,
                )
            time.sleep(0.3)
            if server.poll() is None:
                break
            stop(server)
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
    finally:
        stop(server)

raise SystemExit(result.returncode)
