# DocuFlow 本地开发栈

一个脚本同时拉起 **后端**（`docuflow-hermes-lab`）和 **前端**（`docuflow-workspace`），
并把两个进程的输出合并到同一个控制台。

## 用法

```bash
# 桌面形态（Tauri 窗口）—— 默认
python scripts/dev.py

# 浏览器形态（本机 WebView2 不可用时用这个）
python scripts/dev.py --mode browser

# 端口被占用时强制覆盖
python scripts/dev.py --force

# 停止上一次运行留下的进程
python scripts/dev.py --stop
```

| 参数 | 作用 |
|---|---|
| `--mode desktop` | Tauri 桌面窗口（默认） |
| `--mode browser` | 只起 Vite，并自动打开浏览器 |
| `--force` | 端口被占用时先结束占用进程 |
| `--no-build` | 跳过后端 TypeScript 编译，直接用现有 `dist` |
| `--no-browser` | browser 模式下不自动开浏览器 |
| `--stop` | 结束上一次运行留下的进程与端口占用者 |

## 脚本做什么

1. 检查 node、两个工程的 `node_modules`（缺了就自动 `npm install`）。
2. 编译后端（`tsc -p tsconfig.json`）。**编译报错但 `dist` 已生成时会继续运行**并给出警告——
   `tsc` 遇到类型错误仍会 emit，而 lab 里正在开发的文件经常带临时类型错误。
3. 检查端口 8787（API）和 1420（Vite）。
4. 启动后端，等 `/api/v1/runtimes` 返回 200 才算就绪。
5. 启动前端，等 1420 开始监听。
6. 合并输出到控制台：`[api]` 和 `[ui]` 前缀区分来源。日志同时落在 `.dev-logs/`。

## 停止

按 **Ctrl+C**。脚本会一并结束后端和前端进程（包括 `npm.cmd` → `cmd.exe` → `node`
链条上被孤立的那些）。也可以在另一个终端跑 `python scripts/dev.py --stop`。

## 手动分开启动

```bash
# 后端
cd docuflow-hermes-lab
npm run dev:stack

# 前端
cd docuflow-workspace
npm run dev            # 浏览器形态
npm run tauri -- dev   # 桌面形态（`--` 之后才是传给 tauri 的子命令）
```

## 依赖

Python 3.9+。`psutil` 可选但推荐——没有它时脚本退回用 `taskkill /T`，
清理孤儿进程的效果不如按进程树逐个 kill。

```bash
pip install psutil
```

## 已知问题

- **本机 `npm run tauri dev` 打不开窗口**：建窗时 WebView2 返回 `E_ACCESSDENIED` (os error 5)，
  已排除代码与权限因素，是本机 WebView2 运行时环境问题。用 `--mode browser` 绕过。
- **Vite 绑在 IPv6 `::1`** 而不是 IPv4 `127.0.0.1`。用 IPv4 探测端口会误报"空闲"，
  所以脚本对两种协议族分别尝试 bind 来判断占用。
