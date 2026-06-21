# Agent 评测结果与使用建议

> 本文记录 2026-06-07 的 Docker + OpenCode 评测结果。它描述的是
> CodeTrail 作为 Agent 搜索工具层时的使用边界，不改变 CLI/MCP 的命令契约。

## 评测范围

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
| Session 导出 | 32/32；每次运行导出 events、session list、logs、snapshot、storage 和 manifest |

`computed tokens` 包含 cache read；`non-cache tokens` 只统计 input、output、
reasoning 和 cache write。自动预检只检查 JSON schema、证据位置格式和文件存在性，
不能替代人工质量评审。

## 汇总结果

| 条件 | 自动预检 | computed tokens | non-cache tokens | computed 几何节省 | non-cache 几何节省 | 工具行为 |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| Baseline | 5/8 | 2,303,249 | 438,417 | baseline | baseline | 成功直接 `read/grep/glob` 219 次 |
| CodeTrail CLI + Skill | 7/8 | 4,700,056 | 362,136 | -84.31% | 16.23% | CodeTrail 命令 350 次；成功直接 `read/grep/glob` 0 次 |
| CodeTrail Skill + Subagent | 8/8 | 270,998 | 110,614 | 86.59% | 73.01% | subagent task 8 次；成功直接 `read/grep/glob` 0 次 |
| CodeGraph MCP | 5/8 | 3,820,105 | 391,497 | -46.31% | 8.24% | CodeGraph 工具 219 次；成功直接 `read/grep/glob` 0 次 |

额外工具边界信号（这是 2026-06-07 旧模板的历史数据；当前模板不再要求
subagent 只能使用 CodeTrail）：

- `codetrail_agent` 有 1 次直接 `read` 尝试和 3 次非 CodeTrail bash 尝试，
  都被权限拒绝；没有成功读取仓库源码。
- `codetrail_cli` 没有直接 `read/grep/glob`，但主 Agent 反复调用
  CodeTrail CLI，导致上下文历史变大。
- `codegraph_mcp` 也避免了直接 `read/grep/glob`，但 `explore`/`node`
  返回的大块输出会被反复带入上下文。

## 结论

这次评测支持一个明确边界：省 token 的关键不是把搜索工具换成 CodeTrail，
而是把多轮搜索、读取、筛选和证据压缩放在 Agent/subagent 层。

CodeTrail CLI + Skill 能把主 Agent 引向 index-backed 搜索，但如果主 Agent 仍
自己规划查询循环，输出和上下文会快速变大。8 个样本中它执行了 350 次
CodeTrail 命令，computed token 反而高于 baseline。它适合单点查找，不适合让
主 Agent 长时间自己探索。

CodeTrail Skill + Subagent 把探索循环收进 `codetrail-evidence` subagent，
主 Agent 只消费压缩后的证据包。8 个样本全部通过自动预检，computed token
几何节省 86.59%，non-cache token 几何节省 73.01%。这说明当前更稳定的方向是：
CodeTrail 保持 index-backed 搜索/导航原语，目录浏览和读取保持为验证辅助，
任务级调查由 subagent 完成。

CodeGraph MCP 能提供图式探索入口，但本次数据里工具输出较大，且最终答案仍出现
file-only evidence、逗号拼接多段范围等格式问题。证据格式约束仍需要 Agent 层
执行，不能只依赖工具类型解决。

## 使用建议

单点问题直接使用 CodeTrail CLI：

```bash
codetrail --output json find "literal" --limit 10
codetrail --output json grep "regex" --context 0 --limit 10
codetrail --output json defs SymbolName
```

适用场景：

- 找一个符号、文件、字符串或配置位置。
- 验证某个候选范围是否真的支持结论。
- 在编辑前读取精确源码范围。

多步仓库调查使用 Skill + subagent：

1. 安装 `skills/codetrail/SKILL.md`。
2. 安装 `skills/codetrail/agents/opencode/codetrail-evidence.md` 到
   `.opencode/agents/` 或 `~/.config/opencode/agents/`。
3. 主 Agent 先加载 `codetrail` skill，再把仓库调查委托给
   `codetrail-evidence` subagent。
4. 主 Agent 和 subagent 都应优先用 CodeTrail 的 index-backed 命令做
   discovery；普通 read/grep/glob/list/LSP 可用于验证、补洞或处理索引覆盖不到的
   文件。
5. subagent 返回紧凑 JSON，并在 trace/caveats 中说明何时绕过了 index-backed
   discovery。

