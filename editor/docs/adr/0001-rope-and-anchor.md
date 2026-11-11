# ADR-0001：编辑器文本模型使用 Rope，并引入 Anchor

- 状态：已接受
- 日期：2026-11-11
- 适用范围：`editor/crates/text`、`editor/crates/editor-core`

## 背景

`editor/crates/text` 最初的实现用 `String` 保存全文，编辑操作用
`String::replace_range` 完成。这个模型在阶段 0 的原型规模下可用，但与计划
文档 `doc/EDITOR_DEVELOPMENT_PLAN.md` 的两处硬性要求冲突：

1. §8.1 要求「位置不能长期保存为裸字节偏移，跨编辑引用使用 `Anchor`」；
   原实现只有裸 `TextOffset`，`Selection` 直接存字节偏移。
2. §9 的性能指标要求「100 MB 文本首次打开 < 3 s，且 UI 不冻结」。`String` 的
   每次编辑都是 O(n) 内存搬移，100 MB 文档上单次按键就会搬移上百兆字节，
   这个指标不可能达成。

## 决策

### 1. 文本存储改用 `ropey::Rope`

`Buffer` 内部持有 `ropey::Rope`（MIT/Apache-2.0），对外仍以字节偏移为坐标系。
选择 `ropey` 而不是自研 Rope 的理由：

- `ropey` 是 Rust 生态中最成熟的 Rope 实现，文本编辑场景有大量先例；
- 它只依赖 `str_indices`，不引入 C 代码，与本仓库「不引入需要 C 工具链的依赖」
  的构建约束相容（`criterion 0.8` 的 `alloca` 依赖就违反了这条）；
- 自研 Rope 会把阶段 0 的时间花在重复造轮子上，而不是花在 Agent 工作流上。

`ropey` 的 `insert`/`remove`/`slice` 使用 **char 索引**，而本项目的对外坐标系
是**字节偏移**。转换集中在 `Buffer::char_index` / `Buffer::char_to_byte` 两个
私有方法里，UTF-8 边界校验由 `Buffer::is_char_boundary` 统一承担。

### 2. 位置引用统一为 `Anchor`

`Anchor` 由 `(AnchorId, Bias)` 组成，锚点的当前偏移保存在 `Buffer` 的锚点表中，
每次事务应用后按明确规则重新解析。

`Bias` 语义对齐 Zed 与 VS Code 的既有约定（**对标结论**，不是自创）：

- `Bias::Left`：锚点停在插入文本之前；
- `Bias::Right`：锚点停在插入文本之后。

变换规则：

| 情形 | 锚点新位置 |
|---|---|
| 完全在编辑范围之前 | 不变 |
| 完全在编辑范围之后 | 按 `replacement_len - removed` 平移 |
| 指向被替换范围的字符 | 塌缩到该范围的起点 |
| 指向被替换范围之后的字符 | 跟随该字符，落到 `start + replacement_len` |
| 纯插入且位置恰好重合 | 由 `Bias` 决定落在插入前还是插入后 |

被删除内容的锚点塌缩到删除起点，而不是变成非法值，这样上层（Diff 审批、
Agent 冲突提示）总能得到一个可解释的位置。

### 3. 历史回放与外部写入走两条路径

`Buffer::apply` 强制检查事务的 `base_version`，外部写入（LSP 编辑、Agent
patch、格式化）如果基于过期的文本视图会被拒绝，这是防止 Agent 静默覆盖用户
工作的关键闸门。

`Buffer::replay` 跳过版本检查，仅供撤销/重做回放使用。原因是逆事务的坐标
空间属于「该编辑产生之后的状态」，而版本计数器在历史来回移动时会持续前进，
两者不再同步。历史只在严格 LIFO 顺序下行走，且所有写入都记录在同一份历史
里，所以这条路径是安全的——因此它必须是**独立方法**而不是 `apply` 上的一个
开关，避免外部写入误用它而绕过版本闸门。

### 4. `editor-core` 的 `Selection` 改为锚点对

`Selection { anchor, head }` 全部由 `Anchor` 构成，光标位置通过
`Buffer::resolve_anchor` 解析，不再直接保存字节偏移。

撤销/重做历史条目额外记录 `caret_before` / `caret_after` 两个字节偏移。
这是刻意的：历史严格 LIFO 行走时，被记录的那个坐标空间总是与缓冲区的当前
状态一致，因此记录字节偏移既准确又简单；而如果改用锚点，锚点在历史回放中
同样会被变换，反而多一层不确定性。

## 后果

- 阶段 0 §9 的「100 MB 首次打开 < 3 s」现在有了可测量的基础，基准见
  `editor/crates/text/benches/text_model.rs` 与
  `editor/crates/text/examples/text_report.rs`。
- 上层任何「保存一个位置，以后再用」的逻辑都必须改用 `Anchor`；裸
  `TextOffset` 只允许作为一次性计算的中间值。
- `Buffer::text()` 会物化整份文本，大文件下不可用于每帧调用；只读部分内容
  用 `Buffer::slice`。`editor-core` 因此提供 `version()` 供每帧读取版本号，
  避免此前 `snapshot()` 每帧拷贝全文的问题。
- `Buffer` 现在持有锚点表，长生命周期的锚点需要调用 `release_anchor` 释放，
  否则表会持续增长。

## 被否决的方案

- **继续用 `String`**：无法满足 100 MB 指标，也没有增量编辑能力。
- **自研 Rope**：阶段 0 的时间预算应花在 Agent 工作流，而不是重复实现
  成熟数据结构。
- **对标 Zed 的 CRDT 锚点**（`clock::Lamport` + `ReplicaId` 时间戳）：Zed 的锚点
  建立在 CRDT 操作日志之上，用于多人协作的因果序。本项目当前没有协作需求，
  引入整套 CRDT 层是巨大的过度设计。采用的 bias 语义则与之兼容，未来若要
  协作，可以平滑升级。
- **让 `apply` 接受「忽略版本」的标志位**：会让外部写入有机会静默绕过版本
  闸门。分成 `apply` 与 `replay` 两个方法后，误用需要在 API 层面显式选择。
