# ADR-0002：渲染层自建 wgpu，不依赖 GPUI

- 状态：已接受
- 日期：2026-11-11
- 适用范围：`editor/crates/render`、`editor/crates/platform`、`editor/crates/ui`（待建）

## 背景

计划文档 `doc/EDITOR_DEVELOPMENT_PLAN.md` §7 与 §13 要求明确「GPUI vs 自建
wgpu」的选择，并把它固化成 ADR。§3.1 进一步规定：借鉴 Zed 的原生性能架构，
但「不直接照搬 Zed 的产品状态、UI 结构」，且「不复制 GPUI 的类型体系、宏、
组件实现或调度器代码」。

阶段 0 开始时这个选择其实已经落地在代码里（`editor/crates/render` 直接使用
`wgpu`），但只写在 `editor/docs/ARCHITECTURE.md` 的散文里，没有可追溯的决策
记录。本 ADR 把已经生效的选择补记为正式决定，并把边界写死。

## 决策

### 1. 渲染层直接使用 `wgpu`

`render` crate 拥有 wgpu 的 `Instance`、`Device`、`Queue`、`Surface` 和全部
GPU 资源，对外暴露的是一个不可变的 `FrameScene`：

```text
FrameScene
  clips[]
  quads[]
  shadows[]
  glyph_runs[]
  images[]（待实现）
```

业务层不能直接调用 GPU API。矩形走实例缓冲批量提交，裁剪由每个图元携带；
文字由 cosmic-text shaping，经 glyphon 的 GPU atlas 缓存字形。

### 2. 借鉴 GPUI 的思想，不复制其实现

保留四条思想：

- UI 状态在单一拥有者上更新；
- 后台任务完成后向主线程提交结果；
- 每一帧根据无效区域和状态变化调度，而不是无条件持续重绘；
- 组件通过上下文访问服务，业务对象不直接持有平台资源。

不引入 GPUI 的类型、宏或组件，不把 GPUI 类型暴露为公共 API。

### 3. 依赖方向由分层强制

```text
app -> platform -> render
ui  -> render-api（待从 render 中抽出）
```

`editor-core` 不得依赖 `winit` 或 `wgpu`；`platform` 不向业务模块暴露 winit
类型；Renderer 不持有 Buffer、Workspace 或 Agent 会话。

### 4. 扩展宿主：Wasm 优先，进程宿主按需开放

（计划 §13 要求的第二项 ADR 决策）

- 第一阶段只做纯声明式扩展：主题、图标、语法、片段；
- 需要受限文件/网络能力时用 Wasm 扩展宿主；
- 只有确实需要 Node 或原生依赖的扩展才开放独立进程宿主；
- 不承诺 VS Code 扩展二进制兼容。扩展清单用自有 `extension.json` + JSON
  Schema 声明，`engines.codeweaver` 版本化。

## 后果

- 渲染层的每一处行为都由本仓库拥有，可以按 CodeWeaver 的帧调度需要改造，
  不受 GPUI pre-1.0 API 变动影响。
- 代价是 IME、字体、DPI 这些平台细节要自己踩坑。阶段 0 已经用 Windows 中文
  输入验证过 IME 通路，但高 DPI、emoji、组合字符仍未逐项验证。
- `render-api` 从 `render` 中抽出来是待办：UI 层目前还没有独立 crate，抽取
  时机应在 `ui` crate 落地时决定，而不是现在提前拆。

## 被否决的方案

- **直接 fork Zed**：上游合并困难，许可证和产品边界模糊。计划 §12 已把它列为
  首要风险。
- **用 GPUI 加速开发**：能省下 IME、字体、调度器的实现时间，但会把产品架构
  绑定在 GPUI 的类型体系上。GPUI 仍是 pre-1.0，其 API 变动会直接波及本项目
  的公共接口。
- **用 wgpu + 自建框架但同时保留 GPUI 作为可选后端**：两套渲染后端意味着两套
  调试路径，且在阶段 0 无实际收益。
