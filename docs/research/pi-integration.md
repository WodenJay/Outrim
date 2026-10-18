# Pi 集成研究：入口 tool output 的拦截与改写

研究票：<https://github.com/WodenJay/Outrim/issues/2>
范围：只查 Pi（OpenCode 由他人负责）。只做**入口**（tool output 首次进入上下文时），不做历史上下文重写。不预设 Hook / CLI / proxy 哪个方案获胜，只给事实与边界。

## 0. 结论摘要

- Pi 提供一等公民的 `tool_result` 扩展钩子，位于工具执行完成、结果写入消息之后，能**替换** `content` / `details` / `structuredContent` / `isError` / `usage`。这是修改"首次入上下文"的官方入口。
- 钩子看到的是**各工具自己截断之后**的内容（bash 50KB/2000 行、read 50KB/2000 行、grep 50KB 且单行 500 字符、MCP 20KB 中段）。它无法看到超过这些上限的原文。
- `tool_result` 只对**实际执行过**的工具调用触发。工具不存在 / 参数校验失败 / 被 `tool_call` 钩子阻断 / 执行前就 abort 的情形走 agent-loop 的 "immediate" 分支，不经过 `afterToolCall`。
- 内置工具（read/grep/bash/powershell/find/ls/edit/write）、MCP 工具（`mcp__server__tool`）、其他扩展注册的工具，全部经过同一条 pipeline，一个钩子即可全覆盖。
- 原文可获取途径只有三类：① bash 的 `details.fullOutputPath`（OS 临时文件，0600）；② read 用 `offset/limit` 重读文件；③ 自建缓存（钩子里把原文写出，再给一行指针）。grep/find/ls 的超限内容没有原文出口，只能重跑。
- 自动运行：扩展放在 `~/.pi/agent/extensions/`（或 `pi install` 到 `~/.pi/agent/settings.json` 的 `packages`）即随会话自动加载，无需用户手动调用或提醒。项目级 `.pi/extensions` 需 project trust。
- 薄 TS → Rust 的边界：TS 侧只做"注册钩子 + 转发 + 回填"，Rust 侧做压缩本体。协议不要走 `pi.exec` 的 argv（无 stdin 支持，Windows argv 上限风险），用 node `child_process` 的 stdin/stdout NDJSON。
- 事实/建议区分见每行的 **[事实]** / **[建议]** 标记；未能核实的问题集中在第 9 节。

## 1. 版本与一手来源

**[事实]** 安装包：`@earendil-works/pi-coding-agent@1.3.0`，`engines.node >= 22.19.0`，`type: module`，`main: ./dist/index.js`，`bin: { pi: dist/bundle/cli.js }`，`piConfig.configDir = ".pi"`。本地根目录：`C:/Users/wudon/.pi/agent/install/releases/1.3.0/node_modules/@earendil-works/pi-coding-agent`（`package.json`）。

**[事实]** 上层 agent loop 在 `node_modules/@earendil-works/pi-agent-core`（`dist/agent-loop.js`），`AgentSession` 在 `pi-coding-agent/dist/core/agent-session.js`。

**[未复验]** 交接记录称 git tag `v1.3.0` 对应 commit `71023aec1546d6d677a65669c5665c0927ce0dd0`。本报告的下标均指**本地 1.3.0 发布包内的文件行号**，未逐文件比对 GitHub main。

官方文档（包内 `docs/`）：`extensions.md`、`mcp.md`、`packages.md`、`sdk.md`、`json.md`、`environment-variables.md`、`settings.md`。类型声明：`dist/core/extensions/types.d.ts`。

## 2. `tool_result` 钩子的精确定义

**[事实]** 事件结构（`dist/core/extensions/types.d.ts:985-1001`）：

```ts
interface ToolResultEventBase {
  type: "tool_result";
  toolCallId: string;              // 嵌套调用为 `<parent id>/<n>`
  parentToolCallId?: string;       // 由另一个工具（如 codemode 脚本）发起时存在
  input: Record<string, unknown>;  // 工具入参
  content: (TextContent | ImageContent)[];
  structuredContent?: JsonValue;   // 声明 outputSchema 的工具才有
  isError: boolean;
  usage?: Usage;
}
```

