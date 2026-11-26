# CodeWeaver 项目进度与开发计划 V0.8.0

文档号：`V0.8.0`  
产品版本：`0.8.0`  
状态：进行中（DF-0801 缩略图滑块与 DF-0802 面包屑已实现，窗口级验收待跑；DF-0803 起计划）  
更新日期：2026-11-25

---

## 1. 本期结论

V0.7.0 把编辑器的**外壳**换成了 VS Code 的样子：命令注册表、命令面板、活动栏与侧边栏分区、行号槽、底部状态栏、键位可改、开发模式——自动化测试 654 项全绿（细节见 [V0.7.0 §6.2](./PROJECT_PROGRESS_V0.7.0.md)）。

**窗口像了，用起来还不像**。差距在第二层：**编辑体验**——文本区本身还缺 VS Code 的那些零件：

1. **没有缩略图滑块（minimap）**：右边那条窄条告诉人「整个文件长什么样、现在看到的是哪一段」，是 VS Code 最显眼的特征——**第一轮补上了**。
2. **没有面包屑**：正文上面那一行「文件 › 目录 › 符号」没有，所在位置只能靠路径猜——**第二轮补上了路径那一段**（「符号」那一段要问语言服务器要符号表，还没做）。
3. **文件树里没有文件图标**：一列名字，分不出是代码、配置还是图片。
4. **不能缩放**：`Ctrl+滚轮` 改不了字号，字体在代码里写死（`editor_view::FONT_SIZE`）。
5. **不能分屏**：一个窗口只能看一个文件。
6. **多光标缺两块**：列选择（`Ctrl+Shift+Alt+↓`）与多光标加词。
7. **没有工作区文件与会话恢复**：工程要一个一个开文件，标签页下次启动就没了。

本期做前四件（DF-0801 ~ DF-0804），后三件留在后面。

**第一轮做完的是 DF-0801 缩略图滑块**：正文右边多了一条 90 px 宽的窄条，画的是**整个文件**（不是屏幕上那几行），屏幕上看到的那一段画得更亮并带上下边线（滑块），点它正文跳到那一段——光标不动（地图是「找地方」用的，不是「挪光标」用的）。它是一个**密度图**：每一像素行取落进这一行的最宽行画一条，条的宽度正比于那行的字符数；VS Code 画的是字母，我们画的是形状——整文件逐字渲染的代价太高，而找位置要的是形状。地图按 **buffer 版本**缓存：数每行有多少字符不便宜，改一个字才重算一次，每帧只画条子。中文按**字符**算不算字节（一个汉字是三个字节、一个字）。短文件**不拉伸**——地图只占条子顶部，下面是死区，点它不跳（跟 VS Code 一个路子）；长文件行高不到 1 px 时才挤满整条。

新组件 `crates/platform/src/minimap.rs`（9 个单测），不认识 `NativeApplication`；窗口布局照旧只有一份（`layout::WindowLayout` 里多一块 `minimap`，正文相应窄 102 px）。

**第二轮做完的是 DF-0802 面包屑**：正文上面多一行，把当前文件的路径一段一段写出来——`项目 › 目录 › 文件名`。路径是**磁盘上真实的路径**，用**工程根**截短（文件不在任何工程里就整条写出来）；每一段都记着自己代表哪个文件夹，所以**点哪一段就跳到哪一段**：点一个目录，侧边栏切到资源管理器、文件树换根到那个目录（状态栏明说现在看的是哪个目录），点文件本身则说明「已经在这个文件里」。行太窄时前面的段被丢掉，用 `…` 顶着——**文件名永远留着**。行是**按需算一次**的：工程根是往磁盘上翻找出来的，每帧翻一遍是白费力气，所以路径只在「屏幕上的文件换了」的时候重算。新组件 `crates/platform/src/breadcrumbs.rs`（9 个单测），不认识 `NativeApplication`；窗口布局照旧只有一份（`layout::WindowLayout` 里多一块 `breadcrumbs`，正文、行号槽、缩略图整体下移 24 px）。

