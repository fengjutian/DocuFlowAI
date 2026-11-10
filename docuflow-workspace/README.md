# DocuFlow Workspace（桌面端 UI）

Tauri v2 + React + TypeScript 的 DocuFlow 控制台。它是 `docuflow-hermes-lab` 服务端的**客户端**：界面本身不做任何业务判定，所有状态都来自服务端。

## 技术栈

| 部分 | 选型 | 说明 |
|---|---|---|
| 桌面壳 | Tauri v2 | Rust 侧仅提供两个命令，用于注入后端地址/令牌的默认值 |
| 前端 | React 19 + TypeScript 6 + Vite 8 | HashRouter，打包后不依赖服务器重写规则 |
| 组件 | shadcn/ui（Radix + Tailwind v4） | 组件源码在本仓库 `src/components/ui`，可自由改动 |
| 图标 | lucide-react | 全部图标按语义命名，未使用 emoji |
| 状态 | zustand | 仅用于持久化连接设置 |

## 运行

仓库根目录有一个一键脚本，同时拉起后端和前端：

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\dev.ps1
```

参数：`-Mode browser`（本机 WebView2 不可用时）、`-Force`（端口被占用时强制覆盖）、`-NoBuild`、`-NoBrowser`。
详见 [`scripts/README.md`](../scripts/README.md)。

手动分开启动：

先起后端（`docuflow-hermes-lab`）：

```powershell
cd ..\docuflow-hermes-lab
npm run dev:stack      # API 服务 + lab 自带的 mock Hermes，监听 127.0.0.1:8787
```

`dev:stack` 用的是契约测试里的那个 mock Hermes：事件形状可信，但**不等于**固定版本的真实 Hermes 行为。
接真实运行时时用 `npm run serve` 并设置 `HERMES_HTTP_BASE_URL`；如果服务端开了鉴权，再加 `DOCUFLOW_API_TOKEN`。

再起桌面端：

```powershell
cd ..\docuflow-workspace
npm install
npm run tauri dev      # 开发；npm run tauri build 产出安装包
```

只想看界面：`npm run dev`，浏览器打开 <http://localhost:1420> 即可（后端地址在左下角「连接设置」里改）。

## 连接设置

地址与令牌不写死在代码里：

- 默认地址 `http://127.0.0.1:8787`，可在「连接设置」里改，存在本地。
- 服务端若要求 Bearer Token，在同一处填；未开启鉴权时留空。
- 桌面端启动时可用环境变量 `DOCUFLOW_API_URL` / `DOCUFLOW_API_TOKEN` 注入默认值（Rust 侧读取）。
  用户手动改过的值优先，环境变量不会覆盖。

## 页面与后端接口的对应关系

| 页面 | 用到的接口 |
|---|---|
| 工作台总览 | `GET /api/v1/runtimes`、`GET /api/v1/usage`、`GET /api/v1/skills`、`GET /api/v1/events` |
| Runtime 管理 | `GET /api/v1/runtimes`、`POST /api/v1/runtimes/{id}/health-check` |
| 执行与事件 | `POST /api/v1/tasks/{taskId}/executions`、`GET /api/v1/events`、`GET /api/v1/executions/{id}/events` |
| 执行详情 | `GET /api/v1/executions/{id}`、`POST .../cancel|retry|resume`、`GET .../approvals`、`POST .../approvals/{requestId}`、`GET .../usage`、`GET .../skills` |
| 技能管理 | `GET/POST /api/v1/skills` |
| 用量统计 | `GET /api/v1/usage` |
| 提交追溯 | `GET /api/v1/trace/commits/{sha}` |

`POST /api/v1/runtimes/{id}/disable` 在设计 12.1 中定义但服务端尚未实现，界面上该按钮保持禁用并注明原因，不做假动作。

## 几个刻意的设计决定

- **执行列表来自事件流。** 服务端没有"列出执行"的接口，journal 也是按执行存放的。因此工作台与执行页的执行清单由 `GET /api/v1/events` 的实时事件推导，并额外提供"按执行 ID 直接打开"以覆盖客户端打开前就已完成的任务。
- **收到最后一条消息 ≠ 成功。** 执行完成与否看终态和评估证据（完成门禁）。门禁不通过时界面照实显示"已失败 / 无评估结果"。
- **断线可恢复。** SSE 不可用时自动降级为按 `since` 轮询 journal；重连后按 sequence 补齐，不会静默丢事件。
- **技能校验和在客户端算。** 服务端要求 SHA-256，界面让用户填技能正文，客户端算哈希后只提交清单，不上传正文。
- **未建模的 Runtime 事件不隐藏。** 适配器始终保留原始事件名（`payload.hermesEvent`），界面会把它显示在事件行上，而不是笼统的"未知事件"。

## 跨源说明

Tauri webview 与 Vite 开发服务器都是跨源访问 `127.0.0.1:8787`，服务端因此加了一份**窄范围 CORS 白名单**（`localhost:1420`、`tauri.localhost` 等，可用 `DOCUFLOW_ALLOWED_ORIGINS` 扩展）。
这是允许名单不是通配符：CORS 不承担鉴权，Bearer Token 仍然承担。

## 已验证 / 未验证

已验证（连真实后端跑通）：

- `npx tsc --noEmit` 0 错误；`npx vite build` 成功；`cargo check` 成功（tauri 2.12.5 / wry 0.57.1）。
- Runtime 列表与健康检查（`POST .../health-check` 返回 healthy）。
- 启动执行并实时收到 SSE 事件（`GET /api/v1/events` 保持 200 `text/event-stream`）。
- 执行详情页：8 条事件时间线、检查点快照 ID、完成门禁。
- 技能注册（POST 201 + 列表刷新）、用量页空态、追溯页空态。
- 后端未连接时的降级提示。

未验证：

- **桌面窗口未能在本机打开**：`npm run tauri dev` 编译成功，但建窗时 WebView2 返回 `E_ACCESSDENIED`（os error 5）。已排除会话/权限问题（同为 session 1、WinSta0，WinForms 窗口可正常创建），本机 WebView2 运行时 157.0.4341.42 已安装但 `msedgewebview2.exe` 直接执行即失败且当前账户无 per-user 安装。判定为**本机 WebView2 运行时环境问题，非本工程代码缺陷**；因此上表的交互验证均在浏览器形态（`npm run dev`）下完成。
- 真实 Hermes 运行时下的事件序列（本地用的是 lab 的 mock）。
- `cargo build` 打包产物、macOS/Linux 平台行为。

## 开发期的两个已知现象

- **`events.ts` / `execution-stream.ts` 的拆分**：`useExecutionStream` 等 hook 放在独立的 `src/state/execution-stream.ts`，Provider 组件留在 `ExecutionStreamProvider.tsx`。这样改动 hook 不会让 Fast Refresh 失效整个 provider 模块。
- **React StrictMode 下首个 SSE 请求会 `ERR_ABORTED`**：开发模式双挂载会立即取消第一次订阅，第二次才真正建流。属于预期行为，线上构建（无 StrictMode 双挂载）不出现。
