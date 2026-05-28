# T6 — 索引生命周期

| 字段 | 值 |
|------|-----|
| 状态 | 🔄 进行中 |
| 来源 | [13-implementation-tasks.md](../13-implementation-tasks.md) |
| 相关文档 | [06-index-and-hooks.md](../06-index-and-hooks.md)、[07-index-algorithm-analysis.md](../07-index-algorithm-analysis.md)、[05-implementation-roadmap.md](../05-implementation-roadmap.md) |

## 概述

`index build/status/verify/clean` 现已写入 `.code-search/snapshots/`、`.code-search/text/`、`.code-search/working/` 和 `.code-search/staged/`，产出原生文本 `.idx` 段。仍需完成：源码快照 `files.parquet`/`blobs/`、SCIP occurrence 数据库和图后端。

## 已完成部分

- **T6b 文本索引切片**：`index build` 写入 `text/<snapshot>/{docs.idx, paths.idx, grams.idx}`，`index verify` 在查询复用前校验实时文件哈希。

## 当前跟进范围

1. 将 JSONL 文件目录替换为快照存储：`snapshots/<snapshot_id>/files.parquet` 和内容寻址的 `blobs/`
2. 完成文本索引覆盖：路径索引查找、正则预过滤规划、`docs.idx` 行偏移存储、增量段合并/压缩

## 剩余工作

- T6a：源码快照存储（`files.parquet` 和 `blobs/`）
- T6b/T9 跟进：路径索引查找、正则预过滤、行偏移表、增量段合并/压缩
