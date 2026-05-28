# T2 — 统一 JSON 可靠性契约

| 字段 | 值 |
|------|-----|
| 状态 | ✅ 已完成 |
| 来源 | [13-implementation-tasks.md](../13-implementation-tasks.md) |
| 相关文档 | [02-reliability-model.md](../02-reliability-model.md) |

## 概述

所有响应均包含 `snapshot_id`、`reliability`、`producer`、`exact`、警告和回退元数据。

## 上下文

T1–T5 已实现、测试、提交、推送，且符合目标架构——因为它们基于实时源码/解析器事实运行，并带有显式的可靠性标签。T2 是这一可靠性契约的基础：所有命令输出统一遵循相同的 JSON 结构，确保 LLM Agent 能够可靠地解释结果的可信度。