## 2. 当前完成度

| 模块 | 已完成 | 仍缺失 | 判断 |
|---|---|---|---|
| 文本内核 | Rope/Anchor/事务、撤销重做、多选、多光标、查找替换 | 结构化编辑（折叠/括号匹配） | 可用 |
| 视图与输入 | 布局、命中测试、拖拽选区、滚动、可见区定位、IME | 分屏视图、缩放 | 可用 |
| 工作区 | 文件读写、指纹、外部变更检测、文件树、全局搜索与替换、搜索规则 | 项目索引 | 可用 |
| 标签与会话 | 标签模型、多标签、脏状态、关闭确认 | 会话恢复、最近项目 | 可用 |
| 语言能力 | 语法高亮（后台）、LSP 补全与诊断、格式化、重命名、代码操作 | onType 格式化、重命名预备 | 可用 |
| 调试 | DAP 客户端、断点、单步、变量、条件断点、表达式求值、调用栈、跳源码 | 命中次数、多线程、调试控制台、**真适配器未验** | 最小可用 |
| Agent | ACP 客户端、面板、审批、流式输出、上下文选择、Diff 按块批准、轨迹 | 语义冲突判断 | 可用 |
| 开发工具 | 任务运行器、交互式终端、Git 面板、全局搜索与替换 | 真伪终端（PTY）、终端多标签、分支与远端 | 基础可用 |
| 外壳与命令（V0.7.0） | 命令注册表、命令面板、活动栏与侧边栏分区、行号槽、底部状态栏、键位可改、开发模式 | 窗口级验收（统一回归那轮） | 可用 |
| **编辑体验**（本期） | 缩略图滑块（DF-0801）、面包屑（DF-0802）已实现 | 文件图标、缩放、分屏、多光标补齐、工作区文件 | 进行中 |

## 3. V0.8.0 开发项

### DF-0801 缩略图滑块（minimap）（已实现，窗口级验收待跑）

正文右边一条窄条，画出**整个文件的样子**，并标出现在看到的是哪一段。

验收标准：

- **正文右边有一条窄条**（90 px 宽），在行号槽与 Agent 面板之间；正文相应窄一点。
- **它画的是整个文件**，不是只有屏幕上那几行——它的用处就是「在很大的文件里找一个地方」。
- **它标出屏幕上看到的是哪一段**：现在看的那一段画得更亮（滑块），跟着滚动走。
- **点它能跳**：点地图上的某处，正文跳到那一段（跟 VS Code 一个路子）。
- **大文件不卡**：地图是**按文档版本缓存**的（改一个字才重算一次），每一帧只画条子，不重新数整个文件。
- **组件拆分**：地图是 `crates/platform/src/minimap.rs`，不认识 `NativeApplication`；窗口布局照旧只有一份（`layout::WindowLayout` 里多一块 `minimap`）。
- 单元测试覆盖：地图的行宽、可见段的位置、布局里地图与正文不重叠；窗口级验收脚本证明：窄条真的画出来了、滑块跟着滚动走。

实现落点：

- `crates/platform/src/minimap.rs`：`Minimap::of(buffer)` 按行数字符宽度建图（用 `buffer.slice_from` 数 UTF-8 首字节，不复制文本）、`geometry()`（短文件不拉伸、行高 ≤2 px；长文件挤进整条）、`line_at()`（点地图换算行号，短文件下方空白区不算）、`draw()`（每像素行取落入其中的最宽行，屏幕上那段更亮 + 上下边线）。9 个单测。
- `crates/platform/src/layout.rs`：`WindowLayout` 增 `minimap: Rect`，正文右边让出 90+12 px。
- `crates/platform/src/editor_view.rs`：`EditorView` 增 `minimap` 缓存（按 buffer version 重建）+ `minimap()`。
- `crates/platform/src/lib.rs`：`build_scene` 画地图；`press_minimap()` 点地图跳行（光标不动）；两个集成测试（点地图跳到那一段、文本变了地图重建）。

