# 正确目标架构：Code Search + Code Graph

> 当前设计准绳见 `docs/00-design-summary.md`。本文只展开参考架构和竞品对比。

## 结论

`code-search` 的目标是让 Agent 像使用 IDEA/JetBrains IDE 一样高效获取代码信息：
快速搜索、精确跳转、读取上下文、理解影响面。它不应该只是一个 grep 包装，也不应该只是一个 tree-sitter graph。
正确架构应该是：

```text
Source Snapshot
  -> Text Search Index
  -> SCIP / Symbol Occurrence Index
  -> Code Property Graph
  -> Local Query API / CLI / MCP
  -> Optional Remote Index / Graph
```

这和成熟系统的分层一致：

- GitHub Code Search / Blackbird：为代码搜索自研 Rust 搜索引擎，核心是 code-specific n-gram/regex 检索。
- Sourcegraph：文本搜索用 Zoekt trigram index；精确代码导航用 SCIP/precise index；没有精确索引时才回退到 search-based navigation。
- CodeGraphContext：使用 tree-sitter + SCIP ingestion 构建 graph，并支持 KuzuDB/FalkorDB/Neo4j 等图数据库后端。
- Glean：把代码事实建模为 typed, schema-defined facts，供 IDE/工具查询。

因此，正确方向不是“是否使用 graph DB”，而是明确不同索引层解决不同问题。

## 顶层原则

- Local first：本地仓库、本地索引、本地查询必须完整可用；远程不可用时不影响单仓使用。
- Git first：所有索引和查询都绑定 `commit`、`staged` 或 `worktree` snapshot。
- Remote 可用：远程服务用于团队共享、跨仓搜索和大仓加速，但返回结果必须能落回本地 snapshot/range 验证。
- 高效：Agent 查询应避免反复 grep/glob/read；常见 search/jump/context 查询应命中索引或 precise occurrence store。
- 准确：能由源码、SCIP、语言服务或编译器索引证明的结果才可标记为 exact；启发式图边永远只能是候选。

## 参考系统怎么做

### GitHub Code Search / Blackbird

公开资料显示，GitHub 为新 code search 从零构建了 Rust 搜索引擎 Blackbird。核心原因是通用搜索引擎不适合代码：

- 代码搜索需要搜索标点符号；
- 不需要 stemming；
- 不应该移除 stop words；
- 需要支持 regex；
- 需要在大规模、持续变化的代码 corpus 上保持索引一致性。

对 `code-search` 的启发：

- 文本搜索层应该是专用 code-search index，不应该交给通用全文搜索或 graph DB。
- regex 应该先转为 gram 候选，再读取源码做 verification。
- 搜索结果必须绑定 repo snapshot，避免查询跨越半更新状态。

### Sourcegraph / Zoekt / SCIP

Sourcegraph 的公开架构把 code search 和 code navigation 分开：

- code search：Zoekt 为默认分支建立 trigram index，未索引代码走非索引 searcher。
- code navigation：默认有 search-based navigation，但会产生 false positive/false negative；需要更准时，用 precise code navigation。
- precise code navigation：通过 SCIP 这类语言无关 index 格式，由 build/indexer 产出。

对 `code-search` 的启发：

- `find/files/regex` 属于 text index。
- `defs/refs/symbols` 应优先来自 SCIP 或语言服务产物。
- tree-sitter 可以做 parser fallback，但不能伪装成 precise semantic reference resolution。
- 查询结果要标注 `source = precise | syntactic | search_based | inferred`。

### CodeGraphContext

CodeGraphContext 的公开文档定位是将代码转换为 queryable property graph。其架构包含：

- tree-sitter 和 SCIP ingestion；
- graph builder/linker；
- KuzuDB、FalkorDB、Neo4j 等 pluggable graph backend；
- CLI 和 MCP server；
- 文件系统 watcher 增量更新；
- `.cgc` 便携 bundle。

对 `code-search` 的启发：

- graph 是必要层，但应该服务结构遍历，不应该替代文本搜索。
- 图后端应该可插拔，embedded backend 作为默认，remote/Neo4j 作为可选。
- 文件变更监听和 git hook 都应该进入 index scheduler，而不是散落在命令里。
- 便携 bundle 可以保留，但应绑定 snapshot 和 schema version。

