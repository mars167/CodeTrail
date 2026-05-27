# Agent 搜索命令面

> 当前设计准绳见 `docs/00-design-summary.md`。本文只展开 Agent 搜索命令入口。

## 目标

Agent 常见代码探索动作不应该分散在 `grep`、`find`、`fd`、`rg`、`ls`、`cat`、IDE 跳转和 MCP 工具之间。
`code-search` 应提供统一入口，让 Agent 通过一种工具完成搜索、路径定位、目录查看、文件读取和跳转。

## 命令分层

命令分为两组：

- 正式命令：表达产品语义，如 `find`、`files`、`read`、`defs`、`refs`。
- 兼容命令：贴近 Agent 和 Unix 习惯，如 `grep`、`find-path`、`findpath`、`glob`、`list`、`ls`、`tree`。

兼容命令不能绕过架构。它们必须调用同一个 query service，并返回同一套 JSON schema。

## 必须覆盖的 Agent 动作

| 动作 | 命令 | 说明 |
| --- | --- | --- |
| 内容正则搜索 | `grep <pattern>` | 默认 regex，类似 `rg`/`grep`。 |
| 内容 literal 搜索 | `find <text>` | 默认 literal，适合精确字符串。 |
| 文件路径搜索 | `find-path <pattern>` | 支持 substring、prefix、glob、regex。 |
| 兼容路径搜索 | `findpath <pattern>` | `find-path` 的别名，适配已有 Agent 习惯。 |
| glob 文件 | `glob <pattern>` | 严格 glob，用于快速枚举候选文件。 |
| 列目录 | `list <dir>` / `ls <dir>` | 结构化目录列表。 |
| 目录树 | `tree <dir>` | 限深目录树，默认排除依赖和构建产物。 |
| 读取范围 | `read <file[:range]>` | 所有候选结果的验证入口。 |
| 改动文件 | `changed` | 基于 git staged/worktree/commit diff。 |
| 实时刷新 | `watch` / `serve` | 维护 worktree overlay 和本地 query service。 |
| 定义跳转 | `defs <name>` | 优先 precise occurrence，fallback 到 parser fact。 |
| 引用查找 | `refs <name>` | 优先 precise occurrence，fallback 到 identifier 文本搜索。 |

## 输出原则

所有命令都必须输出：

- `command`：用户调用的入口名，例如 `grep`。
- `canonicalCommand`：标准命令名，例如 `find`。
- `query`：规范化后的查询参数。
- `snapshot_id`：commit/staged/worktree。
- `results`：统一结果结构。
- `reliability`：L0/L1P/L1S/L2。
- `freshness`：索引是否新鲜，是否 fallback。

示例：

```json
{
  "ok": true,
  "command": "grep",
  "canonicalCommand": "find",
  "query": {
    "pattern": "handle[A-Z].*",
    "mode": "regex"
  },
  "snapshot_id": "worktree:abc123",
  "reliability": "source_fact",
  "results": []
}
```

## 设计约束

- `grep` 不等于直接调用系统 grep；它必须经过 TextIndex 和源码 range verification。
- `find-path` 不等于 shell `find`；它搜索 snapshot file catalog。
- `list/tree/read` 必须尊重 git snapshot，不能混淆 HEAD、staged 和 worktree。
- 所有 alias 必须在响应中保留原始命令名，便于 Agent 学习和调试。
- 兼容命令不能产生与正式命令不同的排序规则和 JSON 字段。
