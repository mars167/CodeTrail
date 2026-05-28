# T9 — 索引支持的查询路径

| 字段 | 值 |
|------|-----|
| 状态 | 🔄 进行中 |
| 来源 | [13-implementation-tasks.md](../13-implementation-tasks.md) |
| 相关文档 | [07-index-algorithm-analysis.md](../07-index-algorithm-analysis.md)、[06-index-and-hooks.md](../06-index-and-hooks.md)、[03-command-design.md](../03-command-design.md) |

## 概述

`find`/`grep` 使用最新的 `text/<snapshot>/grams.idx` 作为候选预过滤器，并从实时文件中验证匹配。仍需完成：路径特定索引查找、SCIP occurrence 数据库和 Kuzu 图查询路径。

## 已完成部分

- 文本索引切片（与 T6b 共享）：`find`/`grep` 在 freshness 校验通过后使用 `grams.idx` 进行字面量候选预过滤。

## 当前跟进范围

完成文本索引覆盖，超出当前已完成的字面量内容预过滤：路径索引查找、正则预过滤规划、`docs.idx` 行偏移存储、增量段合并/压缩。

## 剩余工作

- 路径索引查找、正则预过滤、行偏移表、增量段合并/压缩
