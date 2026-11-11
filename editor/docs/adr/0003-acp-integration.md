# ADR-0003：编辑器通过 ACP 接入 CodeWeaver 运行时

- 状态：已接受
- 日期：2026-11-11
- 适用范围：`editor/crates/agent-acp`、`editor/crates/agent-ui`、`editor/crates/platform`

## 背景

计划 `doc/EDITOR_DEVELOPMENT_PLAN.md` §8.4 规定编辑器的 Agent 集成链路：

```text
Agent Panel
  -> Context Builder（文件、选区、诊断、Git 状态）
  -> ACP Client（session/new、session/prompt、cancel、permission）
  -> CodeWeaver Runtime
  -> Stream Mapper（message、tool、approval、usage）
  -> Proposed Transaction / Diff
  -> User Approval
  -> Buffer Transaction
```

阶段 0 的退出条件之一是「ACP 可流式输出和取消」。仓库里已经有一条跑通的
ACP 链路：`codeweaver-hermes-lab` 的 `AcpAgentServer` 在 stdio 上说 JSON-RPC 2.0，
Zed 作为客户端启动它。本 ADR 记录自研编辑器如何接入这条链路。

## 决策

### 1. 传输：JSON-RPC 2.0 over stdio，子进程承载

编辑器不自己实现 CodeWeaver 的调度与策略，它是一个 **ACP 客户端**：启动
`codeweaver-hermes-lab` 的 agent endpoint 作为子进程，通过它的 stdin/stdout
说 JSON-RPC 2.0。

选择 stdio 而不是 HTTP 的理由：

- agent endpoint 本来就是为 stdio 设计的（Zed 用 `agent_servers` 启动它），
  走同一条路径意味着**同一份服务端代码同时被 Zed 和自研编辑器验证**；
- 子进程模型天然隔离：端点崩溃不会带走编辑器；
- 权限审批是端点发起的**请求**（`session/request_permission`），需要双向
  RPC，stdio 直接支持。

### 2. 帧格式：写 ndjson，读两种

端点支持两种帧格式（newline-delimited JSON 与 LSP 风格 `Content-Length`），
并**从收到的第一帧推断**用哪种、再用同一种回答。

客户端因此固定写 ndjson，同时在读取侧实现两种格式的自动探测——与端点的
`AcpFrameReader` 规则一致（前 32 字节以 `content-length:` 开头即为
Content-Length）。`Content-Length` 按**字节**计数，多字节字符跨 chunk 也不会
算错。

### 3. 分层：传输、协议、面板三层分开

| crate | 职责 | 依赖 |
|---|---|---|
| `agent-acp` | 帧格式、stdio 传输、请求分发 | serde / serde_json |
| `agent-ui` | 编辑器自己的会话模型（面板） | agent-acp |
| `platform` | 事件循环、按键、绘制 | agent-acp, agent-ui, editor-core |

`agent-acp` 里放的是**线上类型**（`SessionUpdate`、`StopReason`…），面板里放的是
**编辑器类型**（`AgentPanel`、`ToolEntry`、`PendingPermission`）。这样 ACP 升级
不会渗进编辑器核心——对应计划 §6 的「协议对象和内部对象分离」。

`agent-ui` 不依赖 `editor-core`：审批后的 Diff 应用会需要它，但那是阶段 2 的
事，现在不预先建立这条边。

### 4. 事件驱动：编辑器的事件循环永不阻塞在 Agent 上

`Client::start_prompt` 写出请求就返回；Agent 的输出去事件流上，编辑器每帧
`pump()` 一次收集。这与编辑器的帧调度模型一致：主线程只处理输入和提交画面。

`Client::prompt`（阻塞版）保留给非 UI 调用方（测试、CLI）。它是同一套协议的
另一种消费方式，不是另一条实现。

### 5. 端点由环境变量指定，编辑器不猜路径

端点是一个独立进程，编辑器通过 `CODEWEAVER_ACP_COMMAND` 与
`CODEWEAVER_ACP_ARGS` 知道怎么启动它，`CODEWEAVER_ACP_CWD` 指定会话的工作
目录。**没有这些变量时面板明确显示"未配置"，不静默显示空白**——沿用运行时侧
「默认值必须走真路径」的教训。

会话在**第一次发问时**才创建：启动编辑器不应该拉起一个用户没要求的运行时。

### 6. 权限必须由人回答，缺省为拒绝

端点把运行时的 `permission.requested` 变成 `session/request_permission`
请求，编辑器必须回一个选项。回 `allow` 才会放行；其它情况（包括编辑器关闭、
超时、解析失败）一律拒绝，让运行时不再等待——这是端点侧已有的 fail-closed
语义，编辑器必须对齐，不能因为 UI 没接住就变成"批准"。

## 后果

- 阶段 0 的「ACP 可流式输出和取消」有了可运行的证据：
  `cargo run -p codeweaver-platform --example agent_end_to_end` 走真实端点 +
  mock 运行时，打印会话 id、事件数、停止原因和实际收到的文本。
- 契约测试 `editor/crates/agent-acp/tests/acp_contract.rs` 直接复用编译后的
  `AcpAgentServer`，只替换 backend，因此测的是生产帧格式与生产事件形状。
- 端点字段是 **camelCase**（`sessionId`、`agentInfo`、`stopReason`），Rust 侧
  用 `#[serde(rename_all = "camelCase")]` 映射，字段名保持项目的 snake_case 风格。
- 目前 Agent 面板只能**展示**：回复文本、工具调用、待审批提示。Diff 预览与
  "审批后写入 Buffer"属于阶段 2，届时 `agent-ui` 要开始依赖 `editor-core`
  的事务接口。

## 被否决的方案

- **编辑器直接调 CodeWeaver 的 HTTP API**：会绕开 ACP，而 ACP 正是 Zed 与
  自研编辑器共用的那层协议；两套接入方式意味着两份集成代码。
- **在 Rust 里重写 agent endpoint**：那是运行时侧的职责，编辑器只需要一个
  客户端。重写会立刻产生两份策略/审计实现。
- **用 tokio 做异步 ACP 客户端**：编辑器的事件循环是同步的 winit 循环，
  一个后台读线程加一条 channel 就够了。引入 async 运行时会给一个还没有
  异步基础设施的代码库增加大量依赖，且与「主线程只处理输入和提交画面」
  的目标没有冲突。
- **在编辑器里内置运行时**：会让编辑器背上调度、策略、审计的责任，而那正是
  CodeWeaver 运行时已经做好的部分。
