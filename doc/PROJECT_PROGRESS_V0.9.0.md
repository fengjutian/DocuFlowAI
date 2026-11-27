# CodeWeaver 项目进度与开发计划 V0.9.0

文档号：`V0.9.0`  
产品版本：`0.9.0`  
状态：进行中（DF-0901 工作区文件已实现，窗口级验收待跑）  
更新日期：2026-11-27

---

## 1. 本期结论

V0.8.0 把编辑器的**编辑体验**补齐了：缩略图滑块、面包屑、文件图标、缩放、分屏视图、多光标补齐（细节见 [V0.8.0 §6.2](./PROJECT_PROGRESS_V0.8.0.md)）。自动化测试 747 项全绿，clippy 0 告警。

**但编辑器还只认「一个文件夹」**。VS Code 与 Zed 的工程有两层：**文件夹**（folder）与**工作区**（workspace）。一个 `.code-workspace` 文件可以点名**好几个文件夹**，编辑器把它们一起收在一个工程里；文件树里每个根占一行，文件在各自的根下面；打开别的文件夹时，切换的是整个工程。

现在编辑器只认一个根：文件树跟着打开的文件走（`show_tree_of`），工程根就是那个文件的上一级目录。**多根工程做不了**——这不是「少一个按钮」，是工程模型少了一层。

本期做 **DF-0901 工作区文件**：

1. **工作区文件是 JSON**，跟 VS Code 一个形状：

   ```json
   {
     "folders": [
       { "path": "../editor" },
       { "path": "../console", "name": "控制台" }
     ],
     "settings": { "editor.fontSize": 16 }
   }
   ```

   `folders` 数组每项一个 `path`（可带 `name`），`settings` 是对象。**相对路径按工作区文件自己所在的目录解析**，绝对路径照旧。

2. **打开工作区**：在文件树里点一个 `.code-workspace` 文件，或者启动时把它当参数传进来，它就**开成工作区**，不是当文本文件打开。命令 `workspace.save` 把当前的根写回工作区文件。

3. **文件树变成多根**：打开工作区后，每个根文件夹**占一行**，可展开可收起，文件在各自的根下面；**只有一根时也占一行**（工作区就是工作区）。**没有工作区文件的老样子不变**：一个文件夹的工程还是把内容直接铺开，不多出一行。

4. **工作区名看得见**：侧边栏标题变成工作区的名字，状态栏右边写着「· 工作区 X」，状态栏的消息说明确说「工作区 X 打开了 N 个文件夹」。

5. **读不出来要明说**：文件不在、JSON 坏了、`folders` 不是数组、某个文件夹在磁盘上没了——**哪一种都要说清楚是哪一种**，不静默什么都不发生。

6. **组件拆分**：工作区文件的读写放在新组件 `crates/platform/src/workspace_file.rs`（不认识 `NativeApplication`）；多根的树放在 `crates/workspace/src/tree.rs` 的 `FileForest`（不认识窗口）。窗口只负责把「点了哪个文件」变成「打开哪个工作区」。

本期**不做**的：工作区里的 `settings` 真正生效（解析出来但不用）、会话恢复（下次启动还开着这些文件）、把文件夹加进已经打开的工作区。这三件在 §7 结转里。

本期还做 **DF-0902 全局搜索覆盖工作区的所有根**：工作区能点名好几个文件夹，搜索却只搜一个根——那是不成的。现在**工作区开着时，搜索搜的是它点名的每一个文件夹**，地方按文件路径排在一起，**地方的说法是相对它自己那个根**（`src/main.rs:12`）；**同一个地方只算一处**（两个根套在一起时）；**最多的处数是整次搜索的上限**。没有工作区时老样子不变：搜当前文件所在工程。