按工具名分支的窄化类型（`types.d.ts:1002-1034`）：`bash` → `BashToolDetails`、`powershell`、`read` → `ReadToolDetails`、`edit` → `EditToolDetails`、`write` → `undefined`、`grep` → `GrepToolDetails`、`find`、`ls`，以及 `CustomToolResultEvent`。也就是说 `details`（含截断元数据、`fullOutputPath`）在钩子里可读。

**[事实]** 返回值（`ToolResultEventResult`）：

```ts
{ content?, details?, structuredContent?, isError?, usage? }
```

**[事实]** 多个 handler **组合**，后一个看到前一个的修改（`docs/extensions.md:106`）。只替换 `content` 而不同时替换 `structuredContent` 会丢弃后者（`types.d.ts:993-997` 注释，`agent-loop.js:601-620` 的合并逻辑）；`agent-session.js:375` 注释亦说明这一点。

**[事实]** 调用点（`dist/core/agent-session.js:349-383` `_afterToolCall`）：先跑扩展钩子，随后做图片规范化（`normalizeToolResultImages`，含模型 `inputLimits.images.resize` 与 `autoResizeImages` 设置），最后把 `content/details/structuredContent/isError/usage` 合并回结果。因此**钩子里注入的图片也会被规范化**，钩子产物是最终形态。

**[事实]** 在 agent loop 中的位置（`pi-agent-core/dist/agent-loop.js:601-625` `finalizeExecutedToolCall`）：`afterToolCall` 在 `execute()` 之后、生成 `toolResultMessage` 之前执行；返回的 `content` 覆盖原结果，`terminate` 也可透传。默认值语义：`afterResult.content ?? result.content`。

**[建议]** Outrim 的钩子应只返回 `content`（和必要的 `details`），并在 `content` 被压缩时把 `structuredContent` 一起置空/替换，避免"内容与结构化数据不一致"。

## 3. 覆盖范围：哪些工具、哪些形态

**[事实]** 内置工具集来自 `_baseToolDefinitions`，扩展工具与 SDK 自定义工具通过 `definitionRegistry.set(name, ...)` 覆盖同名项，`toolRegistry` 先装 builtin 再装扩展工具（`agent-session.js:2855-2900`）。因此：

- 内置 `read/grep/bash/powershell/find/ls/edit/write` 全部经过 `afterToolCall`（钩子按 `toolCall.name` 统一处理，无需逐工具注册）。
- **MCP 工具走同一条 pipeline**：MCP 适配层把 server tool 变成普通 pi tool（`dist/extensions/mcp/tools.js`，名称 `mcp__<server>__<tool>`），因此 `tool_result` 对 MCP 结果同样触发。
- 其他扩展注册的工具、`ctx.executeTool()` 触发的**嵌套调用**也经过钩子（`agent-session.js:326-347`，`agent-loop.js:552-566` `runToolCall`），区别是带 `parentToolCallId`，且 `toolCallId` 形如 `<parent id>/<n>`，不进入 transcript。
- **同名注册会覆盖内置工具**（`toolRegistry.set` 后写胜出，`agent-session.js:2895-2900`）。这是"接管工具、自己拿原文"的合法路径，代价是失去该工具的默认渲染/描述（见第 7 节）。

**[建议]** 首版用 `tool_result` 钩子覆盖全部工具；只有当确实需要超过 50KB 的原文时才考虑同名覆盖 read/grep 这类工具。

## 4. 截断时机与原文获取

**[事实]** 通用截断工具（`dist/core/tools/truncate.js:10-11`）：`DEFAULT_MAX_LINES = 2000`，`DEFAULT_MAX_BYTES = 50 * 1024`；`GREP_MAX_LINE_LENGTH = 500`。`truncateHead`（保留头）、`truncateTail`（保留尾）、`truncateMiddle`（保留首尾、中段替换为 `…N chars truncated…`）。

各工具的实际上限与原文出口：

| 工具 | 截断方式 | 上限 | 原文出口 |
|---|---|---|---|
| `bash` / `powershell` | `truncateTail`（`output-accumulator.snapshot`） | 2000 行 / 50KB | `details.fullOutputPath` → OS 临时文件；文本里已附 `[Showing lines X-Y of N. Full output: <path>]`（`bash.js:166, 238-249`） |
| `read` | `truncateHead` | 2000 行 / 50KB | 文件本身；`offset`/`limit` 续读（`read.js:53, 134-163`，返回 `[N more lines…]`） |
| `grep` | 匹配数 + `truncateHead`（无行数限制） | `DEFAULT_LIMIT` 条匹配 / 50KB，单行 500 字符 | 无；超限只能重跑或换参数 |
| `find` / `ls` | 行/字节截断 | 同上 | 无；重跑 |
| MCP | `truncateMiddle` | 20KB（`MCP_OUTPUT_MAX_BYTES = 20 * 1024`，`dist/extensions/mcp/tools.js:31`） | 完整文本写入临时文件，结果中给出路径（`docs/mcp.md:232`） |

