# CodeWeaver 自研编辑器技术方案

> 状态：提案  
> 日期：2026-11-11  
> 目标：参考 Zed 的原生性能设计与 VS Code 的产品/扩展设计，开发拥有独立代码、品牌和演进路线的 CodeWeaver 编辑器。

## 1. 结论

不建议把 Zed 与 VS Code 的源码直接拼接成一个产品。推荐采用以下路线：

1. **编辑器核心自研**：数据模型、窗口布局、命令系统、工作区、插件边界和 AI 交互均由本项目定义。
2. **向 Zed 学性能架构**：Rust 原生进程、GPU 渲染、增量文本结构、异步任务、精细性能测量；只复用经过许可证审查的独立组件。
3. **向 VS Code 学生态架构**：稳定扩展 API、扩展宿主隔离、Contribution Points、命令与上下文系统、LSP/DAP/Tree-sitter 等标准协议。
4. **先完成 AI 原生最小闭环**：文件树、编辑、搜索、终端、Git、诊断、Agent/ACP、Diff 审批；之后再扩展插件兼容范围。
5. **长期不依赖上游 UI 结构**：Zed 和 VS Code 只作为研究对象与兼容性参考，产品能力由 CodeWeaver 自己的接口驱动。

一句话架构：**Zed 式原生内核 + VS Code 式扩展边界 + CodeWeaver 式可审计 Agent 工作流。**

## 2. 项目背景与现状

仓库中已经具备两条可利用的基础：

- `client/zed`：按 `client/PIN.json` 固定的 Zed 源码研究基线，目录不提交到本仓库。
- `codeweaver-hermes-lab`：已经实现 ACP 端点，可形成 `编辑器 -> ACP -> CodeWeaver -> Runtime` 链路。

当前 Zed 基线的价值是验证编辑器接入和研究原生架构，不应自然演变为长期维护的无边界 fork。自研编辑器需要建立自己的模块、接口、测试和发布体系。

## 3. 借鉴范围

### 3.1 从 Zed 借鉴什么

| 领域 | 借鉴原则 | 自研落点 |
|---|---|---|
| 原生性能 | UI、文本处理和后台任务避免跨进程频繁序列化 | Rust 核心，明确 UI 线程与后台任务边界 |
| GPU 渲染 | 文本、光标、选区、装饰层按场景增量绘制 | 独立 `render` 层，不让业务模块直接调用 GPU API |
| 文本模型 | 使用适合大文件和增量编辑的数据结构 | `Buffer`、`Snapshot`、`Anchor`、`Transaction` |
| 并发模型 | 耗时工作异步化，主线程只处理输入和提交画面 | 任务调度器、取消令牌、优先级和背压 |
| 模块化 | 按编辑器、工作区、语言、Git、终端等领域拆分 | 小型 Rust crates，禁止跨层反向依赖 |
| AI 交互 | Agent 与编辑器上下文紧密结合 | ACP 客户端、上下文快照、Diff 审批和审计日志 |

不直接照搬 Zed 的产品状态、云服务、协作协议、账号体系和 UI 结构。对任何 Zed 文件的复制或修改，必须先确认该文件所属 crate 的具体许可证；不能只根据仓库根目录作判断。

### 3.2 从 VS Code 借鉴什么

| 领域 | 借鉴原则 | 自研落点 |
|---|---|---|
| 扩展宿主 | 扩展与 UI/核心隔离，扩展崩溃不能拖垮编辑器 | 独立进程或受限 Wasm 宿主 |
| 声明式贡献 | 菜单、命令、快捷键、语言和主题通过清单声明 | `extension.json` + JSON Schema |
| 延迟激活 | 按语言、命令、工作区事件激活扩展 | Activation Events |
| 命令系统 | UI 入口最终映射到可测试的命令 | Command Registry + Context Keys |
| 标准协议 | 语言、调试、测试能力尽量走通用协议 | LSP、DAP、Tree-sitter、MCP、ACP |
| 远程模型 | UI 与工作区计算可位于不同机器 | 后期引入 Remote Workspace Host |

第一阶段只做“API 设计相似”和“精选扩展适配”，**不承诺完整 VS Code 扩展二进制兼容**。完整兼容会把内部架构长期锁定在 VS Code API、Node.js 和 Electron 的行为细节上。

## 4. 许可证与知识产权边界

本节是工程约束，不是法律意见；正式对外发布前需要完成法务审查。

### 4.1 三种使用方式

| 方式 | 是否允许 | 要求 |
|---|---|---|
| 阅读源码、研究行为、记录设计结论 | 是 | 设计文档记录来源，不复制实现文本 |
| 使用明确许可的独立依赖 | 是 | 固定版本，保留许可证和 NOTICE，生成 SBOM |
| 复制/修改上游源码并发布 | 有条件 | 遵守该文件和组件的许可证、源码公开及署名义务 |

