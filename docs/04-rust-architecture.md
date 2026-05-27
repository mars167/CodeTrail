# Rust 架构

> 当前设计准绳见 `docs/00-design-summary.md`。本文只展开 Rust 模块和依赖。

## Crate 规划

初期只做一个 binary crate。只有当代码规模真正需要时，再拆分 crate。

```text
src/
  main.rs
  cli.rs
  output/
    mod.rs
    schema.rs
    render.rs
  workspace/
    mod.rs
    git.rs
    walk.rs
    ignore.rs
  search/
    mod.rs
    text.rs
    files.rs
    refs.rs
  parser/
    mod.rs
    languages.rs
    symbols.rs
    relations.rs
  index/
    mod.rs
    manifest.rs
    store.rs
    update.rs
    freshness.rs
    hooks.rs
  watcher/
    mod.rs
    events.rs
    debounce.rs
    filters.rs
    reconcile.rs
    status.rs
  scheduler/
    mod.rs
    jobs.rs
    publish.rs
  commands/
    mod.rs
    find.rs
    files.rs
    read.rs
    refs.rs
    symbols.rs
    defs.rs
    calls.rs
    callers.rs
    changed.rs
    status.rs
    index.rs
    hooks.rs
```

## 依赖选择

| 需求 | Rust crate | 原因 |
| --- | --- | --- |
| CLI 参数解析 | `clap` | 稳定的 derive API，并支持补全 |
| ignore 感知文件遍历 | `ignore` | 与 ripgrep 同生态，行为接近 |
| 匹配 | `regex` | 确定性 regex 引擎，无灾难性回溯 |
| 文件读取 | `memmap2` 可选 | 用于大型文件快速扫描和 snapshot blob 读取 |
| JSON | `serde`, `serde_json` | 稳定的 Agent 输出 |
| 错误处理 | `thiserror`, `anyhow` | 类型化领域错误加应用层上下文 |
| 解析 | `tree-sitter` | 面向支持语言的确定性 AST parser |
| 内容 hash | `blake3` 可选 | 索引 freshness 校验，后续阶段引入 |
| 文件 watcher | `notify` | 原生 OS events，维护 worktree overlay |
| 并发执行 | `rayon` / `tokio` 可选 | parser/index job 调度 |
| 测试 | `assert_cmd`, `insta`, `tempfile` | CLI 快照测试和 fixture 仓库 |

## 核心数据流

```text
CLI 参数
  -> 校验命令输入
  -> 解析 workspace 和文件集合
  -> watcher/hook/manual command 统一进入 scheduler
  -> 校验索引 freshness，决定使用缓存或实时扫描
  -> 执行确定性搜索、parser 操作或索引更新
  -> 归一化匹配结果
  -> 附加可靠性契约
  -> 渲染 JSON 或 text
```

## 性能目标

- `--help` 和简单命令启动时间低于 50ms；
- 尽量流式读取文件，而不是先构建整仓状态；
- 搜索前不强制建立索引，但优先使用 freshness 通过的本地缓存；
- 索引用于文件清单、symbol 抽取、声明范围和关系候选缓存，缓存键基于 git HEAD、文件路径、
  大小、mtime 和内容 hash；
- git hook 负责在 commit、checkout、merge、rewrite 后维护缓存状态。

## 缓存规则

缓存永远不是事实来源。如果无法低成本证明缓存是新鲜的，就读取当前文件并重新计算。
