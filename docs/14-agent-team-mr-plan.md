# Agent Team MR 执行计划

> 当前设计准绳见 `docs/00-design-summary.md`。本文把 `docs/13-implementation-tasks.md` 的剩余工作拆成可并行管理的 MR 队列。

## 当前基线

- 基线分支：`main`
- 当前基线提交：`7948695 Implement target text index storage`
- 已完成：CLI 命令面、统一 JSON 可靠性契约、L0 源码事实命令、tree-sitter parser fallback、关系候选、hook/status 入口、shell completions、目标 text index 第一片。
- 已落地存储：`.code-search/snapshots/<snapshot>/manifest.json`、`.code-search/text/<snapshot>/{docs.idx,paths.idx,grams.idx}`、`.code-search/working/manifest.json`、`.code-search/staged/manifest.json`、`scip/<snapshot>/occurrences.idx`。
- 仍未完成：`files.parquet + blobs/`、完整 path/regex/line-offset text index、增量 segment、native `index.scip + occurrences.db`、Kuzu graph、真实 watcher/serve、MCP adapter。

## Team 角色

| 角色 | 代号 | 职责 | 不能做什么 |
| --- | --- | --- | --- |
| 统筹管理角色 | `team-lead` | 维护 MR 队列、分支顺序、worktree 分配、依赖解锁、每日合并窗口；所有 MR 从 `origin/main` 或已合并前置 MR 创建。 | 不直接把未验收 MR 合并进 `main`。 |
| 验收角色 | `acceptance-reviewer` | 独立 worktree 验收每个 MR，运行测试，检查 docs 设计约束和 JSONL 禁令，给出 merge/no-merge 结论。 | 不替 implementation owner 修代码；发现问题退回对应 MR。 |
| Snapshot Agent | `agent-snapshot` | Source Snapshot、`files.parquet`、`blobs/`、freshness 事实层。 | 不修改 graph 或 MCP。 |
| Text Agent | `agent-text` | `docs.idx`、`paths.idx`、`grams.idx`、path/regex/line-offset 查询。 | 不引入 JSONL 或 graph DB。 |
| Scheduler Agent | `agent-scheduler` | `IndexScheduler`、hook promotion、增量 segment、atomic publish。 | 不改变 query schema 语义。 |
| SCIP Agent | `agent-scip` | native `index.scip` protobuf、`occurrences.db`、precise defs/refs/symbols。 | 不把 tree-sitter fallback 标为 precise。 |
| Graph Agent | `agent-graph` | Kuzu backend、graph schema、calls/callers graph 查询。 | 不恢复 JSONL relation store。 |
| Watch/Serve Agent | `agent-watch` | watcher overlay、serve query service、状态输出。 | 不替代 git hook，不修改 staged。 |
| MCP Agent | `agent-mcp` | MCP adapter、schema compatibility、Agent integration docs。 | 不直接绕过 CLI/query service。 |

## 执行规则

1. 每个 MR 使用独立 worktree，目录统一放在 `/Users/mars/dev/git-ai-workspace/worktrees/code-search-cli/<worktree-name>`。
2. 每个 MR 分支从 `origin/main` 创建；如果有前置依赖，必须等前置 MR 合并后再 rebase 到最新 `origin/main`。
3. 每个 MR 只修改自己负责的模块和直接相关测试/文档，避免跨 MR 抢同一边界。
4. 主查询路径不得读写 JSON/JSONL。JSON/JSONL 只允许出现在显式 export、测试 fixture 或人工排查工具中。
5. 所有索引结果必须保留 snapshot/file hash/range freshness 证据；缓存不能成为事实源。
6. 每个 MR 完成前至少运行 `cargo fmt`、`cargo test`、`cargo check`、`git diff --check`。
7. 验收角色必须额外运行与该 MR 直接相关的 CLI 端到端命令，并检查输出 `index.source`、`reliability`、`exact` 是否符合设计。

## MR 队列总览

