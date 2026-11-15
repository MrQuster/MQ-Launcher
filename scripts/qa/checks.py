#!/usr/bin/env python3
"""Automated QA checks for MQ Launcher UI dumps, real screenshots, and scenario results."""

from __future__ import annotations

import argparse
import json
import math
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any

import numpy as np
from PIL import Image

# Technical / brand / UI domain allowlist that legitimately appears alongside Russian copy.
EN_ALLOWLIST = {
    "mq",
    "minecraft",
    "mojang",
    "microsoft",
    "java",
    "jvm",
    "jre",
    "jdk",
    "neoforge",
    "forge",
    "fabric",
    "quilt",
    "vanilla",
    "optifine",
    "modrinth",
    "curseforge",
    "mrpack",
    "mods",
    "mod",
    "fps",
    "cpu",
    "gpu",
    "ram",
    "ssd",
    "mb",
    "gb",
    "ms",
    "px",
    "hz",
    "api",
    "url",
    "uri",
    "dns",
    "http",
    "https",
    "tls",
    "ssl",
    "json",
    "yaml",
    "png",
    "cli",
    "ui",
    "hud",
    "rgb",
    "rgba",
    "ok",
    "id",
    "uuid",
    "pid",
    "github",
    "discord",
    "linux",
    "wayland",
    "x11",
    "wgpu",
    "vulkan",
    "opengl",
    "glow",
    "offline",
    "online",
    "ely.by",
    "ely",
    "xmx",
    "xms",
    "gc",
    "g1gc",
    "zgc",
    "shenandoah",
    "aikar",
    "ru_ru",
    "en_us",
    "ctrl",
    "alt",
    "shift",
    "enter",
    "esc",
    "tab",
    "auto",
    "default",
    "preset",
    "pro",
    "v1",
    "v2",
    "x86_64",
    "aarch64",
}

BANNED_JARGON = (
    "todo",
    "fixme",
    "lorem",
    "ipsum",
    "placeholder",
    "stub",
    "dummy",
    "test123",
    "asdf",
    "qwerty",
    "какашк",
    "хз",
    "лол",
    "кек",
    "фигн",
)

EMOJI_RE = re.compile(
    "["
    "\U0001F300-\U0001F5FF"
    "\U0001F600-\U0001F64F"
    "\U0001F680-\U0001F6FF"
    "\U0001F700-\U0001F77F"
    "\U0001F780-\U0001F7FF"
    "\U0001F800-\U0001F8FF"
    "\U0001F900-\U0001F9FF"
    "\U0001FA00-\U0001FA6F"
    "\U0001FA70-\U0001FAFF"
    "\u2600-\u26FF"
    "\u2700-\u27BF"
    "]+",
    flags=re.UNICODE,
)

EN_WORD_RE = re.compile(r"\b[A-Za-z][A-Za-z0-9_.\-]{1,}\b")
CYRILLIC_RE = re.compile(r"[А-Яа-яЁё]")
LATIN_RE = re.compile(r"[A-Za-z]")


def srgb_to_linear(c: float) -> float:
    if c <= 0.04045:
        return c / 12.92
    return ((c + 0.055) / 1.055) ** 2.4


def relative_luminance(rgba: tuple[float, float, float, float] | list[int]) -> float:
    r, g, b = rgba[0] / 255.0, rgba[1] / 255.0, rgba[2] / 255.0
    return 0.2126 * srgb_to_linear(r) + 0.7152 * srgb_to_linear(g) + 0.0722 * srgb_to_linear(b)


def contrast_ratio(c1: list[int] | tuple[float, ...], c2: list[int] | tuple[float, ...]) -> float:
    l1 = relative_luminance(c1)
    l2 = relative_luminance(c2)
    hi, lo = max(l1, l2), min(l1, l2)
    return (hi + 0.05) / (lo + 0.05)


def rect_intersection_area(a: dict[str, float], b: dict[str, float]) -> float:
    x1 = max(a["x"], b["x"])
    y1 = max(a["y"], b["y"])
    x2 = min(a["x"] + a["w"], b["x"] + b["w"])
    y2 = min(a["y"] + a["h"], b["y"] + b["h"])
    if x2 <= x1 or y2 <= y1:
        return 0.0
    return (x2 - x1) * (y2 - y1)


