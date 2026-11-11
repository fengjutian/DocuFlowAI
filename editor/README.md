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
- 基于 `ropey` 的大文件文本模型：不可变 Snapshot、版本化 Transaction、逆事务；
- `Anchor` 稳定位置引用（带 bias 语义），跨编辑不失效；
- Selection、Unicode 光标移动、撤销和重做；
- ACP 客户端：JSON-RPC 2.0 over stdio，接入 CodeWeaver 的 agent endpoint；
- Agent 面板：流式回复、工具调用、权限审批、取消；
- 文件打开与保存：`workspace` 服务层，保存时拒绝覆盖外部改动；
- `app -> platform -> render` 的单向依赖边界。

当前窗口展示一个可输入中英文的编辑器内核验证场景，支持左右移动、Backspace、Enter、
`Ctrl+Z` 和 `Ctrl+Y`；右侧是 Agent 面板。下一步需要实现 UI 布局、命中测试、鼠标选区和
可见光标。

## 打开文件

```powershell
cargo run -- path/to/file.rs
```

无参数时启动在空文档上。`Ctrl+S` 保存；保存前会检查文件是否在磁盘上被改过，
被改过就**拒绝写入**并提示，而不是覆盖别人的改动。

## 与 Agent 运行时联调

编辑器通过 ACP（JSON-RPC 2.0 over stdio）接入 CodeWeaver 的 agent endpoint。
端点由环境变量指定，编辑器不猜路径：

```powershell
$env:CODEWEAVER_ACP_COMMAND = "node"
$env:CODEWEAVER_ACP_ARGS = "D:\github\DocuFlowAI\codeweaver-hermes-lab\dist\runners\acp-server.js"
$env:CODEWEAVER_ACP_CWD = "D:\github\DocuFlowAI"
cargo run
```

会话在第一次发问时创建。未配置端点时，面板会明确说明原因。

| 按键 | 作用 |
|---|---|
| `Ctrl+M` | 把当前缓冲区内容作为提示发给 Agent |
| `Ctrl+S` | 保存当前文件 |
| `Ctrl+.` | 取消当前轮次 |
| `Ctrl+Alt+A` | 允许待审批的工具调用 |
| `Ctrl+Alt+N` | 拒绝待审批的工具调用 |

会话的工作目录是**当前文件所在目录**，所以 Agent 拿到的是用户正在看的那个文件。

端到端自检（真实端点 + mock 运行时）：

```powershell
cargo run -p codeweaver-platform --example agent_end_to_end
cargo run -p codeweaver-platform --example file_agent_check
```

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

性能基准：

```powershell
cargo bench -p codeweaver-text
cargo run -p codeweaver-text --example text_report --release
```

`text_report` 按阶段 0 的退出条件（100 MB 打开 < 3 s、单次编辑 < 16 ms）输出
对照表，超过目标时以非零码退出，可直接用作 CI 门禁。

## 分层约束

- `app`：组合应用和功能服务，不直接使用 wgpu。
- `platform`：窗口、输入、IME 和事件循环，不暴露 winit 类型给未来业务模块。
- `render`：GPU surface、device、queue、帧与图元渲染，不处理业务状态。
- `text`：Rope 文本模型、Snapshot、UTF-8 offset、Transaction 与逆事务、Anchor。
- `editor-core`：Selection、输入命令与 undo/redo 历史。
- 未来的 `ui`、`text`、`editor-core` 只能通过稳定接口使用下层能力。

详细路线见 [`../doc/EDITOR_DEVELOPMENT_PLAN.md`](../doc/EDITOR_DEVELOPMENT_PLAN.md)，
文本模型的设计决策见 [`docs/adr/0001-rope-and-anchor.md`](docs/adr/0001-rope-and-anchor.md)。

