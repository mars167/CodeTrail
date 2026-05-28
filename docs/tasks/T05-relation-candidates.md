# T5 — 关系候选

| 字段 | 值 |
|------|-----|
| 状态 | ✅ 已完成 |
| 来源 | [13-implementation-tasks.md](../13-implementation-tasks.md) |
| 相关文档 | [02-reliability-model.md](../02-reliability-model.md)、[03-command-design.md](../03-command-design.md) |

## 概述

`calls` 和 `callers` 将 tree-sitter 调用启发式结果暴露为 `inferred_candidate`，绝不标记为 `exact`。

## 上下文

T1–T5 已实现、测试、提交、推送，且符合目标架构。T5 是"诚实推断"原则的具体实现：调用关系和关系类命令均为 best-effort，结果始终以候选形式呈现，提示 LLM 在基于这些结果进行推理之前需要验证。
