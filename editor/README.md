# CodeWeaver Editor

CodeWeaver 的自研原生编辑器。渲染基于 `wgpu`，窗口事件基于 `winit`；项目会借鉴
GPUI 的单向状态更新、帧调度和组件化思想，但不依赖 GPUI，也不复制其实现。

## 当前状态

阶段 0 的原型已经包含：

- 原生窗口生命周期；
- wgpu adapter/device/surface 初始化；
- resize、重绘和 surface 异常恢复；
- 不可变 `FrameScene`，矩形实例批渲染与逐图元裁剪；
- cosmic-text shaping 与 glyphon GPU 字形图集；
- Windows IME `preedit` / `commit` 输入状态；
- UTF-8 安全的 Buffer、不可变 Snapshot 与版本化 Transaction；
- Selection、Unicode 光标移动、撤销和重做；
- `app -> platform -> render` 的单向依赖边界。

当前窗口展示一个可输入中英文的编辑器内核验证场景，支持左右移动、Backspace、Enter、
`Ctrl+Z` 和 `Ctrl+Y`。下一步需要把 String Buffer 升级为适合大文件的 Rope，并实现 UI
布局、命中测试、鼠标选区和可见光标。

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
- `text`：Buffer、Snapshot、UTF-8 offset、Transaction 与逆事务。
- `editor-core`：Selection、输入命令与 undo/redo 历史。
- 未来的 `ui`、`text`、`editor-core` 只能通过稳定接口使用下层能力。

详细路线见 [`../doc/EDITOR_DEVELOPMENT_PLAN.md`](../doc/EDITOR_DEVELOPMENT_PLAN.md)。

