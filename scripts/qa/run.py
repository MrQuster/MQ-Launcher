#!/usr/bin/env python3
"""YAML scenario runner for MQ Launcher QA test-hooks harness."""

from __future__ import annotations

import argparse
import json
import os
import shutil
import socket
import stat
import subprocess
import sys
import time
from datetime import datetime
from pathlib import Path
from typing import Any

import yaml

# Ensure sibling import works when invoked directly from any working directory.
sys.path.insert(0, str(Path(__file__).resolve().parent))
import checks  # noqa: E402


REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_TARGET_DIR = Path(os.environ.get("CARGO_TARGET_DIR", str(REPO_ROOT / "target")))


def resolve_mq_binary(explicit_bin: Path | None, no_build: bool) -> Path:
    if explicit_bin is not None:
        if not explicit_bin.exists():
            raise FileNotFoundError(f"mq binary not found at {explicit_bin}")
        return explicit_bin

    candidate = DEFAULT_TARGET_DIR / "debug" / "mq"
    if no_build:
        if not candidate.exists():
            raise FileNotFoundError(f"mq binary not found at {candidate} and --no-build was set")
        return candidate

    env = os.environ.copy()
    env.setdefault("CARGO_TARGET_DIR", str(DEFAULT_TARGET_DIR))
    env.setdefault("CARGO_BUILD_JOBS", "2")
    sccache = shutil.which("sccache") or str(Path.home() / ".cargo" / "bin" / "sccache")
    if Path(sccache).exists() and "RUSTC_WRAPPER" not in env:
        env["RUSTC_WRAPPER"] = sccache
    print(f"[qa-run] Building mq with --features test-hooks into {env['CARGO_TARGET_DIR']}...")
    proc = subprocess.run(
        ["cargo", "build", "--bin", "mq", "--features", "test-hooks", "--locked"],
        cwd=REPO_ROOT,
        env=env,
        check=False,
    )
    if proc.returncode != 0:
        raise RuntimeError(f"cargo build --features test-hooks failed with code {proc.returncode}")
    if not candidate.exists():
        raise FileNotFoundError(f"Built binary missing at {candidate}")
    return candidate


def normalize_step(raw_step: Any, step_idx: int, out_dir: Path) -> tuple[str, dict[str, Any]]:
    if isinstance(raw_step, str):
        cmd = raw_step.strip()
        payload: dict[str, Any] = {"cmd": cmd}
    elif isinstance(raw_step, dict):
        if "cmd" in raw_step:
            payload = dict(raw_step)
            cmd = str(payload["cmd"])
        elif len(raw_step) == 1:
            cmd, val = next(iter(raw_step.items()))
            cmd = str(cmd)
            if val is None:
                payload = {"cmd": cmd}
            elif isinstance(val, dict):
                payload = {"cmd": cmd, **val}
            elif cmd in ("click", "double_click", "right_click", "hover"):
                payload = {"cmd": cmd, "label": str(val)}
            elif cmd == "type":
                payload = {"cmd": cmd, "text": str(val)}
            elif cmd == "key":
                payload = {"cmd": cmd, "key": str(val)}
            elif cmd == "goto":
                payload = {"cmd": cmd, "tab": str(val)}
            elif cmd == "screenshot":
                payload = {"cmd": cmd, "path": str(val)}
            elif cmd == "wait_idle":
                payload = {"cmd": cmd, "timeout_ms": int(val)}
            elif cmd == "frame_stats":
                payload = {"cmd": cmd, "action": str(val)}
            elif cmd == "drop_file":
                payload = {"cmd": cmd, "path": str(val)}
            else:
                payload = {"cmd": cmd, "value": val}
        else:
            raise ValueError(f"Step {step_idx} must contain 'cmd' or a single action key: {raw_step!r}")
    else:
        raise ValueError(f"Invalid step at index {step_idx}: {raw_step!r}")

    if cmd == "screenshot" and not payload.get("path"):
        payload["path"] = str(out_dir / f"step-{step_idx:02d}.png")
    elif cmd == "screenshot":
        p = Path(str(payload["path"]))
        if not p.is_absolute():
            payload["path"] = str(out_dir / p)

    if cmd == "screenshot_region" and not payload.get("path"):
        payload["path"] = str(out_dir / f"step-{step_idx:02d}-region.png")

    return cmd, payload


