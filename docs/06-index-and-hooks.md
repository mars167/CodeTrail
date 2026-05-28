# 索引与 Git Hook 流程

> 当前设计准绳见 `docs/00-design-summary.md`。本文只展开索引和 hook。

## 目标

`code-search` 需要保留旧项目里“基于 git hook 创建、存储、更新索引”的工作流，但不能继承
embedding、LanceDB、CozoDB 或不透明语义索引。新索引的角色是可验证缓存，用于提升速度和复用
tree-sitter 解析结果。

索引必须满足三个约束：

- 可解释：每条记录都能说明来自哪个文件、哪个 git 状态、哪个解析器。
- 可验证：每条记录都能通过 HEAD、文件大小、mtime 和内容 hash 校验新鲜度。
- 可回退：校验失败时，搜索命令必须读取当前文件实时计算，而不是返回旧缓存。

## 存储位置

默认存储在仓库内：

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

说明：

- `snapshots/<snapshot_id>/manifest.json`：索引版本、工具版本、repo root、HEAD、dirty 状态、schema version。
- `snapshots/<snapshot_id>/files.parquet`：文件清单、大小、mtime、内容 hash、语言、ignore 状态。
- `snapshots/<snapshot_id>/blobs/`：可验证 snapshot blob 或内容寻址缓存。
- `text/<snapshot_id>/grams.idx`：n-gram/trigram 倒排，用于 literal、substring 和 regex prefilter。
- `text/<snapshot_id>/docs.idx`：doc id、path、language、line offsets、file hash。
- `text/<snapshot_id>/paths.idx`：路径搜索索引。
- `scip/<snapshot_id>/index.scip`：native SCIP protobuf 文件。
- `scip/<snapshot_id>/occurrences.db`：用于 `defs`、`refs`、`symbols` 的本地 occurrence store。
- `graph/<snapshot_id>/kuzu/`：默认 embedded KuzuDB property graph，用于结构查询和多跳遍历。
- `graph/<snapshot_id>/export/`：Parquet/CSV 导出，不是查询主存储。
- `working/` 与 `staged/`：未提交状态 overlay，不能与 commit snapshot 混用。

JSON/JSONL 禁止作为主索引存储。它们只允许用于 `code-search index export`、测试 fixture、人工排查和迁移，
不能作为 `find`、`refs`、`symbols`、`calls`、`callers` 的主查询路径。

`.code-search/` 默认不要求提交到 Git。后续如果需要共享索引，可以单独设计 `pack/unpack`
或 artifact 缓存，但不能替代本地 snapshot/freshness 校验。

## Hook 触发点

| Hook | 行为 |
| --- | --- |
| `pre-commit` | 基于 staged snapshot 增量构建索引，写入 `.code-search/staged/manifest.json` 和对应 `text/<snapshot>/`。 |
| `post-commit` | 将 staged 索引标记为新 commit 的已验证快照，必要时同步到 working tree 索引。 |
| `post-checkout` | 切换分支或文件后校验 manifest，按变更文件增量刷新。 |
| `post-merge` | merge 后根据变更文件增量更新索引并记录冲突/partial parse 警告。 |
| `post-rewrite` | rebase 或 amend 后重新校验 commit 映射，必要时重建 manifest。 |

Hook 必须快速失败并给出可读错误。默认不阻塞普通 git 操作，除非用户显式配置
`CODE_SEARCH_HOOK_STRICT=1`。

Watcher 与 Hook 分工：

- watcher：维护 `worktree` overlay，提升本地实时性。
- hook：维护 `staged` 和 `commit` snapshot，保证 git 语义正确。
- 两者都进入 `IndexScheduler`，但不能互相替代。

## CLI 命令

```bash
code-search index build        # 全量构建 working tree 索引
code-search index build --staged
code-search index update       # 基于 git diff / 文件 mtime 增量更新
code-search index status       # 输出 freshness 和健康状态
code-search index verify       # 校验 manifest 与当前文件是否一致
code-search index clean        # 清理索引
code-search index import-scip <index.scip.json>  # 兼容导入 SCIP JSON，写入二进制 occurrences.idx；native index.scip 仍是目标

code-search hooks install
code-search hooks uninstall
code-search hooks status
```

## 搜索命令如何使用索引

搜索命令的决策顺序：

1. 读取命令参数和 file set。
2. 查找目标 snapshot 的 manifest。
3. 对候选文件做 freshness 校验。
4. freshness 通过时使用对应索引层：text gram index、SCIP occurrence DB 或 Kuzu graph。
5. `calls`/`callers` 在 graph snapshot 和 relation source hash 都 fresh 时使用 Kuzu graph backend。
6. freshness 失败时读取当前文件实时扫描，并在响应中写入 fallback 原因。

响应中应包含：

```json
{
  "index": {
    "used": true,
    "fresh": true,
    "source": "text_index",
    "snapshotSource": "working_tree",
    "manifestHead": "abc123",
    "fallback": false
  }
}
```

如果回退：

```json
{
  "index": {
    "used": false,
    "fresh": false,
    "source": "working_tree",
    "fallback": true,
    "reason": "file_hash_mismatch"
  }
}
```

## LLM 约束

Agent 不能因为结果来自索引就认为它更准确。索引只说明“更快”，不说明“更真”。

对于 L0/L1 结果，LLM 可以把索引结果作为候选证据，但在修改代码前仍应使用 `code-search read`
读取精确范围。对于 L2 关系结果，LLM 必须把它当作候选关系，并逐条验证。