**第一轮做完的是 DF-0901 工作区文件**：编辑器原来只认**一个**文件夹（文件树跟着打开的文件走），现在多了一层**工作区**。一个 `.code-workspace` 文件（JSON，`folders` 数组每项一个 `path`、可以带 `name`，外加一个 `settings` 对象）点名好几个文件夹，编辑器把它们收在一个工程里；**相对路径按工作区文件自己所在的目录解析**，所以整个文件夹搬走了那个文件还是同一个意思。**点文件树里的 `.code-workspace` 文件就开成工作区**（不是当文本文件打开），命令行传进来也一样。**文件树里每个根占一行**，可展开可收起，文件在各自的根下面；**没有工作区文件的老样子不变**（一个文件夹的工程还是把内容直接铺开）。**工作区名看得见**（侧边栏标题与状态栏右边）。**读不出来要明说**：文件不在、JSON 坏了、`folders` 不是数组、第几项没有 `path`、某个文件夹在磁盘上没了——**分别说清楚是哪一种**。新组件 `crates/platform/src/workspace_file.rs`（16 个单测）不认识 `NativeApplication`：这个文件的读写、路径的解析、名字的来路，都是**纯计算**；多根的树是 `crates/workspace/src/tree.rs` 里新的 `FileForest`（7 个单测），**原来那棵树一个字没动**，多根是加在上面的一层。命令 `workspace.save`（`Ctrl+Shift+S`）把根写回已经打开的工作区文件；没有工作区就在根的旁边写一个 `<文件夹名>.code-workspace`。`.code-workspace` 在文件树里有**自己的记号**（青色框里一条竖线，像一个分屏的窗口）。本轮把**产品版本从 `0.8.0` 抬到了 `0.9.0`**（ACP 的协议版本仍是 1）。

## 2. 当前完成度

| 模块 | 已完成 | 仍缺失 | 判断 |
|---|---|---|---|
| 文本内核 | Rope/Anchor/事务、撤销重做、多选、多光标、查找替换 | 结构化编辑（折叠/括号匹配） | 可用 |
| 视图与输入 | 布局、命中测试、拖拽选区、滚动、可见区定位、IME、分屏视图、多光标 | 横向分屏、三栏以上 | 可用 |
| 工作区 | 文件读写、指纹、外部变更检测、文件树、全局搜索与替换、搜索规则 | 项目索引 | 可用 |
| **工程模型**（本期） | 单根工程、**工作区文件与多根（DF-0901）**、**全局搜索覆盖所有根（DF-0902）** | 工作区设置生效、会话恢复、增删文件夹 | 进行中 |
| 标签与会话 | 标签模型、多标签、脏状态、关闭确认 | 会话恢复、最近项目 | 可用 |
| 语言能力 | 语法高亮（后台）、LSP 补全与诊断、格式化、重命名、代码操作 | onType 格式化、重命名预备 | 可用 |
| 调试 | DAP 客户端、断点、单步、变量、条件断点、表达式求值、调用栈、跳源码 | 命中次数、多线程、调试控制台、**真适配器未验** | 最小可用 |
| Agent | ACP 客户端、面板、审批、流式输出、上下文选择、Diff 按块批准、轨迹 | 语义冲突判断 | 可用 |
| 开发工具 | 任务运行器、交互式终端、Git 面板、全局搜索与替换 | 真伪终端（PTY）、终端多标签、分支与远端 | 基础可用 |
| 外壳与命令（V0.7.0） | 命令注册表、命令面板、活动栏与侧边栏分区、行号槽、底部状态栏、键位可改、开发模式 | 窗口级验收（统一回归那轮） | 可用 |
| 编辑体验（V0.8.0） | 缩略图滑块、面包屑、文件图标、缩放、分屏视图、多光标补齐 | 窗口级验收 | 可用 |

## 3. V0.9.0 开发项

### DF-0901 工作区文件（多根工作区）（进行中）

编辑器现在只认一个文件夹。本期让它认**工作区**：一个 `.code-workspace` 文件点名好几个文件夹，编辑器把它们收在一个工程里。

验收标准：

