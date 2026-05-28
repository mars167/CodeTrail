# 命令设计

> 当前设计准绳见 `docs/00-design-summary.md`。本文只展开命令面。

## 全局参数

```bash
code-search --path <repo-or-dir> --output json <command>
```

全局选项：

- `--path <path>`：仓库或目录根路径，默认 `.`。
- `--output json|text`：默认 `json`；Agent 应使用 `json`。
- `--include <glob>`：可重复的 include 过滤器。
- `--exclude <glob>`：可重复的 exclude 过滤器。
- `--hidden`：包含隐藏文件，但仍排除 `.git`。
- `--no-ignore`：忽略 `.gitignore` 和 `.ignore`。
- `--limit <n>`：最大匹配数量，默认值按命令区分。
- `--context <n>`：匹配结果上下文行数。

## 文本命令

```bash
code-search find "createCodeContextEngine"
code-search find "fn main" --mode literal
code-search find "handle[A-Z].*" --mode regex
code-search grep "handle[A-Z].*"
code-search refs createCodeContextEngine
```

`find` 是正式内容搜索命令，默认 literal。`grep` 是 Agent/Unix 兼容命令，默认 regex。`refs` 是
identifier 边界文本搜索，不声称做了 symbol resolution。

## 文件命令

```bash
code-search files "src/**/*.rs"
code-search find-path "runtime"
code-search findpath "runtime"
code-search glob "src/**/*.rs"
code-search list src
code-search ls src
code-search tree src --depth 3
code-search read src/main.rs:10-40
code-search changed
code-search watch
code-search watch --once
code-search watch --status
code-search serve
code-search serve --no-watch
```

`files` 是正式路径搜索命令。`find-path` / `findpath` / `path` 是 Agent 兼容路径搜索命令。
`glob` 表示严格 glob 匹配。`list` / `ls` 用于列目录。`tree` 用于浅层目录树。

`read` 是验证命令。LLM 在拿到任何搜索结果后，都应该用它读取精确源码片段。

## Watcher 与服务命令

```bash
code-search watch
code-search watch --once
code-search watch --status
code-search serve
code-search serve --no-watch
```

`watch` 只维护 `worktree` overlay，不替代 git hook。`serve` 启动本地 query service，默认包含 watcher。
`serve --no-watch` 用于只读服务场景。

## Agent 常用搜索命令

`code-search` 应覆盖 Agent 最常用的搜索和读取工具面，让 Agent 通过一个工具完成代码探索：

| Agent 常用动作 | 命令 | 底层能力 | 可靠性 |
| --- | --- | --- | --- |
| grep 内容 | `grep <pattern>` | TextIndex regex/literal search | L0 |
| 查找文本 | `find <text>` | TextIndex literal/regex search | L0 |
| 查找路径 | `find-path <pattern>` | path index / snapshot file catalog | L0 |
| glob 文件 | `glob <pattern>` | ignore-aware glob | L0 |
| 列目录 | `list <dir>` / `ls <dir>` | SnapshotStore directory listing | L0 |
| 目录树 | `tree <dir>` | SnapshotStore tree view | L0 |
| 读取文件 | `read <file[:range]>` | SnapshotStore exact range read | L0 |
| 查改动文件 | `changed` | git diff/staged/worktree snapshot | L0 |
| 实时刷新 | `watch` / `serve` | worktree overlay scheduler | freshness |
| 查定义 | `defs <identifier>` | SCIPIndex -> ParserFallback | L1P/L1S |
| 查引用 | `refs <identifier>` | SCIPIndex -> TextFallback | L1P/L0 |
| 查调用候选 | `calls` / `callers` | GraphStore candidate edges | L2 |

兼容命令只是入口名不同，输出 schema 必须一致。比如 `grep` 和 `find --mode regex` 都返回统一的
match schema；`find-path`、`files` 和 `glob` 都返回统一的 path result schema。

## 索引与 Hook 命令

```bash
code-search index build
code-search index build --staged
code-search index update
code-search index status
code-search index verify
code-search index clean
code-search index import-scip <index.scip.json>

code-search hooks install
code-search hooks uninstall
code-search hooks status
```

索引命令用于创建、存储、验证和刷新 `.code-search/index/` 下的本地缓存。它保留旧项目中
“由 git hook 自动维护索引”的工作流，但索引只作为可验证缓存，不作为不可挑战的事实来源。

建议 hook 行为：

- `pre-commit`：基于 staged snapshot 更新索引，并记录将要提交的 tree 状态。
- `post-commit`：把最近一次 staged 索引标记为对应 commit 的已验证快照。
- `post-checkout`：工作区切换后刷新索引状态，必要时增量重建。
- `post-merge`：merge 后根据变更文件增量更新。
- `post-rewrite`：rebase/amend 后重新校验 commit 映射。

`index status` 必须输出索引 freshness：当前 HEAD、dirty 状态、缓存命中数量、过期文件数量、
unsupported parser 数量，以及是否存在需要回退到实时扫描的情况。

## 解析器命令

```bash
code-search symbols Auth
code-search defs authenticate_user
```

这些命令使用 tree-sitter。输出中应该包含解析语言、可用时的 parser version、symbol kind、
声明范围和 body 范围。

## 关系命令

```bash
code-search calls authenticate_user
code-search callers authenticate_user
```

关系命令是显式 opt-in 能力，并且永远返回 `inferred_candidate` 可靠性级别。
帮助文本必须明确说明：这些结果不是完整调用图。

## 退出码

| 代码 | 含义 |
| --- | --- |
| 0 | 成功 |
| 1 | 参数校验或用法错误 |
| 2 | 搜索完成但没有匹配 |
| 3 | 仓库或路径错误 |
| 4 | 不支持的 parser 或 parser 失败 |
| 5 | 内部错误 |
| 6 | 索引损坏或 freshness 校验失败 |