### Index-first v2 调整

后续评测应使用 index-first 版本的 Skill + subagent。这个版本不再把
`find`/`grep` 和 `defs`/`refs`/`symbols` 放在同一优先级，而是要求多步调查按
下面顺序执行：

1. 先调用 `codetrail --output json index status --summary`，检查 SCIP freshness 和语言覆盖。
2. 从任务中抽取候选 symbol、type、function、method、route 或 config 名称。
3. 首轮优先调用最便宜的相关命令：`routes`、`defs`、`symbols`、
   `refs`、`calls`、`callers`，或一个 bounded path/text discovery。
4. 只有单个必要锚点仍然歧义时，才调用 compact node：
   `codetrail --output json explore node <query> --compact --max-candidates 2 --snippet-lines 8 --relation-limit 4 --max-bytes 8000`。
5. 仍不足时，再用一个窄的 `symbols`、`defs`、`refs`、`routes`、
   `calls` 或 `callers` 补充。
6. 用 `files`、`find-path`、`glob` 只做范围收窄或名称发现，然后回到语义命令。
7. 用普通 source read 验证关键范围；`list`、`tree` 和 `read` 不属于
   CodeTrail CLI/MCP 命令面，也不计入 index-backed discovery。
8. 只有在 literal-text 任务、无候选名称、索引缺失/过期/不支持、无语义命中或结果歧义时，才 fallback 到 `find`/`grep`，并在 subagent JSON 的 `index_usage.text_fallback_reason` 里记录原因。

这个调整的目标是把 CodeTrail 当作 SCIP/LSP/text/path index 优先的导航工具，
而不是受控 `grep`/`read` 包装器。评测报告应新增或单列这些指标：

- `semantic_index_coverage`: 有 trace 的 run 中是否至少使用一次 `symbols`、`defs`、`refs`、`routes`、`calls` 或 `callers`。
- `semantic_event_share`: semantic/navigation command events 占 CodeTrail events 的比例。
- `grep_fallback_rate`: 在 semantic/navigation 尝试之前或没有 fallback reason 的 `find`/`grep` 比例。
- `reliability_weighted_efficiency`: 按 `precise_fact`、`parser_fact`、`source_fact`、`inferred_candidate` 分层加权后的 accepted evidence / 1k non-cache tokens。

推荐在主 Agent 提示中显式写入：

```text
Load the codetrail skill, then delegate repository investigation to the
codetrail-evidence subagent. Do not inspect the agent template file. Do not
force the subagent to use only CodeTrail. The subagent must use an index-first
workflow: check codetrail index status --summary, start with the cheapest
route/symbol/reference/call/path primitive that matches the task, use compact
explore node only if one necessary anchor remains ambiguous, use at most one
narrow semantic/navigation supplement before find/grep, use
files/find-path/glob for indexed path discovery, and explain any non-index or
text-search fallback in index_usage.text_fallback_reason.
Every final evidence string must be path:line or path:start-end.
```

证据格式要严格：

- 使用 `path:line` 或 `path:start-end`。
- 不要使用 file-only 路径，例如 `src/lib.rs`。
- 不要把说明文字放进 evidence 字符串，例如
  `src/lib.rs:12-40 (initialization)`。
- 多段范围拆成多个 evidence 项，不要写成 `src/lib.rs:12,30-40`。
- `calls` 和 `callers` 只当候选，必须再用 source read 验证范围。

## 产品边界

不要把本次有效的 subagent 行为下沉成 CodeTrail CLI 命令。下面这些能力属于
Agent 层：

- 根据任务决定查询顺序。
- 判断调查是否足够。
- 汇总架构、数据模型、调试或代码审查结论。
- 压缩多轮查询结果，生成最终回答。

CodeTrail CLI/MCP 应继续只提供可组合的 index-backed 搜索/路径、源码验证、
符号、关系候选、index freshness、saved query 和 remote snapshot 原语。`brief`、`context`、
`analyze architecture`、`analyze data-model` 这类任务命令不应进入公共命令契约。

## 后续评测建议

- 正式发布前把重复次数从 1 提高到至少 3，降低单次模型随机性的影响。
- 增加人工质量评分，避免只用自动预检代替事实正确性判断。
- 保留 session 导出，继续分析被权限拒绝的工具尝试和证据格式失败类型。
- 分开报告 computed token 与 non-cache token；前者反映上下文压力，后者更接近
  实际模型生成与输入成本。
