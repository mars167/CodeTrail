# 可靠性模型

> 当前设计准绳见 `docs/00-design-summary.md`。本文只展开可靠性分级。

CLI 应该为每个命令响应附带可靠性契约。LLM 必须能够判断一个结果是源码事实、解析器事实，
还是 best-effort 的关系候选。

## 分级

| 级别 | 名称 | 来源 | 含义 |
| --- | --- | --- | --- |
| L0 | source_fact | text/path/git/filesystem | 可直接验证的源码证据 |
| L1P | precise_fact | SCIP/语言服务/编译器索引 | 可作为 IDE 跳转级别结果的精确事实 |
| L1S | parser_fact | tree-sitter AST | 解析得到的声明、范围、类型或语法节点 |
| L2 | inferred_candidate | AST heuristics/search-based inference | 需要二次验证的关系候选 |

## 命令映射

| 命令 | 级别 | 默认 LLM 指令 |
| --- | --- | --- |
| `find` | L0 | 作为精确源码证据处理。 |
| `files` | L0 | 作为精确路径证据处理。 |
| `read` | L0 | 作为精确源码证据处理。 |
| `refs` | L0 | 作为 identifier 文本证据处理，不代表语义引用解析。 |
| `symbols` | L1P/L1S | 优先作为 precise symbol 处理；fallback 时必须标注 parser 来源。 |
| `defs` | L1P/L1S | 优先作为 IDE 级跳转结果处理；fallback 时必须标注可能遗漏。 |
| `calls` | L2 | 只作为候选结果处理；需要用 `read` 验证。 |
| `callers` | L2 | 只作为候选结果处理；不是完整反向调用图。 |

准确性规则：

- `exact=true` 只能出现在 L0 或 L1P 结果上。
- L1S 可以是确定性 parser fact，但不能等同于语义精确引用解析。
- L2 永远不能标记为 exact；只能标记为 candidate。
- Remote 返回的结果必须携带 snapshot 和 hash；无法本地验证时，必须标注 `remote_unverified`。

索引类命令不直接返回代码证据，而是返回 freshness 证据：索引对应的 HEAD、dirty 状态、
文件内容 hash、mtime、staged/working-tree 来源和过期原因。搜索命令如果使用索引，也必须在
响应中声明索引是否 freshness 通过；如果没有通过，必须回退到实时扫描或返回明确错误。

Watcher 类命令只返回 `worktree` overlay 的 freshness 和队列状态。watcher 结果不能提升准确性级别，
只能提升实时性。

## 必需 JSON 形态

```json
{
  "ok": true,
  "command": "callers",
  "reliability": {
    "level": "inferred_candidate",
    "source": "tree_sitter_ast_heuristic",
    "llmInstruction": "这些结果只能作为候选关系，不是完整调用图。推理前必须用 code-search read 验证每个匹配。",
    "accurateFor": [
      "直接函数调用",
      "同文件内本地调用",
      "简单 imported identifier"
    ],
    "notAccurateFor": [
      "动态分派",
      "接口实现",
      "反射",
      "框架注入",
      "宏生成代码",
      "大量别名 import"
    ]
  },
  "matches": []
}
```
