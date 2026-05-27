# Watcher 设计

> 当前设计准绳见 `docs/00-design-summary.md`。本文只展开 watcher。

## 结论

Watcher 只维护 `worktree` overlay。它用于提升本地实时性，不替代 git hook，不改 staged，不维护 commit snapshot。

```text
OS file events
  -> debounce/coalesce
  -> reconcile changed paths
  -> IndexScheduler
  -> update worktree overlay
  -> publish freshness
```

## 命令

```bash
code-search watch
code-search watch --once
code-search watch --status
code-search serve
code-search serve --no-watch
```

- `watch`：只运行 watcher 和 worktree overlay 更新。
- `watch --once`：做一次 reconcile scan 后退出。
- `watch --status`：输出 watcher 状态、队列长度、stale 原因。
- `serve`：启动本地 query service，默认包含 watcher。
- `serve --no-watch`：只提供查询，不监听文件变化。

## Snapshot 规则

- `commit:<sha>`：由 `index build`、`post-commit`、`post-checkout`、`post-merge`、`post-rewrite` 维护。
- `staged:<tree_hash>`：由 `pre-commit` 或 `index build --staged` 维护。
- `worktree:<hash>`：由 watcher 或 `index update` 维护。

查询合并顺序：

```text
commit snapshot
  + staged overlay
  + worktree overlay
```

每条结果必须标注来源，不能把 HEAD、staged、worktree 混成无来源结果。

## 事件处理

Watcher 使用 Rust `notify` crate 接收 OS events。

事件进入队列后必须做：

- debounce：默认 150-300ms，合并编辑器连续写入。
- coalesce：同一路径多事件合并为一个 update。
- normalize：统一 POSIX path，解析 rename old/new。
- filter：忽略 `.git`、`.code-search`、`target`、`node_modules`、`dist`、build 产物，并尊重 `.gitignore`。
- reconcile：实际 `stat` 文件确认最终状态。

## 失败处理

- overflow：标记 `worktree_overlay_stale`，触发全量 reconcile scan。
- rename 不完整：按 delete + create 处理。
- 文件读取失败：记录 warning，不阻塞 watcher。
- parser 失败：更新 file/text facts，parser facts 标记 stale 或 warning。
- graph 更新失败：保留 text/symbol 更新，graph layer 标记 stale。
- 长时间 backlog：`watch --status` 暴露队列长度和最旧事件时间。

## 与 Git Hook 的边界

Watcher 做：

- 更新 worktree overlay；
- 刷新 text index candidate；
- 刷新 parser facts；
- 刷新 graph candidate edges；
- 发布 freshness 状态。

Watcher 不做：

- 不执行 `git add`；
- 不修改 staged；
- 不生成 commit snapshot；
- 不替代 `pre-commit`；
- 不把 heuristic result 标成 exact。

## 输出状态

`watch --status` 输出必须包含：

```json
{
  "ok": true,
  "watcher": {
    "running": true,
    "root": "/repo",
    "snapshot": "worktree:abc123",
    "queueLength": 0,
    "stale": false,
    "lastEventAt": "2026-05-27T00:00:00Z",
    "lastReconcileAt": "2026-05-27T00:00:01Z"
  }
}
```

## 实现模块

```text
src/watcher/
  mod.rs
  events.rs
  debounce.rs
  filters.rs
  reconcile.rs
  status.rs
```

Watcher 只产生 normalized change set，真正索引更新交给 `IndexScheduler`。

