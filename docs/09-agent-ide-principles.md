# Agent IDE 原则

> 当前设计准绳见 `docs/00-design-summary.md`。本文只展开 Agent 使用原则。

## 目标

`code-search` 的目标不是让 Agent “多读代码”，而是让 Agent 像使用 IDEA/JetBrains IDE 一样获取代码信息：

- 搜索文本；
- 跳转定义；
- 查找引用；
- 读取精确范围；
- 查看调用候选；
- 分析局部影响面；
- 判断当前 git 状态下结果是否新鲜。
- 在 watch/serve 模式下获取实时 worktree overlay。

Agent 面对大型代码库时，默认不应该用 `grep -> read -> 猜测` 循环。它应该先查询索引、拿到有 provenance
和 range 的结果，再只读取必要源码做验证。

同时，`code-search` 应该囊括 Agent 常用的基础搜索动作：`grep`、`find-path`、`glob`、`list/ls`、
`tree`、`read`、`changed`、`defs`、`refs`。Agent 不应该为了完成一次定位，在多个 shell 工具和多套输出格式之间切换。

## 原则

### Local First

本地是默认事实源：

- 本地源码；
- 本地 git snapshot；
- 本地索引；
- 本地 query service；
- 本地 MCP/CLI。

远程服务不可用时，单仓 search/jump/read 必须仍然可用。

### Git First

所有事实都绑定 git 语义：

- `commit:<sha>`；
- `staged:<tree_hash>`；
- `worktree:<hash>`。

查询结果必须说明来自哪个 snapshot。不能把 HEAD、staged 和 dirty worktree 混在一个无标注结果集里。

### Remote 可用

Remote 是共享和加速层：

- 团队共享索引；
- 跨 repo 查询；
- CI 生成 precise SCIP；
- 远程 graph UI；
- 大仓预热缓存。

Remote 不是本地事实的替代品。远程结果必须能落回本地 `file_hash + range` 验证；不能验证时必须降级。

### 高效

高效不是只追求速度，而是减少 Agent 的无效探索：

- 常见 symbol 查询不应该扫描全仓；
- 常见 defs/refs 应直接命中 occurrence index；
- 常见 grep/find-path/list/read 应由同一个工具提供统一 JSON 输出；
- watcher 应减少 Agent 看到 stale worktree 信息的概率；
- regex 搜索应先走 gram 候选再验证；
- impact/calls 应先走 graph 候选再读取源码验证；
- query response 应包含足够上下文，避免 Agent 立即二次盲读。

### 准确

准确性分为两类：

- 准确事实：源码范围、路径、内容 hash、precise occurrence。
- 候选事实：parser fallback、search-based refs、heuristic calls、framework bridge。

准确事实可以用于跳转和编辑决策。候选事实只能用于缩小搜索范围，必须验证后才能进入推理链。

## Agent 工具契约

每个工具响应都必须包含：

- `snapshot_id`
- `path`
- `range`
- `file_hash`
- `producer`
- `reliability`
- `exact`
- `freshness`
- `fallback_reason`

`exact=true` 的最低要求：

- L0 源码事实，或
- L1P precise producer 事实，且
- snapshot/file hash 验证通过。

其他结果必须是 candidate。