### Glean

Glean 的核心思路是 typed, schema-defined facts：indexer 分析代码后产出 facts，由查询层使用。

对 `code-search` 的启发：

- 不要把 graph schema 做成随意 JSON。
- 所有节点和边都应该有 schema、版本、producer、source snapshot。
- 派生事实必须能追溯到原始 occurrence 或 parser range。

### GitNexus

GitNexus 的公开定位是本地/浏览器两种使用方式：CLI + MCP 面向日常开发，Web UI 面向图浏览和聊天。
它的架构重点是 index -> graph -> MCP flow：

- CLI 侧通过 `analyze` 运行 ingestion pipeline。
- 12 个 pipeline phase 构建内存 `KnowledgeGraph`，再加载到 LadybugDB。
- 图数据保存在 `.gitnexus/`，全局 repo registry 放在 `~/.gitnexus/registry.json`。
- 查询层同时暴露 MCP、HTTP bridge 和 CLI direct。
- MCP 工具包括 `query`、`context`、`impact`、`detect_changes`、`rename`、`route_map`、`tool_map` 等。
- ingestion DAG 包含 scan、structure、markdown、parse、routes、tools、orm、crossFile、mro、communities、processes。
- call resolution 有多阶段 DAG，并按语言 provider 插入 import、MRO、dispatch 等行为。
- 新的 scope-resolution pipeline 正在把部分语言迁到 registry-primary resolver，强调统一 graph schema 和同图保证。

对 `code-search` 的启发：

- ingestion pipeline 要显式 DAG 化，phase 输出和依赖要有类型约束。
- Graph schema 不能只停在 File/Symbol/CALLS，应该包含 Route、Tool、Process、Community 等高层节点，但每个高层节点都必须保留 provenance。
- Call resolution 可以保留多层置信度，但要把 confidence tier 暴露给 LLM，而不是包装成准确调用图。
- 多 repo / group 模式很重要，但应通过 contract registry 和跨 repo bridge 实现，不应该把所有 repo 粗暴合成一个无边界大图。
- Agent 集成不只是 MCP server，还包括 skills、hooks、staleness hints 和 context augmentation。

需要避免的点：

- GitNexus 包含 embeddings/hybrid search。`code-search` 的默认路径仍应保持 deterministic-first，embedding 只能作为显式 opt-in layer。
- 自动生成 AGENTS/CLAUDE 上下文文件要谨慎，避免污染用户仓库；更适合作为显式 `setup agent` 命令。

### CodeGraph

CodeGraph 的定位是“pre-indexed knowledge graph for agents”：让 Claude Code、Codex、Cursor、OpenCode 等直接查图，
减少 grep/glob/read 探索。它的公开实现和 README 显示：

- 本地 SQLite 数据库 `.codegraph/codegraph.db`。
- FTS5 做 full-text search。
- tree-sitter 解析 20+ 语言。
- 图中包含 symbols、edges、files 和 unresolved refs。
- MCP 工具包括 search、context、callers、callees、impact、node、status、files。
- watcher 使用原生 OS events，debounce 后增量同步。
- framework-aware routes 将 URL pattern 连到 handler。
- 对 React Native / Expo / Swift-ObjC 等跨语言边界，显式合成 bridge edges。
- 合成边带 `provenance:'heuristic'` 和 `metadata.synthesizedBy`，让 Agent 知道关系来源。

对 `code-search` 的启发：

- SQLite/FTS5 是一个实用 local-first baseline，尤其适合单仓本地工具。
- watcher 应该作为 daemon/serve 模式的一部分，和 git hook 一起进入统一 scheduler。
- route/tool/framework-specific extraction 是代码图价值的关键，不能只做语言 AST。
- cross-language bridge 很重要，但必须像 CodeGraph 一样把 provenance 和 synthesized channel 写入边。
- Agent 指令需要强约束工具选择，避免 LLM 已有图还反复 grep/read。

需要避免的点：

- CodeGraph 文档里对图结果使用“authoritative source”式表述。`code-search` 不应这么承诺。
  对 L1/L2 结果仍要要求 range verification，尤其是 heuristic/provenance 边。
- SQLite + FTS5 可以作为 backend 选项，但目标架构仍应拆分 text index、symbol occurrence index 和 graph store，
  避免一个 SQLite schema 承担所有长期演进职责。