### 4.2 具体注意事项

- VS Code 的开源仓库采用 MIT License，但微软发布的 Visual Studio Code 产品、品牌、遥测服务和 Marketplace 不能与开源仓库等同看待。
- 不假设可以在自研编辑器中直接使用 Visual Studio Marketplace。扩展来源优先选择 Open VSX、自建 registry 或取得作者授权的包。
- Zed 仓库包含不同许可证的组件。应用代码与 GPUI 等独立 crate 可能并非同一许可证，必须逐 crate、逐依赖确认。
- 若产品目标为闭源商业发行，不应直接基于强 copyleft 的编辑器应用代码开发，除非团队明确接受相应义务或取得商业授权。
- 图标、名称、欢迎页、默认主题、文案和快捷键集合也要独立设计，避免形成品牌或界面复制。

### 4.3 工程控制

建立 `third_party/manifest.toml`，至少记录：来源仓库、版本/commit、许可证、使用方式、修改文件、分发义务和负责人。CI 增加：

- Rust/Node/Wasm 依赖许可证扫描；
- SBOM 生成；
- 禁止未知许可证依赖进入发布构建；
- 上游源码摘录必须关联审查记录；
- 发布包自动携带第三方许可证清单。

## 5. 产品范围

### 5.1 MVP 必须具备

- 多窗口、多工作区、最近项目；
- 文件树、标签页、拆分编辑器；
- 文本编辑、撤销/重做、多光标、查找替换；
- Tree-sitter 语法高亮与结构导航；
- LSP：补全、诊断、跳转、引用、重命名、格式化；
- 全局搜索、命令面板、快捷键和设置；
- 集成终端与基础 Git 状态/Diff；
- Agent 面板、ACP 会话、上下文选择、流式输出；
- Agent 变更以 Diff 展示，用户审批后应用；
- 崩溃恢复、自动保存、会话恢复和基础遥测开关。

### 5.2 MVP 不做

- 实时多人协作；
- 完整 VS Code 扩展兼容；
- 浏览器版编辑器；
- 云端账号、同步和付费体系；
- Notebook、Debug、Remote SSH 的完整产品化；
- 像素级复制 Zed 或 VS Code 界面。

## 6. 总体架构

```text
┌──────────────────────── CodeWeaver Editor ────────────────────────┐
│ UI Shell / Workbench                                               │
│  Activity Bar · Panels · Tabs · Command Palette · Agent UI         │
├─────────────────────────────────────────────────────────────────────┤
│ Editor Services                                                     │
│  Buffer · View · Selection · Undo · Search · Decorations · Diff     │
├─────────────────────────────────────────────────────────────────────┤
│ Workspace Services                                                  │
│  File System · Index · Git · Terminal · Tasks · Settings · Secrets  │
├──────────────────────┬──────────────────────┬───────────────────────┤
│ Language Host         │ Extension Host       │ Agent Host            │
│ Tree-sitter / LSP     │ Wasm/Process + API   │ ACP + CodeWeaver API  │
├──────────────────────┴──────────────────────┴───────────────────────┤
│ Platform / Runtime                                                   │
│ Window · Input · GPU · Font · Clipboard · Async · IPC · Persistence │
└─────────────────────────────────────────────────────────────────────┘
```

关键规则：

- UI 只能依赖服务接口，不能直接操作文件、网络或子进程。
- `Buffer` 是文本真相源，视图只保存光标、折叠和滚动等展示状态。
- 扩展、语言服务器和 Agent 都是不可信外部执行体，必须经过能力授权。
- 所有 AI 修改都生成事务和可审计 Diff，不允许模型绕过编辑器直接覆盖工作区。
- 协议对象和内部对象分离，避免 ACP/LSP/DAP 升级污染核心模型。

## 7. 推荐代码结构

建议新建独立目录 `editor/`，不要在 `client/zed` 内直接开始长期开发：

```text
editor/
  Cargo.toml
  crates/
    app/                 # 进程入口与生命周期
    platform/            # 窗口、输入、剪贴板、系统集成
    render/              # GPU、字体、图元与帧调度
    ui/                  # 通用组件和布局
    text/                # Rope、位置、Anchor、增量变更
    editor-core/         # Buffer、Selection、Undo、Decoration
    workbench/           # 标签、面板、命令面板、状态栏
    workspace/           # 项目、文件树、搜索、设置
    language/            # Tree-sitter 与语言注册
    lsp-host/            # LSP 进程与协议适配
    terminal/            # PTY 与终端模型
    git/                 # Git 状态、Diff、提交
    extension-api/       # 稳定的扩展类型和版本
    extension-host/      # 扩展隔离、激活与权限
    agent-acp/           # ACP 客户端与会话映射
    agent-ui/            # 对话、上下文、审批、Diff
    storage/             # 设置、会话、恢复与缓存
    telemetry/           # 本地性能指标，可关闭
  schemas/
    extension.schema.json
    settings.schema.json
  tests/
    fixtures/
    integration/
    performance/
```

