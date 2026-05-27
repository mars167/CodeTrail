# Index 算法分析与 Rust 方案

> 当前设计准绳见 `docs/00-design-summary.md`。本文只展开旧实现分析和索引算法取舍。

## 旧实现现状

旧 `git-ai-cli` 的 index 流程可以概括为：

1. 读取 `.aiignore`、`.gitignore` 和 `.git-ai/include.txt`。
2. 用 glob 扫描支持的文件后缀。
3. 按语言打开 LanceDB 表。
4. 对每个文件做 tree-sitter 解析，抽取 symbols、refs、calls。
5. 对 symbol 摘要文本计算 hash embedding，再做 SQ8 量化。
6. 将 chunk/ref 写入 LanceDB，将 AST 关系写入 Cozo。
7. 写 `.git-ai/meta.json`。
8. hook 中执行 incremental staged index，再 pack 成 `.git-ai/lancedb.tar.gz` 并 `git add`。

这个方案的优点是功能面完整，能支持 semantic search、symbol search、graph query、pack/unpack。
但它和 `code-search` 的新定位有明显冲突：新产品要确定性、及时性和可验证证据，而不是语义检索引擎。

## 当前算法的主要问题

### 1. 索引承担了太多职责

旧 index 同时负责：

- 文件清单；
- symbol 表；
- embedding chunk；
- AST 图；
- 调用候选；
- archive 打包；
- hook 生命周期。

这些职责耦合在一次 index 里，导致任何小修改都可能触发过多写入和过多依赖。对新产品来说，
embedding 和 graph DB 都不是默认必需能力。

### 2. Freshness 校验不够细

旧 `checkIndex` 主要检查：

- `.git-ai/meta.json` 是否存在；
- schema version 是否匹配；
- LanceDB 目录和表是否存在；
- AST graph DB 是否存在；
- meta 中 commit hash 是否和 HEAD 一致。

这只能说明“索引大致属于某个 HEAD”，不能证明某个文件的缓存内容和当前 working tree 或 staged
snapshot 一致。对于 LLM 来说，这个粒度不够，会把过期 symbol 或调用关系当成事实。

### 3. Hook 会把大索引归档加入提交

旧 `pre-commit` 会执行 staged incremental index、pack LanceDB，然后 `git add .git-ai/meta.json`
和 `.git-ai/lancedb.tar.gz`。这适合共享语义索引，但对 `code-search` 不合适：

- 提交会携带大二进制 artifact；
- hook 成本高；
- LFS/pack/unpack 成为用户环境风险；
- 仍然不能保证 working tree 查询结果是最新事实。

### 4. 增量删除和写入成本偏高

旧 incremental index 会先删除 changed 文件的 refs 和 AST graph，再重新解析和写入。这个思路正确，
但底层存储是 LanceDB + Cozo，更新路径重，且需要跨语言表、chunk 表、refs 表、graph 表保持一致。

### 5. Worker 并行有价值，但数据边界不够适配新产品

旧 worker pool 的思路值得保留：主线程读文件，worker 做解析和 CPU 工作。但 worker 输出仍然包含
embedding chunk 和 graph 数据。Rust 版应该让 worker 输出更窄：file record、symbol record、
declaration record、relation candidate、warning。

## Rust 版更好的方案

### 总体方向

Rust 版建议采用“分层、可验证、多索引”的正确架构，而不是把所有能力塞进一个数据库：

```text
source snapshot
  -> file catalog
  -> text search index
  -> symbol / occurrence index
  -> code property graph
  -> freshness metadata
```

`source snapshot` 的输入来自 manual index、git hook 和 watcher。watcher 只产生 worktree overlay 变更，
不产生 commit/staged snapshot。

索引不是单一 artifact，而是一组围绕同一个 `snapshot_id` 的派生视图。所有查询结果都必须能回到
source snapshot、file hash 和 range。

## 正确存储结构

推荐存储结构：

```text
.code-search/
  snapshots/
    <snapshot_id>/
      manifest.json
      files.parquet
      blobs/
  text/
    <snapshot_id>/
      grams.idx
      docs.idx
      paths.idx
  scip/
    <snapshot_id>/
      index.scip
      occurrences.db
  graph/
    <snapshot_id>/
      kuzu/
      export/
        nodes.parquet
        edges.parquet
  working/
    manifest.json
  staged/
    manifest.json
```

职责拆分：

- `snapshots/`：文件事实和内容寻址，是一切索引的共同事实层。
- `text/`：n-gram/trigram 倒排索引，服务 literal、substring、regex、path 搜索。
- `scip/`：标准 code intelligence 事实，服务 defs、refs、hover、implementations。
- `graph/`：从 SCIP 和 parser facts 派生的 property graph，服务调用链、依赖、架构视图。
- `working/` 和 `staged/`：未提交状态的 overlay，不和 commit snapshot 混用。

## 为什么不能只用 JSONL

JSONL 适合作为 debug/export 格式，但不应该是目标架构的主存储：

- 大仓库 symbol lookup 需要索引结构；
- regex/substring 搜索需要 gram 倒排，不适合逐行扫 JSONL；
- 图遍历需要邻接结构、路径查询和事务；
- hook 增量更新需要 segment/transaction，而不是整文件重写。

JSONL 应保留为：

- `code-search index export`；
- snapshot test fixture；
- 人工排查格式；
- graph backend 迁移格式。

