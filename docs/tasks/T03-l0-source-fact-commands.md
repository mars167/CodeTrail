# T3 — L0 源码事实命令

| 字段 | 值 |
|------|-----|
| 状态 | ✅ 已完成 |
| 来源 | [13-implementation-tasks.md](../13-implementation-tasks.md) |
| 相关文档 | [03-command-design.md](../03-command-design.md)、[02-reliability-model.md](../02-reliability-model.md) |

## 概述

搜索、路径、读取、git 状态和变更文件命令无需预建索引即可工作。

## 上下文

T1–T5 已实现、测试、提交、推送，且符合目标架构。T3 覆盖所有 L0 源码事实命令（`find`、`grep`、`refs`、`files`、`find-path`、`glob`、`list`、`tree`、`read`、`changed`、`status`），这些命令直接从工作树和源码中读取，不依赖 `.code-search/` 索引即可输出带有完整可靠性元数据的结果。