- **工作区文件是 JSON**：`folders` 是数组，每项至少有 `path`，可以带 `name`；`settings` 是对象（本期只解析，不生效）。**相对路径按工作区文件所在目录解析**，绝对路径照旧。
- **文件夹的名字**：文件里写了 `name` 就用那个；没写就用文件夹自己的名字；文件夹在磁盘上不在了就用文件里写的那串路径，并**明说这个文件夹没了**。
- **打开工作区有两条路**：在文件树里点一个 `.code-workspace` 文件；启动时把它当参数传进来。两条路都是**开成工作区**，不是当文本文件打开。
- **文件树变成多根**：打开工作区后**每个根占一行**（可以展开收起），文件在各自的根下面；**只有一根时也占一行**。**没有工作区文件的老样子不变**：一个文件夹的工程还是把内容直接铺开。
- **工作区名看得见**：侧边栏标题变成工作区名，状态栏明说工作区打开了几个文件夹。
- **读不出来要明说**：文件不在、JSON 坏了、`folders` 不是数组、文件夹没了——**分别说清楚**，不静默什么都不发生。
- **组件拆分**：工作区文件的读写在 `crates/platform/src/workspace_file.rs`，多根的树在 `crates/workspace/src/tree.rs` 的 `FileForest`；两者都不认识 `NativeApplication`。
- **保存工作区**：命令 `workspace.save` 把当前的根写回已经打开的工作区文件；没有打开工作区时，把当前文件夹写成 `<文件夹名>.code-workspace` 放在根的旁边，并明说写到了哪儿。
- 单元测试覆盖：工作区文件读对了、相对路径解析对了、`folders` 坏了要报错、多根树的行对不对、收起的根不再显示它的文件、单根时不额外多一行。

实现落点：

- **新组件 `crates/platform/src/workspace_file.rs`（16 个单测）**，不认识 `NativeApplication`：
  - `Folder{path, name}`——文件里写的那一串路径与它给的名字；`Contents{folders, settings}`——文件里的内容；`Root{name, path, there}`——解出来的文件夹（`there` 说它还在不在）；`Workspace{path, name, roots, settings}`——**打开着的**工作区。
  - `read(path)` / `write(path, &Contents)`：读与写这个文件；`parse(text)` 把 JSON 变成内容，**坏了要说出是哪儿坏了**（不是合法的 JSON / `folders` 不是数组 / 第几项没有 `path`）。
  - `roots(base)`：把 `folders` 里的路径解成磁盘上的路径——**相对路径按工作区文件自己所在的目录解**，绝对路径照旧；名字用文件给的 `name`，没给就用文件夹自己的名字。
  - `is_workspace_file(path)`：按后缀认（大小写不算）；`name_of(path)`：工作区的名字是文件去掉后缀。
  - `Workspace::contents()`：写回去的时候，路径写成**相对于文件自己的目录**的样子，能读回来还是同一个意思。
- **`crates/workspace/src/tree.rs` 新增 `FileForest`（7 个单测）**：几个文件夹就是几棵树。
  - `FileForest::single(root)`：一个文件夹，**它的内容直接铺开**（没有工作区文件时的老样子）。
  - `FileForest::many(folders)`：每个文件夹**占一行**（`name` + 路径），行可以收起；收起的文件夹不显示它的文件。
  - `toggle(path)`：点的是文件夹就收起或展开它，点的是里面的目录就交给那一棵树；`contains`、`roots`、`refresh` 跟单棵树一个路子。
  - 窗口里那棵树（`FileTree`）**一个字没动**，多根是**加在上面的一层**。
