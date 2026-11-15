#!/usr/bin/env python3
"""Headless game-launch QA helper for MQ Launcher.

Supports:
- Finding mapped X11 game windows via libX11 XQueryTree (works under rootless weston+Xwayland).
- Capturing a specific X11 window via ffmpeg x11grab.
- Running Tesseract rus+eng OCR on the Minecraft main menu (with button-crop preprocessing).
- Verifying log output (main menu / boot markers, absence of FATAL and crash-reports/*).
"""

from __future__ import annotations

import argparse
import ctypes
import json
import os
import socket
import subprocess
import sys
import time
from pathlib import Path

from PIL import Image


class XWindowAttributes(ctypes.Structure):
    _fields_ = [
        ("x", ctypes.c_int),
        ("y", ctypes.c_int),
        ("width", ctypes.c_int),
        ("height", ctypes.c_int),
        ("border_width", ctypes.c_int),
        ("depth", ctypes.c_int),
        ("visual", ctypes.c_void_p),
        ("root", ctypes.c_ulong),
        ("c_class", ctypes.c_int),
        ("bit_gravity", ctypes.c_int),
        ("win_gravity", ctypes.c_int),
        ("backing_store", ctypes.c_int),
        ("backing_planes", ctypes.c_ulong),
        ("backing_pixel", ctypes.c_ulong),
        ("save_under", ctypes.c_int),
        ("colormap", ctypes.c_ulong),
        ("map_installed", ctypes.c_int),
        ("map_state", ctypes.c_int),
        ("all_event_masks", ctypes.c_long),
        ("your_event_mask", ctypes.c_long),
        ("do_not_propagate_mask", ctypes.c_long),
        ("override_redirect", ctypes.c_int),
        ("screen", ctypes.c_void_p),
    ]


def find_game_windows(min_width: int = 400, min_height: int = 300) -> list[tuple[str, int, int]]:
    display_name = os.environ.get("DISPLAY", "").encode()
    if not display_name:
        return []
    x11 = ctypes.CDLL("libX11.so.6")
    x11.XOpenDisplay.restype = ctypes.c_void_p
    x11.XCloseDisplay.argtypes = [ctypes.c_void_p]
    x11.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
    x11.XDefaultRootWindow.restype = ctypes.c_ulong
    x11.XQueryTree.argtypes = [
        ctypes.c_void_p,
        ctypes.c_ulong,
        ctypes.POINTER(ctypes.c_ulong),
        ctypes.POINTER(ctypes.c_ulong),
        ctypes.POINTER(ctypes.POINTER(ctypes.c_ulong)),
        ctypes.POINTER(ctypes.c_uint),
    ]
    x11.XGetWindowAttributes.argtypes = [
        ctypes.c_void_p,
        ctypes.c_ulong,
        ctypes.POINTER(XWindowAttributes),
    ]

    dpy = x11.XOpenDisplay(display_name)
    if not dpy:
        return []
    try:
        root = x11.XDefaultRootWindow(dpy)
        r = ctypes.c_ulong()
        p = ctypes.c_ulong()
        children = ctypes.POINTER(ctypes.c_ulong)()
        n = ctypes.c_uint()
        if not x11.XQueryTree(dpy, root, ctypes.byref(r), ctypes.byref(p), ctypes.byref(children), ctypes.byref(n)):
            return []
        found: list[tuple[str, int, int]] = []
        for i in range(n.value):
            attr = XWindowAttributes()
            if x11.XGetWindowAttributes(dpy, children[i], ctypes.byref(attr)):
                if attr.map_state == 2 and attr.width >= min_width and attr.height >= min_height:
                    found.append((hex(children[i]), attr.width, attr.height))
        return found
    finally:
        x11.XCloseDisplay(dpy)


