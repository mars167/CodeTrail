# 设计总览

## 阅读地图

```mermaid
flowchart LR
  S["00 设计总览"] --> A["01 架构"]
  S --> C["02 命令契约"]
  S --> Q["03 质量"]
  S --> B["04 Agent 评测"]
  A --> SRC["src/ 是实现细节权威"]
  C --> HELP["codetrail --help 是参数权威"]
  Q --> SCRIPTS["scripts/ 是门禁权威"]
```

| 文档 | 内容 |
| --- | --- |
| `00-design-summary.md` | 产品定位、系统图、可靠性等级 |
| `01-architecture.md` | snapshot/index/query/freshness 边界 |
| `02-command-contract.md` | 命令族、JSON 响应和 reliability 契约 |
| `03-quality.md` | 验证入口、门禁分层和 CI 映射 |
| `04-agent-benchmark.md` | Docker/OpenCode 评测结果和 Agent 使用建议 |

## 产品定位

CodeTrail 是本地优先的 SCIP/语义索引前端，目标是只在普通 bash 搜索难以可靠回答的场景里提供窄而可靠的代码导航证据：symbol、defs、precise refs 和调用链候选。

它提供：

- 定义、符号和 precise SCIP 引用。
- 调用与被调用候选，用于缩小阅读范围。
- `index build`、`index status` 和 `index doctor`，用于构建与诊断语义索引。
- 每个响应的 snapshot、freshness 与 reliability 信息。
- 一个刻意做小的 Agent Skill 路由卡，用于把 CodeTrail 限定在语义索引缺口。

它不承诺：

- 替代 `rg`、`fd`、`cat`、`git` 或宿主编辑器源码读取。
- 默认 embedding 或语义相似度搜索。
- 自主规划并完成具体开发任务。
- 提供 `brief`、`context`、`explore node` 或 `analyze-*` 这类任务级分析命令。
- 把文本匹配伪装成 precise semantic references。
- 把启发式调用图伪装成精确事实。
- 把源码、测试和脚本中已经明确表达的实现细节重复成第二份说明。

## 系统图

```mermaid
flowchart TB
  Actor["Developer / automation"] --> Entry["CLI / MCP"]
  Entry --> Query["Semantic query layer"]

  Git["Git HEAD / staged / worktree"] --> Snapshot["Snapshot and freshness model"]
  Snapshot --> Scip["SCIP occurrence facts"]
  Snapshot --> Parser["Tree-sitter parser facts"]
  Snapshot --> Graph["Call graph candidates"]

  Scip --> Query
  Parser --> Query
  Graph --> Query

  Query --> VerifyRange["source range targets for host verification"]
  Query --> Json["JSON response with reliability"]
```

索引是加速层，不是事实源。事实源始终是本地源码、Git 状态、文件 hash 和可读取的 range。

任务意图、查询顺序和停止条件属于 Agent 层。CodeTrail 的 CLI/MCP 只执行
可组合的语义索引、调用候选、索引构建和状态诊断原语；文本/路径搜索、目录浏览、
源码读取和 Git 工作流交给宿主编辑器或 Agent 工具。

## 可靠性

| level | 来源 | `exact` | 使用方式 |
| --- | --- | --- | --- |
| `source_fact` | Git、文本和路径匹配 | `true` | 可作为源码证据；编辑前仍用源码读取工具验证精确范围 |
| `precise_fact` | SCIP、语言服务或编译器索引 | `true` | 可作为 IDE 级跳转事实；仍保留 range verification |
| `parser_fact` | tree-sitter AST | `false` | 确定的语法事实，不等于语义精确引用 |
| `inferred_candidate` | 图、AST heuristic、search-based inference | `false` | 只用于缩小范围，必须二次验证 |
| `freshness` | manifest、hash、watcher、index status | `false` | 描述缓存状态，不提升代码事实准确性 |
| `remote_verified` | 与本地 file proof 对齐的 remote snapshot | `false` | 可作为加速结果；关键编辑仍用源码读取工具复核 |
| `remote_unverified` | 未能与本地文件对齐的 remote snapshot | `false` | 只能作为线索，不能直接用于编辑决策 |

## 贡献者参考

- 命令行参数以 `codetrail --help` 和 `src/cli.rs` 为准。
- 行为细节以 `src/`、`tests/` 和 `scripts/` 为准。
- 设计文档描述稳定边界和外部契约，避免重复每个函数的实现细节。
- 新增命令、索引或输出字段时，同时更新对应的测试和契约说明。