### DF-0802 面包屑（已实现，窗口级验收待跑）

正文上面一行：`文件 › 目录 › 名字`。点哪一段跳到哪一段。

验收标准：

- **正文上面有一行路径**：在标签条与正文之间，宽度跟标签条一样；正文、行号槽、缩略图整体下移 24 px。
- **路径是真的**：写的是磁盘上那个文件的路径，用工程根截短；文件不在任何工程里就整条写出来（`盘符 › 目录 › 文件`）。
- **每一段都代表一个地方**：点一个目录 → 侧边栏切到资源管理器、文件树换根到那个目录，状态栏明说现在看的是哪个目录；点文件本身 → 状态栏说明「已经在这个文件里」。
- **行窄了不乱**：前面的段被丢掉，`…` 顶着它们，文件名永远留着。
- **不白费力气**：工程根是往磁盘上翻找出来的，路径只在屏幕上的文件换了的时候重算一遍，每帧只画不重算。
- **组件拆分**：路径是 `crates/platform/src/breadcrumbs.rs`，不认识 `NativeApplication`；窗口布局照旧只有一份（`layout::WindowLayout` 里多一块 `breadcrumbs`）。
- 单元测试覆盖：路径分段、每段代表的文件夹、行放不下时丢前面的段、点中哪一段；窗口级验收脚本证明：那一行画出来了、文件名与目录名写在里面、点目录之后文件树换到了那个目录。

实现落点：

- `crates/platform/src/breadcrumbs.rs`：`of(path, root)` 把路径切成一段段（工程内的相对路径 / 工程外的整条路径）、`strip()` 排版并在放不下时丢前面的段、`hit()` 判定点中哪一段、`draw()` 画那一行与段之间的 `›`。9 个单测。
- `crates/platform/src/layout.rs`：`WindowLayout` 增 `breadcrumbs: Rect`；正文/行号槽/缩略图整体下移 24 px；三个布局单测。
- `crates/platform/src/lib.rs`：`NativeApplication` 缓存 `crumbs`（换文件或换标签时重算）、`build_scene` 画那一行、`press_breadcrumbs()` 处理点击（纯逻辑拆成 `go_to_the_place_of_the_path()`，能被单测钉住）；四个集成测试（路径写对了、点目录文件树换根、点文件说明已经打开、换标签后路径重算）。

### DF-0803 文件图标（计划）

文件树里每个文件前面一个图标（按后缀分颜色/形状）：代码、配置、文本、图片各不一样。

### DF-0804 缩放（计划）

`Ctrl+滚轮` 改字号（80%–300%），字号存进设置。这一项要动 `editor_view::FONT_SIZE`——它现在是常量，不是状态。

## 4. 未开发功能拆分

### 仍留在 V0.8.0 后面的

- 多光标编辑的补齐：列选择（`Ctrl+Shift+Alt+↓`）、多光标加词。
- 分屏视图：一个窗口看两个文件。
- 工作区文件（`.code-workspace`）与会话恢复。

### V0.9.0：扩展生态（VS Code 的护城河）

- 扩展宿主：扩展是 Node/TypeScript 的程序，跑在已有的 Node 运行时里（`codeweaver-hermes-lab` 那一套）。
- 扩展 API 的第一版：命令、菜单、侧边栏视图、文件读写、编辑器访问。
- 扩展的安装与清单。

### V1.0.0：可发布版本

- 安装、升级、回滚、崩溃报告与数据迁移。
- SBOM、许可证清单、签名和发布通道。
- Agent 运行时 OS 级隔离及最小网络放行。
- 性能、故障恢复和安全验收全部进入 CI 发布门禁。

## 5. 开发规则

1. 每个版本先更新对应版本文档，再实现文档内编号任务。
2. 产品版本必须与主进度文档号相同；跨子项目不得出现不同产品版本。
3. “完成”必须同时具备实现、自动化测试和可追溯验收结果。
4. Mock 契约通过与真实运行时通过分别记录，不互相替代。
5. 未完成项只能标记为计划、进行中或阻塞，不以界面占位视为完成。
6. **开发优先，测试收口**：实现过程中不跑全量回归，也不跑窗口级脚本（它们会抢焦点、打断人）；回归集中到一轮里跑完，验收数字回填进 §6.2。