def rect_area(r: dict[str, float]) -> float:
    return max(0.0, r["w"]) * max(0.0, r["h"])


def check_geometry(ui_dump: dict[str, Any]) -> dict[str, Any]:
    elements = ui_dump.get("elements", [])
    vp = ui_dump.get("viewport", {})
    vp_w = float(vp.get("width", 1280.0))
    vp_h = float(vp.get("height", 800.0))

    overlaps: list[dict[str, Any]] = []
    clipped: list[dict[str, Any]] = []
    small_targets: list[dict[str, Any]] = []
    duplicate_ids: list[dict[str, Any]] = []
    missing_tooltips: list[dict[str, Any]] = []

    seen_ids: dict[str, list[dict[str, Any]]] = {}
    visible_interactive: list[dict[str, Any]] = []

    for el in elements:
        rect = el.get("rect", {})
        w = float(rect.get("w", 0.0))
        h = float(rect.get("h", 0.0))
        x = float(rect.get("x", 0.0))
        y = float(rect.get("y", 0.0))
        visible = bool(el.get("visible", True))
        interactive = bool(el.get("interactive", False))
        kind = str(el.get("kind", ""))
        el_id = str(el.get("id", ""))

        in_viewport = visible and (x + w > 1.0 and y + h > 1.0 and x < vp_w - 1.0 and y < vp_h - 1.0)
        if not in_viewport:
            continue

        if el_id:
            seen_ids.setdefault(el_id, []).append(el)

        if el.get("clipped", False):
            # Ignore items that lie mostly outside clip_rect due to scroll offset.
            clip = el.get("clip_rect", {})
            cw = float(clip.get("w", vp_w))
            ch = float(clip.get("h", vp_h))
            inter = rect_intersection_area(rect, clip) if cw > 0 and ch > 0 else rect_area(rect)
            area = rect_area(rect)
            if area > 0 and 0.1 < (inter / area) < 0.98 and kind not in ("list_row", "card"):
                clipped.append(
                    {
                        "id": el_id,
                        "kind": kind,
                        "label": el.get("label", ""),
                        "rect": rect,
                        "clip_rect": clip,
                    }
                )

        if interactive:
            visible_interactive.append(el)
            if w < 23.5 or h < 23.5:
                small_targets.append(
                    {
                        "id": el_id,
                        "kind": kind,
                        "label": el.get("label", ""),
                        "w": round(w, 2),
                        "h": round(h, 2),
                    }
                )
            if (kind == "icon_button" or (not el.get("label") and not el.get("text"))) and not el.get("has_tooltip", False):
                missing_tooltips.append(
                    {
                        "id": el_id,
                        "kind": kind,
                        "tech_name": el.get("tech_name"),
                    }
                )

    for el_id, group in seen_ids.items():
        if len(group) > 1:
            duplicate_ids.append(
                {
                    "id": el_id,
                    "count": len(group),
                    "kinds": [g.get("kind") for g in group],
                }
            )

    for i in range(len(visible_interactive)):
        a = visible_interactive[i]
        ra = a["rect"]
        aa = rect_area(ra)
        if aa <= 0:
            continue
        for j in range(i + 1, len(visible_interactive)):
            b = visible_interactive[j]
            rb = b["rect"]
            ab = rect_area(rb)
            if ab <= 0:
                continue
            inter = rect_intersection_area(ra, rb)
            if inter <= 4.0:
                continue
            smaller = min(aa, ab)
            # Container/child or intentionally stacked header action vs row:
            # Flag collisions unless one cleanly contains the other AND they are different structural roles,
            # or flag all non-contained overlaps.
            containment = inter / smaller
            if containment < 0.85 or a.get("kind") == b.get("kind"):
                overlaps.append(
                    {
                        "a": {"id": a.get("id"), "kind": a.get("kind"), "label": a.get("label"), "rect": ra},
                        "b": {"id": b.get("id"), "kind": b.get("kind"), "label": b.get("label"), "rect": rb},
                        "intersection_area": round(inter, 2),
                    }
                )

    return {
        "overlaps": overlaps,
        "clipped": clipped,
        "small_targets": small_targets,
        "duplicate_ids": duplicate_ids,
        "missing_tooltips": missing_tooltips,
    }


