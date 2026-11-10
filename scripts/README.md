# DocuFlow 本地开发栈

一个脚本同时拉起 **后端**（`docuflow-hermes-lab`）和 **前端**（`docuflow-workspace`），
并把两个进程的输出合并到同一个控制台。

## 用法

```powershell
# 桌面形态（Tauri 窗口）—— 默认
powershell -ExecutionPolicy Bypass -File .\scripts\dev.ps1

# 浏览器形态（本机 WebView2 不可用时用这个）
powershell -ExecutionPolicy Bypass -File .\scripts\dev.ps1 -Mode browser

# 端口被占用时强制覆盖
powershell -ExecutionPolicy Bypass -File .\scripts\dev.ps1 -Force
```

| 参数 | 作用 |
|---|---|
| `-Mode desktop` | Tauri 桌面窗口（默认） |
| `-Mode browser` | 只起 Vite，并自动打开浏览器 |
| `-Force` | 端口被占用时先结束占用进程 |
| `-NoBuild` | 跳过后端 TypeScript 编译，直接用现有 `dist` |
| `-NoBrowser` | browser 模式下不自动开浏览器 |

## 脚本做什么

1. 检查 node、两个工程的 `node_modules`（缺了就自动 `npm install`）。
2. 编译后端（`tsc -p tsconfig.json`）。**编译报错但 `dist` 已生成时会继续运行**并给出警告——
   `tsc` 遇到类型错误仍会 emit，而 lab 里正在开发的文件经常带临时类型错误。
3. 检查端口 8787（API）和 1420（Vite）。
4. 启动后端，等 `/api/v1/runtimes` 返回 200 才算就绪。
5. 启动前端，等 1420 开始监听。
6. 合并输出到控制台：`[api]` 和 `[ui]` 前缀区分来源。日志同时落在 `.dev-logs/`。

## 停止

按 **Ctrl+C**。脚本会一并结束后端和前端进程。

## 手动分开启动

```powershell
# 后端
cd docuflow-hermes-lab
npm run dev:stack

# 前端
cd docuflow-workspace
npm run dev          # 浏览器形态
npm run tauri dev    # 桌面形态
```

## 已知问题

- **本机 `npm run tauri dev` 打不开窗口**：建窗时 WebView2 返回 `E_ACCESSDENIED` (os error 5)，
  已排除代码与权限因素，是本机 WebView2 运行时环境问题。用 `-Mode browser` 绕过。
- **Ctrl+C 会被 PowerShell 当成调试中断**（"正在进入调试模式"），脚本本身仍会正常退出并清理子进程，
  只是提示信息可能出现偏差。
- **Vite 绑在 IPv6 `::1`** 而不是 IPv4 `127.0.0.1`。这意味着用 IPv4 探测端口会误报"空闲"，
  脚本改用 `Get-NetTCPConnection` 判断占用。