- **`crates/platform/src/lib.rs`**：
  - `tree: Option<FileForest>` 取代 `Option<FileTree>`；新增 `open_workspace: Option<workspace_file::Workspace>`（**名字跟那个「文件服务」的 `workspace` 字段分开**，两个是不同东西）。
  - `open_file(path)`：**先看是不是工作区文件**，是就开工作区，不是文本文件。
  - `open_the_workspace(path)`：读文件 → 建多根的树 → 状态栏说「工作区 X 打开了 N 个文件夹」，**少了的文件夹当场说出来**；读不出来就说读不出来的原因。
  - `show_tree_of(path)`：**有工作区时树不跟着文件走**——工作区点名的文件夹不因为打开了别的文件就变。
  - `go_to_the_place_of_the_path()`：点面包屑里的文件夹 → 树换成那个文件夹（`single`），**工作区让位**（`open_workspace = None`）。
  - `sidebar_title()`：侧边栏标题在有工作区时是**工作区的名字**，没有时还是「资源管理器」。
  - `save_the_workspace()`：写回已经打开的工作区文件；没有工作区就在根的旁边写一个 `<文件夹名>.code-workspace`（`folders` 是 `[{"path": "."}]`）。
- **`crates/platform/src/commands.rs`**：新命令 `workspace.save`（「保存工作区」，`Ctrl+Shift+S`）。
- **`crates/platform/src/file_icons.rs`**：`.code-workspace` 有**自己的记号**（青色框里一条竖线，像一个分屏的窗口），跟别的记号都不像。
- 单测：`workspace_file.rs` 16 条（文件读对了、相对路径、绝对路径、名字、缺文件夹、读写往返、`folders` 坏了要报错）、`tree.rs` 7 条（单根不额外多一行、多根每个一行、收起的根不显示文件、单根的文件夹不能收起、里面的目录能开能关、refresh）、应用层 10 条（点开工作区文件 → 两个根各占一行、不是文本文件、侧边栏标题、状态栏、读不出来要说、缺文件夹要说、树不跟着文件走、写回自己的文件、没有工作区就自己建一个、命令行传进来也算）。

已知边界（本版本内不做，写在 §7 结转里）：工作区里的 `settings` **只解析不生效**；全局搜索仍然只搜一个根；工作区的根不能增删。

### DF-0902 全局搜索覆盖工作区的所有根（已实现，窗口级验收待跑）

工作区能点名好几个文件夹，但搜索还是只搜**一个**根（当前文件所在工程的那个）。一个工作区搜不到自己另一个根里的东西，这是不成的。

验收标准：

- **工作区开着时，搜索搜的是工作区点名的所有文件夹**，不是当前文件所在工程的一个根。
- **没有工作区时老样子不变**：搜的是当前文件所在工程（往上找到带清单的那一层）。
- **几个根里的地方放在一起排**，按文件路径排；地方的说法是**相对于它自己那个根**（`src/main.rs:12`，不是 `D:\…\src\main.rs:12`）。
- **同一个地方只算一处**：两个根套在一起（一个根在另一个根里面）时，那个文件里的那一行只出现一次。
- **最多的处数是整次搜索的上限**，不是每个根各算一份。
- **状态栏明说在几个文件夹里找**：「在 2 个文件夹里找：写要找的文字，回车开始找」。
- **组件拆分**：搜哪几个根是**纯计算**，放在 `codeweaver_workspace::search::search_in`；面板只负责说「往哪找」。
- 单元测试覆盖：两个根都搜到、套着的根只算一次、上限是整次的、地方相对自己那个根、没有工作区时搜整个工程。

实现落点：

- **`crates/workspace/src/search.rs`**：新增 `search_in(roots: &[PathBuf], needle, options)`——**一个根一个根地搜，把地方放在一起**：
  - **上限是整次的**：前面的根已经找到多少，后面的根就只能再找多少（`walk` 收到的 `max_hits` 是剩下的那一份）。
  - **同一个地方只算一次**：按「文件 + 行」记（**不是只按文件**——同一个文件里好几行都有那个词，每一行都是一处地方）。
  - `search(root, …)` 现在是 `search_in(&[root], …)`，**一个根的用法一个字没变**。