若早期决定直接使用 GPUI，应把它包在 `platform`/`render`/`ui` 三层之下，业务 crate 不直接依赖 GPUI 类型。这样可以控制其 pre-1.0 API 变化带来的影响，也保留未来更换渲染层的可能。

## 8. 核心技术设计

### 8.1 文本与编辑事务

`Buffer` 对外提供不可变 `Snapshot`，编辑通过 `Transaction` 提交：

```rust
pub struct Edit {
    pub range: AnchorRange,
    pub replacement: String,
}

pub struct Transaction {
    pub base_version: BufferVersion,
    pub edits: Vec<Edit>,
    pub source: EditSource,
}
```

要求：

- 位置不能长期保存为裸字节偏移，跨编辑引用使用 `Anchor`；
- 一次用户动作、格式化或 Agent patch 对应一个可撤销事务；
- 保存、LSP 同步、Git Diff 和 Agent 审批都订阅同一变更流；
- 大文件模式关闭高成本能力，但保留基本编辑和保存。

### 8.2 命令与上下文

所有交互入口注册为命令，例如 `editor.formatDocument`、`workspace.openFile`、`agent.applyDiff`。菜单和快捷键使用 Context Key 表达显示条件：

```json
{
  "command": "agent.applyDiff",
  "when": "agent.diffPending && workspace.trusted"
}
```

命令处理器返回结构化结果，支持日志、测试、重放和权限判定。

### 8.3 扩展模型

扩展清单使用声明式贡献：

```json
{
  "id": "publisher.language-demo",
  "engines": { "codeweaver": "^0.1" },
  "activationEvents": ["onLanguage:demo"],
  "permissions": ["workspace.read"],
  "contributes": {
    "languages": [{ "id": "demo", "extensions": [".demo"] }],
    "commands": [{ "command": "demo.run", "title": "Run Demo" }]
  }
}
```

推荐顺序：

1. 纯声明扩展：主题、图标、语法、片段；
2. Wasm 扩展：受限文件和网络能力；
3. 独立进程扩展：仅对确实需要 Node/原生依赖的扩展开放；
4. 精选 VS Code 扩展适配层：按 API 使用量逐个支持，不伪装成全兼容。

### 8.4 Agent 集成

沿用现有 `Zed -> ACP -> CodeWeaver -> Runtime` 的验证经验，自研端实现 `AgentClient` 接口：

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

安全约束：

- 默认只发送用户明确选择的上下文；
- 密钥、`.env` 和凭证文件默认排除；
- 工具权限按工作区和会话授予，拒绝时 fail closed；
- Agent 输出不能直接成为“任务成功”的证据，必须检查终态与评估结果；
- 每次应用修改记录执行 ID、基础版本、Diff、审批人和验证结果；
- Buffer 已变化时禁止静默套用旧 patch，必须重算或显示冲突。

## 9. 性能与稳定性指标

以下是 MVP 验收目标，需在固定硬件与固定样例仓库上测量：

| 指标 | 目标 |
|---|---:|
| 冷启动到可输入 | P50 < 800 ms，P95 < 1.5 s |
| 键盘输入到画面更新 | P95 < 16 ms，P99 < 32 ms |
| 100 MB 文本首次打开 | < 3 s，且 UI 不冻结 |
| 10 万文件工作区可交互 | < 2 s，索引后台继续 |
| 全局搜索首批结果 | < 500 ms |
| 扩展崩溃 | 不导致主进程退出 |
| Agent 流式事件积压 | 有界队列，可取消，不阻塞 UI |
| 异常退出恢复 | 已确认事务不丢失，未确认事务可识别 |

每个性能目标必须配套 benchmark，不能只靠人工体感。性能回归超过阈值时阻止合并。

## 10. 开发阶段

### 阶段 0：技术定型（2～3 周）

- 完成许可证清单和 clean-room 工作规则；
- 已决定采用自建 `wgpu` UI；继续以 ADR 固化渲染、事件和组件边界；
- 验证字体、IME、输入法、剪贴板、窗口和 Windows 打包；
- 完成 10 MB/100 MB 文件的文本模型与渲染 benchmark；
- 跑通自研壳到现有 ACP 服务的最短链路。

退出条件：Windows 中文输入可靠；连续输入无明显掉帧；ACP 可流式输出和取消。

### 阶段 1：可用编辑器内核（6～8 周）

- Buffer、视图、撤销、多光标、搜索替换；
- 工作区、文件树、标签与拆分；
- Tree-sitter、LSP 基础能力；
- 设置、快捷键、命令面板、会话恢复；
- 性能基线进入 CI。

