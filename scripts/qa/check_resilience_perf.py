#!/usr/bin/env python3
"""Perf, idle CPU/RSS, headless weston, stress-data, SIGKILL recovery, and security audit suite."""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import signal
import socket
import stat
import subprocess
import sys
import time
import tomllib
from pathlib import Path
from typing import Any

import yaml

REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_BIN = Path(os.environ.get("CARGO_TARGET_DIR", "/home/honey/mq/target")) / "debug" / "mq"

SENSITIVE_FILENAMES = {
    "accounts.json",
    "config.toml",
    "curseforge_api_key.secret",
    "test-hooks.sock",
}

SECRET_PATTERNS = [
    re.compile(r"ghp_[A-Za-z0-9]{20,}"),
    re.compile(r"github_pat_[A-Za-z0-9_]{20,}"),
    re.compile(r"Bearer\s+[A-Za-z0-9\-._~+/]+=*"),
    re.compile(r"\$2a\$10\$[A-Za-z0-9./]{20,}"),
    re.compile(r"(?i)(api_key|access_token|refresh_token|client_secret)\s*[:=]\s*['\"]?[A-Za-z0-9\-._~+/]{12,}"),
]


def make_env(data_dir: Path, deterministic: bool = False, display: str | None = None, wayland_display: str | None = None, xdg_runtime: Path | None = None) -> dict[str, str]:
    data_dir = data_dir.resolve()
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
    env.setdefault("NO_COLOR", "1")
    if wayland_display is not None:
        env.pop("DISPLAY", None)
        env["WAYLAND_DISPLAY"] = wayland_display
        if xdg_runtime is not None:
            env["XDG_RUNTIME_DIR"] = str(xdg_runtime)
    elif display is not None:
        env["DISPLAY"] = display
        env.pop("WAYLAND_DISPLAY", None)
    return env


def wait_for_socket(sock_path: Path, proc: subprocess.Popen[str], timeout_s: float = 30.0) -> socket.socket:
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            raise RuntimeError(f"mq exited early with code {proc.returncode}")
        if sock_path.exists():
            mode = stat.S_IMODE(sock_path.stat().st_mode)
            if (mode & 0o077) != 0:
                raise RuntimeError(f"Insecure mode {oct(mode)} on {sock_path}")
            sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            try:
                sock.settimeout(30.0)
                sock.connect(str(sock_path))
                return sock
            except OSError:
                sock.close()
        time.sleep(0.05)
    raise TimeoutError(f"Timed out waiting for {sock_path}")


class Client:
    def __init__(self, sock: socket.socket) -> None:
        self.sock = sock
        self.r = sock.makefile("r", encoding="utf-8")
        self.w = sock.makefile("w", encoding="utf-8")

    def req(self, payload: dict[str, Any]) -> dict[str, Any]:
        self.w.write(json.dumps(payload, ensure_ascii=False) + "\n")
        self.w.flush()
        line = self.r.readline()
        if not line:
            raise EOFError(f"EOF waiting for {payload.get('cmd')}")
        return json.loads(line)

    def close(self) -> None:
        for h in (self.w, self.r, self.sock):
            try:
                h.close()
            except OSError:
                pass


def read_proc_rss_kb(pid: int) -> int:
    status = Path(f"/proc/{pid}/status").read_text(encoding="utf-8")
    for line in status.splitlines():
        if line.startswith("VmRSS:"):
            return int(line.split()[1])
    return 0


def read_proc_cpu_ticks(pid: int) -> int:
    stat_txt = Path(f"/proc/{pid}/stat").read_text(encoding="utf-8")
    rparen = stat_txt.rfind(")")
    fields = stat_txt[rparen + 2 :].split()
    # utime=fields[11], stime=fields[12]
    return int(fields[11]) + int(fields[12])


