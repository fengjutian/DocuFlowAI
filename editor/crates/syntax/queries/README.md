# 语法高亮的查询

这些查询来自 [nvim-treesitter](https://github.com/nvim-treesitter/nvim-treesitter)
的 `queries/highlights.scm`，MIT 许可，随各个 tree-sitter 语法包一起发布。

原样收在这里，是因为 tree-sitter 的查询语言没有 nvim 的 `; inherits:` 指令：
TypeScript 和 C++ 的查询只写自己特有的部分，剩下的在 JavaScript 和 C 的查询里，
要拼起来才能用（见 `src/language.rs`）。

| 文件 | 来源 |
|---|---|
| `rust.scm` | `tree-sitter-rust` |
| `python.scm` | `tree-sitter-python` |
| `javascript.scm` | `tree-sitter-javascript` |
| `typescript.scm` | `tree-sitter-typescript`（拼在 JavaScript 之后） |
| `json.scm` | `tree-sitter-json` |
| `c.scm` | `tree-sitter-c` |
| `cpp.scm` | `tree-sitter-cpp`（拼在 C 之后） |
| `go.scm` | `tree-sitter-go` |

升级语法包时，把新的 `highlights.scm` 复制过来即可；查询能不能用在语法上，
由 `syntax` crate 里的测试 `every_language_has_queries_that_fit_its_grammar`
把关。