**[事实]** 临时文件实现（`dist/utils/output-files.js`）：写在 OS `tmpdir()`，随机名 `<prefix>-<16hex><ext>`，以 `mode 0o600` + `flag "wx"` 创建（不允许跟随他人预先放置的链接）。**[未复验]** 未在代码中找到清理逻辑，语义上是"进程/系统清理前一直存在"。

**[事实]** `read` 的路径解析 `resolveToCwd`（`dist/core/tools/path-utils.js:42-44`）支持 `~` 展开与绝对路径，没有沙箱限制 → 钩子返回的指针行即使指向 `tmpdir` 或 Outrim 自己的缓存目录，模型也能用内置 `read` 打开（受同样的 2000 行/50KB 与 `offset` 分页约束）。

**[建议（产品决策，用户已定）]** 授权极短一行指针，例如 `[outrim: 3.1KB→0.4KB, raw: C:\...\outrim-cache\<id>.log]`，不写免责声明。原文落地策略二选一：
1. 复用 Pi 已有的 `fullOutputPath`（仅 bash/MCP 且仅当已截断），零额外写盘；
2. Outrim 在钩子里把**钩子看到的完整 content**（即 ≤50KB）落盘并给指针 —— 这是能覆盖 read/grep/find/ls 的唯一通用做法。

## 5. 错误、并发、取消

**[事实]** 工具抛错被 `executePreparedToolCall` 的 `catch` 转成 `isError: true` 的错误结果（`agent-loop.js:566-590`），随后 `finalizeExecutedToolCall` 仍会调用 `afterToolCall` → **错误输出同样可被压缩/改写**（钩子拿到 `isError: true` 时应保守，见建议）。

**[事实]** "immediate" 分支（`agent-loop.js:386-392` 串行、`421-430` 并行）：工具不存在、参数校验失败、被 `tool_call` 钩子 `block`、或在准备阶段已 abort，都会直接产出结果并 `emitToolExecutionEnd`，**不经过 `afterToolCall`**，`tool_result` 不触发。约束含义：Outrim 只能覆盖真正跑了的调用；被阻断的调用不会产生需要压缩的输出，所以这是可接受的事实，而非缺陷。

**[事实]** 执行中途取消：`signal` 传入 `execute()`；中断通常以抛错形式返回 → 走 `catch` → `isError` 结果 → 钩子仍执行。串行模式下 `if (signal?.aborted) break;`（`agent-loop.js:404`）会停止后续调用。

**[事实]** 两种执行模式并存（`config.toolExecution === "sequential" | "parallel"`）。`_executeNestedToolCall` 也读取该模式。**未逐行确认**并行分支的 `finalize` await 细节，可见的部分是每个调用独立 prepare→execute→finalize；**能以确定的只有一点**：钩子是 `async` 且每调用一次 await 一次，因此 Outrim 压缩耗时直接叠加在该次工具调用上（串行模式下必现；并行模式下是否并发叠加未验证）。

**[事实]** `_afterToolCall` 没有 try/catch 包裹 `emitToolResult`（对比 `_beforeToolCall` 有 catch 并重抛，`agent-session.js:341-346`）。**[未复验]** 若 `finalizeExecutedToolCall` 外层捕获了 `afterToolCall` 抛出的异常，则钩子出错只会降级；否则可能打断本轮。因此：

**[建议]** Outrim 钩子必须全包裹 try/catch，任何异常/超时都返回 `undefined`（不改写），绝不透传异常；并设自身超时（例如 200–500ms），超时即放弃压缩。

## 6. 自动安装与全局配置

**[事实]** 扩展发现顺序（`dist/core/extensions/loader.js:673-690`）：① `cwd/.pi/extensions/`（项目级）；② `<agentDir>/extensions/`（全局）；③ 显式配置路径（settings 的 `packages`/extensions 条目、`-e` 参数）。`agentDir` 默认 `~/.pi/agent`，可用 `PI_CODING_AGENT_DIR` 覆盖（`docs/environment-variables.md:81`；`dist/config.js:491`）。

