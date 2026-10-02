#!/usr/bin/env python3
"""Real pointer delivery in private headless Weston -> nested Hyprland.

Requires Weston with its GL renderer, Hyprland, GNU cc and wayland-scanner.
MITHSHELL_TEST_WESTON may name an extracted/private Weston executable.
For an extracted <prefix>/bin/weston, its <prefix>/lib, lib/weston and
lib/libweston-* module paths are discovered automatically (parent process only).
For Aquamarine fixed-version binding failures against Weston, explicitly set
MITHSHELL_TEST_NESTED_COMPAT=1. That shim applies ONLY to Hyprland's parent
connection. Native tests use no-motion mode; Broadway covers animated lifecycle
because the nested backend's frame acknowledgements are unreliable here.
Pinned-source fixtures precommit the intended Overlay stacking and use fresh
clients/compositors. All native fixtures run under a private session D-Bus.
"""
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = pathlib.Path(__file__).resolve().parents[1]
TEST = "ui::island::tests::exclusive_outside_click_routes_through_native_input"
SEARCH_IDLE_TEST = "ui::island::tests::dashboard_and_weather_search_outside_dismiss_returns_to_idle"

if os.environ.get("MITHSHELL_PRIVATE_NATIVE_BUS") != "1":
    private_env = os.environ.copy()
    private_env["MITHSHELL_PRIVATE_NATIVE_BUS"] = "1"
    completed = subprocess.run(["dbus-run-session", "--", sys.executable, __file__, *sys.argv[1:]],
                               env=private_env)
    raise SystemExit(completed.returncode)


def freeze_test_binary(env, destination):
    build = subprocess.run(["cargo", "test", "--offline", "--no-run", "--message-format=json"],
                           cwd=ROOT, env=env, stdout=subprocess.PIPE, text=True, check=True)
    executable = next(item["executable"] for line in build.stdout.splitlines()
                      if (item := json.loads(line)).get("reason") == "compiler-artifact"
                      and item.get("executable") and "lib" in item["target"]["kind"])
    shutil.copy2(executable, destination)
    listed = subprocess.check_output([str(destination), "--list", "--ignored"], env=env, text=True)
    for test in (TEST, SEARCH_IDLE_TEST):
        assert test + ": test" in listed, "stale shared-cache binary; rebuild this worktree's package"
    return str(destination)


if "--variant" not in sys.argv:
    with tempfile.TemporaryDirectory(prefix="od-tests-", dir="/tmp/opencode") as name:
        variant = os.environ.copy()
        variant["CARGO_TARGET_DIR"] = str(ROOT / "target/ui-regressions-cargo")
        variant["MITHSHELL_NATIVE_TEST_BINARY"] = freeze_test_binary(variant, pathlib.Path(name) / "tests")
        for independent in ("0", "1"):
            for circles in ("0", "1"):
                for source in ("base", "dashboard", "weather"):
                    variant.update(MITHSHELL_TEST_INDEPENDENT=independent, MITHSHELL_TEST_CIRCLES=circles,
                                   MITHSHELL_TEST_PINNED_SOURCE=source)
                    print(f"native fixture: independent={independent} circles={circles} source={source}", flush=True)
                    subprocess.run([sys.executable, __file__, "--variant"], env=variant, check=True)
    raise SystemExit(0)