## 新增参考对比矩阵

| 系统 | 文本搜索 | Symbol/Defs | Graph | Freshness | Agent 集成 | 对 code-search 的取舍 |
| --- | --- | --- | --- | --- | --- | --- |
| GitHub Code Search | Rust code-specific n-gram/regex search | 不是重点 | 不是重点 | 大规模索引系统 | GitHub UI/API | 学 text index，不学 graph |
| Sourcegraph | Zoekt trigram | SCIP/precise navigation | code intel backend | indexed + searcher fallback | Web/API | 学 search/navigation 分层 |
| CodeGraphContext | 不是重点 | tree-sitter + SCIP ingestion | Kuzu/FalkorDB/Neo4j | watcher/ingestion | CLI/MCP | 学 pluggable graph backend |
| Glean | 非核心 | typed facts | facts query | build/indexer 产物 | IDE/tools | 学 schema-defined facts |
| GitNexus | BM25 + vector hybrid | tree-sitter/provider pipeline | LadybugDB graph | staleness hints, registry | MCP/HTTP/skills/hooks | 学 DAG ingestion 和 agent ops；默认不学 embedding |
| CodeGraph | SQLite FTS5 | tree-sitter extraction | SQLite graph tables | OS watcher debounce sync | MCP + installer + instructions | 学 local-first graph + watcher + provenance；不学“authoritative”表述 |

## 目标架构

```text
                         +----------------------+
Git HEAD / Staged / WT ->| Snapshot Builder     |
                         +----------+-----------+
                                    |
                  +-----------------+------------------+
                  |                 |                  |
          +-------v------+  +-------v-------+  +-------v-------+
          | Text Index   |  | SCIP / Symbol |  | Parser Facts  |
          | n-gram       |  | Occurrences   |  | tree-sitter   |
          +-------+------+  +-------+-------+  +-------+-------+
                  |                 |                  |
                  +-----------------+------------------+
                                    |
                         +----------v-----------+
                         | Graph Builder        |
                         | nodes / edges / ids  |
                         +----------+-----------+
                                    |
                         +----------v-----------+
                         | Property Graph Store |
                         | Kuzu default         |
                         +----------+-----------+
                                    |
                         +----------v-----------+
                         | Query Layer          |
                         | CLI / MCP / JSON     |
                         +----------------------+
```

Watcher 是 `WT -> Snapshot Builder` 的实时输入之一，只产生 worktree overlay。

## Snapshot 层

Snapshot 是唯一事实入口。任何索引都不能直接代表事实。

Snapshot 类型：

- `commit:<sha>`：提交快照。
- `staged:<tree_hash>`：暂存区快照。
- `worktree:<hash>`：工作区 overlay。

Remote index 也必须声明自己对应的 snapshot。远程结果如果不能和本地 `snapshot_id` 或 file hash 对齐，
查询层必须标注 `remote_unverified`，不能作为准确跳转结果。

每个索引记录必须包含：

- `snapshot_id`
- `path`
- `file_hash`
- `range`
- `producer`
- `reliability`

## Text Search Index

用途：

- literal search；
- substring search；
- regex prefilter；
- path search；
- changed files 中的文本搜索。

设计：

- Rust 自研 n-gram/trigram segment。
- doc table 保存 path、language、line offsets、file hash。
- gram postings 指向 doc ids 和可选位置。
- regex 查询先提取必要 gram，再候选验证。
- 最终结果必须读取 snapshot blob 或当前文件验证 range。

不要把全文搜索放进 graph DB。

## SCIP / Symbol Occurrence Index

用途：

- definitions；
- references；
- implementations；
- hover；
- symbol search。

设计：

- 支持读取 `index.scip`。
- 对 Rust 优先接入 rust-analyzer/scip-rust。
- 对 TypeScript/JavaScript 优先接入 scip-typescript。
- 对没有 precise indexer 的语言使用 tree-sitter fallback。
- fallback 结果必须标注为 `syntactic_parser_fact`，不能标注为 precise。

## Code Property Graph

用途：

- imports；
- defines；
- contains；
- calls candidate；
- inherits/implements；
- module dependency；
- impact traversal；
- architecture slice。

