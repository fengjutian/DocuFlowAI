# ADR-0009：语法高亮用 tree-sitter，查询自带，解析增量

- 状态：已接受
- 日期：2026-11-12
- 适用范围：`editor/crates/syntax`、`editor/crates/text`、`editor/crates/editor-core`、`editor/crates/platform`

## 背景

计划 §5.1 的 MVP 清单里有"语法高亮"。这一条看着只是"给字换个颜色"，实际上要回答
四个问题：

1. **语法树谁来解析**：自己写解析器不可能；业界的事实标准是 tree-sitter，Zed、
   nvim、Helix 都用它。
2. **哪些文字算什么**：这是"查询"（query）的事。tree-sitter 的查询语言和
   nvim-treesitter 的命名习惯是配套的，主流语言的语法包里都自带
   `queries/highlights.scm`。
3. **改了一个字之后怎么办**：重新解析整篇文档会让每一次按键都付全文的代价。
4. **高亮怎么画**：渲染器是"先画所有色块、再画所有文字"，一段文字要分成几段、
   每段一个颜色。

## 决策

### 1. 独立的 `syntax` crate，只依赖 `text`

`editor/crates/syntax` 依赖 `codeweaver-text` 和 tree-sitter，**不依赖**
`editor-core`（不需要选区模型），也**不依赖** `render`（颜色是 platform 的事）。
依赖方向是 `platform -> syntax -> text`，和 `platform -> editor-core -> text` 平行。

高亮结果是 `Vec<Highlight>`，每项是"字节范围 + 种类"。谁画、什么颜色，是上层的事。

### 2. 语言按扩展名判定，特殊文件名另有规则

`Language::from_path` 先看文件名（`Cargo.toml` → Rust，`package.json` →
JSON），再看扩展名。第一版支持 8 种：Rust、Python、JavaScript、TypeScript、
TSX、JSON、C、C++、Go。

### 3. 查询用语法包自带的，补丁语言要把两份拼起来

tree-sitter 的语法包里**带着** `queries/highlights.scm`（这是第一次探测时的
误判——以为要自己写）。它们来自 nvim-treesitter，MIT 许可，**原样收进仓库**
`crates/syntax/queries/`，出处记在 `queries/README.md`。

但 tree-sitter 的查询语言**没有** nvim 的 `; inherits:` 指令：TypeScript 的查询
只写"TypeScript 特有的部分"，C++ 的查询只写"C++ 特有的部分"，剩下的在
JavaScript / C 的查询里。所以这两种语言要把两份查询**拼起来**再交给
`Query::new`。

拼接是否合法不能靠眼睛看：`syntax` crate 里有一条测试
（`every_language_has_queries_that_fit_its_grammar`）对每种语言都编译一遍查询，
不合就直接红。

### 4. 捕获名按 nvim 的层级命名映射到种类

查询里的捕获名是分层的：`function.method` 是方法，`type.builtin` 是内建类型，
`comment.documentation` 是文档注释。映射规则很简单：**第一个点之前的那一段**
决定种类（`function` → 函数，`type` → 类型……）。点之后的部分只用于区分同一个
大类的深浅，主题不必管。

不认识的名字（`embedded`、`spell` 之类）直接丢掉——那不是给编辑器看的。

### 5. 重叠的捕获要消解

一条文字可能被好几个查询命中（`FOO()` 既是"全大写常量"又是"函数调用"）。
消解规则：

- **短的赢**：字符串里的转义比字符串本身更具体；
- **同长度时，靠后写的赢**：查询里越靠后的规则越具体（TypeScript 的规则因此
  压过 JavaScript 的）。

赢不了的字节就不高亮，所以高亮结果**互不重叠**（有测试钉死这一点）。

### 6. 增量解析要知道"文本怎么动的"

tree-sitter 的增量解析要两样东西：改动的字节范围，和**被替换掉的旧文本**
（用来算位置）。缓冲区只留现在的文本，旧文本没了。

所以 `text` crate 的 `AppliedTransaction` 多带了一个 `change`：

```rust
pub struct BufferChange {
    pub edits: Vec<TextEdit>,     // 改了哪些地方
    pub replaced: Vec<String>,    // 那里原来是什么
}
```

`editor-core` 的 `Editor::last_change()` 把它交给上层。`syntax` 用它算出
tree-sitter 的 `InputEdit`，于是：

- **一次编辑**：跟着旧树走（`tree.edit()` + 重解析），只重解析改动附近；
- **一次编辑以上**（比如"全部替换"）：文本在多个地方动过，树跟不上，**整篇重解析**。

### 7. 只给看得见的文字算高亮

`QueryCursor::set_byte_range` 把查询限制在可见的行上。所以：