以上规则的完整版（含常用命令、代码风格、Windows 环境约束与 Git 约定）见仓库根的 [AGENTS.md](../AGENTS.md)。

## 6. V0.8.0 验收命令与结果

### 6.1 命令

```powershell
cargo test --manifest-path D:\github\DocuFlowAI\editor\Cargo.toml --workspace
cargo clippy --manifest-path D:\github\DocuFlowAI\editor\Cargo.toml --workspace --all-targets
cargo build --manifest-path D:\github\DocuFlowAI\editor\Cargo.toml -p codeweaver-editor
# 单元级证据：git 模块的检查要在工作区之外跑（低完整性进程起不了 git）
powershell -NoProfile -ExecutionPolicy Bypass -File editor\tools\git-unit-check.ps1
# 运行时与控制台（版本号跟着文档号走，两边都要对）
cd codeweaver-hermes-lab; npm test
cd codeweaver-workspace; npm run build
```

### 6.2 实测结果

**2026-11-25（V0.8.0 第一轮：缩略图滑块 DF-0801）**

| 检查 | 命令 | 结果 |
|---|---|---|
| 编辑器全量单元测试 | `cargo test --manifest-path D:\github\DocuFlowAI\editor\Cargo.toml --workspace --no-fail-fast` | **665 项全过，0 失败**（其中 platform 363 项，本轮 +11） |
| 编辑器静态检查 | `cargo clippy --manifest-path D:\github\DocuFlowAI\editor\Cargo.toml --workspace --all-targets` | **0 告警**，exit 0 |
| 编辑器构建 | `cargo build --manifest-path D:\github\DocuFlowAI\editor\Cargo.toml -p codeweaver-editor` | **成功**，`editor/Cargo.toml` 版本 = `0.8.0` |
| git 模块单元检查（工作区之外跑） | `powershell -NoProfile -ExecutionPolicy Bypass -File editor\tools\git-unit-check.ps1` | **12 项全过，0 失败**（在 `cargo test` 里这组会整组跳过并打印 `BLOCKED:`——跳过不算通过） |
| 运行时测试 | `cd codeweaver-hermes-lab; npm test` | **689 项，688 过 / 1 跳过 / 0 失败**，exit 0 |
| 控制台构建 | `cd codeweaver-workspace; npm run build` | **成功**（`tsc && vite build`，10.88 s） |
| 窗口级验收 | `editor\tools\minimap-check.ps1`（本轮新写） | **未跑**：脚本会抢桌面（杀进程、抢焦点、注入按键），留给统一回归那一轮。本轮只做了静态解析校验：**0 解析错误、无 BOM、无非 ASCII 字节** |

已写但未跑的窗口级脚本（连同 V0.7.0/V0.6.0 留下的十四个）都还没有窗口证据：外壳（活动栏、行号槽、状态栏）与本期的缩略图**一次都没在窗口里看过**。

**2026-11-25（V0.8.0 第二轮：面包屑 DF-0802）**

| 检查 | 命令 | 结果 |
|---|---|---|
| 编辑器全量单元测试 | `cargo test --manifest-path D:\github\DocuFlowAI\editor\Cargo.toml --workspace --no-fail-fast` | **680 项全过，0 失败**（其中 platform 378 项，本轮 +15） |
| 编辑器静态检查 | `cargo clippy --manifest-path D:\github\DocuFlowAI\editor\Cargo.toml --workspace --all-targets` | **0 告警**，exit 0 |
| 编辑器构建 | `cargo build --manifest-path D:\github\DocuFlowAI\editor\Cargo.toml -p codeweaver-editor` | **成功**，版本仍是 `0.8.0` |
| git 模块单元检查（工作区之外跑） | `powershell -NoProfile -ExecutionPolicy Bypass -File editor\tools\git-unit-check.ps1` | **12 项全过，0 失败** |
| 运行时测试 | `cd codeweaver-hermes-lab; npm test` | **689 项，688 过 / 1 跳过 / 0 失败**，exit 0 |
| 控制台构建 | `cd codeweaver-workspace; npm run build` | **成功** |
| 窗口级验收 | `editor\tools\breadcrumbs-check.ps1`（本轮新写） | **未跑**：脚本会抢桌面。本轮只做了静态解析校验：**0 解析错误、无 BOM、无非 ASCII 字节** |