| 顺序 | MR | 状态 | Worktree 名称 | 分支名称 | Owner | Reviewer | 依赖 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | MR-01 Source Snapshot Store | Ready | `mr01-snapshot-store` | `feat/mr01-snapshot-store` | `agent-snapshot` | `acceptance-reviewer` | 无 |
| 2 | MR-02 Complete Text Index | Ready after MR-01 | `mr02-text-index` | `feat/mr02-text-index` | `agent-text` | `acceptance-reviewer` | MR-01 |
| 3 | MR-03 Scheduler and Hook Lifecycle | Ready after MR-01 | `mr03-scheduler-hooks` | `feat/mr03-scheduler-hooks` | `agent-scheduler` | `acceptance-reviewer` | MR-01 |
| 4 | MR-04 Native SCIP Occurrence Store | Can start in parallel, merge after MR-01 | `mr04-native-scip` | `feat/mr04-native-scip` | `agent-scip` | `acceptance-reviewer` | MR-01 |
| 5 | MR-05 Kuzu Graph Backend | Ready after MR-04 | `mr05-kuzu-graph` | `feat/mr05-kuzu-graph` | `agent-graph` | `acceptance-reviewer` | MR-01, MR-04 |
| 6 | MR-06 Watcher Overlay and Serve | Ready after MR-03 | `mr06-watch-serve` | `feat/mr06-watch-serve` | `agent-watch` | `acceptance-reviewer` | MR-03 |
| 7 | MR-07 Query Service and MCP Adapter | Ready after MR-02/MR-04 | `mr07-query-mcp` | `feat/mr07-query-mcp` | `agent-mcp` | `acceptance-reviewer` | MR-02, MR-04 |
| 8 | MR-08 Remote/Pack Mode | Deferred | `mr08-remote-pack` | `feat/mr08-remote-pack` | `agent-mcp` | `acceptance-reviewer` | MR-07 |

## MR-01 Source Snapshot Store

- 执行顺序：第 1 个 MR，所有后续存储 MR 的基础。
- Worktree：`/Users/mars/dev/git-ai-workspace/worktrees/code-search-cli/mr01-snapshot-store`
- 分支：`feat/mr01-snapshot-store`
- Owner：`agent-snapshot`
- 任务目的：把 `snapshots/<snapshot>/` 从只有 `manifest.json` 的状态补成事实层，提供 `files.parquet` 和 content-addressed `blobs/`，让 text/scip/graph 都从同一 snapshot 派生。
- 设计方案：
  - 新增 `src/snapshot_store.rs` 或 `src/index/snapshot.rs`，封装 snapshot publish/read/verify。
  - `index build` 先写 temp snapshot，再写派生 text index，最后 atomic publish。
  - `files.parquet` 保存 path、language、size、mtime、hash、ignore/source 状态；`blobs/` 按 blake3 内容寻址保存可验证内容或 staged blob。
  - `index status/verify` 从 snapshot fact 层读取文件事实，再校验当前 worktree 或 staged blob。
  - 当前 `docs.idx` 中的文件事实保留为 text index doc table，但不再作为唯一 file catalog。
- 验收标准：
  - `code-search index build` 创建 `snapshots/<snapshot>/files.parquet` 和 `snapshots/<snapshot>/blobs/`。
  - `code-search index build --staged` 通过 `git show :path` 写 staged snapshot，不读取 working tree 伪装 staged 内容。
  - 修改文件后 `code-search index verify` 返回 code 6，并指出 stale/missing path。
  - `cargo test` 覆盖 working tree、staged、missing file、hash mismatch。
  - `rg -n "files.jsonl|relations.jsonl|occurrences.jsonl" src` 没有主路径残留。

## MR-02 Complete Text Index

- 执行顺序：第 2 个 MR，可在 MR-01 合并后开始。
- Worktree：`/Users/mars/dev/git-ai-workspace/worktrees/code-search-cli/mr02-text-index`
- 分支：`feat/mr02-text-index`
- Owner：`agent-text`
- 任务目的：把 text index 从 literal gram candidate 扩展到完整 path search、regex prefilter、line-offset doc table，并减少 live scan 依赖。
- 设计方案：
  - 扩展 `src/text_index.rs`：`docs.idx` 保存 line offsets，`paths.idx` 支持 path substring/prefix/glob 候选，`grams.idx` 支持必要 gram 提取。
  - `files/find-path/glob` 优先读 `paths.idx`；`grep --mode regex` 从 regex 提取必要 gram 后做候选预过滤。
  - 查询最终仍读取 snapshot blob 或 live file 验证 range，不能把 index hit 直接当事实。
  - 为大型 postings 加 seek/table-of-contents，避免每次查询线性读取整个 `grams.idx`。
- 验收标准：
  - `files`、`find-path`、`glob` 输出 producer 为 text/path index 相关 producer，index fresh 时 `index.used=true`。
  - regex 查询能在可提取 gram 时输出 `prefilter=trigram_regex`，无法提取时明确 fallback reason。
  - range 计算使用 line-offset table，结果与 live scan 一致。
  - 新增测试覆盖 path index、regex prefilter、短 pattern fallback、stale index fallback。

## MR-03 Scheduler and Hook Lifecycle