**[事实]** 项目级资源（含 `.pi/extensions`、项目 `packages`）需要 project trust；全局扩展无条件加载（`docs/extensions.md:50`，`docs/packages.md:19-21`；`resource-loader.js:946-957` 对 trust 的判断用于项目资源）。扩展加载用 `jiti`，TS 无需预编译（`docs/extensions.md:38`）。

**[事实]** 分发方式（`docs/packages.md:12-33`）：`pi install npm:@scope/pkg[@ver]`、`pi install git:...`、`pi install ./local`；个人安装写入 `~/.pi/agent/settings.json` 的 `packages`，`--local` 写到 `.pi/settings.json`；`pi -e npm:...` 临时加载。包用 `package.json` 的 `pi` 清单（`pi.extensions` 等）声明扩展入口。

**[建议]** 用户"一次配置后自动运行"的最短路径：安装器把 Outrim 扩展写进 `~/.pi/agent/extensions/outrim.ts`（或 `pi install npm:@outrim/pi`），用户无需任何后续操作。避免项目级安装（要 trust，且每仓一配）。**不要**在扩展工厂里起常驻进程/socket/timer（`docs/extensions.md:58` 明确警告部分场景会加载扩展而不启动会话）；Rust 二进制应**按需 spawn、调用后退出**，或由钩子内惰性启动 + `session_shutdown` 回收。

## 7. 薄 TS 适配到 Rust 的可行边界

**[事实]** `ExtensionAPI.exec(command, args, options?)` 返回 `{ stdout, stderr, code, killed }`（`types.d.ts` `ExtensionAPI` 成员；`dist/core/exec.d.ts` 的 `execCommand`，`ExecOptions = { signal?, timeout?, cwd? }`）。**没有 stdin 参数**。

**[事实]** 扩展与 Pi 同进程、同 OS 权限，可读写文件、环境变量、凭据（`docs/extensions.md:5`）。

因此可行的分层：

| 层 | 职责 | 大小 |
|---|---|---|
| TS 薄壳（Pi 扩展） | `pi.on("tool_result", ...)`；构造请求；调 Rust；把返回文本写回 `content`；失败静默放行 | 数百行以内 |
| Rust 核心（单一二进制） | 压缩策略、缓存写盘、指针行生成 | 主体 |

**[建议]** 传输用 node `child_process.spawn` + stdin/stdout 的 NDJSON，而不是 `pi.exec`：`pi.exec` 无 stdin，把 ≤50KB 的 tool output 塞进 argv 在 Windows（约 32KB 命令行上限）会失败。请求字段建议：`toolName`、`toolCallId`、`input`、`isError`、`content`、`details.truncation`/`fullOutputPath`；响应：`{ content?: [...], pointer?: string, dropStructured?: boolean }`。

边界（Rust 侧做不到的事）：

- **看不到超过工具自身截断上限的原文**（bash 除外，经 `fullOutputPath`；read 除外，经 `offset` 重读）。想突破必须同名覆盖工具（下条）。
- **改写历史不适用**（本产品明确不做）。
- 覆盖内置工具需要重写其 tool 定义（`createReadToolDefinition` / `createBashToolDefinition` 等由包导出，`dist/index.d.ts` 可见 `create*ToolDefinition` 一族），并自行承担描述、渲染、`details` 结构 —— 工作量为钩子方案的数倍，首版不建议。

**[事实]** 同名覆盖技术上成立（第 3 节）：扩展 `registerTool({name: "read", ...})` 会替换 registry 中的 builtin 条目。

## 8. 可选集成形态对比（不裁决）

| 形态 | 覆盖 | 能否拿到 >50KB 原文 | 每工具额外延迟 | 用户配置成本 | 主要风险 |
|---|---|---|---|---|---|
| A. 扩展 + `tool_result` 钩子 | 全部内置 + MCP + 扩展工具 | 否（bash/MCP 经临时文件例外） | 钩子执行时间（一次进程往返） | 一条 settings/一个文件 | 钩子异常/超时需自保；`structuredContent` 一致性 |
| B. 扩展 + 同名覆盖工具 | 仅被覆盖的少数工具 | 是 | 同 A + 自定义实现开销 | 同 A | 需复刻工具语义/渲染；上游改版即碎 |
| C. SDK 内嵌（`createAgentSession` + `customTools`/`baseToolsOverride`） | 取决于宿主选择 | 是（自建工具） | 可控 | 需替换用户的 Pi 使用方式（侵入） | 用户不再直接跑 `pi` |
| D. RPC 客户端（`pi --mode rpc`，事件流 `tool_execution_*`） | 取决于客户端是否愿改写消息 | 否（`tool_execution_end.result` 已是工具产物） | 进程间 + 需把改后内容送回会话 | 高（包裹启动） | 改写路径非官方"改消息"入口，见下 |

