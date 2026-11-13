# AGENTS.md

CodeWeaver：一个自研原生编辑器（Rust + wgpu/winit）、一个 Agent 运行时（Node/TypeScript，ACP 端点）与一个 Tauri 控制台组成的 monorepo。本文件是所有 agent 在本仓库工作时的约定。

## 仓库结构

- `editor/` — 自研原生编辑器，Rust workspace（13 个 crate，产品版本 `0.6.0`）
- `codeweaver-hermes-lab/` — Agent 运行时与 ACP 端点（TypeScript/Node，`0.6.0`）
- `codeweaver-workspace/` — 控制台前端 + Tauri 外壳（React/Vite/Tauri，`0.6.0`）
- `client/` — 上游研究基线（Zed，只读，不提交）
- `doc/` — 版本文档与设计文档（进度文档、ADR、方案）
- `scripts/` — 本地开发栈（`dev.py`、`check-runtime.py`）
- `editor/tools/` — 窗口级验收脚本（PowerShell）

## 常用命令

所有 cargo 命令都从仓库根执行，必须带 `--manifest-path`：

```powershell
# 编辑器
cargo build --manifest-path D:\github\DocuFlowAI\editor\Cargo.toml -p codeweaver-editor
cargo test  --manifest-path D:\github\DocuFlowAI\editor\Cargo.toml --workspace
cargo clippy --manifest-path D:\github\DocuFlowAI\editor\Cargo.toml --workspace --all-targets

# 运行时（ACP 端点）
cd codeweaver-hermes-lab; npm test

# 控制台
cd codeweaver-workspace; npm run build

# 本地全栈（后端 + 控制台）
python scripts/dev.py --runtime pi
```

窗口级验收脚本（`editor/tools/*.ps1`）**必须串行跑**：每个脚本开头都会 `Stop-Process -Name codeweaver-editor`，并行会互相杀掉对方的窗口。

**开发过程中不要跑窗口级脚本**。它们会杀掉编辑器进程、抢走键盘焦点、往系统里注入按键，会打断人正在做的事。只在统一回归的那一轮跑它们；开发阶段只跑 `cargo test` 与 `npm test` 这类不影响桌面的检查。

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File D:\github\DocuFlowAI\editor\tools\format-check.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File D:\github\DocuFlowAI\editor\tools\rename-check.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File D:\github\DocuFlowAI\editor\tools\task-check.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File D:\github\DocuFlowAI\editor\tools\settings-check.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File D:\github\DocuFlowAI\editor\tools\agent-check.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File D:\github\DocuFlowAI\editor\tools\workbench-check.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File D:\github\DocuFlowAI\editor\tools\window-check.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File D:\github\DocuFlowAI\editor\tools\acp-contract-check.ps1
```

## 开发流程

1. **先写版本文档再实现**：新版本先建 `doc/PROJECT_PROGRESS_V0.X.0.md`（写清本期做什么、每项的验收标准），再动代码。
2. **产品版本必须与主进度文档号一致**：`0.6.0` ↔ `V0.6.0`。改动版本时 `editor/Cargo.toml`、`editor/Cargo.lock`、两个 Node 子项目、`src-tauri/tauri.conf.json`、ACP 握手版本要一起改。
3. **「完成」= 实现 + 自动化测试 + 可追溯验收结果**。三项缺一不可。
4. **Mock 契约通过与真实运行时通过分开记录**，不互相替代。
5. **未完成项只能标「计划 / 进行中 / 阻塞」**，界面占位不算完成。
6. **开发优先，测试收口**：实现过程中不要每步都跑全量回归；把回归集中到一轮里跑完，验收数字回填进版本文档 §6。开发阶段只跑不影响桌面的检查（`cargo test`、`npm test`），窗口级脚本留到统一回归那一轮。

## 代码风格

- Rust edition 2024；注释与文档注释一律英文，UI 文案用中文。
- **不要跑 `cargo fmt`**：仓库当前未按 rustfmt 格式化（`--all --check` 有 47 处差异），格式化会淹没 diff。手工保持缩进。
- 分层单向依赖：`app`（组合）→ `platform`（窗口/输入/IME）→ `render`（GPU），`ui`/`text`/`editor-core` 只通过稳定接口用下层能力；业务状态不进渲染层。
- 一次语义变更 = **一次事务**（如格式化、Agent 批量改动），保证撤销一次整体回退。
- 失败要**明说原因**：没有语言服务器、服务器断开、状态冲突等，都要在状态栏给出原因，不静默什么都不发生。

## 环境约束（Windows / PowerShell 5.1）

- 仓库目录带 **Low 完整性标签** → 在仓库里构建出的二进制以低完整性运行，**起不了 Medium 完整性的 `node.exe`（os error 5）**。验收脚本因此把编辑器复制到 `%LOCALAPPDATA%\CodeWeaver\check` 再运行。
- 本机 `rust-analyzer` 是 rustup 代理且组件未装在默认工具链，代理报「Unknown binary」；可用二进制在 `%USERPROFILE%\.rustup\toolchains\1.98.1-x86_64-pc-windows-msvc\bin\rust-analyzer.exe`，用编辑器的 `--lsp <program>` 点名它。
- 验收用的工程必须自带空 `[workspace]` 表，并放在工作区之外，否则 `cargo locate-project` 会报「believes it's in a workspace when it's not」。
- PowerShell 5.1：不支持 `&&` / `||`，不要用 `/dev/null`、heredoc、`sed -i`；写文件用 `[System.IO.File]::WriteAllText($p, $s, [Text.UTF8Encoding]::new($false))` 防 BOM，临时 `.ps1` 含中文必须存成 UTF-8 **BOM**（否则按 GBK 解析）。

## Git

- 默认分支 `main`。**不要 `git add` / `git commit` / `git stash`** —— 本仓库有并发自动提交机制，工作区脏是正常的。

## 安全

- 密钥不落仓库、不进命令行：模型 key 从 Hermes profile 的 `.env` 读取。
- 第三方代码与许可证：`client/` 下只做研究，不复制实现文本；引入依赖要固定版本并更新 `third_party/manifest.toml` 与许可证清单。
- 危险操作要能被审批拦住：涉及删除、推送、发布的动作走策略门，指名审批到具体命令。