- 执行顺序：第 3 个 MR，可在 MR-01 合并后与 MR-02 并行开发，但合并时需避开 `src/index.rs` 冲突。
- Worktree：`/Users/mars/dev/git-ai-workspace/worktrees/code-search-cli/mr03-scheduler-hooks`
- 分支：`feat/mr03-scheduler-hooks`
- Owner：`agent-scheduler`
- 任务目的：实现统一 `IndexScheduler`、增量 update、staged/commit/worktree promotion，避免每次 hook 都全量重建。
- 设计方案：
  - 新增 `src/scheduler/`：jobs、change set、publish、compaction。
  - `index update` 根据 `git diff --name-status`、`git diff --cached --name-status`、mtime scan 生成 change set。
  - text/snapshot/scip/graph 后端都通过 segment delta 接收变更，删除/rename 写 tombstone。
  - hook 行为按 docs：`pre-commit` 构建 staged，`post-commit` promote，checkout/merge/rewrite 校验 lineage。
- 验收标准：
  - `index update` 对单文件修改只更新对应 segment，测试中能观察 changed path 数量。
  - `post-commit` 能把 staged manifest promote 到 commit snapshot，不混用 worktree snapshot。
  - 中断或失败不会留下半发布目录；temp 目录可清理或被下一次 build 覆盖。
  - `index status` 输出 scheduler health、last job、stale reason。

## MR-04 Native SCIP Occurrence Store

- 执行顺序：第 4 个 MR，可在 MR-01 开发期间预研，正式合并在 MR-01 后。
- Worktree：`/Users/mars/dev/git-ai-workspace/worktrees/code-search-cli/mr04-native-scip`
- 分支：`feat/mr04-native-scip`
- Owner：`agent-scip`
- 任务目的：从兼容 SCIP JSON 导入升级为 native `index.scip` protobuf + `occurrences.db`，支撑 IDE 级 defs/refs/symbols。
- 设计方案：
  - 增加 SCIP protobuf parser，读取标准 `index.scip`。
  - 新建 `scip/<snapshot>/occurrences.db`，保存 symbol、role、range、file hash、language、kind、snapshot。
  - `defs/refs/symbols` 优先读 occurrence DB，freshness 不通过时退回 tree-sitter 或 identifier text search。
  - 保留 `import-scip <index.scip.json>` 仅作为兼容/debug 输入，但内部仍写 occurrence DB 或明确非主路径。
- 验收标准：
  - fixture binary `index.scip` 可驱动 `defs`、`refs`、`symbols`，返回 `reliability.level=precise_fact`。
  - 文件 hash 不匹配时 precise 查询自动 fallback，不能返回过期 precise 结果。
  - tree-sitter fallback 仍为 parser/source fact，不得标成 precise。
  - `scip/<snapshot>/occurrences.db` 存在；主查询不读取 `occurrences.idx` 或 JSONL。

## MR-05 Kuzu Graph Backend

- 执行顺序：第 5 个 MR，在 MR-04 合并后开始。
- Worktree：`/Users/mars/dev/git-ai-workspace/worktrees/code-search-cli/mr05-kuzu-graph`
- 分支：`feat/mr05-kuzu-graph`
- Owner：`agent-graph`
- 任务目的：实现 `graph/<snapshot>/kuzu/` 默认 embedded property graph，使 `calls/callers` 从真实 graph backend 查询，不恢复 JSONL relation store。
- 设计方案：
  - 新增 `src/graph/` backend trait：`apply_snapshot`、`upsert_nodes`、`upsert_edges`、`traverse`、`export`。
  - Kuzu schema 包含 Repository、Snapshot、File、Module、Symbol、Occurrence、Diagnostic 节点和 CONTAINS/DEFINES/REFERENCES/IMPORTS/CALLS_CANDIDATE 等边。
  - Graph builder 从 SCIP occurrence DB 和 tree-sitter parser facts 派生节点/边；每条边保留 reliability。
  - `calls/callers` 在 graph fresh 时输出 graph producer 和 `index.used=true`，否则保持 tree-sitter heuristic fallback。
- 验收标准：
  - `index build` 创建 `graph/<snapshot>/kuzu/`。
  - `calls`、`callers` 使用 Kuzu 后仍返回 `reliability.level=inferred_candidate`，不声称 exact。
  - 删除或修改源文件后 graph freshness 失效并 fallback。
  - 不存在 `relations.jsonl` 主路径。

## MR-06 Watcher Overlay and Serve

- 执行顺序：第 6 个 MR，在 MR-03 合并后开始。
- Worktree：`/Users/mars/dev/git-ai-workspace/worktrees/code-search-cli/mr06-watch-serve`
- 分支：`feat/mr06-watch-serve`
- Owner：`agent-watch`
- 任务目的：把当前 status-only watch/serve 变成真实 watcher overlay 和本地 query service。
- 设计方案：
  - 新增 `src/watcher/`：events、debounce、filters、reconcile、status。
  - watcher 只产生 normalized change set 并交给 `IndexScheduler`，只维护 worktree overlay。
  - `serve` 启动 query service，默认带 watcher；`serve --no-watch` 只提供查询。
  - overflow、rename 不完整、backlog 过长时标记 stale 并触发 reconcile。