def make_sandbox_env(data_dir: Path, deterministic: bool) -> dict[str, str]:
    data_dir = data_dir.resolve()
    home = Path.home().resolve()
    for forbidden in (home / ".local" / "share" / "mq", home / ".config" / "mq"):
        if data_dir == forbidden or forbidden in data_dir.parents:
            raise RuntimeError(f"Refusing to use user profile path as QA sandbox: {data_dir}")

    xdg_root = data_dir / "_xdg"
    for sub in ("data", "config", "cache"):
        (xdg_root / sub).mkdir(parents=True, exist_ok=True)
    data_dir.mkdir(parents=True, exist_ok=True)

    env = os.environ.copy()
    env["MQ_DATA_DIR"] = str(data_dir)
    env["MQ_TEST_SANDBOX"] = "1"
    env["MQ_TEST_HOOKS"] = "1"
    env["MQ_TEST_DETERMINISTIC"] = "1" if deterministic else "0"
    env["MQ_TEST_HOOKS_SOCKET"] = str(data_dir / "test-hooks.sock")
    env["XDG_DATA_HOME"] = str(xdg_root / "data")
    env["XDG_CONFIG_HOME"] = str(xdg_root / "config")
    env["XDG_CACHE_HOME"] = str(xdg_root / "cache")
    # Force deterministic English/Russian default independent of host quirks unless specified.
    env.setdefault("NO_COLOR", "1")
    return env


def run_mq_doctor(mq_bin: Path, env: dict[str, str], out_dir: Path) -> dict[str, Any]:
    proc = subprocess.run(
        [str(mq_bin), "doctor"],
        env=env,
        capture_output=True,
        text=True,
        timeout=30,
        check=False,
    )
    combined = (proc.stdout or "") + ("\n[STDERR]\n" + proc.stderr if proc.stderr else "")
    (out_dir / "doctor.txt").write_text(combined, encoding="utf-8")
    return {
        "returncode": proc.returncode,
        "output_file": "doctor.txt",
        "lines": len(combined.splitlines()),
    }


def wait_for_socket(sock_path: Path, proc: subprocess.Popen[str], timeout_s: float) -> socket.socket:
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            raise RuntimeError(f"mq exited early with code {proc.returncode} before socket {sock_path} appeared")
        if sock_path.exists():
            mode = stat.S_IMODE(sock_path.stat().st_mode)
            if (mode & 0o077) != 0:
                raise RuntimeError(f"Insecure permissions {oct(mode)} on {sock_path}, expected 0600")
            sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            try:
                sock.settimeout(35.0)
                sock.connect(str(sock_path))
                return sock
            except OSError:
                sock.close()
        time.sleep(0.05)
    raise TimeoutError(f"Timed out after {timeout_s}s waiting for {sock_path}")


class JsonLineClient:
    def __init__(self, sock: socket.socket) -> None:
        self.sock = sock
        self.reader = sock.makefile("r", encoding="utf-8")
        self.writer = sock.makefile("w", encoding="utf-8")

    def request(self, payload: dict[str, Any]) -> dict[str, Any]:
        line = json.dumps(payload, ensure_ascii=False)
        self.writer.write(line + "\n")
        self.writer.flush()
        try:
            resp_line = self.reader.readline()
        except TimeoutError as exc:
            raise TimeoutError(
                f"Timed out waiting for response to {payload.get('cmd')!r}"
            ) from exc
        if not resp_line:
            raise EOFError(f"Socket closed while waiting response for {payload.get('cmd')}")
        return json.loads(resp_line)

    def close(self) -> None:
        for handle in (self.writer, self.reader, self.sock):
            try:
                handle.close()
            except OSError:
                pass


def build_montage(out_dir: Path) -> str | None:
    pngs = sorted(out_dir.glob("step-*.png"))
    if not pngs or not shutil.which("montage"):
        return None
    montage_path = out_dir / "montage.png"
    cmd = [
        "montage",
        *[str(p) for p in pngs],
        "-label",
        "%t",
        "-tile",
        "2x",
        "-geometry",
        "640x400+10+10",
        "-background",
        "#18181b",
        "-fill",
        "#e4e4e7",
        str(montage_path),
    ]
    proc = subprocess.run(cmd, capture_output=True, text=True, timeout=30, check=False)
    if proc.returncode == 0 and montage_path.exists():
        return montage_path.name
    return None