### 6.3 环境约束（影响验收怎么跑）

沿用 V0.7.0 §6.3：工作区带 **Low 完整性标签**，在仓库里构建出的程序都以低完整性运行，低完整性进程起不了中等完整性的 `node.exe` 与 `git.exe`。因此窗口级脚本把编辑器复制到 `%LOCALAPPDATA%\CodeWeaver\check` 再运行，验收用的工程也放在工作区之外并带一张空的 `[workspace]` 表。

### 6.4 待跑的验收

- V0.7.0 与 V0.6.0 留下来的窗口级脚本（`activity-bar`/`command-palette`/`format`/`rename`/`task`/`command`/`git`/`debug`/`search`/`terminal`）都还没跑；本期的缩略图与面包屑各补了一个（`minimap-check.ps1` 4 项：窄条画出来了、条子画出来了、可见段更亮、点下半张地图视图跟过去；`breadcrumbs-check.ps1` 4 项：那一行画出来了、文件名写在里面、目录名写在里面、点目录之后文件树换到了那个目录），随统一回归补齐。
- 本轮写完的 `breadcrumbs-check.ps1`、`minimap-check.ps1` 与 `activity-bar-check.ps1` 都只做了**静态**校验（PowerShell 解析 0 错误、无 BOM、无非 ASCII 字节），没有运行——运行会抢桌面。
- **窗口级脚本的坐标已经跟新布局对齐**：正文下移 24 px 之后，脚本里写死的正文位置就偏了。本轮把这些脚本里的坐标一起改了（正文左上角 `340, 58` = 活动栏 48 + 侧边栏 220 + 间距 16 + 行号槽 56，标签条 34 + 路径行 24；缩略图顶边同样下移；`search-check.ps1` 里那行状态栏的坐标也跟着搬到了窗口底部）。改动覆盖 `minimap`/`activity-bar`/`breadcrumbs`/`settings`/`format`/`rename`/`agent`/`command-palette`/`terminal`/`debug`/`search`/`window` 十二个脚本，全部只做了**静态**校验（0 解析错误）。**但它们一次都没跑过**：外壳自 V0.7.0 起就换过一轮（活动栏、行号槽、底部状态栏），这批脚本里的其它假设（比如缩略图出现在正文右边、路径行压在行号槽上方）仍可能过期，统一回归那一轮要按实际画面再校一遍。

## 7. 本期结转

- **窗口级的证据仍然空着**：V0.6.0 与 V0.7.0 留下的十四个脚本，加上本期新写的 `activity-bar-check.ps1`、`minimap-check.ps1` 与 `breadcrumbs-check.ps1`，一次都没跑（会抢桌面），统一回归那一轮跑全套。
- **调试仍只对着假适配器验过**：真适配器（debugpy）一次都没碰过。
- **缩放（DF-0804）要动常量**：`editor_view::FONT_SIZE` 与 `LINE_HEIGHT` 现在是常量，不是状态；缩放要把它们变成「跟着设置走」的东西，涉及布局与命中测试，是本期最难的一件。
- **面包屑只到目录一级**：VS Code 的面包屑最后还会跟到当前符号（函数、结构体那一层），那要问语言服务器要符号表；现在只写到文件，符号那一层留着。
- **下一轮自然接 DF-0803 文件图标**：它跟面包屑共用「文件树里长什么样」这件事，图标是画出来的（`icons.rs` 已有画形状的底子），按后缀分颜色与形状。