## 推荐核心算法

### 全量构建

```text
index build
  1. resolve repo root
  2. walk files with ignore crate
  3. sort paths lexicographically
  4. for each file:
     - stat size/mtime
     - compute blake3 content hash
     - infer language
     - parse supported language with tree-sitter
     - emit file record
     - update text gram index
     - emit SCIP occurrences when precise indexer is available
     - emit tree-sitter parser facts as fallback
     - derive graph nodes/edges
  5. write temp snapshot and index segments
  6. fsync important files
  7. atomic publish snapshot
```

关键点：

- 路径排序保证输出稳定。
- 所有 record 带 `snapshot_id`、`source = working_tree | staged | commit`。
- 每条 parser/code-intel record 带 `file_hash`。
- 写入必须先到 temp，再 atomic swap，避免中断后索引半损坏。

### 增量更新

```text
index update
  1. load manifest
  2. collect changed files:
     - git diff --name-status
     - git diff --cached --name-status for staged
     - optional mtime scan for untracked/dirty files
     - watcher normalized change set for worktree overlay
  3. remove old records for changed/deleted/renamed files
  4. parse changed files
  5. merge records into new segment
  6. compact when segment count exceeds threshold
```

正确实现应使用 segment：

```text
segments/
  000001.jsonl
  000002.jsonl
  tombstones.jsonl
```

segment 模式能避免每次 hook 重写大文件。

### Staged Snapshot

`pre-commit` 不能读取 working tree 文件来代表 staged 内容。必须通过 git 读取 staged blob：

```bash
git show :path/to/file
```

staged index 单独写到：

```text
.code-search/index/staged/
```

并在 `post-commit` 之后标记为对应 commit 的 verified snapshot。working tree index 和 staged index
不能混用。

## 推荐数据模型

### Snapshot

```json
{
  "snapshotId": "commit:abc123",
  "source": "commit",
  "repoRoot": "/repo",
  "head": "abc123",
  "dirty": false,
  "createdAt": "2026-05-26T00:00:00Z",
  "indexVersions": {
    "text": 1,
    "scip": 1,
    "graph": 1
  }
}
```

### FileRecord

```json
{
  "path": "src/main.rs",
  "language": "rust",
  "snapshotId": "commit:abc123",
  "source": "commit",
  "size": 1234,
  "mtimeMs": 1760000000000,
  "hash": "blake3:...",
  "indexedAt": "2026-05-26T00:00:00Z"
}
```

### SymbolRecord

优先来自 SCIP occurrence；没有 precise index 时才来自 tree-sitter fallback。

```json
{
  "path": "src/main.rs",
  "snapshotId": "commit:abc123",
  "fileHash": "blake3:...",
  "language": "rust",
  "producer": "scip-rust-analyzer",
  "name": "search_files",
  "kind": "function",
  "declarationRange": { "start": { "line": 10, "column": 1 }, "end": { "line": 10, "column": 38 } },
  "bodyRange": { "start": { "line": 10, "column": 39 }, "end": { "line": 42, "column": 1 } },
  "reliability": "parser_fact"
}
```

### RelationRecord

```json
{
  "path": "src/main.rs",
  "snapshotId": "commit:abc123",
  "fileHash": "blake3:...",
  "fromSymbol": "search_files",
  "relation": "calls",
  "targetName": "walk",
  "range": { "start": { "line": 20, "column": 8 }, "end": { "line": 20, "column": 12 } },
  "reliability": "inferred_candidate"
}
```

## Rust 技术栈建议

| 能力 | 推荐 |
| --- | --- |
| CLI | `clap` |
| 文件遍历 | `ignore` |
| 内容 hash | `blake3` |
| 文本索引 | 自研 n-gram/trigram segment，底层可用 `fst`、`roaring` 或 bitset |
| 精确代码智能 | SCIP protobuf，`prost` 生成绑定 |
| 图数据库 | KuzuDB 作为 embedded property graph；保留 Neo4j/FalkorDB 导出适配 |
| 并行 | `rayon` 或 tokio worker pool |
| JSON/导出 | `serde`, `serde_json` |
| 原子写入 | temp dir + `std::fs::rename` |
| parser fallback | `tree-sitter` |
| 错误 | `thiserror`, `anyhow` |
| 快照测试 | `insta` |

并行建议：

- 文件遍历单线程收集并排序；
- 解析阶段用 `rayon::par_iter`；
- 写入阶段单线程稳定排序后输出；
- 不在 worker 内直接写文件，避免顺序不稳定和锁竞争。

## 是否需要数据库

需要，但不是一个数据库解决所有问题。

- 文本搜索不应该进 graph DB。它需要 code-search 专用 n-gram/trigram 倒排。
- 精确导航不应该从 tree-sitter 猜。它应该优先吃 SCIP/语言服务产物。
- 图查询应该进入 property graph。它面向结构关系和多跳遍历，而不是全文搜索。

推荐默认：

- `text index`：Rust 原生 gram index。
- `symbol/code-intel index`：SCIP + 本地 occurrence store。
- `graph index`：KuzuDB embedded property graph。
- `debug/export`：JSONL/Parquet。

最终原则保持不变：索引只提升速度和结构化检索能力，不自动提升真实性。真实性来自 source snapshot、
precise producer、file hash、range 和 reliability 标注。