- 打开一个 100MB 的文件时，高亮的成本是**一屏**，不是全文；
- 滚动时重算一次（可见范围变了）。

树的解析仍然是全文的（见 §8）。

### 8. 解析的成本：量出来，写进基准

`crates/syntax/benches/highlight.rs` 量三件事（release，10K 行 Rust 文件）：

| 场景 | 中位数 | 说明 |
|---|---|---|
| 打开文件（整篇解析） | 167 ms | 计划给 100MB 文件 3 秒 |
| 敲一个键（增量解析），2000 行 | 3.9 ms | 在一帧（16 ms）之内 |
| 敲一个键（增量解析），10000 行 | 23 ms | **超了一帧** |
| 一帧的高亮（可见 40 行） | 0.8 ms | 只和屏幕大小有关 |

tree-sitter 的增量解析**要走过旧树**才能找到改动的地方，所以一次按键的成本
随文档变大而变大。2000 行的文件没问题，10000 行的文件就超了预算。

**这是一个已知的缺口，不是这次要解决的**：解法是**把解析放到后台线程**
（Zed 就是这么做的：解析在别的线程上跑，结果好了再拿回来）。解析状态
（`Parser`、`Tree`）现在在 UI 线程上，要挪到后台得先把它们和 UI 解耦——
那是另一件工作。基准里的预算按现在的实测值定，先挡住"变慢"的回归。

### 9. 文字的颜色和色块的颜色走的不是同一条路

查颜色时撞见的：渲染器把**色块**的颜色当线性值写进 sRGB 的 surface（surface
会转换），而**文字**的颜色是按字节直接写进图集、再原样贴到屏幕上（不转换）。

也就是说：

- 色块：代码里 `0.075, 0.086, 0.11` → 屏幕上 `(77, 83, 93)`；
- 文字：代码里 `0.76, 0.36, 0.90` → 屏幕上 `(193, 91, 229)`。

这不是这次引入的，是渲染器原有的行为。这次**照它写**（主题的颜色按"屏幕上看到
的就是我写的"来挑），但这是两套颜色空间混用，应该统一——那是渲染器自己的
一件事。

### 10. 绘制：一行拆成几段，每段一次

`EditorView::draw` 把一行按高亮区间切成若干段，每段用 `line.offset_at()` 定位、
用主题的颜色画。没有高亮的文字仍然是一整段（和以前一样）。

IME 的合成文字夹在光标处：合成文字不在缓冲区里，没有字节偏移，所以做法是
**把那一行在光标处切开**，合成文字画在中间，它后面的文字往右挪合成文字的宽度。

## 后果

- 8 种语言有语法高亮，颜色可读，关键字/字符串/数字/注释一眼能分出来。
- 高亮的成本只和**屏幕大小**有关（0.8 ms），和文档大小无关。
- 打开文件要整篇解析一次（167 ms / 10K 行）。
- **敲一个键的解析成本随文档变大**：2000 行 3.9 ms，10000 行 23 ms（超了
  计划的 16 ms 一帧）。解法是后台解析，是下一步。
- 查询来自 nvim-treesitter（MIT），**收在仓库里**，出处记在
  `crates/syntax/queries/README.md`。
- 颜色有两套空间（文字是 sRGB、色块是线性），见 §9。
- Markdown 暂时不支持：`tree-sitter-md` 依赖 `tree-sitter 0.24`，和这里的
  `0.25` 抢同一个 `links = "tree-sitter"`，Cargo 不允许同时存在。

## 被否决的方案

- **自己写查询**：语法包**自带** `queries/highlights.scm`，而且是
  nvim-treesitter 打磨过的。自己写只会更差。
- **把全文拷出来喂给 tree-sitter**（`buffer.text()`）：100MB 的文档一次按键就
  拷 100MB。改用 **rope 的分块**喂（`Buffer::slice_from`），代价是每次读的分块
  多一次查找（实测 10K 行的解析从 21.4 ms 变成 23.3 ms）。
- **每次编辑整篇重解析**：那等于放弃 tree-sitter 的增量能力，10K 行的文件
  一次按键要 167 ms。
- **用 `set_included_ranges` 只解析可见范围**：那样解析出来的树是**片段**，
  脱离上下文（函数体在屏幕外、括号不配对）时语法会认错，高亮会错。
- **在 `syntax` crate 里定义颜色**：颜色要用 `render::Color`，那会让 `syntax`
  依赖 `render`，依赖方向就反了（渲染层不该被语法层拖着走）。
- **把查询的捕获直接当高亮**：捕获会重叠（一段文字既是常量又是函数），
  必须消解，否则绘制时同一段字会被画两次。
- **在 `text` crate 里做语法分析**：文本模型不该知道语法。
