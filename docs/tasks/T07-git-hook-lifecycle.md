# T7 — Git Hook 生命周期

| 字段 | 值 |
|------|-----|
| 状态 | ✅ 已完成 |
| 来源 | [13-implementation-tasks.md](../13-implementation-tasks.md) |
| 相关文档 | [06-index-and-hooks.md](../06-index-and-hooks.md) |

## 概述

Hook 安装/状态/卸载支持 staged 和 commit update 入口点，但不使 hook 成为权威数据源。

## 上下文

T7–T8 在命令/状态级别已实现，并保持为有效的生命周期入口点。Hook 用于触发索引更新，但索引本身始终以源码文件为最终权威——hook 只是自动化的触发器，不能替代直接文件校验。