节点：

- `Repository`
- `Snapshot`
- `File`
- `Module`
- `Symbol`
- `Occurrence`
- `Diagnostic`

边：

- `CONTAINS`
- `DEFINES`
- `REFERENCES`
- `IMPORTS`
- `CALLS_CANDIDATE`
- `IMPLEMENTS`
- `EXTENDS`
- `GENERATED_FROM`

可靠性：

- `DEFINES` 来自 SCIP：`precise`。
- `DEFINES` 来自 tree-sitter：`parser_fact`。
- `CALLS_CANDIDATE` 来自 tree-sitter：`inferred_candidate`。
- search-based refs：`search_based_candidate`。

## Graph Store 选择

目标架构建议默认使用 KuzuDB：

- embedded；
- property graph；
- 支持 Cypher 风格查询；
- 适合本地 code graph；
- 可导出到 Parquet/CSV 供迁移或调试。

同时保留 backend trait：

```text
GraphStore
  - apply_snapshot(snapshot)
  - upsert_nodes(nodes)
  - upsert_edges(edges)
  - traverse(query)
  - export()
```

可选后端：

- KuzuDB：默认 embedded。
- Neo4j：企业/可视化/远程共享。
- FalkorDB：低延迟 remote 或 embedded 场景。
- JSONL/Parquet：导出和测试，不作为主后端。

## Index Scheduler

所有 index 更新都进入 scheduler：

```text
git hook / file watcher / manual command
  -> detect changed snapshot
  -> enqueue indexing job
  -> parse / precise index / text index
  -> graph build
  -> atomic publish
```

Hook 行为：

- `pre-commit`：构建 staged snapshot。
- `post-commit`：promote staged snapshot 为 commit snapshot。
- `post-checkout`：切换 snapshot。
- `post-merge`：增量重建受影响文件。
- `post-rewrite`：重写 snapshot lineage。

文件 watcher 用于 long-running daemon，不替代 git hook。

Watcher 细节见 `docs/11-watcher-design.md`。

## 查询层

命令不直接读数据库细节，而是走 query service：

```text
FindQuery -> TextIndex
DefsQuery -> SCIPIndex -> ParserFallback
RefsQuery -> SCIPIndex -> TextFallback
CallsQuery -> GraphStore
ReadQuery -> SnapshotStore
```

每个响应必须包含：

- 使用了哪个 index；
- index 是否 fresh；
- producer；
- reliability；
- verification range；
- fallback reason。

## Remote 模式

Remote 是可用能力，不是默认事实源。

Remote 适用场景：

- 团队共享已构建的大仓索引；
- 跨 repo / group 查询；
- CI 产出的 precise SCIP index；
- 远程 graph 可视化和协作浏览。

Remote 约束：

- 必须返回 `snapshot_id`、`repo_id`、`file_hash`、`range`、`producer`。
- 本地仓库存在时，必须尽量用本地文件验证 range。
- 本地验证失败时，结果降级为 `remote_unverified`。
- Remote 不允许覆盖 local dirty/staged 状态。

## 对当前文档的修正

此前“JSONL + manifest 足够”的说法是错误架构，不应出现在可验收方案中。目标架构必须直接按多索引系统设计：

- text index 类似 Blackbird/Zoekt；
- precise code intelligence 类似 SCIP；
- graph layer 类似 CodeGraphContext；
- local-first graph 和 Agent 工具参考 GitNexus / CodeGraph；
- fact schema 思路参考 Glean；
- tree-sitter 是 parser fallback 和 graph enrichment，不是唯一真相。

## 外部参考

- GitHub Code Search / Blackbird: https://github.blog/engineering/architecture-optimization/the-technology-behind-githubs-new-code-search/
- Sourcegraph Architecture: https://sourcegraph.com/docs/admin/architecture
- SCIP: https://scip-code.org/
- Sourcegraph Indexers: https://sourcegraph.com/docs/code-navigation/writing-an-indexer
- CodeGraphContext: https://codegraphcontext.github.io/
- CodeGraphContext Backends: https://codegraphcontext.github.io/concepts/backends/
- Glean: https://glean.software/
- GitNexus: https://github.com/abhigyanpatwari/GitNexus
- CodeGraph: https://github.com/colbymchenry/codegraph
