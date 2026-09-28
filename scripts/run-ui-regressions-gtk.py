#!/usr/bin/env python3
"""Run every ignored GTK UI regression against a fresh private Broadway display.

The runner deliberately builds into a project-local target directory and gives
GTK, Chromium, and Broadway a disposable runtime.  GtkWindow layer warnings
are expected from ``new_for_test``; fatal GTK criticals are not.
"""

import json
import os
import pathlib
import re
import signal
import socket
import shutil
import subprocess
import sys
import tempfile
import time


ROOT = pathlib.Path(__file__).resolve().parents[1]
TARGET = ROOT / "target" / "ui-regressions-cargo"
TARGET.mkdir(mode=0o700, parents=True, exist_ok=True)


def free_port():
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def stop(process, group=False):
    if process is None or process.poll() is not None:
        return
    if group:
        os.killpg(process.pid, signal.SIGTERM)
    else:
        process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        if group:
            os.killpg(process.pid, signal.SIGKILL)
        else:
            process.kill()
        process.wait()


with tempfile.TemporaryDirectory(prefix="ui-", dir=ROOT / "target") as name:
    runtime = pathlib.Path(name)
    for directory in ("tmp", "cache", "config", "data", "chromium"):
        (runtime / directory).mkdir(mode=0o700)
    gtk_css = pathlib.Path.home() / ".config" / "gtk-4.0" / "gtk.css"
    if gtk_css.is_file():
        gtk_config = runtime / "config" / "gtk-4.0"
        gtk_config.mkdir(mode=0o700)
        shutil.copyfile(gtk_css, gtk_config / "gtk.css")
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
        [binary, "--list", "--ignored"],
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
    tests = sys.argv[1:] or sorted(available)
    if not tests:
        raise RuntimeError("no ignored GTK regressions were discovered")
    invalid = [test for test in tests if available.get(test, 0) != 1]
    if invalid:
        details = ", ".join(
            f"{test} (listed {available.get(test, 0)} times)" for test in invalid
        )
        raise RuntimeError(
            "expected GTK regression test filter did not match exactly one listed test: "
            + details
        )

    # Each Rust test is a new GTK client. Reusing the Broadway/WebGL session
    # across their disconnects can leave later clients waiting indefinitely
    # for frame acknowledgements, with every widget stuck at an old allocation.
    for index, test in enumerate(tests):
        server = None
        browser = None
        # Chromium's Unix sockets need short paths in deeply nested worktrees.
        with tempfile.TemporaryDirectory(prefix="ms-gtk-", ignore_cleanup_errors=True) as chrome_dir:
            try:
                port = free_port()
                env["BROADWAY_DISPLAY"] = f":{os.getpid() * 100 + index}"
                server_log = runtime / f"broadway-{index}.log"
                browser_log = runtime / f"chromium-{index}.log"
                with server_log.open("w") as log:
                    server = subprocess.Popen(
                        ["gtk4-broadwayd", "-a", "127.0.0.1", "-p", str(port), env["BROADWAY_DISPLAY"]],
                        cwd=ROOT, env=env, stdout=log, stderr=log,
                    )
                time.sleep(0.3)
                if server.poll() is not None:
                    raise RuntimeError(server_log.read_text())
                # Rendering is required for real frame-clock allocation.
                chrome_env = env.copy()
                chrome_env["TMPDIR"] = chrome_dir
                with browser_log.open("w") as log:
                    browser = subprocess.Popen(
                        ["chromium", "--headless", "--no-sandbox", "--enable-webgl",
                         "--use-gl=angle", "--use-angle=swiftshader", "--disable-dev-shm-usage",
                         "--disable-background-timer-throttling", "--disable-renderer-backgrounding",
                         "--disable-backgrounding-occluded-windows",
                         "--remote-debugging-port=0", "--window-size=3840,2160",
                         "--user-data-dir=" + chrome_dir + "/profile",
                         "http://127.0.0.1:" + str(port)],
                        env=chrome_env, stdout=subprocess.DEVNULL, stderr=log,
                        start_new_session=True,
                    )
                time.sleep(0.5)
                if browser.poll() is not None:
                    raise RuntimeError("Broadway rendering client exited: " + browser_log.read_text())
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
                    print(browser_log.read_text(), file=sys.stderr)
                    raise SystemExit(completed.returncode)
            finally:
                stop(browser, group=True)
                stop(server)

raise SystemExit(0)
