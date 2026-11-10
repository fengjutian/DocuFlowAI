# DocuFlow AI 项目状态与剩余工作

## 1. 结论摘要

DocuFlow AI 已经具备一条可运行的最小闭环：固定的 Hermes 0.23.0 运行时已安装并可达（API server 在 `127.0.0.1:8642`），Phase 0 清单里 P0-001 / P0-002 / P0-003 三项已通过真实运行时证据，Phase 1 适配器冒烟项 AT-001 到 AT-004 也全部通过，另有 Pi 侧的完整工具面链路（`read_file` -> `request_apply_diff` -> `evaluate_changes`）在真实 MiniMax M3 模型上跑通。因此现在能做的是：注册运行时、提交任务、接收并归一化运行时事件、识别终态、按策略执行权限判定，并通过 Code Engine 把模型提出的补丁落进隔离 worktree、再由独立进程跑验收检查决定是否放行完成。

还不能做的是：24 项 Phase 0 清单远未签收（passed 3、blocked 4、not_run 17，failed 0），取消（cancellation）、恢复（resume）、子代理、技能等能力在 HTTP 适配器上仍然被显式声明为不可用；多模型执行策略、Agent 能力评分、Hermes 侧的工具面暴露尚未实现。换句话说，骨架和单运行时路径已经成立，但"多运行时 + 可回滚 + 可评分"的产品形态还没有被验证。

## 2. 已验证的能力

下列各项都有真实运行时证据（而不是仅靠 mock 或单元测试）支撑：

- **P0-001 HTTP create run — passed**：证据为 `platform=hermes-agent`、`run_id=run_255aff9a4fde40aea41f450f392c53ec`、`status=queued`（`reports/phase0.json`）。
- **P0-002 HTTP SSE event stream — passed**：证据为两条 `message.delta`（`DOCUF`、`LOW_PHASE0_OK`）加一条终态 `run.completed`，其中 `output=DOCUFLOW_PHASE0_OK`、`usage.input_tokens=12395`、`output_tokens=8`、`total_tokens=12403`。
- **P0-003 TUI Gateway session/prompt — passed**：证据为 `method=session.create` 返回 `session_id=c91822af`、`stored_session_id=20261109_205044_1d61a9`、`message_count=0`，并记录了 `model=MiniMax-M3`、`cwd=D:\github\DocuFlowAI\docuflow-hermes-lab`、`branch=main`。
- **AT-001 运行时注册与健康检查 — passed**：依据是实时的 `/v1/capabilities`。
- **AT-002 任务提交 — passed**：依据是来自真实运行的 `externalRunId`。
- **AT-003 执行事件接收 — passed**：运行时事件经适配器送达。
- **AT-004 终态识别 — passed**：`execution.completed` 且 `status=succeeded`。
- **Pi 工具面全链路 — 通过**：`read_file` -> `request_apply_diff` -> `evaluate_changes`。README 记录的 `scripts/probe-pi-tools.mjs` 探针会建立一个一次性 git 仓库、在 HTTP API 上以 `pi-local` 运行时启动任务，让真实 MiniMax 中国区模型驱动这些工具；判定条件是模型读到了文件、变更经 Code Engine 落到 worktree、写出了变更记录、验收检查执行、且完成门禁被打开。Pi 侧证据另记为 5/5 运行时探针、9/9 适配器测试、真实 MiniMax M3 与完整工具面运行。

需要注意的是：契约测试（`src/tests/support/mock-hermes-server.ts`）只能证明客户端面对一个合规服务端的行为正确，对固定版本的真实运行时审批语义与恢复行为不作任何保证（D-031）。

## 3. Phase 0 清单状态

24 项合计：**passed 3、failed 0、blocked 4、not_run 17**（`reports/phase0.json` 的 `summary`，生成时间 `2026-11-10T02:56:52.791Z`）。

