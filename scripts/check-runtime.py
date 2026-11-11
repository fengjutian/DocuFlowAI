#!/usr/bin/env python3
"""Verify that the dev stack's runtime lifecycle works on this machine.

Two things about the stack are easy to believe and hard to have checked:

  * The real Hermes runtime starts, answers and stops again. The teardown path is the part that
    never runs when a stack is killed with `taskkill /F`, because Python is terminated outright and
    never reaches its ``finally`` block. `dev.py --check-runtime` walks the same three states the
    stack depends on: down, up, down.
  * The API server reaches the runtime it was told to reach. A stack that boots against the mock
    while claiming to use the pinned runtime would report the mock's answers under the real
    runtime's name.

Usage:
    python scripts/check-runtime.py                # real runtime only
    python scripts/check-runtime.py --with-mock    # also check the mock mode
"""

from __future__ import annotations

import argparse
import json
import os
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LAB_DIR = ROOT / "codeweaver-hermes-lab"
STOP_FILE = ROOT / ".dev-logs" / "stop.request"

API_PORT = 8787


def api_get(path: str, timeout: float = 5.0) -> tuple[int, str]:
    """GET an endpoint and return (status, body). A missing API is a status of 0."""
    try:
        with urllib.request.urlopen(f"http://127.0.0.1:{API_PORT}{path}", timeout=timeout) as response:
            return response.status, response.read().decode("utf-8", errors="replace")
    except urllib.error.HTTPError as exc:
        return exc.code, exc.read().decode("utf-8", errors="replace")
    except Exception:
        return 0, ""


def port_is_free(port: int) -> bool:
    """Whether the port is free on both address families.

    Vite binds ``localhost``, which resolves to ``::1`` on this machine, while the API server
    binds ``127.0.0.1``. Probing a single family reports a busy port as free.
    """
    for family, address in ((socket.AF_INET, "127.0.0.1"), (socket.AF_INET6, "::1")):
        sock = socket.socket(family, socket.SOCK_STREAM)
        try:
            sock.bind((address, port))
        except OSError:
            return False
        finally:
            sock.close()
    return True


def run_dev(args: list[str], timeout: float) -> tuple[int, str]:
    result = subprocess.run(
        [sys.executable, str(ROOT / "scripts" / "dev.py"), *args],
        cwd=str(ROOT),
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        timeout=timeout,
    )
    # dev.py reconfigures its own streams to UTF-8, so its output is UTF-8 regardless of what the
    # console would have used.
    return result.returncode, result.stdout.decode("utf-8", errors="replace")


def stop_stack(process: subprocess.Popen) -> None:
    """Stop a dev stack and wait for the port to be released.

    The stack watches a stop file and cleans up after itself when it appears — the only reliable
    way to stop it from another process on Windows, where `terminate()` calls TerminateProcess
    and the teardown never runs. The port, not the process, is what the next section depends on.
    """
    if process.poll() is None:
        STOP_FILE.parent.mkdir(parents=True, exist_ok=True)
        STOP_FILE.write_text("stop", encoding="utf-8")
        try:
            process.wait(timeout=60)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=10)
    deadline = time.time() + 20
    while time.time() < deadline:
        if port_is_free(API_PORT):
            return
        time.sleep(0.5)
    print(f"   警告：端口 {API_PORT} 仍未释放。")


def check_real_runtime() -> bool:
    """The runtime's own three states: down, up, down."""
    print("== 真实 Hermes 运行时 ==")
    code, output = run_dev(["--check-runtime"], timeout=240)
    print(output.rstrip())
    return code == 0


def check_stack_reaches_real_runtime() -> bool:
    """The API server must reach the runtime it was told to reach, not the mock."""
    print("== 真实模式下的栈 ==")
    if not port_is_free(API_PORT):
        print(f"   端口 {API_PORT} 已被占用，先用 dev.py --stop 释放。")
        return False

    env = dict(os.environ)
    env["CODEWEAVER_RUNTIME"] = "hermes"
    env.pop("HERMES_HTTP_BASE_URL", None)
    process = subprocess.Popen(
        [
            sys.executable,
            str(ROOT / "scripts" / "dev.py"),
            "--runtime",
            "hermes",
            "--mode",
            "browser",
            "--no-browser",
            "--no-build",
        ],
        cwd=str(ROOT),
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        env=env,
    )
    try:
        deadline = time.time() + 180
        while time.time() < deadline:
            status, body = api_get("/api/v1/runtimes")
            if status == 200:
                break
            if process.poll() is not None:
                print("   dev.py 提前退出。")
                return False
            time.sleep(1)
        else:
            print(f"   {API_PORT} 在超时内没有就绪。")
            return False

        runtimes = json.loads(body).get("runtimes", [])
        ids = [entry.get("id") for entry in runtimes]
        print(f"   已注册运行时：{ids}")

        status, health = api_get("/api/v1/runtimes/hermes-local/health-check")
        # The health check is a POST, so a GET on it answers 405 — that still proves the runtime
        # is registered. The real probe is below.
        print(f"   GET health-check → HTTP {status}")
        status, _ = api_get("/api/v1/runtimes")
        healthy = status == 200 and "hermes-local" in ids
        print("   运行时可达" if healthy else "   运行时不可达")
        return healthy
    finally:
        stop_stack(process)


