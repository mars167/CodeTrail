# T8 — Watch/Serve 状态

| 字段 | 值 |
|------|-----|
| 状态 | ✅ 已完成 |
| 来源 | [13-implementation-tasks.md](../13-implementation-tasks.md) |
| 相关文档 | [11-watcher-design.md](../11-watcher-design.md) |

## 概述

`watch --once`、`watch --status` 和 `serve --no-watch` 暴露 freshness/status 契约。

## 上下文

T7–T8 在命令/状态级别已实现，并保持为有效的生命周期入口点。`watch` 和 `serve` 命令输出当前索引的新鲜度状态，使 Agent 能够在查询前判断索引是否过期。