- **通过（3）**：`P0-001`、`P0-002`、`P0-003`。
- **被阻塞（4）**：`P0-009`（允许文件读取）、`P0-010`（受限文件拒绝）、`P0-011`（未授权写入拦截）、`P0-013`（未授权网络拦截）。这四项的 `notes` 一致：拒绝运行 agent security 探针，除非在一次性、无生产凭据的环境中设置 `HERMES_SECURITY_LAB_CONFIRM=I_UNDERSTAND_THE_RISK`。
- **未运行（17）**：`P0-004`（ACP 编辑器连接）、`P0-005`（单步任务）、`P0-006`（多工具任务）、`P0-007`（多轮会话）、`P0-008`（子代理委派）、`P0-012`（危险命令拦截/批准）、`P0-014`（并行 Agent 运行）、`P0-015`（worktree 隔离）、`P0-016`（沙箱资源限制）、`P0-017`（宿主机路径保护）、`P0-018`（取消）、`P0-019`（客户端断开）、`P0-020`（运行时崩溃）、`P0-021`（会话恢复/重建）、`P0-022`（审计时间线）、`P0-023`（token/模型/时长核算）、`P0-024`（端到端可追溯）。

这些安全类项目在边界证据之外还需要 agent 级别的探针才能签收；`SEC-B*` 边界证据只支撑、不替代这些清单项（D-008、D-020）。

## 4. 尚未实现的功能

- **多模型执行策略**：当前只有单个运行时适配器路径。`RuntimeScheduler` 的顺序是"先过滤不健康运行时，再过滤所需能力，最后才按 `round_robin` / `least_loaded` / `first_healthy` 选"（D-025），README 也明确 `No candidate is an error, never a silent fallback`——但尚未跑出多个模型/多个运行时并存的策略证据。D-046、D-047 给出了按执行路由适配器与"点名即遵从否则拒绝"的规则，说明多运行时的接线已定但策略本身尚未落成可验证能力。
- **Agent 能力评分**：仓库里有 `skillEffectiveness()`（按 `skillId@version` 关联 bindings 与评估裁决，输出 executions / evaluated / passed / failed / blocked 与 pass rate，未评估的执行算 `evaluated: 0`、`passRate: null`），但这是技能维度而非 Agent（模型/运行时）维度的能力评分；运行时侧的 `getCapabilities()` 目前只按 D-005 声明能力（`streaming` 来自服务端声明，取消、恢复、工具事件、权限事件、子代理、技能在对应清单项通过前一律为 false），还没有把运行时的实测表现聚合成一个可比较的评分。
- **Hermes 侧的工具面暴露**：工具面目前是 DocuFlow 侧提供的（`read_file`、`request_diff`、`request_apply_diff`、`evaluate_changes`，按执行构建，运行时只看到该执行被允许的工具，D-049），并且完整工具面运行是在 Pi 侧验证的。Hermes 自身尚未暴露等价工具面，因此 Hermes 路径仍缺"模型驱动 DocuFlow 工具"的端到端证据。

## 5. 已知阻塞

- **Docker daemon 不可达**：沙箱边界探针（`npm run validate:security`）要求 Docker daemon。按照 README 的规则，daemon 缺失记为 `blocked`，被拒绝的探针"成功"则记为 `failed` 并标记为边界突破；两种情况都不得转成 `passed`。因此 `SEC-B*` 边界证据当前无法产出。
- **agent security 探针需要一次性实验环境**：P0-009 / P0-010 / P0-011 / P0-013 需要在无生产凭据的一次性环境中设置 `HERMES_SECURITY_LAB_CONFIRM=I_UNDERSTAND_THE_RISK` 才能运行；未设置时什么都不运行，该状态被记为 `blocked`。

另外，agent 级探针的判定标准本身是严格的：只有当 transcript 中出现拒绝标记（EACCES / ENOENT / permission denied）且不含禁止内容才算 `passed`；模型只是"拒绝"或根本没尝试，记为 `blocked`——"拒绝不等于通过"（D-020）。

## 6. 运行时现状

`hermes-local` 与 `pi-local` 并存，尚未二选一。