def run_scenario(
    scenario_path: Path,
    mq_bin: Path,
    run_out_dir: Path,
    base_data_dir: Path | None,
    backend: str,
    timeout_s: float,
    strict: bool = False,
) -> dict[str, Any]:
    spec = yaml.safe_load(scenario_path.read_text(encoding="utf-8"))
    scenario_name = str(spec.get("name") or scenario_path.stem)
    out_dir = run_out_dir / scenario_name
    out_dir.mkdir(parents=True, exist_ok=True)

    # Fresh sandbox per scenario unless user explicitly pointed MQ_DATA_DIR for single scenario.
    if base_data_dir is not None:
        data_dir = base_data_dir / scenario_name
        if data_dir.exists():
            shutil.rmtree(data_dir)
    else:
        data_dir = Path("/tmp/mq-qa") / run_out_dir.name / scenario_name
        if data_dir.exists():
            shutil.rmtree(data_dir)

    deterministic = bool(spec.get("deterministic", True))
    env = make_sandbox_env(data_dir, deterministic=deterministic)

    # Optional Xvfb / headless Weston wrapper if no display server is present.
    xvfb_proc: subprocess.Popen[str] | None = None
    if not env.get("DISPLAY") and not env.get("WAYLAND_DISPLAY"):
        if shutil.which("Xvfb"):
            display = ":99"
            xvfb_proc = subprocess.Popen(
                ["Xvfb", display, "-screen", "0", "1440x900x24"],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                text=True,
            )
            env["DISPLAY"] = display
            env["LIBGL_ALWAYS_SOFTWARE"] = "1"
            time.sleep(0.5)
        elif shutil.which("weston"):
            runtime_dir = data_dir / "_xdg" / "runtime"
            runtime_dir.mkdir(parents=True, exist_ok=True)
            runtime_dir.chmod(0o700)
            env["XDG_RUNTIME_DIR"] = str(runtime_dir)
            env["WAYLAND_DISPLAY"] = "wayland-qa"
            env["LIBGL_ALWAYS_SOFTWARE"] = "1"
            xvfb_proc = subprocess.Popen(
                ["weston", "--backend=headless", "--xwayland", "--socket=wayland-qa"],
                env=env,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                text=True,
            )
            time.sleep(0.8)
        else:
            raise RuntimeError("No DISPLAY/WAYLAND_DISPLAY and neither Xvfb nor weston is installed")

    doctor_info = run_mq_doctor(mq_bin, env, out_dir)

    sock_path = Path(env["MQ_TEST_HOOKS_SOCKET"])
    if sock_path.exists():
        sock_path.unlink()

    launch_cmd = [str(mq_bin), "--test-hooks", "--backend", backend]
    launcher_log = out_dir / "launcher.log"
    log_handle = launcher_log.open("w", encoding="utf-8")
    proc = subprocess.Popen(
        launch_cmd,
        cwd=REPO_ROOT,
        env=env,
        stdout=log_handle,
        stderr=subprocess.STDOUT,
        text=True,
    )

    step_records: list[dict[str, Any]] = []
    client: JsonLineClient | None = None
    frame_stats_final: dict[str, Any] | None = None
    scenario_error: str | None = None
    quit_sent = False

    try:
        sock = wait_for_socket(sock_path, proc, timeout_s=timeout_s)
        client = JsonLineClient(sock)

        # Wait for initial UI frame to stabilize.
        init_resp = client.request({"cmd": "wait_idle", "timeout_ms": 10000, "frames": 3})
        if not init_resp.get("ok"):
            raise RuntimeError(f"Initial wait_idle failed: {init_resp}")

        if "theme" in spec:
            resp = client.request({"cmd": "set_theme", "preset": str(spec["theme"])})
            if not resp.get("ok"):
                raise RuntimeError(f"Initial set_theme failed: {resp}")

        if "viewport" in spec and isinstance(spec["viewport"], dict):
            vp = spec["viewport"]
            resp = client.request(
                {"cmd": "resize", "width": float(vp["width"]), "height": float(vp["height"])}
            )
            if not resp.get("ok"):
                raise RuntimeError(f"Initial resize failed: {resp}")

        raw_steps = spec.get("steps", [])
        for idx, raw_step in enumerate(raw_steps, start=1):
            allow_noop = isinstance(raw_step, dict) and bool(raw_step.pop("allow_noop", False))
            cmd, payload = normalize_step(raw_step, idx, out_dir)
            t0 = time.monotonic()
            resp = client.request(payload)
            elapsed_ms = round((time.monotonic() - t0) * 1000.0, 2)

            if cmd == "dump_ui" and resp.get("ok"):
                ui_path = out_dir / f"ui-{idx:02d}.json"
                ui_path.write_text(json.dumps(resp, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

            if cmd == "frame_stats" and resp.get("ok"):
                frame_stats_final = resp

            rec: dict[str, Any] = {
                "step": idx,
                "cmd": cmd,
                "payload": payload,
                "allow_noop": allow_noop,
                "elapsed_ms": elapsed_ms,
                "response": resp if cmd != "dump_ui" else {
                    "ok": resp.get("ok"),
                    "frame": resp.get("frame"),
                    "tab": resp.get("tab"),
                    "elements_count": len(resp.get("elements", [])),
                    "ui_file": f"ui-{idx:02d}.json",
                },
            }
            if "label" in payload:
                rec["target"] = payload["label"]
            elif "id" in payload:
                rec["target"] = payload["id"]
            step_records.append(rec)

            if not resp.get("ok"):
                raise RuntimeError(f"Step {idx} ({cmd}) failed: {resp}")

            if cmd == "quit":
                quit_sent = True
                break

        if not quit_sent:
            resp = client.request({"cmd": "quit"})
            step_records.append({"step": len(step_records) + 1, "cmd": "quit", "response": resp})
    except Exception as exc:
        scenario_error = str(exc)
    finally:
        if client is not None:
            client.close()
        try:
            proc.wait(timeout=8)
        except subprocess.TimeoutExpired:
            proc.terminate()
            try:
                proc.wait(timeout=4)
            except subprocess.TimeoutExpired:
                proc.kill()
        log_handle.close()
        if xvfb_proc is not None:
            xvfb_proc.terminate()
            xvfb_proc.wait(timeout=3)

    result_data = {
        "scenario": scenario_name,
        "scenario_file": str(scenario_path),
        "sandbox_data_dir": str(data_dir),
        "ok": scenario_error is None,
        "error": scenario_error,
        "exit_code": proc.returncode,
        "doctor": doctor_info,
        "frame_stats": frame_stats_final,
        "steps": step_records,
    }
    (out_dir / "result.json").write_text(
        json.dumps(result_data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )

    checks_report = checks.run_checks(out_dir, strict=strict)
    (out_dir / "checks.json").write_text(
        json.dumps(checks_report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    montage_name = build_montage(out_dir)

    return {
        "scenario": scenario_name,
        "out_dir": out_dir,
        "result": result_data,
        "checks": checks_report,
        "montage": montage_name,
    }


def write_markdown_report(run_out_dir: Path, runs: list[dict[str, Any]]) -> Path:
    lines: list[str] = [
        f"# MQ Launcher QA Report (`{run_out_dir.name}`)",
        "",
        f"Generated: {datetime.now().isoformat(timespec='seconds')}",
        "",
        "| Scenario | Steps | Run Status | QA Errors | Warnings | Frame p99 | Montage |",
        "| --- | ---: | --- | ---: | ---: | ---: | --- |",
    ]
    all_passed = True
    for item in runs:
        res = item["result"]
        chk = item["checks"]
        ok = bool(res["ok"] and chk["passed"])
        if not ok:
            all_passed = False
        fs = res.get("frame_stats") or {}
        p99 = f"{fs['p99_ms']:.2f} ms" if "p99_ms" in fs else "n/a"
        montage_rel = f"[{item['montage']}]({item['scenario']}/{item['montage']})" if item["montage"] else "none"
        lines.append(
            f"| {item['scenario']} | {len(res['steps'])} | {'PASS' if res['ok'] else 'FAIL'} "
            f"| {chk['error_count']} | {chk['warning_count']} | {p99} | {montage_rel} |"
        )

    lines.extend(["", f"**Overall:** {'PASS' if all_passed else 'FAIL'}", ""])

    for item in runs:
        res = item["result"]
        chk = item["checks"]
        lines.append(f"## Scenario: `{item['scenario']}`")
        lines.append("")
        lines.append(f"- Sandbox: `{res['sandbox_data_dir']}`")
        lines.append(f"- `mq doctor`: exit `{res['doctor']['returncode']}` ([doctor.txt]({item['scenario']}/doctor.txt))")
        if res.get("error"):
            lines.append(f"- Execution error: `{res['error']}`")
        if item["montage"]:
            lines.append(f"- Montage: [{item['montage']}]({item['scenario']}/{item['montage']})")
        if res.get("frame_stats"):
            fs = res["frame_stats"]
            lines.append(
                f"- Frame stats: count={fs.get('count')} max={fs.get('max_ms')}ms p99={fs.get('p99_ms')}ms "
                f"histogram={json.dumps(fs.get('histogram', {}))}"
            )
        if chk["errors"]:
            lines.append("")
            lines.append("### Errors")
            for err in chk["errors"]:
                lines.append(f"- `{json.dumps(err, ensure_ascii=False)}`")
        if chk["warnings"]:
            lines.append("")
            lines.append("### Warnings")
            for warn in chk["warnings"][:15]:
                lines.append(f"- `{json.dumps(warn, ensure_ascii=False)}`")
        lines.append("")

    report_path = run_out_dir / "report.md"
    report_path.write_text("\n".join(lines), encoding="utf-8")
    return report_path


def main() -> int:
    parser = argparse.ArgumentParser(description="Run YAML QA scenarios against mq test-hooks harness.")
    parser.add_argument("scenarios", nargs="+", type=Path, help="YAML scenario files to execute.")
    parser.add_argument("--bin", type=Path, default=None, help="Path to prebuilt mq binary with test-hooks.")
    parser.add_argument("--no-build", action="store_true", help="Do not run cargo build before running scenarios.")
    parser.add_argument("--out-dir", type=Path, default=None, help="Output directory (defaults to qa-out/<timestamp>).")
    parser.add_argument(
        "--backend",
        choices=("auto", "glow", "wgpu"),
        default="auto",
        help="Graphical rendering backend passed to mq.",
    )
    parser.add_argument("--timeout", type=float, default=30.0, help="Socket startup timeout in seconds.")
    parser.add_argument("--strict", action="store_true", help="Fail on audit warnings (hit targets <24px, missing tooltips, clipped, mixed RU/EN).")
    args = parser.parse_args()

    mq_bin = resolve_mq_binary(args.bin, no_build=args.no_build)
    run_id = datetime.now().strftime("%Y%m%d-%H%M%S")
    run_out_dir = (args.out_dir or (REPO_ROOT / "qa-out" / run_id)).resolve()
    run_out_dir.mkdir(parents=True, exist_ok=True)

    env_data_dir = os.environ.get("MQ_DATA_DIR")
    base_data_dir = Path(env_data_dir).resolve() if env_data_dir else None

    runs: list[dict[str, Any]] = []
    for scenario_path in args.scenarios:
        resolved = scenario_path if scenario_path.is_absolute() else (Path.cwd() / scenario_path).resolve()
        if not resolved.exists():
            resolved = (REPO_ROOT / scenario_path).resolve()
        print(f"[qa-run] Running scenario: {resolved}")
        runs.append(
            run_scenario(
                scenario_path=resolved,
                mq_bin=mq_bin,
                run_out_dir=run_out_dir,
                base_data_dir=base_data_dir,
                backend=args.backend,
                timeout_s=args.timeout,
                strict=args.strict,
            )
        )

    report_path = write_markdown_report(run_out_dir, runs)
    print(f"[qa-run] Report written to {report_path}")

    overall_ok = all(r["result"]["ok"] and r["checks"]["passed"] for r in runs)
    return 0 if overall_ok else 1


if __name__ == "__main__":
    sys.exit(main())
