# 编辑器架构约束

## 技术选择

编辑器使用 `wgpu` 自建 GPU 渲染层。GPUI 只作为研究对象，主要借鉴以下思想：

- UI 状态在单一拥有者上更新；
- 后台任务完成后向主线程提交结果；
- 每一帧根据无效区域和状态变化调度，而不是无条件持续重绘；
- 组件通过上下文访问服务，业务对象不直接持有平台资源；
- 输入、布局、绘制分阶段执行，便于性能测量。

不复制 GPUI 的类型体系、宏、组件实现或调度器代码。

## 依赖方向

```text
app -> workbench/ui/editor-core（待建）
app -> platform -> render
ui  -> render-api（待从 render 中抽出）
editor-core -> text
agent-acp -> editor-core transaction（待建）
```

实际落地为：

```text
platform -> agent-ui -> agent-acp
platform -> editor-core -> text
platform -> syntax -> text
platform -> lsp -> text
platform -> workspace
platform -> render
platform -> ui -> text
```

`agent-acp` 是纯传输层，只依赖 serde；`agent-ui` 是编辑器的会话模型。阶段 2
的 Diff 审批会让 `agent-ui` 依赖 `editor-core` 的事务接口，届时该边才建立。

`workspace` 是文件系统的唯一入口：UI 不直接读写文件，只调用
`Workspace::open` / `Workspace::save`。保存前会检查文件在磁盘上是否被改过，
被改过就拒绝写入。

`ui` 是正文区域的布局与命中测试，只依赖 `text`，**不依赖 `render`**：它需要
知道字符落在哪，但字体与整形只有渲染器知道，于是 `ui` 声明 `TextMeasurer`
trait，由 `platform` 用 `Renderer::measure_text` 实现并注入。换行只由 `ui` 做
一次，渲染器不再二次折行。详见 [ADR-0005](adr/0005-ui-layer.md)。

`syntax` 是语法高亮：把缓冲区变成"哪些字节是什么种类"的列表。它只依赖
`text`（要字节偏移和改动记录），**不依赖 `editor-core`（不需要选区模型），
也不依赖 `render`（颜色是 platform 的事）**。详见
[ADR-0009](adr/0009-syntax-highlighting.md)。

`lsp` 是语言服务器客户端：起子进程、收发 LSP 消息、把协议里的位置（行 + UTF-16
列）换算成字节偏移。它只依赖 `text` 和 serde，**不依赖 `render`、不依赖
`editor-core`**；颜色和界面都是 platform 的事。进程本身由 `platform` 启动
（UI 不启动进程），诊断和补全在 platform 里画。详见
[ADR-0010](adr/0010-language-server.md)。

禁止事项：

- `editor-core` 依赖 `winit` 或 `wgpu`；
- UI 组件直接读写文件或启动进程；
- Renderer 持有 Buffer、Workspace 或 Agent 会话；
- 扩展和 Agent 绕过事务接口修改工作区；
- 为了短期方便将上游 GPUI 类型暴露为公共 API。

## 文本模型

`text` crate 是文本的真相源，详见 [ADR-0001](adr/0001-rope-and-anchor.md)。

- `Buffer` 内部是 `ropey::Rope`，对外以字节偏移为坐标系；
- `BufferSnapshot` 是一次不可变视图，持有文本拷贝，编辑器后续的修改不影响它；
- `Transaction` 带 `base_version`，过期版本写入被拒绝；
- `Anchor` 是跨编辑稳定的位置引用，配合 `Bias` 表达插入方向。

写入分两条路径：

| 路径 | 用途 | 版本检查 |
|---|---|---|
| `Buffer::apply` | 外部写入：LSP、Agent patch、格式化 | 强制，`base_version` 不匹配即拒绝 |
| `Buffer::replay` | 撤销/重做历史回放 | 跳过，因为逆事务的坐标空间属于编辑之后的状态 |

`Buffer::text()` 会物化整份文本，大文件下不能每帧调用；只读局部用
`Buffer::slice`，只读版本号用 `Buffer::version()`。

## Agent 接入

编辑器是 ACP 客户端，通过 stdio 与 CodeWeaver 的 agent endpoint 说 JSON-RPC 2.0，
详见 [ADR-0003](adr/0003-acp-integration.md)。

- `agent-acp`：帧格式（写 ndjson、读 ndjson 与 Content-Length）、stdio 传输、
  请求分发；对外是 `SessionEvent`。
- `agent-ui`：编辑器自己的面板模型，把 `SessionEvent` 变成可绘制的内容。
- 事件循环不阻塞：每帧 `pump()` 一次收集 Agent 输出。
- 权限必须由人回答；端点关闭、超时、解析失败一律按拒绝处理。
- 端点路径由环境变量给出，不在代码里猜。

## 下一层接口

渲染层不接收业务组件，而接收一帧不可变 `FrameScene`：

```text
FrameScene
  clips[]
  quads[]
  shadows[]
  glyph_runs[]
  images[]（待实现）
```

UI 层负责状态、布局、命中测试、焦点和无障碍树；渲染层只负责批处理、图集、GPU
资源缓存和提交。当前矩形通过实例缓冲批量提交，裁剪由每个图元携带；文字由
cosmic-text shaping，并通过 glyphon 的 GPU atlas 缓存字形。该边界使 UI 测试不依赖
GPU，也使渲染 benchmark 可以使用合成场景。

## 输入法边界

`platform::TextInputState` 将 winit 的 IME 事件归一化为两类状态：

- `preedit`：正在组合的文本，仅用于视图展示，不进入正式 Buffer；
- `commit`：输入法确认的文本，通过一个编辑事务写入 Buffer。

当前 commit 已通过 `editor-core::Editor::insert` 生成版本化事务；preedit 不进入 Buffer，
只在生成 `FrameScene` 时插入光标位置。撤销不会留下输入法组合过程，只撤销最终提交。

IME 候选窗位置应由获得文本焦点的 UI 组件按光标位置更新。当前原型已经开启原生
IME 并处理 enabled、disabled、preedit 和 commit，候选窗定位会在编辑器视图实现时接入。