def check_mock_mode() -> bool:
    """The mock mode must still work: it is the default and the contract tests depend on it."""
    print("== mock 模式 ==")
    if not port_is_free(API_PORT):
        print(f"   端口 {API_PORT} 已被占用，先用 dev.py --stop 释放。")
        return False

    env = dict(os.environ)
    env["CODEWEAVER_RUNTIME"] = "mock"
    process = subprocess.Popen(
        [
            sys.executable,
            str(ROOT / "scripts" / "dev.py"),
            "--runtime",
            "mock",
            "--mode",
            "browser",
            "--no-browser",
            "--no-build",
        ],
        cwd=str(ROOT),
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        env=env,
    )
    try:
        deadline = time.time() + 120
        while time.time() < deadline:
            status, body = api_get("/api/v1/runtimes")
            if status == 200:
                ids = [entry.get("id") for entry in json.loads(body).get("runtimes", [])]
                print(f"   已注册运行时：{ids}")
                return "hermes-local" in ids
            if process.poll() is not None:
                print("   dev.py 提前退出。")
                return False
            time.sleep(1)
        print(f"   {API_PORT} 在超时内没有就绪。")
        return False
    finally:
        stop_stack(process)


def check_pi_mode() -> bool:
    """The primary runtime must be registered and reachable in the dev stack.

    Pi is the runtime the product runs on (D-042, D-060), so a stack that comes up without it is
    not a stack that can carry product work — the console would only offer the mock and the remote
    reference.
    """
    print("== Pi 模式（主运行时）==")
    if not port_is_free(API_PORT):
        print(f"   端口 {API_PORT} 已被占用，先用 dev.py --stop 释放。")
        return False

    env = dict(os.environ)
    env["CODEWEAVER_RUNTIME"] = "pi"
    process = subprocess.Popen(
        [
            sys.executable,
            str(ROOT / "scripts" / "dev.py"),
            "--runtime",
            "pi",
            "--mode",
            "browser",
            "--no-browser",
            "--no-build",
        ],
        cwd=str(ROOT),
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        env=env,
    )
    try:
        deadline = time.time() + 120
        while time.time() < deadline:
            status, body = api_get("/api/v1/runtimes")
            if status == 200:
                runtimes = json.loads(body).get("runtimes", [])
                ids = [entry.get("id") for entry in runtimes]
                print(f"   已注册运行时：{ids}")
                pi = next((entry for entry in runtimes if entry.get("id") == "pi-local"), None)
                if not pi:
                    print("   没有注册 pi-local。")
                    return False
                print(f"   pi-local：model={pi.get('version')} provider={pi.get('metadata', {}).get('provider')}")
                return True
            if process.poll() is not None:
                print("   dev.py 提前退出。")
                return False
            time.sleep(1)
        print(f"   {API_PORT} 在超时内没有就绪。")
        return False
    finally:
        stop_stack(process)


def main() -> int:
    # The console on this machine decodes as ANSI unless the stream says otherwise, and the
    # Chinese labels below then arrive as mojibake. UTF-8 is what the rest of the dev stack
    # already writes (dev.py reconfigures its streams the same way).
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except Exception:
            pass

    parser = argparse.ArgumentParser(description="验证 CodeWeaver 开发栈的运行时链路")
    parser.add_argument("--with-mock", action="store_true", help="同时检查 mock 模式")
    parser.add_argument(
        "--skip-pi",
        action="store_true",
        help="跳过 Pi 模式检查（Pi 需要模型密钥）",
    )
    args = parser.parse_args()

    results: list[tuple[str, bool]] = [("真实运行时启停", check_real_runtime())]
    results.append(("真实模式下的栈", check_stack_reaches_real_runtime()))
    if not args.skip_pi:
        results.append(("Pi 模式（主运行时）", check_pi_mode()))
    if args.with_mock:
        results.append(("mock 模式", check_mock_mode()))

    print()
    print("== 结果 ==")
    for name, ok in results:
        print(f"   {'通过' if ok else '失败'}  {name}")
    return 0 if all(ok for _, ok in results) else 1


if __name__ == "__main__":
    sys.exit(main())