def run_idle_60s(mq_bin: Path, data_root: Path) -> dict[str, Any]:
    data_dir = data_root / "idle_60s"
    if data_dir.exists():
        shutil.rmtree(data_dir)
    env = make_env(data_dir, deterministic=False)
    xvfb = None
    if not env.get("DISPLAY") and not env.get("WAYLAND_DISPLAY"):
        xvfb = subprocess.Popen(["Xvfb", ":97", "-screen", "0", "1280x800x24"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        env["DISPLAY"] = ":97"
        env["LIBGL_ALWAYS_SOFTWARE"] = "1"
        time.sleep(0.5)

    log_path = data_dir / "launcher.log"
    data_dir.mkdir(parents=True, exist_ok=True)
    log_f = log_path.open("w", encoding="utf-8")
    proc = subprocess.Popen([str(mq_bin), "--test-hooks", "--backend", "auto"], cwd=REPO_ROOT, env=env, stdout=log_f, stderr=subprocess.STDOUT, text=True)
    try:
        sock = wait_for_socket(Path(env["MQ_TEST_HOOKS_SOCKET"]), proc)
        cli = Client(sock)
        cli.req({"cmd": "wait_idle", "timeout_ms": 10000, "frames": 5})
        cli.req({"cmd": "frame_stats", "action": "reset"})

        clk_tck = os.sysconf(os.sysconf_names["SC_CLK_TCK"])
        rss_samples: list[int] = []
        cpu_t0 = read_proc_cpu_ticks(proc.pid)
        wall_t0 = time.monotonic()
        for _ in range(60):
            if proc.poll() is not None:
                raise RuntimeError(f"mq exited during 60s idle with {proc.returncode}")
            rss_samples.append(read_proc_rss_kb(proc.pid))
            time.sleep(1.0)
        cpu_t1 = read_proc_cpu_ticks(proc.pid)
        wall_dt = time.monotonic() - wall_t0
        avg_cpu_pct = round(((cpu_t1 - cpu_t0) / clk_tck) / wall_dt * 100.0, 2)

        fs = cli.req({"cmd": "frame_stats", "action": "get"})
        cli.req({"cmd": "quit"})
        cli.close()
        proc.wait(timeout=8)
        return {
            "ok": True,
            "duration_s": round(wall_dt, 2),
            "rss_start_kb": rss_samples[0],
            "rss_end_kb": rss_samples[-1],
            "rss_max_kb": max(rss_samples),
            "rss_growth_kb": rss_samples[-1] - rss_samples[0],
            "avg_cpu_pct": avg_cpu_pct,
            "frame_stats": fs,
        }
    finally:
        if proc.poll() is None:
            proc.kill()
        log_f.close()
        if xvfb is not None:
            xvfb.terminate()
            xvfb.wait(timeout=3)


def run_weston_resize(mq_bin: Path, data_root: Path) -> dict[str, Any]:
    if not shutil.which("weston"):
        return {"ok": False, "error": "weston not installed"}
    data_dir = data_root / "weston_resize"
    if data_dir.exists():
        shutil.rmtree(data_dir)
    runtime_dir = data_dir / "_xdg" / "runtime"
    runtime_dir.mkdir(parents=True, exist_ok=True)
    runtime_dir.chmod(0o700)
    env = make_env(data_dir, deterministic=False, wayland_display="wayland-qa-perf", xdg_runtime=runtime_dir)
    env["LIBGL_ALWAYS_SOFTWARE"] = "1"
    weston = subprocess.Popen(
        ["weston", "--backend=headless", "--xwayland", "--socket=wayland-qa-perf"],
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    time.sleep(1.0)
    log_f = (data_dir / "launcher.log").open("w", encoding="utf-8")
    proc = subprocess.Popen([str(mq_bin), "--test-hooks", "--backend", "auto"], cwd=REPO_ROOT, env=env, stdout=log_f, stderr=subprocess.STDOUT, text=True)
    try:
        sock = wait_for_socket(Path(env["MQ_TEST_HOOKS_SOCKET"]), proc)
        cli = Client(sock)
        cli.req({"cmd": "wait_idle", "timeout_ms": 10000, "frames": 3})
        spec = yaml.safe_load((REPO_ROOT / "qa/scenarios/resilience_theme_scale_resize.yaml").read_text(encoding="utf-8"))
        fs = None
        for step in spec["steps"]:
            if isinstance(step, str):
                payload = {"cmd": step}
            else:
                k, v = next(iter(step.items()))
                if k == "frame_stats":
                    payload = {"cmd": "frame_stats", "action": str(v)}
                elif isinstance(v, dict):
                    payload = {"cmd": k, **v}
                else:
                    payload = {"cmd": k}
            resp = cli.req(payload)
            if not resp.get("ok"):
                raise RuntimeError(f"weston step {payload} failed: {resp}")
            if payload.get("cmd") == "frame_stats":
                fs = resp
            if payload.get("cmd") == "quit":
                break
        cli.close()
        proc.wait(timeout=8)
        return {"ok": True, "frame_stats": fs}
    finally:
        if proc.poll() is None:
            proc.kill()
        log_f.close()
        weston.terminate()
        weston.wait(timeout=3)


def run_stress_data(mq_bin: Path, data_root: Path) -> dict[str, Any]:
    data_dir = data_root / "stress_data"
    if data_dir.exists():
        shutil.rmtree(data_dir)
    inst_root = data_dir / "instances"
    inst_root.mkdir(parents=True, exist_ok=True)

    # 1050 instances + 1 with 200-char name
    for i in range(1050):
        slug = f"stress-inst-{i:04d}"
        idir = inst_root / slug
        idir.mkdir(parents=True, exist_ok=True)
        name = ("СтрессИнстанс_" + "X" * 185) if i == 0 else f"Stress Instance {i:04d}"
        (idir / "instance.toml").write_text(
            f'id = "{slug}"\nname = "{name[:200]}"\nmc_version = "1.20.4"\nloader = "vanilla"\n',
            encoding="utf-8",
        )

    # Cyrillic account nickname in accounts.json with 0600
    acct_path = data_dir / "accounts.json"
    acct_path.write_text(
        json.dumps(
            {
                "active_account_id": "00000000-0000-0000-0000-000000000001",
                "accounts": [
                    {
                        "id": "00000000-0000-0000-0000-000000000001",
                        "kind": "offline",
                        "username": "ИгрокКириллица_Тест2026",
                        "uuid": "00000000-0000-0000-0000-000000000001",
                        "access_token": "offline",
                    }
                ],
            },
            ensure_ascii=False,
        ),
        encoding="utf-8",
    )
    acct_path.chmod(0o600)

    env = make_env(data_dir, deterministic=False)
    xvfb = None
    if not env.get("DISPLAY") and not env.get("WAYLAND_DISPLAY"):
        xvfb = subprocess.Popen(["Xvfb", ":96", "-screen", "0", "1280x800x24"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        env["DISPLAY"] = ":96"
        env["LIBGL_ALWAYS_SOFTWARE"] = "1"
        time.sleep(0.5)

    log_f = (data_dir / "launcher.log").open("w", encoding="utf-8")
    proc = subprocess.Popen([str(mq_bin), "--test-hooks", "--backend", "auto"], cwd=REPO_ROOT, env=env, stdout=log_f, stderr=subprocess.STDOUT, text=True)
    try:
        sock = wait_for_socket(Path(env["MQ_TEST_HOOKS_SOCKET"]), proc)
        cli = Client(sock)
        cli.req({"cmd": "wait_idle", "timeout_ms": 15000, "frames": 3})
        spec = yaml.safe_load((REPO_ROOT / "qa/scenarios/resilience_stress_data.yaml").read_text(encoding="utf-8"))
        fs = None
        ui_count = 0
        for step in spec["steps"]:
            if isinstance(step, str):
                payload = {"cmd": step}
            else:
                k, v = next(iter(step.items()))
                if k == "frame_stats":
                    payload = {"cmd": "frame_stats", "action": str(v)}
                elif k == "goto":
                    payload = {"cmd": "goto", "tab": str(v)}
                elif isinstance(v, dict):
                    payload = {"cmd": k, **v}
                else:
                    payload = {"cmd": k}
            resp = cli.req(payload)
            if not resp.get("ok"):
                raise RuntimeError(f"stress_data step {payload} failed: {resp}")
            if payload.get("cmd") == "dump_ui":
                ui_count = max(ui_count, len(resp.get("elements", [])))
            if payload.get("cmd") == "frame_stats":
                fs = resp
            if payload.get("cmd") == "quit":
                break
        cli.close()
        proc.wait(timeout=8)
        return {"ok": True, "instances_created": 1050, "max_ui_elements": ui_count, "frame_stats": fs}
    finally:
        if proc.poll() is None:
            proc.kill()
        log_f.close()
        if xvfb is not None:
            xvfb.terminate()
            xvfb.wait(timeout=3)


def run_sigkill_recovery(mq_bin: Path, data_root: Path) -> dict[str, Any]:
    data_dir = data_root / "sigkill"
    if data_dir.exists():
        shutil.rmtree(data_dir)
    (data_dir / "tmp").mkdir(parents=True, exist_ok=True)
    (data_dir / "cache").mkdir(parents=True, exist_ok=True)
    (data_dir / "instances" / "sigkill-inst").mkdir(parents=True, exist_ok=True)

    cfg_path = data_dir / "_xdg" / "config" / "mq" / "config.toml"
    cfg_path.parent.mkdir(parents=True, exist_ok=True)
    cfg_path.write_text('launcher_locale = "ru_RU"\ndefault_mc_locale = "ru_ru"\n', encoding="utf-8")
    cfg_path.chmod(0o600)

    inst_path = data_dir / "instances" / "sigkill-inst" / "instance.toml"
    inst_path.write_text('id = "sigkill-inst"\nname = "SigkillTest"\nmc_version = "1.20.4"\nloader = "vanilla"\n', encoding="utf-8")

    # Simulate partial downloads left behind by mid-download SIGKILL
    part1 = data_dir / "tmp" / "artifact.jar.e1.part"
    part2 = data_dir / "cache" / "modpack.mrpack.part"
    part1.write_bytes(b"partial-bytes-1234")
    part2.write_bytes(b"partial-bytes-5678")

    env = make_env(data_dir, deterministic=True)
    xvfb = None
    if not env.get("DISPLAY") and not env.get("WAYLAND_DISPLAY"):
        xvfb = subprocess.Popen(["Xvfb", ":95", "-screen", "0", "1280x800x24"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        env["DISPLAY"] = ":95"
        env["LIBGL_ALWAYS_SOFTWARE"] = "1"
        time.sleep(0.5)

    try:
        # Run 1: start, mutate theme/config, then SIGKILL
        log1 = (data_dir / "launcher-run1.log").open("w", encoding="utf-8")
        proc1 = subprocess.Popen([str(mq_bin), "--test-hooks", "--backend", "auto"], cwd=REPO_ROOT, env=env, stdout=log1, stderr=subprocess.STDOUT, text=True)
        sock1 = wait_for_socket(Path(env["MQ_TEST_HOOKS_SOCKET"]), proc1)
        cli1 = Client(sock1)
        cli1.req({"cmd": "wait_idle", "timeout_ms": 8000, "frames": 2})
        cli1.req({"cmd": "set_theme", "preset": "Nord"})
        os.kill(proc1.pid, signal.SIGKILL)
        proc1.wait(timeout=5)
        cli1.close()
        log1.close()

        # Run 2: restart in same MQ_DATA_DIR
        log2 = (data_dir / "launcher-run2.log").open("w", encoding="utf-8")
        proc2 = subprocess.Popen([str(mq_bin), "--test-hooks", "--backend", "auto"], cwd=REPO_ROOT, env=env, stdout=log2, stderr=subprocess.STDOUT, text=True)
        sock2 = wait_for_socket(Path(env["MQ_TEST_HOOKS_SOCKET"]), proc2)
        cli2 = Client(sock2)
        cli2.req({"cmd": "wait_idle", "timeout_ms": 8000, "frames": 3})
        state = cli2.req({"cmd": "state"})
        cli2.req({"cmd": "quit"})
        cli2.close()
        proc2.wait(timeout=8)
        log2.close()

        # Verify TOML files are valid
        cfg_ok = True
        try:
            if cfg_path.exists():
                tomllib.loads(cfg_path.read_text(encoding="utf-8"))
            if (data_dir / "config" / "config.toml").exists():
                tomllib.loads((data_dir / "config" / "config.toml").read_text(encoding="utf-8"))
        except Exception:
            cfg_ok = False

        inst_ok = True
        try:
            tomllib.loads(inst_path.read_text(encoding="utf-8"))
        except Exception:
            inst_ok = False

        leftover_parts = [str(p.relative_to(data_dir)) for p in data_dir.rglob("*.part")]
        leftover_tmps = [str(p.relative_to(data_dir)) for p in data_dir.rglob("*.tmp*")]
        return {
            "ok": cfg_ok and inst_ok and len(leftover_parts) == 0,
            "config_toml_valid": cfg_ok,
            "instance_toml_valid": inst_ok,
            "leftover_part_files": leftover_parts,
            "leftover_tmp_files": leftover_tmps,
            "restart_state_ok": bool(state.get("ok")),
        }
    finally:
        if xvfb is not None:
            xvfb.terminate()
            xvfb.wait(timeout=3)


def run_security_audit(scan_dirs: list[Path]) -> dict[str, Any]:
    perm_issues: list[dict[str, str]] = []
    secret_leaks: list[dict[str, Any]] = []

    # Load known secret values from ~/.config/mq/dev.env in memory ONLY (never print them)
    known_secrets: list[str] = []
    dev_env = Path.home() / ".config" / "mq" / "dev.env"
    if dev_env.exists():
        for line in dev_env.read_text(encoding="utf-8", errors="ignore").splitlines():
            line = line.strip()
            if not line or line.startswith("#") or "=" not in line:
                continue
            _, val = line.split("=", 1)
            val = val.strip().strip('"').strip("'")
            if len(val) >= 8:
                known_secrets.append(val)

    text_exts = {".log", ".txt", ".json", ".toml", ".yaml", ".yml", ".md"}
    for root in scan_dirs:
        if not root.exists():
            continue
        for p in root.rglob("*"):
            if not (p.is_file() or p.is_socket()):
                continue
            if p.name in SENSITIVE_FILENAMES:
                mode = stat.S_IMODE(p.stat().st_mode)
                if (mode & 0o077) != 0:
                    perm_issues.append({"path": str(p), "mode": oct(mode), "expected": "0o600"})
            if p.is_file() and (p.suffix.lower() in text_exts or p.name == "launcher.log"):
                if p.name in {"accounts.json", "curseforge_api_key.secret"}:
                    continue
                content = p.read_text(encoding="utf-8", errors="ignore")
                for idx, sec in enumerate(known_secrets):
                    if sec in content:
                        secret_leaks.append({"path": str(p), "kind": f"dev_env_secret_{idx}"})
                for pat in SECRET_PATTERNS:
                    if pat.search(content):
                        secret_leaks.append({"path": str(p), "kind": f"pattern:{pat.pattern[:24]}"})

    return {
        "ok": len(perm_issues) == 0 and len(secret_leaks) == 0,
        "perm_issues": perm_issues,
        "secret_leaks": secret_leaks,
        "checked_dev_env_secrets": len(known_secrets),
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--bin", type=Path, default=DEFAULT_BIN)
    parser.add_argument("--data-root", type=Path, default=Path("/tmp/mq-qa/perf"))
    parser.add_argument("--out", type=Path, default=REPO_ROOT / "qa-out" / "resilience_perf_summary.json")
    parser.add_argument("--only", choices=["all", "idle", "weston", "stress", "sigkill", "security"], default="all")
    args = parser.parse_args()

    results: dict[str, Any] = {}
    if args.only in ("all", "idle"):
        print("[suite] Running 60s idle CPU/RSS...")
        results["idle_60s"] = run_idle_60s(args.bin, args.data_root)
    if args.only in ("all", "weston"):
        print("[suite] Running headless weston theme/scale/resize...")
        results["weston_resize"] = run_weston_resize(args.bin, args.data_root)
    if args.only in ("all", "stress"):
        print("[suite] Running 1000+ instances / 200-char / Cyrillic stress...")
        results["stress_data"] = run_stress_data(args.bin, args.data_root)
    if args.only in ("all", "sigkill"):
        print("[suite] Running SIGKILL recovery...")
        results["sigkill_recovery"] = run_sigkill_recovery(args.bin, args.data_root)
    if args.only in ("all", "security"):
        print("[suite] Running security & permissions audit...")
        results["security_audit"] = run_security_audit([args.data_root, Path("/tmp/mq-qa"), REPO_ROOT / "qa-out"])

    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(results, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(results, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