def measure_png_patch_contrast(img_rgb: np.ndarray, box: tuple[int, int, int, int]) -> dict[str, Any] | None:
    x0, y0, x1, y1 = box
    h, w, _ = img_rgb.shape
    x0 = max(0, min(w - 1, x0))
    x1 = max(x0 + 1, min(w, x1))
    y0 = max(0, min(h - 1, y0))
    y1 = max(y0 + 1, min(h, y1))
    if (x1 - x0) < 6 or (y1 - y0) < 6:
        return None
    patch = img_rgb[y0:y1, x0:x1].astype(np.float32) / 255.0
    # Compute WCAG relative luminance per pixel.
    lin = np.where(patch <= 0.04045, patch / 12.92, ((patch + 0.055) / 1.055) ** 2.4)
    lum = 0.2126 * lin[:, :, 0] + 0.7152 * lin[:, :, 1] + 0.0722 * lin[:, :, 2]

    # Compare outer background quantiles (border pixels) vs contrasting foreground extremes.
    border = np.concatenate([lum[0, :], lum[-1, :], lum[:, 0], lum[:, -1]])
    bg_lum = float(np.median(border))
    # Foreground pixels differ maximally from bg_lum (top 5% most distant luminance pixels).
    flat = lum.reshape(-1)
    dists = np.abs(flat - bg_lum)
    k = max(4, int(len(flat) * 0.04))
    top_idx = np.argpartition(dists, -k)[-k:]
    fg_lum = float(np.median(flat[top_idx]))
    if abs(fg_lum - bg_lum) < 0.02:
        return None
    hi, lo = max(fg_lum, bg_lum), min(fg_lum, bg_lum)
    ratio = (hi + 0.05) / (lo + 0.05)
    return {
        "png_ratio": round(ratio, 2),
        "bg_lum": round(bg_lum, 4),
        "fg_lum": round(fg_lum, 4),
    }


def check_contrast_and_color_budget(ui_dump: dict[str, Any], png_path: Path) -> dict[str, Any]:
    with Image.open(png_path) as im:
        rgb = np.asarray(im.convert("RGB"))

    img_h, img_w, _ = rgb.shape
    vp = ui_dump.get("viewport", {})
    ppp = float(vp.get("pixels_per_point", 1.0) or 1.0)
    # If viewport dimensions differ from actual PNG pixel size, infer scale.
    vp_w = float(vp.get("width", img_w / ppp) or (img_w / ppp))
    scale_x = img_w / vp_w if vp_w > 0 else ppp
    vp_h = float(vp.get("height", img_h / ppp) or (img_h / ppp))
    scale_y = img_h / vp_h if vp_h > 0 else ppp

    contrast_failures: list[dict[str, Any]] = []
    contrast_checked = 0

    for el in ui_dump.get("elements", []):
        if not el.get("visible", True):
            continue
        label = str(el.get("label", "") or "")
        text = str(el.get("text", "") or "")
        kind = str(el.get("kind", ""))
        if not (label or text):
            continue
        rect = el.get("rect", {})
        x0 = int(round(float(rect.get("x", 0.0)) * scale_x))
        y0 = int(round(float(rect.get("y", 0.0)) * scale_y))
        x1 = int(round((float(rect.get("x", 0.0)) + float(rect.get("w", 0.0))) * scale_x))
        y1 = int(round((float(rect.get("y", 0.0)) + float(rect.get("h", 0.0))) * scale_y))
        if x1 <= 0 or y1 <= 0 or x0 >= img_w or y0 >= img_h:
            continue

        fg = el.get("fg_color", [0, 0, 0, 0])
        bg = el.get("bg_color", [0, 0, 0, 0])
        reported_ratio = None
        if fg[3] > 0 and bg[3] > 0:
            reported_ratio = round(contrast_ratio(fg, bg), 2)

        png_meas = measure_png_patch_contrast(rgb, (x0, y0, x1, y1))
        required = 3.0 if kind in ("badge", "progress", "toggle") else 4.5
        effective_ratio = png_meas["png_ratio"] if png_meas else reported_ratio
        if effective_ratio is None:
            continue

        contrast_checked += 1
        # Flag only when BOTH reported (if present) and real PNG measurement fall short,
        # or when PNG measurement clearly fails (< required) on solid painted widgets.
        if png_meas and png_meas["png_ratio"] < required and (reported_ratio is None or reported_ratio < required):
            contrast_failures.append(
                {
                    "id": el.get("id"),
                    "kind": kind,
                    "label": label or text,
                    "required": required,
                    "png_ratio": png_meas["png_ratio"],
                    "reported_ratio": reported_ratio,
                }
            )
        elif reported_ratio is not None and reported_ratio < required and (png_meas is None or png_meas["png_ratio"] < required):
            contrast_failures.append(
                {
                    "id": el.get("id"),
                    "kind": kind,
                    "label": label or text,
                    "required": required,
                    "png_ratio": png_meas["png_ratio"] if png_meas else None,
                    "reported_ratio": reported_ratio,
                }
            )

    # Color budget <= 2%: vivid accent pixels across the full viewport.
    arr = rgb.astype(np.float32) / 255.0
    cmax = arr.max(axis=2)
    cmin = arr.min(axis=2)
    delta = cmax - cmin
    sat = np.where(cmax > 1e-5, delta / np.maximum(cmax, 1e-5), 0.0)
    # Vivid accent threshold: high saturation & noticeable chroma & brightness
    accent_mask = (sat > 0.45) & (delta > 0.22) & (cmax > 0.25)
    accent_pixels = int(np.count_nonzero(accent_mask))
    total_pixels = int(img_w * img_h)
    accent_pct = (accent_pixels / total_pixels * 100.0) if total_pixels > 0 else 0.0

    return {
        "contrast_checked": contrast_checked,
        "contrast_failures": contrast_failures,
        "color_budget": {
            "accent_pixels": accent_pixels,
            "total_pixels": total_pixels,
            "accent_percent": round(accent_pct, 3),
            "limit_percent": 2.0,
            "within_budget": accent_pct <= 2.0,
        },
    }


