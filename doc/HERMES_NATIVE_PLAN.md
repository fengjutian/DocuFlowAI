# Hermes 本机集成规划（无 Docker）

## 目标

CodeWeaver 在 Windows 上直接运行 Hermes，不安装、不启动、也不依赖 Docker。Hermes 作为独立本机进程提供 HTTP/SSE 与 Gateway 接口；CodeWeaver 负责调度、策略、审计、worktree 隔离和验收。

固定运行时版本为 `hermes-agent v0.23.0`（commit `2bd378ed1b8d6bf2192cb26e0a55ff3091e6bd19`），版本来源以 `codeweaver-hermes-lab/hermes.lock.json` 为准。

## 运行拓扑

```text
CodeWeaver Workspace (Tauri / Browser)
        |
        v
CodeWeaver API 127.0.0.1:8787
        |
        +-- Hermes Adapter --HTTP/SSE--> Hermes 127.0.0.1:8642
        |
        +-- Policy Engine
        +-- Git Worktree
        +-- Evaluation Process
        +-- JSONL Journal
```

所有服务默认只监听 `127.0.0.1`。Hermes 安装在 `%LOCALAPPDATA%\hermes\hermes-agent-0.23.0` 的独立 Python 虚拟环境中，不污染系统 Python。

## 实施阶段

### P1：本机安装与可重复启动

- 使用 `scripts/install-hermes.ps1` 从锁定的 tag/commit 安装 Hermes。
- 安装脚本校验版本、写入 `install-stamp.json`，并补齐 Hermes 首次调用才会加载的可选依赖。
- 提供独立的启动、停止、状态检查入口；进程 PID 和日志写到 CodeWeaver 数据目录。
- 启动前检查端口 8642，避免误连到其他 Hermes 实例。
- 验收：`npm run check:hermes` 通过，`/v1/capabilities` 与 `/v1/models` 可访问。

### P2：CodeWeaver 启动编排

- 为 `scripts/dev.py` 增加真实 Hermes 模式，例如 `--runtime hermes`；开发默认仍可选择 mock，真实运行时必须显式选择。
- 启动顺序：Hermes → 健康检查 → CodeWeaver API → Workspace。
- 任一子进程提前退出时打印对应日志路径；CodeWeaver 退出时只终止本次启动的 Hermes，不杀死用户手工启动的实例。
- 验收：一条命令完成整套本机启动，并能干净退出和再次启动。

### P3：配置与凭据

- 模型密钥只保存在 Hermes 的独立 profile 或用户环境变量中，不写入仓库、日志和执行 journal。
- CodeWeaver 使用 `HERMES_HTTP_BASE_URL=http://127.0.0.1:8642` 连接 Hermes。
- 对日志中的 Authorization、API key、token 和模型请求头做统一脱敏。
- 启动时检查必要配置并快速失败，不允许在第一次 agent turn 时临时下载依赖。
- 验收：仓库扫描无凭据；缺少配置时给出可操作错误；真实模型任务可以完成。

### P4：Hermes 工具面接入 —— 已决定不做（D-060）

工具面（`read_file`、`request_diff`、`request_apply_diff`、`evaluate_changes`）**只在 Pi 侧提供**。

按 D-042，Pi 是主运行时候选，Hermes 保留为远程运行时参考。工具面在 Pi 侧已经跑通完整链路
（`read_file` -> `request_apply_diff` -> `evaluate_changes`，真实 MiniMax M3），也已经在 Pi 侧签收
了 agent 级安全探针、取消与会话恢复。为一个参考运行时再实现一套等价的 MCP 接入，投入的是重复
劳动，换不到 Pi 上已经拿到的能力（工具执行前拦截、`AbortSignal` 取消、transcript 续跑）。

Hermes 保留的价值是另一件事：它是**远程 HTTP + SSE** 这一类运行时形态的唯一实例，其适配器证明
了契约在远程形态下成立（run 创建、SSE 事件流、终态识别、审批往返）。这份契约证据不因为不做
工具面而失效。

因此本规划的范围收敛为：让 Hermes 能在本机可靠启停（P1/P2/P3）、把边界落到 Windows 原生隔离
（P5），并保持远程运行时形态可用。**不在本规划中的内容**追加一条：不为 Hermes 暴露 CodeWeaver
工具面。

### P5：无 Docker 安全边界

- 第一层：每个任务使用独立 Git worktree，禁止直接修改主工作树。
- 第二层：文件、命令和网络操作全部经 CodeWeaver 工具与策略门；默认拒绝未知操作。
- 第三层：Evaluation 使用独立子进程、超时、受限环境变量和显式工作目录。
- 第四层：采用 Windows 原生隔离实现宿主机边界。优先评估 AppContainer；若 Hermes/Python 兼容性不足，则使用 Windows Sandbox 或 Hyper-V 隔离虚拟机。此层不得退化为普通用户进程后仍宣称“已隔离”。
- 重新定义 `SEC-B*` 证据提供器，使其接受 `appcontainer`、`windows-sandbox` 或 `hyper-v`，不再要求 Docker daemon。
- 验收：越权读写、工作区外访问、未授权网络、子进程逃逸和资源耗尽测试均由 Windows 边界实际拒绝；没有边界证据时保持 `blocked`，不得记为 `passed`。

### P6：稳定性与发布

- 增加 Hermes 崩溃、端口占用、SSE 中断、超时、重复启动和 CodeWeaver 重启测试。
- 明确 HTTP Hermes 暂不支持的能力（取消、恢复、子代理等），不做静默降级。
- 打包时不捆绑用户密钥；Hermes 可选择首次安装或检测已有的锁定版本。
- 验收：冷启动、异常恢复、升级回滚、卸载和数据保留流程都有自动化测试或操作记录。

## 近期执行顺序

1. ~~完成真实 Hermes 的 start/stop/status PowerShell 脚本。~~ **已完成**：`codeweaver-hermes-lab/scripts/hermes.ps1`（D-056）。
2. ~~给 `scripts/dev.py` 增加 `mock` / `hermes` 运行时选择和健康检查。~~ **已完成**：`scripts/dev.py --runtime mock|hermes`（D-056）。
3. ~~跑通 Hermes 工具面的真实端到端证据。~~ **已取消**：工具面只在 Pi 侧做（D-060），Hermes 保持为远程参考运行时。
4. 抽象安全边界 provider，移除验证代码对 Docker 的硬编码。
5. 先完成 AppContainer 兼容性原型，再决定使用 AppContainer 或 Windows Sandbox/Hyper-V。
6. 更新 Phase 0 清单和 CI，使 Docker 不再是安装项或验收前置条件。

## 不在本规划中的内容

- 不使用 Docker Desktop、Docker Engine、WSL Docker 或 Compose。
- 不把普通进程、提示词约束或 Hermes 的批准弹窗当成宿主机隔离证据。
- 不自动上传代码、日志、凭据或执行记录到外部服务。
- **不为 Hermes 暴露 CodeWeaver 工具面**（D-060）：主运行时是 Pi，工具面与它的安全探针、取消、
  恢复能力都已在 Pi 侧签收。

