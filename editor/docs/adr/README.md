# 编辑器 ADR

记录 CodeWeaver 编辑器（`editor/`）的关键架构决策。

每条 ADR 说明：背景、决策、后果，以及**被否决的方案**——后者与决策本身同等
重要，因为未来的人会重新考虑同一个问题，那时他们需要知道当初为什么没选。

编辑器侧决策记录在此，与 Agent 运行时的
[`codeweaver-hermes-lab/docs/DECISION_LOG.md`](../../../codeweaver-hermes-lab/docs/DECISION_LOG.md)
分开：后者是 Phase 0 运行时决策日志，编号为 D-0xx；本目录使用独立编号。

## 索引

| 编号 | 主题 | 状态 |
|---|---|---|
| [ADR-0001](0001-rope-and-anchor.md) | 文本模型使用 Rope，并引入 Anchor | 已接受 |
| [ADR-0002](0002-wgpu-rendering.md) | 渲染层自建 wgpu，不依赖 GPUI；扩展宿主选型 | 已接受 |

## 待补

计划 `doc/EDITOR_DEVELOPMENT_PLAN.md` §13 列出的以下项目尚无 ADR：

- 第三方依赖清单与许可证 CI（`third_party/manifest.toml`）——阶段 0 未开始；
- Agent 补丁与编辑器事务的冲突检测（`agent-acp` crate 未建立）。
