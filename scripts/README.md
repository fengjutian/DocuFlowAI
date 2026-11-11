# CodeWeaver 本地开发栈

一个脚本同时拉起 **后端**（`codeweaver-hermes-lab`）和 **前端**（`codeweaver-workspace`），
并把两个进程的输出合并到同一个控制台。

## 用法

```bash
# 桌面形态（Tauri 窗口）—— 默认
python scripts/dev.py

# 浏览器形态（本机 WebView2 不可用时用这个）
python scripts/dev.py --mode browser

# 主运行时：Pi（真实 MiniMax，带完整工具面）
python scripts/dev.py --runtime pi

# 远程参考运行时：真实锁定的 Hermes 0.23.0
python scripts/dev.py --runtime hermes

# 端口被占用时强制覆盖
python scripts/dev.py --force

# 停止上一次运行留下的进程
python scripts/dev.py --stop
```

| 参数 | 作用 |
|---|---|
| `--mode desktop` | Tauri 桌面窗口（默认） |
| `--mode browser` | 只起 Vite，并自动打开浏览器 |
| `--runtime mock` | 内置 mock 运行时（默认） |
| `--runtime pi` | 主运行时：Pi（进程内，真实 MiniMax，完整工具面） |
| `--runtime hermes` | 远程参考运行时：真实锁定的 Hermes 0.23.0 |
| `--force` | 端口被占用时先结束占用进程 |
| `--no-build` | 跳过后端 TypeScript 编译，直接用现有 `dist` |
| `--no-browser` | browser 模式下不自动开浏览器 |
| `--stop` | 结束上一次运行留下的进程与端口占用者 |

三个运行时的定位（见 `doc/PROJECT_STATUS.md`）：**Pi 是产品主运行时**（D-042、D-060），工具面、
agent 级安全探针、取消与会话恢复都在它上面签收；Hermes 是**远程运行时参考**，保留远程
HTTP + SSE 形态的契约证据，不提供工具面；mock 是对照用的替身。

`--runtime pi` 与 `--runtime hermes` 都会同时注册另一个运行时，因此控制台里两个都可见，
"点名即遵从否则拒绝"（D-047）可以直接验证。模型密钥从 Hermes profile 的 `.env` 读取
（Pi 的 `minimax-cn` provider 读同一个 `MINIMAX_CN_API_KEY`），不落在仓库、不进命令行。

## 脚本做什么

1. 检查 node、两个工程的 `node_modules`（缺了就自动 `npm install`）。
2. 编译后端（`tsc -p tsconfig.json`）。**编译报错但 `dist` 已生成时会继续运行**并给出警告——
   `tsc` 遇到类型错误仍会 emit，而 lab 里正在开发的文件经常带临时类型错误。
3. 检查端口 8787（API）和 1420（Vite）。
4. `--runtime hermes` 时先启动真实 Hermes 运行时并等它能应答，再起后端——后端启动时要读
   运行时的 capabilities，顺序反了会把"还在启动中"报成"运行时不可达"。API key 从 Hermes
   profile 读，不走命令行。`--runtime pi` 不需要独立进程（Pi 跑在后端进程里），但同样需要
   模型密钥。
5. 启动后端，等 `/api/v1/runtimes` 返回 200 才算就绪。
6. 启动前端，等 1420 开始监听。
7. 合并输出到控制台：`[api]` 和 `[ui]` 前缀区分来源。日志同时落在 `.dev-logs/`。

## 停止

按 **Ctrl+C**。脚本会一并结束后端和前端进程（包括 `npm.cmd` → `cmd.exe` → `node`
链条上被孤立的那些）。也可以在另一个终端跑 `python scripts/dev.py --stop`。

从别的进程停止时，写 `.dev-logs/stop.request` 这个文件即可——脚本会看到它并自行清理退出。
Windows 上这是唯一可靠的停法：`Popen.terminate()` 走的是 `TerminateProcess`，不会投递信号，
因此 Python 的清理逻辑根本没机会运行，端口会被占住。

`--runtime hermes` 模式下，只有**本次启动**的 Hermes 会在退出时被停掉；本来就在跑的
Hermes 属于启动它的人，不会被这个脚本带走。

## 自检

```bash
# 只验证真实 Hermes 的启停闭环（不可达 → 可达 → 不可达）
python scripts/dev.py --check-runtime

# 验证整条链路：Hermes 启停 + Hermes 模式的栈 + Pi 模式（主运行时）+ mock 模式
python scripts/check-runtime.py --with-mock

# 没有模型密钥时跳过 Pi 那一项
python scripts/check-runtime.py --with-mock --skip-pi
```

`--check-runtime` 存在的原因是 teardown 最难被观察到：用 `taskkill /F` 杀栈时 Python 是被
直接终止的，永远走不到 `finally` 里的清理，于是"退出时会停掉自己启的 Hermes"这条路径根本没
被执行过。`check-runtime.py` 把三个状态都走一遍并报告到达了哪一个。

Pi 模式之所以要单独查：Pi 是主运行时，一个起不来 Pi 的栈并不是能承载产品工作的栈——控制台
里只剩 mock 和远程参考运行时。

注意 Vite 绑在 IPv6 `::1`、API 绑在 IPv4 `127.0.0.1`：判断端口是否释放必须两个协议族都试，
只探 IPv4 会把"还占着"报成"空着"。

## 手动分开启动

```bash
# 后端
cd codeweaver-hermes-lab
npm run dev:stack

# 前端
cd codeweaver-workspace
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
