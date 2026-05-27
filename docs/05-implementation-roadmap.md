# 实现路线图

> 当前设计准绳见 `docs/00-design-summary.md`。本文只展开实现顺序。

## 阶段 0：设计骨架

- 创建 Rust binary crate。
- 固定命令名和 JSON schema。
- 添加预期输出示例 fixture。

## 阶段 1：源码事实

- 实现 `status`。
- 实现 ignore-aware 文件遍历。
- 实现 `files`。
- 实现 `read`。
- 实现 `list`、`tree`、`grep`、`find-path`、`glob`。
- 实现支持 literal 和 regex 模式的 `find`。
- 实现 identifier-boundary 匹配的 `refs`。

验收标准：

- 不需要预先建立索引；
- JSON 输出有快照测试；
- 行号和列号定位稳定；
- 无匹配行为有专门退出码。

## 阶段 2：IndexScheduler、Hook 与 Watcher 生命周期

- 实现 `.code-search/index/` 存储布局。
- 实现统一 `IndexScheduler`，接收 manual command、git hook、watcher change set。
- 实现 `index build`、`index update`、`index status`、`index verify`、`index clean`。
- 实现 `hooks install`、`hooks uninstall`、`hooks status`。
- 实现 `watch`、`watch --once`、`watch --status`、`serve --no-watch`。
- 接入 `pre-commit`、`post-commit`、`post-checkout`、`post-merge`、`post-rewrite`。
- 为每条索引记录保存 freshness 证据：repo root、HEAD、文件路径、大小、mtime、内容 hash。

验收标准：

- hook 可以自动创建和增量更新索引；
- 索引损坏或过期时搜索命令自动回退到实时扫描；
- `index status` 能清楚解释哪些文件新鲜、哪些文件过期、为什么过期；
- staged 索引和 working-tree 索引不会混淆。
- watcher 只更新 worktree overlay，事件丢失时能标记 stale 并 reconcile。

## 阶段 3：解析器事实

- 添加 tree-sitter language registry。
- 实现 `symbols`。
- 实现 `defs`。
- 返回 declaration range、body range、kind、language 和 parser source。

验收标准：

- 语法错误文件应返回部分结果和 parser warnings；
- 不支持的语言要清晰降级；
- parser facts 不能在没有可靠性标注的情况下与 text facts 混在一起。

## 阶段 4：推断关系

- 实现同文件 `calls`。
- 通过匹配 call expression candidates 实现项目级 `callers`。
- 输出强制 LLM 指令和已知盲区。

验收标准：

- 文档和 help text 明确称其为 best-effort；
- 每个候选结果都包含精确源码范围；
- callers 输出永远不能声称完整性。

## 阶段 5：Agent 集成

- 添加 shell completions。
- 只有在 CLI schema 稳定后才添加 MCP wrapper。
- 创建 agent skill，要求 LLM 对 L2 候选结果使用 `read` 验证。