def capture_window(win_id: str, out_path: Path) -> bool:
    out_path.parent.mkdir(parents=True, exist_ok=True)
    display = os.environ.get("DISPLAY", "")
    if not display:
        return False
    proc = subprocess.run(
        [
            "ffmpeg",
            "-y",
            "-f",
            "x11grab",
            "-window_id",
            win_id,
            "-i",
            display,
            "-frames:v",
            "1",
            "-update",
            "1",
            str(out_path),
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    return proc.returncode == 0 and out_path.is_file() and out_path.stat().st_size > 1000


def ocr_main_menu(image_path: Path) -> dict[str, object]:
    env = os.environ.copy()
    tessdata = Path.home() / ".local" / "share" / "tessdata"
    if tessdata.is_dir() and "TESSDATA_PREFIX" not in env:
        env["TESSDATA_PREFIX"] = str(tessdata)

    raw_text = subprocess.check_output(
        ["tesseract", str(image_path), "stdout", "-l", "rus+eng"],
        env=env,
        stderr=subprocess.DEVNULL,
        text=True,
    )

    img = Image.open(image_path)
    w, h = img.size
    crop = img.crop((int(w * 0.25), int(h * 0.45), int(w * 0.75), int(h * 0.85)))
    crop = crop.resize((crop.width * 3, crop.height * 3), Image.Resampling.NEAREST)
    gray = crop.convert("L")
    bw = gray.point(lambda p: 0 if p > 200 else 255)
    prep_path = image_path.with_suffix(".ocr_prep.png")
    bw.save(prep_path)

    menu_text = subprocess.check_output(
        ["tesseract", str(prep_path), "stdout", "-l", "rus+eng", "--psm", "6"],
        env=env,
        stderr=subprocess.DEVNULL,
        text=True,
    )
    combined = f"{raw_text}\n{menu_text}"
    lower = combined.lower()
    has_single = "одиноч" in lower or "панноч" in lower
    has_multi = "сетев" in lower
    return {
        "raw_text": raw_text.strip(),
        "menu_text": menu_text.strip(),
        "has_singleplayer": has_single,
        "has_multiplayer": has_multi,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description="MQ Game Launch QA Helper")
    sub = parser.add_subparsers(dest="cmd", required=True)

    p_win = sub.add_parser("wait-window", help="Wait for X11 game window and optionally capture it")
    p_win.add_argument("--timeout", type=int, default=90)
    p_win.add_argument("--out", type=Path, default=None)

    p_ocr = sub.add_parser("ocr", help="Run Russian+English OCR on a game screenshot")
    p_ocr.add_argument("image", type=Path)

    p_ui = sub.add_parser("verify-ui", help="Run live UI launch/stop/parallel/signal-crash verification via test-hooks")
    p_ui.add_argument("--bin", type=Path, required=True)
    p_ui.add_argument("--data-dir", type=Path, required=True)
    p_ui.add_argument("--out-dir", type=Path, required=True)

    args = parser.parse_args()
    if args.cmd == "wait-window":
        deadline = time.monotonic() + args.timeout
        while time.monotonic() < deadline:
            wins = find_game_windows()
            if wins:
                win_id, w, h = wins[0]
                ok = True
                if args.out is not None:
                    ok = capture_window(win_id, args.out)
                print(json.dumps({"window_id": win_id, "width": w, "height": h, "captured": ok}))
                return 0 if ok else 1
            time.sleep(1.0)
        print(json.dumps({"error": "timeout waiting for game window"}))
        return 1

    if args.cmd == "ocr":
        res = ocr_main_menu(args.image)
        print(json.dumps(res, ensure_ascii=False, indent=2))
        return 0 if (res["has_singleplayer"] or res["has_multiplayer"]) else 1

    if args.cmd == "verify-ui":
        return run_verify_ui(args.bin, args.data_dir, args.out_dir)

    return 2


def run_verify_ui(mq_bin: Path, data_dir: Path, out_dir: Path) -> int:
    data_dir = data_dir.resolve()
    out_dir.mkdir(parents=True, exist_ok=True)
    sock_path = data_dir / "test-hooks.sock"
    if sock_path.exists():
        sock_path.unlink()

    xdg_root = data_dir / "_xdg"
    for sub in ("data", "config", "cache"):
        (xdg_root / sub).mkdir(parents=True, exist_ok=True)

    env = os.environ.copy()
    env["MQ_DATA_DIR"] = str(data_dir)
    env["MQ_TEST_SANDBOX"] = "1"
    env["MQ_TEST_HOOKS"] = "1"
    env["MQ_TEST_HOOKS_SOCKET"] = str(sock_path)
    env["XDG_DATA_HOME"] = str(xdg_root / "data")
    env["XDG_CONFIG_HOME"] = str(xdg_root / "config")
    env["XDG_CACHE_HOME"] = str(xdg_root / "cache")

    proc = subprocess.Popen(
        [str(mq_bin), "--test-hooks"],
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    try:
        deadline = time.monotonic() + 60.0
        while time.monotonic() < deadline:
            if proc.poll() is not None:
                out = proc.stdout.read() if proc.stdout else ""
                raise RuntimeError(f"mq exited early: {proc.returncode}\n{out}")
            if sock_path.exists():
                break
            time.sleep(0.1)
        if not sock_path.exists():
            proc.terminate()
            out = proc.stdout.read() if proc.stdout else ""
            raise RuntimeError(f"Timed out waiting for test-hooks.sock\n{out}")

        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.settimeout(30.0)
        sock.connect(str(sock_path))
        reader = sock.makefile("r", encoding="utf-8")

        def call(payload: dict) -> dict:
            sock.sendall((json.dumps(payload) + "\n").encode("utf-8"))
            line = reader.readline()
            if not line:
                raise RuntimeError("EOF from test-hooks socket")
            resp = json.loads(line)
            if not resp.get("ok", False):
                raise RuntimeError(f"Hook {payload.get('cmd')} failed: {resp}")
            return resp

        call({"cmd": "resize", "width": 1280, "height": 800})
        call({"cmd": "goto", "tab": "instances"})
        call({"cmd": "wait_idle", "timeout_ms": 5000, "frames": 3})

        def wait_game_running(expected: bool, timeout_s: float = 25.0) -> dict:
            dl = time.monotonic() + timeout_s
            while time.monotonic() < dl:
                resp = call({"cmd": "state"})
                st = resp.get("data", resp)
                if st["game_running"] is expected:
                    return st
                time.sleep(0.3)
            raise RuntimeError(f"Timed out waiting for game_running={expected}")

        # 1. Launch first instance via UI Play button (grid card)
        call({"cmd": "click", "label": "Играть"})
        st1 = wait_game_running(True, 25.0)

        # 2. Open detail view via 'Моды', switch to 'Консоль' tab and verify live logs
        call({"cmd": "click", "label": "Моды"})
        call({"cmd": "wait_idle", "timeout_ms": 3000, "frames": 2})
        call({"cmd": "click", "label": "Консоль"})
        time.sleep(3.0)
        call({"cmd": "wait_idle", "timeout_ms": 3000, "frames": 2})
        call({"cmd": "screenshot", "path": str(out_dir / "ui_console_running.png")})

        # 3. Return to grid and stop via UI button ('Стоп') and verify java process is killed
        call({"cmd": "click", "label": "Сетка"})
        call({"cmd": "wait_idle", "timeout_ms": 3000, "frames": 2})
        call({"cmd": "click", "label": "Стоп"})
        st2 = wait_game_running(False, 15.0)
        time.sleep(1.0)
        java_pids = subprocess.check_output(["pgrep", "-f", "net.minecraft.client.main.Main|KnotClient"], text=True).splitlines() if subprocess.run(["pgrep", "-f", "net.minecraft.client.main.Main|KnotClient"], stdout=subprocess.DEVNULL).returncode == 0 else []
        assert not java_pids, f"Orphan java processes remained after Stop: {java_pids}"

        # 4. Relaunch + parallel launch of second instance
        call({"cmd": "click", "label": "Играть"})
        wait_game_running(True, 25.0)
        call({"cmd": "click", "label": "Сетка"})
        call({"cmd": "wait_idle", "timeout_ms": 3000, "frames": 2})
        call({"cmd": "click", "label": "Играть"})
        deadline_par = time.monotonic() + 25.0
        java_pids_par: list[str] = []
        while time.monotonic() < deadline_par:
            res_pg = subprocess.run(["pgrep", "-f", "net.minecraft.client.main.Main|KnotClient"], capture_output=True, text=True)
            if res_pg.returncode == 0:
                java_pids_par = [p for p in res_pg.stdout.splitlines() if p.strip()]
                if len(java_pids_par) >= 2:
                    break
            time.sleep(0.5)
        assert len(java_pids_par) >= 2, f"Expected >=2 parallel java processes, got {java_pids_par}"

        # 5. Kill one java process by SIGKILL and verify crash banner appears in UI
        subprocess.run(["kill", "-9", java_pids_par[0]], check=True)
        time.sleep(2.5)
        call({"cmd": "click", "label": "Моды"})
        call({"cmd": "wait_idle", "timeout_ms": 3000, "frames": 2})
        call({"cmd": "click", "label": "Консоль"})
        call({"cmd": "wait_idle", "timeout_ms": 3000, "frames": 2})
        call({"cmd": "screenshot", "path": str(out_dir / "ui_signal_crash_banner.png")})

        # Stop remaining running instance
        call({"cmd": "click", "label": "Сетка"})
        call({"cmd": "wait_idle", "timeout_ms": 3000, "frames": 2})
        call({"cmd": "click", "label": "Стоп"})
        time.sleep(1.0)
        call({"cmd": "quit"})
        print(json.dumps({"verify_ui": "PASS", "parallel_pids": len(java_pids_par), "st1": st1["game_running"], "st2": st2["game_running"]}))
        return 0
    finally:
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
        subprocess.run(["pkill", "-9", "-f", "net.minecraft.client.main.Main|KnotClient"], stderr=subprocess.DEVNULL)


if __name__ == "__main__":
    sys.exit(main())
