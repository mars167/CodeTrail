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
  index/
    manifest.json
    files.jsonl
    symbols.jsonl
    declarations.jsonl
    relations.jsonl
    warnings.jsonl
    staged/
      manifest.json
      symbols.jsonl
```

说明：

- `manifest.json`：索引版本、工具版本、repo root、HEAD、dirty 状态、schema version。
- `files.jsonl`：文件清单、大小、mtime、内容 hash、语言、ignore 状态。
- `symbols.jsonl`：tree-sitter 抽取的 symbol 列表。
- `declarations.jsonl`：声明范围、body 范围、签名摘要。
- `relations.jsonl`：best-effort calls/callers 候选，必须标注 `inferred_candidate`。
- `warnings.jsonl`：unsupported language、parser error、partial parse 等警告。
- `staged/`：`pre-commit` 使用的 staged snapshot 索引，不能与 working tree 索引混用。

`.code-search/index/` 默认不要求提交到 Git。后续如果需要共享索引，可以单独设计 `pack/unpack`
或 artifact 缓存，但不能替代本地 snapshot/freshness 校验。

## Hook 触发点

| Hook | 行为 |
| --- | --- |
| `pre-commit` | 基于 staged snapshot 增量构建索引，写入 `.code-search/index/staged/`。 |
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
code-search index import-scip <index.scip.json>  # 导入 SCIP JSON occurrence

code-search hooks install
code-search hooks uninstall
code-search hooks status
```

## 搜索命令如何使用索引

搜索命令的决策顺序：

1. 读取命令参数和 file set。
2. 查找 `.code-search/index/manifest.json`。
3. 对候选文件做 freshness 校验。
4. freshness 通过时使用索引中的 file/symbol/declaration 缓存。
5. freshness 失败时读取当前文件实时扫描，并在响应中写入 fallback 原因。

响应中应包含：

```json
{
  "index": {
    "used": true,
    "fresh": true,
    "source": "working_tree",
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