def check_copy_and_localization(ui_dump: dict[str, Any]) -> dict[str, Any]:
    emoji_hits: list[dict[str, Any]] = []
    jargon_hits: list[dict[str, Any]] = []
    mixed_en_hits: list[dict[str, Any]] = []

    for el in ui_dump.get("elements", []):
        if not el.get("visible", True):
            continue
        el_id = str(el.get("id", ""))
        for field in ("label", "text", "tooltip"):
            val = el.get(field)
            if not val or not isinstance(val, str):
                continue
            if EMOJI_RE.search(val):
                emoji_hits.append({"id": el_id, "field": field, "value": val})
            low = val.lower()
            for bad in BANNED_JARGON:
                if re.search(rf"\b{re.escape(bad)}", low):
                    jargon_hits.append({"id": el_id, "field": field, "token": bad, "value": val})
            if CYRILLIC_RE.search(val) and LATIN_RE.search(val):
                unknown_en = []
                for m in EN_WORD_RE.findall(val):
                    tok = m.strip("._-").lower()
                    if not tok or tok.isdigit() or tok in EN_ALLOWLIST:
                        continue
                    # Ignore version/size specs like 1.20.1-alpha or 2x.
                    if re.match(r"^(v?\d+[a-z0-9.\-]*|[a-z]\d+)$", tok):
                        continue
                    unknown_en.append(m)
                if unknown_en:
                    mixed_en_hits.append(
                        {"id": el_id, "field": field, "tokens": unknown_en, "value": val}
                    )

    return {
        "emoji_hits": emoji_hits,
        "jargon_hits": jargon_hits,
        "mixed_en_hits": mixed_en_hits,
    }


def tesseract_env() -> dict[str, str]:
    env = os.environ.copy()
    if not env.get("TESSDATA_PREFIX"):
        user_tess = Path.home() / ".local" / "share" / "tessdata"
        if (user_tess / "rus.traineddata").exists():
            env["TESSDATA_PREFIX"] = str(user_tess)
    return env


def detect_tesseract_langs() -> str | None:
    if not shutil.which("tesseract"):
        return None
    proc = subprocess.run(
        ["tesseract", "--list-langs"],
        env=tesseract_env(),
        capture_output=True,
        text=True,
        timeout=5,
        check=False,
    )
    langs = {line.strip() for line in proc.stdout.splitlines()[1:] if line.strip()}
    if "rus" in langs and "eng" in langs:
        return "rus+eng"
    if "rus" in langs:
        return "rus"
    if "eng" in langs:
        return "eng"
    return None


