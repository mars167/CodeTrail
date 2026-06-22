# Agent 评测结果与战略结论

> 本文记录 2026-06-07 的 Docker + OpenCode 评测结果，以及 2026-06-21
> 战略检视后的产品边界。历史数据用于解释为什么 CodeTrail 不再作为
> explore-agent 底座。

## 历史评测范围

评测使用本地 Docker 环境运行 OpenCode，并保留每次运行的 session 导出，
用于后续复盘工具调用和回答质量。

| 项 | 值 |
| --- | --- |
| 测试时间 | 2026-06-07 11:16:33Z 至 12:37:40Z；北京时间 19:16:33 至 20:37:40 |
| 运行规模 | 4 个仓库 x 2 个任务 x 4 个条件 x 1 次重复，共 32 次运行 |
| 仓库 | `go-gin`、`java-junit4`、`rust-ripgrep`、`ts-express` |
| 任务 | 架构理解、数据模型理解 |
| OpenCode | `1.16.2` |
| CodeTrail | `codetrail 0.1.5`，从 release 资产安装 |
| CodeGraph | `0.9.9` |
| 模型 | DeepSeek official API，`deepseek-v4-flash` |
| Skill/Agent 来源 | `codex/agent-template-boundary@447af8a` |

`computed tokens` 包含 cache read；`non-cache tokens` 只统计 input、output、
reasoning 和 cache write。自动预检只检查 JSON schema、证据位置格式和文件存在性，
不能替代人工质量评审。

## 历史汇总

| 条件 | 自动预检 | computed tokens | non-cache tokens | computed 几何节省 | non-cache 几何节省 | 工具行为 |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| Baseline | 5/8 | 2,303,249 | 438,417 | baseline | baseline | 成功直接 `read/grep/glob` 219 次 |
| CodeTrail CLI + Skill | 7/8 | 4,700,056 | 362,136 | -84.31% | 16.23% | CodeTrail 命令 350 次；成功直接 `read/grep/glob` 0 次 |
| CodeTrail Skill + Subagent | 8/8 | 270,998 | 110,614 | 86.59% | 73.01% | subagent task 8 次；成功直接 `read/grep/glob` 0 次 |
| CodeGraph MCP | 5/8 | 3,820,105 | 391,497 | -46.31% | 8.24% | CodeGraph 工具 219 次；成功直接 `read/grep/glob` 0 次 |

旧模板的额外边界信号：

- `codetrail_cli` 让主 Agent 反复调用 CodeTrail，导致上下文历史变大，computed token 高于 baseline。
- `codetrail_agent` 的节省主要来自 subagent 压缩调查过程，而不是 CodeTrail 自身替代 bash 搜索。
- CodeGraph MCP 也避免了直接 `read/grep/glob`，但工具输出较大，最终答案仍出现 evidence 格式问题。

## 2026-06-21 战略结论

评测不支持把 CodeTrail 作为 explore-agent 底座。把文本搜索、路径搜索、读取、
Git 状态、watch、remote、saved query 和可靠性框架都包装进 CodeTrail，会增加
工具层和上下文成本；在主 Agent 自己规划循环时尤其明显。

本轮收敛后的方向：

- CodeTrail 只作为 SCIP/语义索引前端。
- 只解决 `symbol`、`defs`、precise `refs`、`calls`、`callers` 这些 bash 搜索不擅长的问题。
- `refs` 必须是 precise SCIP occurrence；没有 fresh SCIP 时返回 caveat，不把文本匹配伪装成引用。
- `defs` 和 `symbols` 可以保留 thin tree-sitter fallback，但它们是语法事实，不是语义证明。
- `calls` 和 `callers` 始终是候选关系。
- Agent skill 必须先用原生工具：`rg`、`fd`、host read/editor、`git`。只有遇到语义索引缺口才调用 CodeTrail。

## 新使用建议

直接使用普通工具完成常规探索：

```bash
rg "literal"
fd Service
git status --short
```

只有需要语义索引时使用 CodeTrail：

```bash
codetrail --output json symbols <query>
codetrail --output json defs <identifier>
codetrail --output json refs <identifier>
codetrail --output json calls <identifier>
codetrail --output json callers <identifier>
codetrail --output json index doctor
```

不要在 agent prompt 中要求：

- index-first 全仓探索；
- compact `explore node`；
- CodeTrail text/path fallback；
- CodeTrail route discovery；
- CodeTrail read/list/tree；
- subagent 只能使用 CodeTrail。

新的 prompt 应表达为：

```text
Use normal agent tools first: rg/fd/git/host source reads. Use CodeTrail only
for symbol definitions, precise SCIP references, and call/caller relationships
that bash search cannot answer cleanly. If refs reports missing precise SCIP
index, do not use CodeTrail text fallback; use rg for textual matches or run
codetrail --output json index doctor to diagnose SCIP provider setup.
```

## 后续评测建议

- 单独评测语义索引缺口任务，例如 symbol disambiguation、precise refs、call chain tracing。
- 不再用架构理解/数据模型理解这类 broad exploration 任务证明 CodeTrail 工具层价值。
- 比较 `rg/fd/read/git baseline` 与 `baseline + CodeTrail semantic lookup`，而不是比较 `baseline` 与 `CodeTrail-only exploration`。
- 指标应关注语义问题的命中质量和节省的源码读取量，而不是把所有搜索事件都导入 CodeTrail。