with tempfile.TemporaryDirectory(prefix="od-", dir="/tmp/opencode") as name:
    runtime = pathlib.Path(name)
    runtime.chmod(0o700)
    env = os.environ.copy()
    for key in ("WAYLAND_DISPLAY", "DISPLAY", "HYPRLAND_INSTANCE_SIGNATURE"):
        env.pop(key, None)
    env.update(XDG_RUNTIME_DIR=str(runtime), XDG_CONFIG_HOME=str(runtime),
               XDG_CACHE_HOME=str(runtime), AQ_BACKENDS="headless",
               HYPRLAND_NO_SD_NOTIFY="1", HYPRLAND_NO_RT="1", GTK_A11Y="none",
               GDK_BACKEND="wayland", GTK_USE_PORTAL="0", G_DEBUG="fatal-criticals",
               CARGO_TARGET_DIR=str(ROOT / "target/ui-regressions-cargo"))
    # Do not let the isolated compositor acquire the human's logind seat.
    env.update(LIBSEAT_BACKEND="seatd", SEATD_SOCK=str(runtime / "no-seatd.sock"))
    config = runtime / "hyprland.conf"
    config.write_text("monitor = ,3840x2160@60,0x0,1\ndebug {\n disable_logs = false\n enable_stdout_logs = true\n}\n" +
                      "misc {\n disable_hyprland_logo = true\n disable_splash_rendering = true\n}\n")
    for kind, filename in (("client-header", "virtual-pointer.h"),
                           ("private-code", "virtual-pointer.c")):
        subprocess.run(["wayland-scanner", kind, str(ROOT / "scripts/virtual-pointer.xml"),
                        str(runtime / filename)], check=True)
    helper = runtime / "pointer"
    subprocess.run(["cc", "-I" + str(runtime), str(ROOT / "scripts/test-virtual-pointer.c"),
                    str(runtime / "virtual-pointer.c"), "-lwayland-client", "-o", str(helper)], check=True)
    env["MITHSHELL_PRIVATE_POINTER"] = str(helper)
    binary = env.get("MITHSHELL_NATIVE_TEST_BINARY") or freeze_test_binary(env, runtime / "tests")
    shim = None
    if os.environ.get("MITHSHELL_TEST_NESTED_COMPAT") == "1":
        shim = runtime / "nested-compat.so"
        subprocess.run(["cc", "-shared", "-fPIC", str(ROOT / "scripts/test-nested-wayland-compat.c"),
                        "-ldl", "-lwayland-client", "-o", str(shim)], check=True)
    # Aquamarine's headless backend still needs an allocator. Provide a
    # private headless Weston Wayland parent, never the human's compositor.
    weston = os.environ.get("MITHSHELL_TEST_WESTON", "weston")
    parent_env = env.copy()
    if "MITHSHELL_TEST_WESTON" in os.environ:
        prefix = pathlib.Path(weston).resolve().parent.parent
        library_dirs = [prefix / "lib", prefix / "lib/weston"]
        parent_env["LD_LIBRARY_PATH"] = ":".join(
            [str(path) for path in library_dirs if path.is_dir()]
            + ([env["LD_LIBRARY_PATH"]] if env.get("LD_LIBRARY_PATH") else [])
        )
        modules = list((prefix / "lib/weston").glob("*.so"))
        for directory in (prefix / "lib").glob("libweston-*"):
            modules.extend(directory.glob("*.so"))
        if modules:
            parent_env.setdefault("WESTON_MODULE_MAP", ";".join(f"{path.name}={path}" for path in modules))
    parent_log = (runtime / "weston.log").open("w")
    parent = subprocess.Popen([weston, "--backend=headless", "--renderer=gl",
                               "--shell=kiosk", "--fake-seat", "--no-config",
                               "--width=3840", "--height=2160", "--socket=parent"],
                              env=parent_env, stdout=parent_log, stderr=parent_log,
                              start_new_session=True)
    deadline = time.monotonic() + 15
    while not (runtime / "parent").is_socket():
        if parent.poll() is not None or time.monotonic() > deadline:
            parent.terminate()
            parent.wait()
            raise RuntimeError((runtime / "weston.log").read_text())
        time.sleep(.1)
    env["WAYLAND_DISPLAY"] = "parent"
    with (runtime / "hyprland.log").open("w") as log:
        compositor_env = env.copy()
        if shim:
            compositor_env["LD_PRELOAD"] = str(shim)
        compositor = subprocess.Popen(["Hyprland", "-c", str(config)], env=compositor_env,
                                      stdout=log, stderr=log, start_new_session=True)
        try:
            deadline = time.monotonic() + 15
            while not list(runtime.glob("wayland-*")):
                if compositor.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError((runtime / "hyprland.log").read_text())
                time.sleep(.1)
            env["WAYLAND_DISPLAY"] = next(p.name for p in runtime.glob("wayland-*")
                                          if not p.name.endswith(".lock"))
            result = subprocess.run([binary, TEST, "--ignored", "--exact", "--nocapture"],
                                    cwd=ROOT, env=env, timeout=90)
            if result.returncode:
                print((runtime / "hyprland.log").read_text())
            raise SystemExit(result.returncode)
        finally:
            compositor.terminate()
            try:
                compositor.wait(timeout=5)
            except subprocess.TimeoutExpired:
                compositor.kill()
                compositor.wait()
            parent.terminate()
            parent.wait(timeout=5)
            parent_log.close()