def check_ocr(ui_dump: dict[str, Any], png_path: Path, lang: str | None) -> dict[str, Any]:
    if lang is None:
        return {"available": False, "reason": "tesseract or required tessdata missing"}
    proc = subprocess.run(
        ["tesseract", str(png_path), "stdout", "-l", lang, "--psm", "11"],
        env=tesseract_env(),
        capture_output=True,
        text=True,
        timeout=20,
        check=False,
    )
    ocr_text = proc.stdout or ""
    ocr_low = ocr_text.lower()

    expected_labels: list[str] = []
    matched: list[str] = []
    missed: list[str] = []
    for el in ui_dump.get("elements", []):
        if not el.get("visible", True):
            continue
        lbl = str(el.get("label", "") or "").strip()
        if len(lbl) < 3 or len(lbl) > 40:
            continue
        # Only verify ASCII-matchable tokens when rus tessdata is absent.
        if "rus" not in lang and CYRILLIC_RE.search(lbl):
            continue
        expected_labels.append(lbl)
        words = [w.lower() for w in re.findall(r"[A-Za-zА-Яа-яЁё0-9]{3,}", lbl)]
        if words and any(w in ocr_low for w in words):
            matched.append(lbl)
        elif words:
            missed.append(lbl)

    return {
        "available": proc.returncode == 0,
        "lang": lang,
        "chars_extracted": len(ocr_text.strip()),
        "expected_checked": len(expected_labels),
        "matched_count": len(matched),
        "missed_sample": missed[:10],
    }


def check_dead_buttons(result_data: dict[str, Any] | None) -> list[dict[str, Any]]:
    if not result_data:
        return []
    dead: list[dict[str, Any]] = []
    for step in result_data.get("steps", []):
        if step.get("cmd") != "click":
            continue
        resp = step.get("response", {})
        if not resp.get("ok"):
            continue
        if resp.get("was_active"):
            continue
        # If click matched a button/nav_item/tab/icon_button and reported zero ui_changed:
        kind = resp.get("matched_kind", "")
        if kind in ("button", "icon_button", "nav_item", "tab", "segmented_option", "toggle", "chip"):
            if resp.get("ui_changed") is False and not step.get("allow_noop", False):
                dead.append(
                    {
                        "step": step.get("step"),
                        "target": step.get("target"),
                        "matched_id": resp.get("matched_id"),
                        "matched_label": resp.get("matched_label"),
                        "kind": kind,
                    }
                )
    return dead