**[事实]** RPC 事件流中 `tool_execution_end` 携带 `result`（`docs/json.md:107-114`），但 `tool_result` 钩子属于**进程内扩展 API**；RPC 文档列出的客户端能力是转发扩展交互、命令与事件，未见"改写 tool result 内容再送回"的记录。**[未复验]** RPC 客户端能否回写工具结果，需要单独验证。

**[建议]** 在未定约束排序前，把 A 作为默认推断路径（最低配置成本、最全覆盖），B 留作"需要原文"的增量，C/D 记录为后续扩展形态（用户已定：首版 Pi + OpenCode 同时支持，之后可扩展）。

## 9. 未核实 / 待验证清单

1. `tool_result` handler 抛异常时的最终行为（是否中断本轮、是否降级为未改写）。代码中 `_afterToolCall` 无 catch，`finalizeExecutedToolCall` 的 catch 边界未逐行确认。
2. 并行执行模式（`toolExecution: "parallel"`）下多个 `afterToolCall` 是否并发等待。
3. 临时输出文件的生命周期：是否存在清理逻辑（未找到）。
4. RPC 客户端能否把改写后的 tool result 送进会话（第 8 节 D）。
5. tag `v1.3.0` → commit `71023ae…` 的对应关系（沿用交接记录，未复验）。
6. `read` 对二进制/超大文件的额外拒绝规则（只确认了行/字节截断与分页）。
7. 扩展加载是否需要 `jiti` 之外的编译产物（已确认 TS 直载，未验证 Rust 二进制随包分发的官方约定 —— `docs/packages.md` 未提及二进制附带方式）。

## 10. 引用

本地发布包（`…/@earendil-works/pi-coding-agent@1.3.0/`）：

- `dist/core/extensions/types.d.ts:985-1034`（`ToolResultEventBase` 及各工具窄化）；`ExtensionAPI.on("tool_result"|"tool_call")`、`exec`、`registerTool`、`ToolCallEventResult`、`ToolResultEventResult`
- `dist/core/agent-session.js:326-383`（`_beforeToolCall` / `_afterToolCall`）、`2845-2900`（工具注册表与同名覆盖）、`_executeNestedToolCall`
- `dist/core/tools/truncate.js:10-11`（2000 行 / 50KB / grep 行 500 字符）
- `dist/core/tools/bash.js:166, 238-249`（bash 描述与截断提示、`fullOutputPath`）
- `dist/core/tools/output-accumulator.js:60-95`（`truncateTail` 快照与 `fullOutputPath`）
- `dist/utils/output-files.js`（`tmpdir()`、`0o600`、`wx`）
- `dist/core/tools/read.js:53, 134-163`；`dist/core/tools/path-utils.js:42-44`（绝对路径 / `~`）
- `dist/core/tools/grep.js:34, 214-216`
- `dist/extensions/mcp/tools.js:31`（`MCP_OUTPUT_MAX_BYTES = 20 * 1024`）
- `dist/core/extensions/loader.js:673-690`（扩展发现顺序）、`521-528`（jiti 加载）
- `dist/core/exec.d.ts`（`execCommand`、`ExecOptions`）
- `dist/config.js:491`（`getAgentDir`）
- `node_modules/@earendil-works/pi-agent-core/dist/agent-loop.js:386-404`（immediate 跳过）、`552-566`（`runToolCall`）、`566-625`（错误转换与 `finalizeExecutedToolCall`）

包内文档：

- `docs/extensions.md:5, 38, 50, 58, 106, 149, 204`
- `docs/mcp.md:232`
- `docs/packages.md:12-21, 125`
- `docs/json.md:105-114`
- `docs/environment-variables.md:81`
- `docs/sdk.md`（`createAgentSession`、`customTools`、`SessionManager` 为权威上下文）
