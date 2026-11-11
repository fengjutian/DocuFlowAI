#!/usr/bin/env python3
"""Start the DocuFlow API server and the Workspace UI together.

The backend is the lab stack (mock Hermes + the real API server) on port 8787;
the frontend is either the Tauri desktop window or the plain Vite dev server on
port 1420. Both processes stream into this console with an ``[api]`` / ``[ui]``
prefix, and Ctrl+C tears down the whole tree.

Usage:
    python scripts/dev.py                    # desktop (Tauri window)
    python scripts/dev.py --mode browser     # browser only
    python scripts/dev.py --force            # take over busy ports
    python scripts/dev.py --stop             # stop a previous run
"""

from __future__ import annotations

import argparse
import os
import shutil
import signal
import socket
import subprocess
import sys
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LAB_DIR = ROOT / "docuflow-hermes-lab"
UI_DIR = ROOT / "docuflow-workspace"
LOG_DIR = ROOT / ".dev-logs"
PID_FILE = LOG_DIR / "pids.json"

API_PORT = 8787
WEB_PORT = 1420

# How long the backend may take to answer once it is spawned.
API_READY_TIMEOUT = 60.0
# The desktop build compiles Rust first, so it needs a much longer window.
WEB_READY_TIMEOUT_DESKTOP = 300.0
WEB_READY_TIMEOUT_BROWSER = 60.0


# --------------------------------------------------------------------- output


def _enable_vt() -> bool:
    """Turn on ANSI escapes for the Windows console."""
    if os.name != "nt":
        return True
    try:
        import ctypes

        kernel32 = ctypes.windll.kernel32
        handle = kernel32.GetStdHandle(-11)
        mode = ctypes.c_uint32()
        if not kernel32.GetConsoleMode(handle, ctypes.byref(mode)):
            return False
        # ENABLE_VIRTUAL_TERMINAL_PROCESSING
        return bool(kernel32.SetConsoleMode(handle, mode.value | 0x0004))
    except Exception:
        return False


_VT = _enable_vt()


def _colour(code: str, text: str) -> str:
    if not _VT:
        return text
    return f"\033[{code}m{text}\033[0m"


def step(message: str) -> None:
    print(_colour("36", f"==> {message}"), flush=True)


def note(message: str) -> None:
    print(_colour("90", f"    {message}"), flush=True)


def error(message: str) -> None:
    print(_colour("31", message), flush=True)


def _configure_streams() -> None:
    """Make stdout UTF-8 so the Chinese log lines survive a redirect."""
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except Exception:
            pass


def _install_signal_handlers() -> None:
    """Route every console interrupt to KeyboardInterrupt.

    Ctrl+C raises SIGINT, which Python already turns into KeyboardInterrupt.
    Ctrl+Break raises SIGBREAK, whose default action terminates the process
    outright and would skip the teardown, so it is mapped onto the same path.
    """
    if os.name != "nt":
        return

    def _raise_interrupt(signum: int, frame: object) -> None:
        raise KeyboardInterrupt

    for name in ("SIGBREAK", "SIGINT"):
        sig = getattr(signal, name, None)
        if sig is None:
            continue
        try:
            signal.signal(sig, _raise_interrupt)
        except (ValueError, OSError):
            # Not on the main thread, or the signal is unavailable here.
            pass


# --------------------------------------------------------------------- ports


def _can_bind(family: int, address: str, port: int) -> tuple[bool, str]:
    """Try to bind one address family; report whether the port is free."""
    sock = socket.socket(family, socket.SOCK_STREAM)
    try:
        # No SO_REUSEADDR on purpose: on Windows it would let the bind succeed
        # even while another process is listening.
        sock.bind((address, port))
        return True, ""
    except OSError as exc:
        return False, str(exc)
    finally:
        sock.close()


def port_is_free(port: int) -> bool:
    """A port is free only when neither IPv4 nor IPv6 can be bound.

    Vite binds ``localhost``, which resolves to ``::1`` on this machine, while
    the API server binds ``127.0.0.1``. Probing a single family therefore
    reports a busy port as free.
    """
    v4_ok, _ = _can_bind(socket.AF_INET, "127.0.0.1", port)
    if not v4_ok:
        return False
    try:
        v6_ok, _ = _can_bind(socket.AF_INET6, "::1", port)
    except OSError:
        # No IPv6 stack on this host: the IPv4 result is the whole answer.
        return True
    return v6_ok


def port_owners(port: int) -> list[int]:
    """PIDs listening on the port, from either address family."""
    try:
        import psutil
    except ImportError:
        return []
    owners: list[int] = []
    try:
        for conn in psutil.net_connections(kind="tcp"):
            if conn.status != "LISTEN" or conn.laddr is None:
                continue
            if conn.laddr.port == port and conn.pid:
                owners.append(conn.pid)
    except Exception:
        return []
    return sorted(set(owners))


