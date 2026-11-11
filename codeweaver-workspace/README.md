# CodeWeaver · 代码织造（桌面端 UI）

**Weaving Ideas Into Software.**

Tauri v2 + React + TypeScript 的 CodeWeaver 控制台。它是 `codeweaver-hermes-lab` 服务端的**客户端**：界面本身不做任何业务判定，所有状态都来自服务端。

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

```bash
python scripts/dev.py
```

参数：`--mode browser`（本机 WebView2 不可用时）、`--force`（端口被占用时强制覆盖）、
`--no-build`、`--no-browser`、`--stop`。
详见 [`scripts/README.md`](../scripts/README.md)。

手动分开启动：

先起后端（`codeweaver-hermes-lab`）：

```powershell
cd ..\codeweaver-hermes-lab
npm run dev:stack      # API 服务 + lab 自带的 mock Hermes，监听 127.0.0.1:8787
```

`dev:stack` 用的是契约测试里的那个 mock Hermes：事件形状可信，但**不等于**固定版本的真实 Hermes 行为。
接真实运行时时用 `npm run serve` 并设置 `HERMES_HTTP_BASE_URL`；如果服务端开了鉴权，再加 `CODEWEAVER_API_TOKEN`。

再起桌面端：

```powershell
cd ..\codeweaver-workspace
npm install
npm run tauri -- dev      # 开发；npm run tauri -- build 产出安装包
```

只想看界面：`npm run dev`，浏览器打开 <http://localhost:1420> 即可（后端地址在左下角「连接设置」里改）。

## 连接设置

地址与令牌不写死在代码里：

- 默认地址 `http://127.0.0.1:8787`，可在「连接设置」里改，存在本地。
- 服务端若要求 Bearer Token，在同一处填；未开启鉴权时留空。
- 桌面端启动时可用环境变量 `CODEWEAVER_API_URL` / `CODEWEAVER_API_TOKEN` 注入默认值（Rust 侧读取）。
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
- **未建模的 Runtime 事件不隐藏，但也不淹没别的内容。** 适配器始终保留原始事件名（Hermes 在 `payload.hermesEvent`，Pi 在 `payload.runtimeEvent`），界面会把它显示在事件行上。但 Pi 的流式生命周期事件（`turn_start` / `message_start` / `message_update` / `message_end` / `turn_end`）是**每个分片一条**，一次运行能产生上百条；这类行折叠成一行紧凑摘要，只有带信息的少数事件才占完整卡片。原始数据仍可在执行详情与 journal 里查到。
- **答案要看得见。** 界面从 `agent.message.completed` 里抽出助手的原文，放在「Agent 输出」卡片里；时间线上的消息增量、工具结果也直接显示正文，不用点开"原始数据"才知道模型说了什么。

## 运行时的选择与取消

- 新建执行时可以点名运行时（默认"自动选择（由服务端调度）"）。点名的运行时要么就用它，要么被拒绝，不会被悄悄换成别的（D-047）。
- 执行详情页头部显示这次执行实际用的运行时 id。
- 「取消」把执行送进 `cancelling`，等运行时自己确认中止后才落到终态；终态事件带 `runtimeConfirmed` 与运行时自己的 `runtimeStopReason`（D-052）。

## 重启之后

服务重启会清空内存里的会话，但 journal 在磁盘上，执行的历史还在。这时：

- 执行详情仍然读得到——运行时 id 来自执行启动时写进 journal 的那条记录（不是从服务端默认运行时猜的），状态来自终态事件。
- 页面会写明「该执行由更早的服务进程运行」，并禁用「取消」：没有活的会话可取消。服务端对这种执行也直接拒绝取消并说明原因（409 `execution_not_live`），而不是在适配器深处炸开。
- 运行时 id 说不出来的执行就显示「未知运行时」，不填一个像样的默认值。
- 「重试」走的是恢复决策：服务端先问**拥有这个执行的运行时**会话还在不在。Pi 的会话在 worker 进程里，worker 死了会话就没了，所以结论是「重建」——新开一个执行，把旧执行的 checkpoint 注入请求，并且**继续跑在原来那个运行时上**，而不是被悄悄换成服务端默认的那个。

## 跨源说明

Tauri webview 与 Vite 开发服务器都是跨源访问 `127.0.0.1:8787`，服务端因此加了一份**窄范围 CORS 白名单**（`localhost:1420`、`tauri.localhost` 等，可用 `CODEWEAVER_ALLOWED_ORIGINS` 扩展）。
这是允许名单不是通配符：CORS 不承担鉴权，Bearer Token 仍然承担。

## 已验证 / 未验证

已验证（连真实后端跑通）：

- `npx tsc --noEmit` 0 错误；`npx vite build` 成功；`cargo check` 成功（tauri 2.12.5 / wry 0.57.1）。
- Runtime 列表与健康检查（`POST .../health-check` 返回 healthy）。
- 启动执行并实时收到 SSE 事件（`GET /api/v1/events` 保持 200 `text/event-stream`）。
- 执行详情页：8 条事件时间线、检查点快照 ID、完成门禁。
- 技能注册（POST 201 + 列表刷新）、用量页空态、追溯页空态。
- 后端未连接时的降级提示。
- **真实 Pi 运行时（`pi-local` + MiniMax-M3）下的完整一轮**：新建执行时点名 `pi-local` → 执行详情页头部显示 `pi-local` → 模型输出期间点「取消」→ 状态 `running` → `已取消`，终态事件 `agent.execution.cancelled` 带 `runtimeConfirmed=true` 与 `runtimeStopReason=aborted`，运行时的 `agent.message.completed` 同时记录 `stopReason=aborted`（中止真的到达了运行时）。
- 同一次运行里助手的中文输出在「Agent 输出」卡片与时间线上原样显示（未出现编码错乱）；一次 216 条事件的时间线里，131 条 Pi 流式生命周期事件折叠为紧凑行，带信息的消息 / 工具事件仍占完整卡片。
- **服务重启后历史执行仍然可读**：重启前后同一个执行 `exec_84e47a87…` 的详情都返回 `runtimeId=pi-local`、`status=succeeded`、353 条事件完整；重启后 `live=false`，「取消」按钮禁用，服务端对取消返回 409 `execution_not_live`；页面显示「该执行由更早的服务进程运行」。

未验证：

- **桌面窗口未能在本机打开**：`npm run tauri dev` 编译成功，但建窗时 WebView2 返回 `E_ACCESSDENIED`（os error 5）。已排除会话/权限问题（同为 session 1、WinSta0，WinForms 窗口可正常创建），本机 WebView2 运行时 157.0.4341.42 已安装但 `msedgewebview2.exe` 直接执行即失败且当前账户无 per-user 安装。判定为**本机 WebView2 运行时环境问题，非本工程代码缺陷**；因此上表的交互验证均在浏览器形态（`npm run dev`）下完成。
- 真实 **Hermes** 运行时下的事件序列（本地用的是 lab 的 mock）；Pi 路径已用真实运行时验过。
- `cargo build` 打包产物、macOS/Linux 平台行为。

## 开发期的两个已知现象

- **`events.ts` / `execution-stream.ts` 的拆分**：`useExecutionStream` 等 hook 放在独立的 `src/state/execution-stream.ts`，Provider 组件留在 `ExecutionStreamProvider.tsx`。这样改动 hook 不会让 Fast Refresh 失效整个 provider 模块。
- **React StrictMode 下首个 SSE 请求会 `ERR_ABORTED`**：开发模式双挂载会立即取消第一次订阅，第二次才真正建流。属于预期行为，线上构建（无 StrictMode 双挂载）不出现。