退出条件：团队能够用它完成本仓库的小型日常修改。

### 阶段 2：AI 原生工作流（4～6 周）

- ACP 会话、上下文构建和流式事件；
- 工具审批、Diff 预览、冲突处理和事务应用；
- 执行时间线、用量和审计入口；
- Runtime 断开、编辑器重启和历史执行恢复。

退出条件：Agent 能在不直接写宿主文件的前提下完成“理解 -> 修改 -> 验证 -> 审批 -> 应用”闭环。

### 阶段 3：开发工具闭环（6～8 周）

- 终端、任务、Git、问题面板；
- 格式化、重命名、Code Action；
- DAP 最小调试能力；
- 安装包、升级、崩溃报告和发布通道。

### 阶段 4：扩展生态（持续）

- 公开版本化的扩展 API；
- 主题、语法、LSP 扩展市场；
- Wasm 扩展与权限模型；
- 根据真实用户需求增加 VS Code API 适配面。

## 11. 测试策略

- **单元测试**：Rope/Anchor、事务、撤销、命令条件、协议转换。
- **属性测试**：随机编辑后文本、Anchor 和撤销栈保持一致。
- **快照测试**：主题、布局、Diff 和诊断装饰。
- **集成测试**：真实 LSP、PTY、Git 仓库、ACP mock 与真实 CodeWeaver。
- **故障测试**：扩展崩溃、LSP 卡死、Runtime 断线、半帧协议、磁盘写满。
- **性能测试**：启动、输入延迟、大文件、十万文件索引、长时间 Agent 事件流。
- **发布门禁**：许可证、SBOM、签名、恶意扩展样例和升级/回滚。

测试结果要区分“mock 契约成立”和“真实运行时已验证”，不能用 mock 结果代替真实端到端证据。

## 12. 关键决策与风险

| 风险 | 影响 | 控制措施 |
|---|---|---|
| 直接 fork Zed 后改动过深 | 上游合并困难、许可证和产品边界模糊 | 自研目录；上游仅作研究基线 |
| 一开始追求 VS Code 全兼容 | 周期失控、架构被 Node API 锁定 | 先声明式扩展，再做精选适配 |
| 自研 GPU UI 低估 IME/字体复杂度 | Windows 中文输入与渲染不可用 | 阶段 0 先做平台验证和压力测试 |
| 扩展或 Agent 获得过大权限 | 工作区、凭证和主机受损 | 进程/Wasm 隔离、能力授权、审计 |
| AI patch 与用户编辑冲突 | 覆盖用户工作 | 基于版本的事务、三方 Diff、禁止静默应用 |
| 核心 API 过早稳定 | 被错误抽象长期拖累 | 内部 API 快速迭代，扩展 API 小步版本化 |

## 13. 近期可执行任务

1. 建立 `editor/` Rust workspace 和最小窗口程序。
2. 新增 ADR：`GPUI vs wgpu 自建 UI`、`Wasm vs 进程扩展宿主`。
3. 建立第三方依赖清单、SBOM 和许可证 CI。
4. 实现 `Buffer + Snapshot + Transaction + Anchor` 原型及随机编辑测试。
5. 实现最小 GPU 文本视图，优先验证中文 IME、emoji、组合字符和高 DPI。
6. 抽取现有 ACP 契约测试，作为自研 `agent-acp` crate 的兼容测试。
7. 用本仓库作为 dogfood 项目，建立启动、输入、大文件和索引 benchmark。

## 14. 决策门

进入完整开发前，负责人需要明确确认：

- 产品是否允许强 copyleft 代码进入发布物；
- UI 技术选择 GPUI 还是自建 `wgpu`；
- MVP 只支持 Windows，还是同步覆盖 macOS/Linux；
- 扩展目标是自有 API，还是必须兼容一部分 VS Code 扩展；
- 第一个可交付版本是“AI 编辑器”还是“通用编辑器 + AI”。

在这些问题没有明确前，可以完成阶段 0 原型，但不应大规模复制或迁移上游编辑器代码。

## 15. 参考资料

- Zed 源码与 crate 划分：<https://github.com/zed-industries/zed>
- GPUI（仍处于 pre-1.0）：<https://github.com/zed-industries/zed/tree/main/crates/gpui>
- VS Code 源码组织：<https://github.com/microsoft/vscode/wiki/Source-Code-Organization>
- VS Code Extension Host：<https://code.visualstudio.com/api/advanced-topics/extension-host>
- VS Code 开源仓库许可证：<https://github.com/microsoft/vscode/blob/main/LICENSE.txt>
- 本仓库 Zed 基线说明：`client/README.md`
- 本仓库 ACP 接入说明：`codeweaver-hermes-lab/docs/ZED.md`

