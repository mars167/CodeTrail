# code-search-cli

面向人类开发者和 LLM Agent 的本地优先代码搜索与跳转 CLI。

`code-search` 的核心承诺不是“理解代码”，而是快速给出可验证的代码证据：搜索、路径定位、范围读取、定义、引用、调用候选、索引状态和 MCP 工具输出都带有 snapshot 与 reliability 信息。

## 文档

长期设计文档只保留 Markdown：

| 文档 | 内容 |
| --- | --- |
| [`docs/00-design-summary.md`](docs/00-design-summary.md) | 产品定位、文档边界、总览图 |
| [`docs/01-architecture.md`](docs/01-architecture.md) | snapshot、索引、查询、watcher、remote 架构 |
| [`docs/02-command-contract.md`](docs/02-command-contract.md) | 命令族、JSON 响应、可靠性契约 |
| [`docs/03-quality.md`](docs/03-quality.md) | 本地质量门禁、CI 映射、性能看护边界 |

过程材料不进入 `docs/`：task breakdown、MR plan、临时测试计划、专项报告和历史竞品长报告应放在 issue、PR、CI artifact 或外部记录里。命令参数以 `code-search --help` 和 `src/cli.rs` 为准；实现细节以 `src/`、`tests/` 和 `scripts/` 为准。

## 快速使用

```bash
cargo build
cargo test

cargo run -- find "Workspace"
cargo run -- grep "fn .*status"
cargo run -- read src/main.rs:1-40
cargo run -- defs Workspace
cargo run -- index build
cargo run -- index status
cargo run -- mcp
```

默认输出是 JSON；Agent 应优先使用 JSON 输出，并在修改代码前用 `read` 验证搜索或图候选结果。

## 当前实现

- CLI 命令面由 `clap` 定义，支持 JSON 与 text 输出。
- L0 源码事实命令覆盖内容搜索、路径搜索、目录浏览、范围读取、git changed/status。
- `index build` 使用 LanceDB 作为主要本地索引存储，保存 snapshot、file catalog、file proof 和 gram postings，并保留 manifest 供 pack/unpack 兼容。
- `defs`、`refs`、`symbols` 优先使用 SCIP occurrence store；没有 precise index 时回退到 tree-sitter 或文本搜索。
- `calls`、`callers` 通过当前 petgraph 后端返回调用候选，可靠性始终是 `inferred_candidate`。
- `watch --once` 提供按需 reconcile；`serve` 暴露本地 query service 状态；`mcp` 通过 stdio JSON-RPC 包装同一套查询能力。
- `scripts/quality-gate.sh` 是本地与 CI 的统一质量入口。