- **`crates/platform/src/search.rs`**：`SearchPanel` 的 `root: PathBuf` 变成 `roots: Vec<PathBuf>`。
  - `new(roots, needle)` / `with_replace(roots, needle)`：**构造面板就带上一组根**（原来只带一个）。
  - `root_of(path)`：**这个地方在哪个根里**——套着的根算**最里面那个**（`C:\a` 与 `C:\a\b` 都在时，文件在 `C:\a\b` 里就按 `C:\a\b` 说）。
  - `place_of(hit)`：地方**相对它自己那个根**说。
  - `start_search()`：搜的是**全部的根**（`search_in`）。
- **`crates/platform/src/lib.rs`**：
  - `the_roots_to_search()`：**工作区开着就用工作区的根**，否则用当前文件所在工程的根；一个都没有就说「没有打开的文件，不知道在哪儿找」。
  - `open_search_panel()`：把那一组根交给面板；**根多于一个时**状态栏说「在 N 个文件夹里找」。
  - `open_search_hit(hit, root)`：根变成**可选的**——那个地方不在任何根里时就说完整的路径。
- 单测：`workspace` crate 4 条（两个根都搜到、套着的根只算一次、上限是整次的、有一个根不在时跳过它照样搜）、`platform/src/search.rs` 3 条（面板搜遍所有根、根是哪一个、地方相对自己那个根）、应用层 2 条（工作区里两个根都搜到、没有工作区时搜的是整个工程）。

已知边界：搜索**还是按文件夹一个个走**（没有索引，大工程里慢）；`路径包含` 那个过滤是**相对每个根**的路径说的；替换（`Ctrl+Shift+H`）跟着搜索走，因此也会改到**所有根**里的文件。

## 4. 未开发功能拆分

### 仍留在 V0.9.0 后面的

- 工作区里的 `settings` 真正生效（工作区设置盖过用户设置）。
- 会话恢复：下次启动还开着上次的文件与分屏。
- 把文件夹加进已经打开的工作区、从工作区里拿掉一个文件夹。
- 项目索引与全局符号搜索（大工程里的搜索还是按文件夹走，没有索引）。

### V0.10.0：扩展生态（VS Code 的护城河）

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

## 6. V0.9.0 验收命令与结果

### 6.1 命令

```powershell
# 编辑器
cargo build --manifest-path D:\github\DocuFlowAI\editor\Cargo.toml -p codeweaver-editor
cargo test  --manifest-path D:\github\DocuFlowAI\editor\Cargo.toml --workspace
cargo clippy --manifest-path D:\github\DocuFlowAI\editor\Cargo.toml --workspace --all-targets

# 单元级证据：git 模块的检查要在工作区之外跑（低完整性进程起不了 git）
powershell -NoProfile -ExecutionPolicy Bypass -File D:\github\DocuFlowAI\editor\tools\git-unit-check.ps1

# 运行时与控制台（版本号跟着文档号走，两边都要对）
cd D:\github\DocuFlowAI\codeweaver-hermes-lab; npm test
cd D:\github\DocuFlowAI\codeweaver-workspace; npm run build
```

### 6.2 实测结果

**2026-11-27（V0.9.0 第一轮：工作区文件 DF-0901）**

| 检查 | 命令 | 结果 |
|---|---|---|
| 编辑器全量单元测试 | `cargo test --manifest-path D:\github\DocuFlowAI\editor\Cargo.toml --workspace --no-fail-fast` | **781 项全过，0 失败**（本轮 +34：workspace 7 + platform 27：workspace_file 16 + 应用层 10 + 文件记号 1） |
| 编辑器静态检查 | `cargo clippy --manifest-path D:\github\DocuFlowAI\editor\Cargo.toml --workspace --all-targets` | **0 告警**，exit 0 |
| 编辑器构建 | `cargo build --manifest-path D:\github\DocuFlowAI\editor\Cargo.toml -p codeweaver-editor` | **成功**，`editor/Cargo.toml` 版本 = `0.9.0` |
| git 模块单元检查（工作区之外跑） | `powershell -NoProfile -ExecutionPolicy Bypass -File editor\tools\git-unit-check.ps1` | **12 项全过，0 失败** |
| 运行时测试 | `cd codeweaver-hermes-lab; npm test` | **689 项，688 过 / 1 跳过 / 0 失败**，exit 0 |
| 控制台构建 | `cd codeweaver-workspace; npm run build` | **成功**（`tsc && vite build`） |
| 窗口级验收 | `editor\tools\workspace-check.ps1`（本轮新写） | **未跑**：脚本会抢桌面。本轮只做了静态解析校验：**0 解析错误、无 BOM、无非 ASCII 字节** |