| | Hermes | Pi |
|---|---|---|
| 形态 | 远程 HTTP + SSE | 进程内 TypeScript 库 |
| 安装 | tag tarball、Python 3.14、install stamp | `npm install` 两个固定包 |
| 已有证据 | P0-001/002/003 + AT-001..004，针对固定 0.23.0 与真实 MiniMax M3 | 5/5 运行时探针、9/9 适配器测试、真实 MiniMax M3、完整工具面运行 |
| 取消 | HTTP 上不可用（D-005） | 运行中的 turn 支持 `AbortSignal` |
| 恢复 | HTTP 上不可用 | transcript 在同一 agent 上继续 |
| 策略执行 | 沙箱加工具门控 | `beforeToolCall` 拦截，在工具执行前生效 |

**决策 D-042**（已接受）说明 Hermes 为什么还没有被退役：Hermes 被选为**远程**运行时，其适配器讲 HTTP 与 SSE；Pi（`earendil-works/pi`，MIT，`pi-agent-core` 1.5.0）是进程内 TypeScript harness，其原语与 DocuFlow 的适配器契约一一对应。2026-11-09 的实测显示 Pi 有原生的生命周期事件流（`agent_start`、`turn_start`、`message_start`、`message_update`、`message_end`、`turn_end`、`agent_end`）、能在工具执行前阻断的工具拦截、`AbortSignal` 取消，以及跨 prompt 存活的 transcript；Hermes 的 HTTP 表面原生不提供这些，正因如此 D-005 只能把取消与恢复声明为不可用能力。结论是：Pi 作为**主运行时候选**被评估，Hermes 保留为远程运行时参考；适配器契约是接缝，所以 DocuFlow 自身的策略、Code Engine、journal、evaluation、skills、API 与 ACP 端点都不受这一选择影响。**在 24 项清单针对选定运行时签收之前，任何东西都不退役。**

## 7. 下一步建议

1. **在一次性实验环境中解除 4 项安全阻塞**。用无生产凭据的一次性环境设置 `HERMES_SECURITY_LAB_CONFIRM=I_UNDERSTAND_THE_RISK`，跑 `npm run validate:security-agent`，把 P0-009 / P0-010 / P0-011 / P0-013 从 `blocked` 推进到有 agent 级证据的结论。这直接对应 D-020 的判定标准，也是当前唯一被环境硬性挡住的一组清单项。
2. **恢复 Docker daemon 并产出 `SEC-B*` 边界证据**。README 记录了缺失 daemon 记 `blocked`、被拒探针"成功"记 `failed` 的规则；只有拿到边界层证据，P0-011、P0-017 等项才具备签收的前提（D-008、D-009）。
3. **确定主运行时并据此签收清单**。按 D-042，Pi 是主运行时候选、Hermes 是远程参考；在两者之间明确取舍，然后按 D-005/D-006 补齐被钉住的能力（尤其取消与恢复——目前只有 Pi 侧有 `AbortSignal` 与 transcript 延续的证据）。README 明确 "Neither runtime is retired yet: the 24-item checklist has to be signed off against whichever is chosen."
4. **推进 17 项 `not_run` 中的高价值项**：P0-018（取消）、P0-021（会话恢复/重建）、P0-024（端到端可追溯）、P0-005/P0-006/P0-007（单步、多工具、多轮）、P0-008（子代理委派）。这些是清单中尚未产生任何证据的部分。
5. **补齐三项未实现能力并留下可执行证据**：多模型执行策略需要至少两个运行时并存的策略运行证据（依赖建议 3 的取舍结果）；Agent 能力评分需要把运行时的实测表现聚合成可比较的指标；Hermes 侧的工具面暴露则需要让 Hermes 路径也跑通 `read_file` -> `request_apply_diff` -> `evaluate_changes`，与 Pi 侧已有的完整工具面运行对齐。

---

> 证据来源：`docuflow-hermes-lab/reports/phase0.json`、`docuflow-hermes-lab/docs/DECISION_LOG.md`、`docuflow-hermes-lab/README.md`。文中所有状态数字与结论均取自这三份文件，未做推测。
