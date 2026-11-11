# CodeWeaver Editor

CodeWeaver 的自研原生编辑器。渲染基于 `wgpu`，窗口事件基于 `winit`；项目会借鉴
GPUI 的单向状态更新、帧调度和组件化思想，但不依赖 GPUI，也不复制其实现。

## 当前状态

阶段 0 的最小原型已经包含：

- 原生窗口生命周期；
- wgpu adapter/device/surface 初始化；
- resize、重绘和 surface 异常恢复；
- `app -> platform -> render` 的单向依赖边界。

目前只绘制背景色，尚未包含文本、控件和编辑器数据模型。

## 运行

```powershell
cd editor
cargo run
```

验证：

```powershell
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## 分层约束

- `app`：组合应用和功能服务，不直接使用 wgpu。
- `platform`：窗口、输入、IME 和事件循环，不暴露 winit 类型给未来业务模块。
- `render`：GPU surface、device、queue、帧与图元渲染，不处理业务状态。
- 未来的 `ui`、`text`、`editor-core` 只能通过稳定接口使用下层能力。

详细路线见 [`../doc/EDITOR_DEVELOPMENT_PLAN.md`](../doc/EDITOR_DEVELOPMENT_PLAN.md)。

