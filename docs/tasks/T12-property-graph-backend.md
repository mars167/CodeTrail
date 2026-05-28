# T12 — 属性图后端

| 字段 | 值 |
|------|-----|
| 状态 | ⏳ 待开始 |
| 来源 | [13-implementation-tasks.md](../13-implementation-tasks.md) |
| 相关文档 | [04-rust-architecture.md](../04-rust-architecture.md)、[07-index-algorithm-analysis.md](../07-index-algorithm-analysis.md)、[08-reference-architecture.md](../08-reference-architecture.md) |

## 概述

需要 KuzuDB 嵌入式后端。之前的 JSONL 关系存储已从 `index build` 和查询分发中移除；关系输出保持 tree-sitter `inferred_candidate`，直到 Kuzu 就位。

## 当前跟进范围

将 JSONL 关系记录替换为 `graph/<snapshot_id>/kuzu/`。

## 剩余工作

- KuzuDB 图后端、后端 trait 和影响遍历