本轮把**产品版本从 `0.8.0` 抬到了 `0.9.0`**：`editor/Cargo.toml`（`Cargo.lock` 由 cargo 自己更新）、`codeweaver-hermes-lab/package.json` 与它的 `package-lock.json`、`codeweaver-workspace/package.json` 与它的 `package-lock.json`、`src-tauri/tauri.conf.json`。顺手把 `src-tauri/Cargo.toml` 里那个**一直停在 `0.6.0`** 的 crate 版本也同步了（它的 `Cargo.lock` 条目一起改）——`AGENTS.md` 说「跨子项目不得出现不同产品版本」。**ACP 的协议版本仍是 1**（`PROTOCOL_VERSION` / `ACP_PROTOCOL_VERSION`）：那是协议版本，跟着协议走，不跟着产品版本走。

本轮修掉的两个真问题：

- **字段重名**：新加的「打开着的工作区」本来也叫 `workspace`，而那个名字**已经被文件服务占着**（`codeweaver_workspace::Workspace`，负责读写文档）。两个是不同东西，却撞了名——编译直接报 `field workspace is already declared`。新的那个改叫 **`open_workspace`**。
- **借用撞车**：侧边栏的标题要问「有没有工作区」，而画窗口的那一段**先把渲染器借走了**（`self.renderer.as_mut()` 是对整个 `self` 的可变借用）。标题只能在**借渲染器之前**算好——和旁边那些「在借渲染器之前读好」的值（状态栏、任务面板、断点）是同一套路。

**2026-11-27（V0.9.0 第二轮：全局搜索覆盖所有根 DF-0902）**

| 检查 | 命令 | 结果 |
|---|---|---|
| 编辑器全量单元测试 | `cargo test --manifest-path D:\github\DocuFlowAI\editor\Cargo.toml --workspace --no-fail-fast` | **790 项全过，0 失败**（本轮 +9：workspace 4 + platform 5） |
| 编辑器静态检查 | `cargo clippy --manifest-path D:\github\DocuFlowAI\editor\Cargo.toml --workspace --all-targets` | **0 告警**，exit 0 |
| 编辑器构建 | `cargo build --manifest-path D:\github\DocuFlowAI\editor\Cargo.toml -p codeweaver-editor` | **成功**，版本仍是 `0.9.0` |
| git 模块单元检查（工作区之外跑） | `powershell -NoProfile -ExecutionPolicy Bypass -File editor\tools\git-unit-check.ps1` | **12 项全过，0 失败** |
| 运行时测试 | `cd codeweaver-hermes-lab; npm test` | **689 项，688 过 / 1 跳过 / 0 失败**，exit 0 |
| 控制台构建 | `cd codeweaver-workspace; npm run build` | **成功**（`tsc && vite build`） |
| 窗口级验收 | `editor\tools\search-roots-check.ps1`（本轮新写） | **未跑**：脚本会抢桌面。本轮只做了静态解析校验：**0 解析错误、无 BOM、无非 ASCII 字节** |

本轮踩到的一个坑，写在这里是为了下一个人别再踩：**「同一个地方只算一次」不能只按文件记**。头一版按 `PathBuf` 去重，结果**同一个文件里两行都有那个词时，第二行被吞了**——而那明明是两处地方。判据是「**文件 + 行**」：两个根套在一起时，那个文件的**同一行**只出现一次，同一个文件的**不同行**是不同的地方。头一版的两个既有用例（「处数有上限」「找整词」）当场变红——**它们本来就是对的，是新代码错了**。