# ------------------------------------------------------------------ processes


class Managed:
    """A child process whose output is tee'd to the console with a prefix."""

    def __init__(self, tag: str, popen: subprocess.Popen, log_path: Path) -> None:
        self.tag = tag
        self.popen = popen
        self.log_path = log_path
        self._lock = threading.Lock()

    def pump(self) -> None:
        stream = self.popen.stdout
        assert stream is not None
        with self.log_path.open("a", encoding="utf-8") as log:
            for raw in iter(stream.readline, b""):
                line = raw.decode("utf-8", errors="replace").rstrip("\r\n")
                with self._lock:
                    log.write(line + "\n")
                    log.flush()
                    print(f"[{self.tag}] {line}", flush=True)

    def start_pump(self) -> None:
        thread = threading.Thread(target=self.pump, daemon=True)
        thread.start()

    @property
    def exited(self) -> bool:
        return self.popen.poll() is not None

    @property
    def exit_code(self) -> int | None:
        return self.popen.poll()

    def kill_tree(self) -> None:
        """Kill the process and every descendant it spawned.

        npm.cmd is a batch file: Windows puts an extra cmd.exe between it and
        the real node process, so killing only the recorded PID leaves the dev
        server running and holding the port.
        """
        pid = self.popen.pid
        try:
            import psutil
        except ImportError:
            self._kill_with_taskkill(pid)
            return

        try:
            parent = psutil.Process(pid)
        except psutil.Error:
            return

        children = parent.children(recursive=True)
        # Children first: killing the parent re-parents them and orphans them.
        for child in children:
            try:
                child.kill()
            except psutil.Error:
                pass
        try:
            parent.kill()
        except psutil.Error:
            pass
        psutil.wait_procs(children + [parent], timeout=5)

    @staticmethod
    def _kill_with_taskkill(pid: int) -> None:
        subprocess.run(
            ["taskkill", "/PID", str(pid), "/T", "/F"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )


def spawn(tag: str, command: list[str], cwd: Path) -> Managed:
    LOG_DIR.mkdir(parents=True, exist_ok=True)
    log_path = LOG_DIR / f"{tag}.log"
    log_path.write_text("", encoding="utf-8")

    creation_flags = 0
    if os.name == "nt":
        # A separate process group keeps the console Ctrl+C from reaching the
        # child directly, so teardown happens exactly once, in one place.
        creation_flags = subprocess.CREATE_NEW_PROCESS_GROUP

    popen = subprocess.Popen(
        command,
        cwd=str(cwd),
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        stdin=subprocess.DEVNULL,
        creationflags=creation_flags,
        bufsize=0,
    )
    managed = Managed(tag, popen, log_path)
    managed.start_pump()
    return managed


def npm_command(script: str, *args: str) -> list[str]:
    """npm is a batch file on Windows; CreateProcess handles it directly.

    Extra arguments go after a ``--`` separator: ``npm run tauri dev`` is
    read by npm as a script literally named "tauri dev" ("Missing script"),
    while ``npm run tauri -- dev`` forwards "dev" to the tauri binary.
    """
    npm = shutil.which("npm.cmd" if os.name == "nt" else "npm") or "npm"
    command = [npm, "run", script]
    if args:
        command += ["--", *args]
    return command


# ------------------------------------------------------------------ lifecycle


def run_build() -> None:
    step("编译后端（docuflow-hermes-lab）")
    result = subprocess.run(
        npm_command("build"), cwd=str(LAB_DIR), stdout=subprocess.PIPE, stderr=subprocess.STDOUT
    )
    if result.returncode == 0:
        return
    output = result.stdout.decode("utf-8", errors="replace")
    print(output, flush=True)
    if not (LAB_DIR / "dist" / "runners" / "dev-stack.js").exists():
        raise SystemExit("后端编译失败且没有可用的 dist，请先修复 docuflow-hermes-lab 的类型错误。")
    # tsc emits even when it reports type errors, so a usable dist can exist.
    note("编译报错但 dist 已生成（tsc 仍会 emit），继续使用现有 dist。")
    note("如需强制干净构建，请先修复 lab 的类型错误后重试。")


def ensure_dependencies() -> None:
    step("检查运行环境")
    if shutil.which("node") is None:
        raise SystemExit("找不到 node，请先安装 Node.js。")
    note(f"node {subprocess.run(['node', '--version'], capture_output=True, text=True).stdout.strip()}")

    for directory, name in ((LAB_DIR, "docuflow-hermes-lab"), (UI_DIR, "docuflow-workspace")):
        if not (directory / "node_modules").exists():
            note(f"{name} 缺少 node_modules，正在安装依赖…")
            subprocess.run(
                ["npm", "install"],
                cwd=str(directory),
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                check=False,
            )


def assert_port_free(port: int, what: str, force: bool) -> None:
    if port_is_free(port):
        return
    owners = port_owners(port)
    who = f" (PID {', '.join(str(p) for p in owners)})" if owners else ""
    if not force:
        raise SystemExit(
            f"端口 {port} 已被占用{who} —— {what} 需要该端口。"
            "确认要覆盖运行中的实例后，加 --force 重试。"
        )
    note(f"端口 {port} 被占用{who}，--force 已启用，正在结束占用进程。")
    for pid in owners:
        Managed._kill_with_taskkill(pid)
    deadline = time.time() + 5
    while time.time() < deadline and not port_is_free(port):
        time.sleep(0.3)
    if not port_is_free(port):
        raise SystemExit(f"端口 {port} 仍被占用，{what} 无法启动。")


def wait_for_api(api: Managed) -> None:
    step(f"启动后端（{API_PORT}）")
    deadline = time.time() + API_READY_TIMEOUT
    while time.time() < deadline:
        time.sleep(0.5)
        if api.exited:
            raise SystemExit(
                f"后端进程启动即退出（退出码 {api.exit_code}）。日志：{api.log_path}"
            )
        try:
            with socket.create_connection(("127.0.0.1", API_PORT), timeout=2):
                pass
            if urllib_request_json(f"http://127.0.0.1:{API_PORT}/api/v1/runtimes"):
                note(f"后端就绪：http://127.0.0.1:{API_PORT}")
                return
        except Exception:
            continue
    raise SystemExit(f"后端在 {int(API_READY_TIMEOUT)} 秒内没有就绪，日志：{api.log_path}")


def urllib_request_json(url: str) -> bool:
    """Cheap GET that only reports whether the endpoint answered with 200."""
    import urllib.error
    import urllib.request

    try:
        with urllib.request.urlopen(url, timeout=3) as response:
            return response.status == 200
    except urllib.error.HTTPError as exc:
        return exc.code == 200
    except Exception:
        return False


def wait_for_web(ui: Managed, timeout: float) -> None:
    deadline = time.time() + timeout
    while time.time() < deadline:
        time.sleep(0.5)
        if not port_is_free(WEB_PORT):
            return
        if ui.exited:
            raise SystemExit(
                f"前端进程启动即退出（退出码 {ui.exit_code}）。日志：{ui.log_path}"
            )
    raise SystemExit(
        f"前端在 {int(timeout)} 秒内没有监听 {WEB_PORT}，日志：{ui.log_path}"
    )


def stop_previous_run() -> None:
    if not PID_FILE.exists():
        note("没有找到运行中的实例记录。")
        return
    import json

    try:
        records = json.loads(PID_FILE.read_text(encoding="utf-8"))
    except Exception:
        records = []
    for record in records:
        pid = record.get("pid")
        if pid:
            note(f"结束 {record.get('tag', '?')} (PID {pid})")
            Managed._kill_with_taskkill(int(pid))
    PID_FILE.write_text("[]", encoding="utf-8")


def write_pid_file(handles: list[Managed]) -> None:
    import json

    LOG_DIR.mkdir(parents=True, exist_ok=True)
    payload = [
        {"tag": handle.tag, "pid": handle.popen.pid} for handle in handles
    ]
    PID_FILE.write_text(json.dumps(payload, indent=2), encoding="utf-8")


def kill_orphan_listeners() -> None:
    """Kill whatever still holds the ports, whatever its parent was.

    The npm.cmd -> cmd.exe -> node chain can leave the dev server running after
    the wrapper is gone, so the listeners themselves are the reliable handle.
    """
    for port in (API_PORT, WEB_PORT):
        for pid in port_owners(port):
            Managed._kill_with_taskkill(pid)
        deadline = time.time() + 3
        while time.time() < deadline and not port_is_free(port):
            time.sleep(0.3)


def _webview_access_denied(ui: Managed) -> bool:
    """Return whether Tauri failed while the Windows webview was created."""
    try:
        tail = ui.log_path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return False
    return "os error 5" in tail and "Failed to setup app" in tail


def _diagnose_ui_exit(args: argparse.Namespace, ui: Managed) -> None:
    """Explain a Windows webview creation failure without claiming it is absent."""
    if args.mode != "desktop":
        return
    if _webview_access_denied(ui):
        error("桌面窗口创建失败：WebView2 已安装，但 Windows 拒绝创建该应用的 WebView（os error 5）。")
        note("这不代表 WebView2 未安装；浏览器模式不受影响。手动启动方式：")
        note("    python scripts/dev.py --mode browser")


def main() -> int:
    _configure_streams()
    _install_signal_handlers()

    parser = argparse.ArgumentParser(
        description="同时启动 DocuFlow 后端与 Workspace 前端",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "--mode",
        choices=("desktop", "browser"),
        default="desktop",
        help="desktop = Tauri 窗口（默认）；browser = 只起 Vite",
    )
    parser.add_argument("--force", action="store_true", help="端口被占用时先结束占用进程")
    parser.add_argument("--no-build", action="store_true", help="跳过后端 TypeScript 编译")
    parser.add_argument("--no-browser", action="store_true", help="browser 模式下不自动开浏览器")
    parser.add_argument(
        "--no-browser-fallback",
        action="store_true",
        help="桌面 WebView 创建失败时不要自动切换到浏览器模式",
    )
    parser.add_argument("--stop", action="store_true", help="停止上一次运行留下的进程")
    args = parser.parse_args()

    if args.stop:
        stop_previous_run()
        kill_orphan_listeners()
        return 0

    ensure_dependencies()

    if not args.no_build:
        run_build()

    step("检查端口")
    assert_port_free(API_PORT, "后端 API", args.force)
    # Both modes need Vite on 1420: in desktop mode `tauri dev` starts it as a
    # beforeDevCommand, in browser mode it is the dev server itself.
    assert_port_free(WEB_PORT, "前端 Vite", args.force)

    handles: list[Managed] = []
    try:
        api = spawn("api", ["node", "dist/runners/dev-stack.js"], LAB_DIR)
        handles.append(api)
        wait_for_api(api)

        step(f"启动前端（{args.mode}）")
        if args.mode == "desktop":
            # `tauri` is the npm script; `dev` is the tauri sub-command.
            ui_command = npm_command("tauri", "dev")
        else:
            ui_command = npm_command("dev")
        ui = spawn("ui", ui_command, UI_DIR)
        handles.append(ui)
        write_pid_file(handles)

        timeout = (
            WEB_READY_TIMEOUT_DESKTOP if args.mode == "desktop" else WEB_READY_TIMEOUT_BROWSER
        )
        wait_for_web(ui, timeout)
        note(f"前端就绪：http://localhost:{WEB_PORT}")

        if args.mode == "browser" and not args.no_browser:
            import webbrowser

            webbrowser.open(f"http://localhost:{WEB_PORT}")

        step("运行中 —— Ctrl+C 结束")
        note(f"日志文件：{LOG_DIR}")

        while True:
            for handle in handles:
                if handle.exited:
                    if (
                        handle is ui
                        and args.mode == "desktop"
                        and not args.no_browser_fallback
                        and _webview_access_denied(ui)
                    ):
                        error("桌面 WebView 创建被 Windows 拒绝，正在自动切换到浏览器模式……")
                        ui.kill_tree()
                        for pid in port_owners(WEB_PORT):
                            Managed._kill_with_taskkill(pid)
                        deadline = time.time() + 5
                        while time.time() < deadline and not port_is_free(WEB_PORT):
                            time.sleep(0.2)
                        if not port_is_free(WEB_PORT):
                            raise RuntimeError(f"无法释放前端端口 {WEB_PORT}，浏览器模式未启动")

                        ui = spawn("ui", npm_command("dev"), UI_DIR)
                        handles[handles.index(handle)] = ui
                        args.mode = "browser"
                        write_pid_file(handles)
                        wait_for_web(ui, WEB_READY_TIMEOUT_BROWSER)
                        note(f"浏览器模式就绪：http://localhost:{WEB_PORT}")
                        if not args.no_browser:
                            import webbrowser

                            webbrowser.open(f"http://localhost:{WEB_PORT}")
                        break
                    error(f"[{handle.tag}] 进程已退出，退出码 {handle.exit_code}")
                    _diagnose_ui_exit(args, handle)
                    raise SystemExit(f"进程 {handle.tag} 提前退出，日志：{handle.log_path}")
            time.sleep(0.15)
    except KeyboardInterrupt:
        print()
        step("收到中断，正在结束子进程…")
        return 0
    finally:
        for handle in handles:
            handle.kill_tree()
        kill_orphan_listeners()
        for handle in handles:
            try:
                if handle.popen.stdout:
                    handle.popen.stdout.close()
            except Exception:
                pass
        if PID_FILE.exists():
            PID_FILE.write_text("[]", encoding="utf-8")


if __name__ == "__main__":
    try:
        sys.exit(main())
    except SystemExit:
        raise
    except Exception as exc:  # noqa: BLE001 - top level guard
        error(f"启动失败：{exc}")
        sys.exit(1)
