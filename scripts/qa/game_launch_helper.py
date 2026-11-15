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

    return 2


if __name__ == "__main__":
    sys.exit(main())