def run_checks(scenario_dir: Path, strict: bool = False) -> dict[str, Any]:
    scenario_dir = scenario_dir.resolve()
    result_path = scenario_dir / "result.json"
    result_data = json.loads(result_path.read_text(encoding="utf-8")) if result_path.exists() else None

    ui_files = sorted(scenario_dir.glob("ui-*.json"))
    png_files = sorted(scenario_dir.glob("step-*.png"))
    tess_lang = detect_tesseract_langs()

    step_reports: list[dict[str, Any]] = []
    errors: list[dict[str, Any]] = []
    warnings: list[dict[str, Any]] = []

    # Pair ui-NN.json with step-NN.png (or nearest preceding/following screenshot if index differs).
    png_by_idx = {}
    for p in png_files:
        m = re.search(r"step-(\d+)\.png$", p.name)
        if m:
            png_by_idx[int(m.group(1))] = p

    for ui_file in ui_files:
        m = re.search(r"ui-(\d+)\.json$", ui_file.name)
        idx = int(m.group(1)) if m else -1
        ui_dump = json.loads(ui_file.read_text(encoding="utf-8"))
        geom = check_geometry(ui_dump)
        copy_res = check_copy_and_localization(ui_dump)

        png_path = png_by_idx.get(idx)
        if png_path is None and idx >= 0:
            # Find screenshot with matching step or latest prior screenshot if explicit
            same_or_next = png_by_idx.get(idx + 1) or png_by_idx.get(idx - 1)
            png_path = same_or_next

        visual_res: dict[str, Any] | None = None
        ocr_res: dict[str, Any] | None = None
        if png_path is not None and png_path.exists():
            visual_res = check_contrast_and_color_budget(ui_dump, png_path)
            ocr_res = check_ocr(ui_dump, png_path, tess_lang)

        # Record hard errors vs audit warnings (promoted to errors under --strict).
        soft_bucket = errors if strict else warnings
        for item in geom["small_targets"]:
            soft_bucket.append({"step": idx, "check": "hit_target_lt_24px", **item})
        for item in geom["duplicate_ids"]:
            errors.append({"step": idx, "check": "duplicate_id", **item})
        for item in geom["missing_tooltips"]:
            soft_bucket.append({"step": idx, "check": "missing_tooltip", **item})
        for item in geom["overlaps"]:
            errors.append({"step": idx, "check": "overlap", **item})
        for item in geom["clipped"]:
            soft_bucket.append({"step": idx, "check": "clipped", **item})
        for item in copy_res["emoji_hits"]:
            errors.append({"step": idx, "check": "emoji_in_ui", **item})
        for item in copy_res["jargon_hits"]:
            errors.append({"step": idx, "check": "banned_jargon", **item})
        for item in copy_res["mixed_en_hits"]:
            soft_bucket.append({"step": idx, "check": "mixed_ru_en", **item})

        if visual_res is not None:
            for item in visual_res["contrast_failures"]:
                errors.append({"step": idx, "check": "low_contrast", **item})
            if not visual_res["color_budget"]["within_budget"]:
                errors.append(
                    {
                        "step": idx,
                        "check": "color_budget_exceeded",
                        **visual_res["color_budget"],
                    }
                )

        step_reports.append(
            {
                "step": idx,
                "ui_file": ui_file.name,
                "png_file": png_path.name if png_path else None,
                "elements_count": len(ui_dump.get("elements", [])),
                "geometry": geom,
                "visual": visual_res,
                "copy": copy_res,
                "ocr": ocr_res,
            }
        )

    # Also verify standalone PNGs that didn't have a paired ui-NN.json for color budget & OCR.
    paired_pngs = {r["png_file"] for r in step_reports if r["png_file"]}
    for p in png_files:
        if p.name in paired_pngs:
            continue
        with Image.open(p) as im:
            rgb = np.asarray(im.convert("RGB"))
        arr = rgb.astype(np.float32) / 255.0
        cmax = arr.max(axis=2)
        cmin = arr.min(axis=2)
        delta = cmax - cmin
        sat = np.where(cmax > 1e-5, delta / np.maximum(cmax, 1e-5), 0.0)
        accent_pct = float(np.count_nonzero((sat > 0.45) & (delta > 0.22) & (cmax > 0.25))) / float(
            rgb.shape[0] * rgb.shape[1]
        ) * 100.0
        if accent_pct > 2.0:
            errors.append(
                {
                    "png_file": p.name,
                    "check": "color_budget_exceeded",
                    "accent_percent": round(accent_pct, 3),
                    "limit_percent": 2.0,
                }
            )

    dead_buttons = check_dead_buttons(result_data)
    for item in dead_buttons:
        errors.append({"check": "dead_button", **item})

    report = {
        "scenario_dir": str(scenario_dir),
        "tesseract_lang": tess_lang,
        "ui_dumps_checked": len(ui_files),
        "screenshots_checked": len(png_files),
        "error_count": len(errors),
        "warning_count": len(warnings),
        "passed": len(errors) == 0,
        "errors": errors,
        "warnings": warnings,
        "dead_buttons": dead_buttons,
        "steps": step_reports,
    }
    return report


def main() -> int:
    parser = argparse.ArgumentParser(description="Run automated QA checks on a scenario output directory.")
    parser.add_argument("scenario_dir", type=Path, help="Directory with ui-*.json, step-*.png, result.json")
    parser.add_argument("--out", type=Path, default=None, help="Optional JSON output path (defaults to <scenario_dir>/checks.json)")
    parser.add_argument("--strict", action="store_true", help="Promote geometry/tooltip/copy warnings to hard errors.")
    args = parser.parse_args()

    report = run_checks(args.scenario_dir, strict=args.strict)
    out_path = args.out or (args.scenario_dir / "checks.json")
    out_path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

    print(
        f"[qa-checks] dir={args.scenario_dir} ui_dumps={report['ui_dumps_checked']} "
        f"pngs={report['screenshots_checked']} errors={report['error_count']} warnings={report['warning_count']} "
        f"tesseract={report['tesseract_lang']}"
    )
    if report["errors"]:
        for err in report["errors"][:20]:
            print(f"  ERROR: {json.dumps(err, ensure_ascii=False)}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