### 6.3 环境约束（影响验收怎么跑）

沿用 V0.8.0 §6.3：工作区带 **Low 完整性标签**，在仓库里构建出的程序都以低完整性运行，低完整性进程起不了中等完整性的 `node.exe` 与 `git.exe`。因此窗口级脚本把编辑器复制到 `%LOCALAPPDATA%\CodeWeaver\check` 再运行，验收用的工程也放在工作区之外并带一张空的 `[workspace]` 表。

单元测试落文件要写进 **crate 自己的 `target/`**（`crates/platform/src/testing.rs` 的 `scratch()` / `scratch_dir()` / `scratch_project()`），不能写 `%TEMP%`。

### 6.4 待跑的验收

- V0.8.0、V0.7.0 与 V0.6.0 留下来的窗口级脚本都还没跑；本期补两个（`workspace-check.ps1` 5 项、`search-roots-check.ps1` 4 项），随统一回归一起跑。
- **`workspace-check.ps1` 起两个编辑器**：一个用普通文件打开（标题是「资源管理器」、状态栏没有工作区），一个用工作区文件打开（标题是工作区名、状态栏有「· 工作区 proj」）。两者的**标题那一块**和**状态栏右边那一块**比一比，像素不一样就说明它变了。这么比是因为**没法读屏幕上的字**——只能比像素。
- 行的位置是**算出来的**：侧边栏从 48 开始，段标题 28 高，一行 22 高，一级缩进 14，记号占 15。所以第一个根在第 0 行（y=28）、它的文件在第 1 行（y=50）、第二个根在第 2 行（y=72）、它的文件在第 3 行（y=94）。点第一个根之后行会往上挪一格，所以「第二个根下面那个文件」的位置会空出来——这就是「点一下能收起」的证据。
- **`search-roots-check.ps1` 量的是「地方的列」**：搜索面板的框在 (16,16)，「找到几处」那行在 y=66，地方那一列从 y=90 开始，一行 22 高。地方的字色是 `rgb(0.62,0.70,0.86)`——**文字不用 sRGB 换算**（`To-Bytes`，不是 `To-Pixel`）。两个根各有一处时，第一行和第二行**都**该有字；只搜一个根时第二行是空的。
- 本轮新写的两个脚本只做了**静态**校验（PowerShell 解析 0 错误、无 BOM、无非 ASCII 字节），**没有运行**——运行会抢桌面。

## 7. 本期结转

- **窗口级的证据仍然空着**：V0.6.0 与 V0.7.0 留下的十四个脚本，加上 V0.8.0 的七个与本期的两个（`workspace-check.ps1`、`search-roots-check.ps1`），一次都没跑（会抢桌面），统一回归那一轮跑全套。
- **工作区里的设置还没生效**：`settings` 段**解析出来了**，但**没有**盖过用户设置——本期的字号还是只从 `%APPDATA%\CodeWeaver\settings.json` 读。下一步要定的是**谁盖谁**（VS Code 是工作区设置盖用户设置）。
- **会话恢复没有做**：下次启动还是空的编辑器。
- **工作区的根不能改**：只能整个打开一个工作区文件，不能把文件夹加进去或拿掉。
- **搜索还是按文件夹一个个走**：没有索引，大工程里搜一遍要时间；每个根搜完才轮到下一个。
- **搜索的「路径包含」过滤是相对每个根说的**：两个根里各有一个 `src` 时，过滤 `src` 两个都算数。
- **工作区名只在两个地方看得见**：侧边栏标题与状态栏右边；**窗口标题条上还没有**（编辑器现在不设窗口标题）。
- **调试仍只对着假适配器验过**：真适配器（debugpy）一次都没碰过。
