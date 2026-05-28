# T11 — 精确 SCIP 集成

| 字段 | 值 |
|------|-----|
| 状态 | 🔄 进行中 |
| 来源 | [13-implementation-tasks.md](../13-implementation-tasks.md) |
| 相关文档 | [02-reliability-model.md](../02-reliability-model.md)、[07-index-algorithm-analysis.md](../07-index-algorithm-analysis.md)、[08-reference-architecture.md](../08-reference-architecture.md) |

## 概述

SCIP JSON 兼容导入现已存储二进制 `scip/<snapshot>/occurrences.idx`，不再使用 JSONL。仍需完成：原生 `index.scip` protobuf 解析和 `occurrences.db`。

## 已完成部分

- SCIP JSON 兼容导入不再使用 JSONL 存储；现写入二进制 `occurrences.idx`，但不算作完整的原生 SCIP 架构。

## 当前跟进范围

将 SCIP JSON 兼容导入替换为原生 `scip/<snapshot_id>/index.scip` protobuf 解析和 `occurrences.db`。

## 剩余工作

- 二进制 `index.scip` protobuf 解析和 occurrence 数据库