- 验收标准：
  - `watch --once` 做一次 reconcile 后退出，并更新 worktree overlay。
  - `watch --status` 输出 running、queueLength、stale、lastEventAt、lastReconcileAt。
  - watcher 不执行 `git add`，不修改 staged，不生成 commit snapshot。
  - 端到端测试证明文件修改后 `find` 使用更新后的 overlay 或明确 fallback。

## MR-07 Query Service and MCP Adapter

- 执行顺序：第 7 个 MR，在 MR-02 和 MR-04 后开始；可与 MR-05/MR-06 后半段并行，但最终需 rebase。
- Worktree：`/Users/mars/dev/git-ai-workspace/worktrees/code-search-cli/mr07-query-mcp`
- 分支：`feat/mr07-query-mcp`
- Owner：`agent-mcp`
- 任务目的：把 CLI command service 抽出为稳定 query service，并提供 MCP adapter 给 Agent 使用。
- 设计方案：
  - 抽出 `src/query/`，CLI 和 MCP 共用同一 schema、reliability、freshness 逻辑。
  - MCP tools 对应 `find/files/read/defs/refs/symbols/calls/callers/changed/status`。
  - MCP adapter 不绕过本地 snapshot/freshness 校验，不读取后端私有文件。
  - 文档补充 Agent 使用规则：L2 候选必须 `read` 验证。
- 验收标准：
  - CLI 既有测试全部通过，MCP adapter 有 schema/contract 测试。
  - 同一查询通过 CLI 和 MCP 返回等价 canonicalCommand、reliability、index metadata。
  - MCP 工具在索引 stale 时与 CLI 一样 fallback。

## MR-08 Remote/Pack Mode

- 执行顺序：第 8 个 MR，当前 deferred，只有本地 dirty/staged 语义稳定后才启动。
- Worktree：`/Users/mars/dev/git-ai-workspace/worktrees/code-search-cli/mr08-remote-pack`
- 分支：`feat/mr08-remote-pack`
- Owner：`agent-mcp`
- 任务目的：设计并实现 remote index/graph artifact pack/unpack，不让 remote 覆盖本地状态。
- 设计方案：
  - `index pack` 导出 snapshot-scoped artifact，包含 manifest、checksums、schema version。
  - `index unpack` 只接受 snapshot/hash 可验证 artifact。
  - remote query 结果如果不能和本地 snapshot/file hash 对齐，必须标注 `remote_unverified`。
- 验收标准：
  - pack/unpack 不改变 working/staged 状态。
  - remote mismatch 时不返回 precise 结果。
  - 所有 remote 输出保留 provenance 和 verification status。

## 统筹管理流程

1. `team-lead` 每轮只放行最多 3 个 active MR：MR-01 必须先行；MR-02/MR-03/MR-04 可在 MR-01 后并行；MR-05/MR-06/MR-07 等依赖满足后启动。
2. 每个 owner 创建 worktree 后，在 MR 描述中贴出：base commit、worktree path、branch、影响文件、测试命令、风险点。
3. 每个 MR 合并前由 `acceptance-reviewer` 在独立验收 worktree 执行验收，不使用 owner 的未清理本地状态。
4. 合并顺序由依赖决定；如果两个 MR 同时改 `src/index.rs` 或输出 schema，后合并者必须 rebase 并重新跑全量测试。
5. 每次 MR 合并后，`team-lead` 更新 `docs/13-implementation-tasks.md` 的状态，不能把部分实现标成完整目标完成。

## 验收角色统一检查清单

每个 MR 的验收报告必须包含：

- 当前 branch 和 base commit。
- 修改文件列表和是否越过责任边界。
- 运行命令：`cargo fmt --check`、`cargo test`、`cargo check`、`git diff --check`。
- 至少一个 CLI 端到端验证命令及关键 JSON 字段。
- JSON/JSONL 主路径扫描结果。
- reliability 检查：L0/L1P/L1S/L2 是否匹配设计。
- snapshot/freshness 检查：stale、missing、hash mismatch 是否 fallback。
- merge 结论：`merge`、`merge after fix` 或 `reject`。

## Agent 分配起始命令

```bash
git fetch origin
mkdir -p /Users/mars/dev/git-ai-workspace/worktrees/code-search-cli
git worktree add /Users/mars/dev/git-ai-workspace/worktrees/code-search-cli/mr01-snapshot-store -b feat/mr01-snapshot-store origin/main
git worktree add /Users/mars/dev/git-ai-workspace/worktrees/code-search-cli/mr04-native-scip -b feat/mr04-native-scip origin/main
```

MR-02、MR-03、MR-05、MR-06、MR-07、MR-08 不应提前从旧 base 长时间开发；等依赖 MR 合并后再创建或 rebase。
