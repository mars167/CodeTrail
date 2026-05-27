# 产品定位

> 当前设计准绳见 `docs/00-design-summary.md`。本文只展开产品定位。

## 一句话

`code-search` 是面向人类开发者和 LLM Agent 的本地优先代码搜索与跳转系统。

它的目标是让 Agent 像使用 IDEA/JetBrains IDE 一样高效获取代码信息：搜索文本、跳转定义、
查找引用、读取精确范围、理解局部影响，并且每一步都能回到源码和 git snapshot 验证。

## 相比 git-ai 的变化

旧方向以 engine/runtime 为中心，试图通过语义搜索、图扩展和任务上下文包来组装高层代码上下文。
新方向以 CLI 命令为中心，只返回窄而可验证的代码证据。

这个产品不应该承诺不可验证的“理解代码”。它应该承诺：

- 快速找到精确的代码事实；
- 说明每个结果是如何产生的；
- 对定义、引用和跳转类结果提供 IDE 级别的精确性目标；
- 将解析器输出和文本匹配分开标注；
- 将推断关系标注为候选结果，而不是完整事实。

## 产品原则

- Local first：本地仓库、本地索引、本地查询是默认路径；离线可用。
- Git first：所有事实绑定 `commit`、`staged` 或 `worktree` snapshot；不允许无 git 语义的模糊缓存。
- Remote 可用：远程索引和远程 graph 可以用于团队共享、大仓加速和跨仓查询，但本地验证链路必须存在。
- 高效准确：Agent 查询路径必须接近 IDE 的 search/jump 体验；准确性由 producer、snapshot、file hash、range 证明。
- 绝不伪精确：没有 precise producer 的调用链、动态分派、跨框架推断只能返回候选，并强制暴露 provenance。

## 定位边界

范围内：

- literal、regex、identifier-boundary 文本搜索；
- 路径搜索；
- 精确文件范围读取；
- git changed-file 感知；
- commit/staged/worktree snapshot 区分；
- 基于 git hook 的索引创建、存储和更新流程；
- SCIP/语言服务产生的 precise symbol/occurrence；
- tree-sitter 声明和 symbol fallback；
- 本地优先、远程可用的 graph/index 查询；
- watcher 维护 worktree overlay，提供本地实时更新；
- 带警告的 best-effort calls/callers 候选结果；
- 面向 Agent 的稳定 JSON 输出。

默认产品范围外：

- embedding 搜索；
- 语义相似度排序；
- 向量数据库；
- 将索引当作不可验证事实来源；
- 不透明的 confidence score；
- 自动任务上下文组装；
- 对无法由 precise producer 证明的结果声称绝对准确。
- 用 watcher 替代 git hook 或覆盖 staged/commit snapshot。

## 设计准则

不禁止推断，但禁止静默推断。

索引可以保留，但它必须是可验证缓存：任何来自索引的结果都要能用当前文件内容、git HEAD、
文件大小、mtime 或内容 hash 证明新鲜度；证明不了就回退到读取当前文件。

“绝对准确”不是营销词，而是工程约束：只有 L0 源码事实和 precise producer 生成的 L1 事实可以作为准确结果。
任何 heuristic、search-based 或 parser fallback 结果都必须显式降级，不能混入准确结果集合。
