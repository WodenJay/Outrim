# OpenCode 集成研究：入口 tool output 的拦截与改写

研究票：<https://github.com/WodenJay/Outrim/issues/2>
范围：只查 **OpenCode**（Pi 侧已由他人完成，见 `docs/research/pi-integration.md`）。只做**入口**（tool output 首次进入上下文时），不做历史上下文重写。不预设 Hook / CLI / proxy 哪个方案获胜，只给事实与边界。

一手来源分两份本地 clone，行号均指这两个快照：

| 代号 | 版本 | commit | 路径 |
|---|---|---|---|
| **v2** | `2.0.32`（CHANGELOG `## v2.0.32 — 2026-10-16`） | `843a6c514e606b436a7fe713422ace0a93ad3d61`（tag `v2.0.32`），工作树 HEAD `501b15af26915ba52682e20e220e7e137e89e92f` | `C:/Users/wudon/AppData/Local/Temp/oc-src/opencode` |
| **v1** | `1.19.3` | `e3c4eb242585dcd77be1a867391b7c25fd47ce52` | `C:/Users/wudon/AppData/Local/Temp/oc-src/opencode-v1` |

`origin` 为 `github.com/sst/opencode`，README 已指向 `github.com/anomalyco/opencode`；两个仓库对同一 commit 的 `blob` 链接都返回 200，本报告统一用 `anomalyco/opencode` 的 permalink。v2 依赖 `effect@4.0.1`。

**[事实] 快照口径**：本地工作树 HEAD 是 tag 之后的 `501b15af2`（仅一条 README 文档提交）。本报告引用到的关键文件里，`packages/core/src/tool.ts`、`codemode/tool.ts`、`session/runner/step.ts`、`tool-output.ts`、`plugin/src/effect/tool.ts`、`shell/result.ts`、`tool/plugin/{shell,read,grep,webfetch}.ts`、`tool/mcp.ts`、`permission.ts`、`file-access.ts`、`tool/runtime.ts`、`session/model-request.ts`、`config/plugin/source.ts`、`plugin/module.ts`、`cli/.../plugin/add.ts` 在 tag→HEAD 之间**逐字节相同**（`git diff --quiet` 校验过），故行号对两份都成立。**唯一有差异的是 `packages/core/src/shell.ts`**（HEAD 改了超时/结束竞态的实现，`EXITED_LIMIT`/`RETENTION`/`DIRECTORY`/`outputDir`/`result()` 的截断与提示逻辑本身未变，仅行号下移 8 行）；引用该文件时本报告同时给出 tag 行号（permalink 用 tag）与 HEAD 行号。

本机实际安装的 CLI 是 **v2.0.26**（`opencode --version`）。本报告不对 26 与 32 的差异下结论：只做过 `--help`/`--version` 观察，**未** 逐 commit diff，也不据 CHANGELOG 推断兼容性。

---

## 0. 结论摘要

- **[事实]** v2 提供一等公民的 `ctx.tool.hook("execute.after", …)`，位置在工具执行完成、`Tool.Success` 持久化之前；事件里 `result`（`{output?, content?, metadata?}`）与 `error`（`Tool.Error`）都是**可整体替换的裸字段**，`status: "completed" | "error"` 是判别式。这是修改"首次入上下文"的官方入口。
- **[事实]** "钩子看到的上限"**按工具而异，不能一律说成 50 KiB**。有工具内部截断的只有：`shell`（尾部 2000 行 / 50 KiB）、`read`（头部 2000 行 / 50 KiB + 单行 2000 字符）、`grep`/`glob`（条数，默认 100）、`webfetch`（响应体 5 MiB）。**上述之外——MCP server tool、插件注册的自定义工具、`websearch`、`mcp-resource`——工具自身不截断**，钩子里的 `content` 可以**超过 50 KiB**。通用 `ToolOutput.truncate`（2000 行 / 50 KiB）是在钩子**之后**才跑的，所以 **50 KiB 不是钩子的上界**；它是对"钩子之后"那份结果的约束（MCP 另有 16 MiB 的 stdio 帧上限，见 §4.3）。
- **[事实]** v1 的内置工具同样在 `Tool.execute` 内部就截断（`Truncate.output`），所以 v1 的 `tool.execute.after` 拿到的也是截断后的字符串。仅就此点而言两代相同；v1 的提示还带一段长免责声明（"…Do NOT read the full file yourself - delegate to save context."），正是用户明确否掉的形态。**本报告不比较两代的其他差异。**
- **[事实]** 原文出口**都不在钩子里**：`shell` 的完整输出落在按 shell 持久化的 `.out` 文件（7 天 + 最近 25 条已退出记录），但路径只在 `content` 文本尾部与 `shellID` 里；通用溢出文件 `tool-output/tool_<id>` 是 `ToolOutput.truncate` **在钩子之后**才写的，**若 Outrim 先把内容压到限额以下，这个文件根本不会被创建**——所以不能当成 after 阶段可复用的原文路径。`read` 可 `offset` 续读；`grep`/`glob` 超限部分进程内无副本；`webfetch`/`websearch` 只有工具自身上限，没有落盘副本。→ **唯一可靠的原文来源是自己把钩子实际收到的输入落盘。**
- **[事实]** 覆盖范围：内置工具、MCP 工具、插件注册的自定义工具全部经过同一个 `execute.after`。例外有三处：① 后台 shell / 后台 subagent 的输出通过 `sessions.synthetic()` 以合成消息进入会话，绕过 `execute.after`；② provider hosted 工具；③ 未进入 `Tool.execute` 的失败。**另外 Code Mode（`execute`）的子调用虽然也触发钩子，但压子工具的 `content` 不等于给模型省 token**（见 §4.2）。
- **[事实]** 自动运行：`plugins` 数组里的 npm/Git 包会在激活时 `npm.add` 安装；`opencode plugin add <npm|git spec>` 非交互地把 spec 写进 `~/.config/opencode/opencode.json(c)`（jsonc 保留注释、tmp+rename 原子写、Unix 上 `0o600`）。**这是推荐路径**；只有"本地目录/文件"这种 spec 形态 CLI 才拒绝，需要直接改 jsonc。插件不能用自带 `node_modules` 里的 `effect` 打包。
- **[事实]** 钩子在每次工具调用的 fiber 内、被 `Effect.uninterruptibleMask(restore => …)` 包住 `restore` 区里执行，并通过 `Effect.forkScoped` 并发；钩子耗时推迟该次 `Tool.Success` 的发布时间。**并发只是逻辑上不互等**——它们仍共享同一个进程的 CPU、事件循环与你说过的任何队列，见 §6.3。钩子抛异常（defect）会让该 fiber 以 die 结束，`classifyToolExits` 记为本轮失败原因，**不是静默降级**。
- **[建议]** 薄 adapter → Rust 的边界：TS 侧只做"注册 `execute.after` + 构造请求 + 回填 `event.result.content` + 失败 fail-open（但**不吞用户取消/中断**）"；Rust 侧做压缩与缓存。传输用子进程 stdin/stdout（NDJSON），不要用 argv 传正文。原文缓存位置是**待定权衡**（项目内 / 系统 tmp / 宿主 data 各有代价，见 §5.2），本报告不裁定。
- **[建议]** 极短一行位置提示形态上可行：v2 的 `read` 本来就会输出 `Read file <path>, lines a-b` 头 + `[Output truncated. Continue reading with offset: N]`，模型对"路径 + 行号 + 续读"已有先验。但**"能指回原文"与"能指回当前磁盘文件"不等价**（见 §5.2）。

---

## 1. 为什么必须先分清 v1 / v2

**[事实]** v2 是一次破坏性重写：官方 `services/www/src/docs/content/migrate-v1.mdx:7-12` 明说三处 breaking change，其一即"Plugins use a new plugin API"；`:553` 加了 warning callout：**"V1 plugin implementations do not run in V2. Moving a file or renaming its config entry is not enough."**

**[事实]** 两代 API 的形态差异（都读过源码）：

| 维度 | v1.19.3 | v2.0.32 |
|---|---|---|
| 插件导出 | `export const X: Plugin = async (input) => ({ tool, "tool.execute.after": … })`（`packages/plugin/src/index.ts`，11.4 KB 的 hook 联合类型） | `export default Plugin.define({ id, effect })` 或 `{ id, setup }`（`packages/plugin/src/effect/plugin.ts`） |
| 工具钩子名 | `tool.execute.before` / `tool.execute.after` | `ctx.tool.hook("execute.before" \| "execute.after", …)` |
| 自定义工具 | `tool: { name: tool({...}) }` 或 `{tool,tools}/*.ts` 文件 | 只能 `ctx.tool.transform(editor => editor.add({...}))`，输入用 **JSON Schema**（`migrate-v1.mdx:219-262`） |
| shell 环境变量钩子 | `shell.env` | `ctx.shell.hook("create.before", e => e.env…)` |
| 插件发现目录 | `~/.config/opencode/plugin|plugins/`、`.opencode/plugin|plugins/` | 同名前缀，但目录条目发现列表是 `["plugin","plugins"]`（`packages/core/src/plugin/source-directory.ts`），且 config 的 `plugins` 数组是主入口 |
| 配置键 | `plugin: string[]`（旧键仍被兼容） | `plugins: (string \| {package, options?})[]`；旧 `plugin` 键经 `packages/core/src/config/normalize.ts:185-190` 归一合并进 `plugins` |

**[事实]** 兼容层的存在说明现实世界仍以 v1 形态为主：本机 `C:/Users/wudon/.config/opencode/opencode.json` 就是 `plugin: ["superpowers@git+https://github.com/obra/superpowers.git"]` + 空 `plugins/` 目录 + `@opencode-ai/plugin@1.14.23` 依赖。

**[建议]** Outrim 的 OpenCode 集成只面向 v2 写一套 adapter；v1 若要支持，需要第二套 adapter（钩子名、导出形状、工具注册方式全不同）。**本报告不估算 v1 的额外成本**（无任何测量依据）。是否支持 v1 属于产品决策，本研究不作裁决。

---

## 2. `execute.before` / `execute.after` 的精确契约

### 2.1 类型定义（v2 权威）

**[事实]** 类型声明见 `packages/plugin/src/effect/tool.ts:19-57`（`packages/plugin/src/promise/tool.ts` 是同一结构的 Promise 镜像）：

```ts
interface ToolHooks {
  "execute.before": {
    tool: string                              // 可变：可改路由
    readonly sessionID: Session.ID
    readonly agent: Agent.ID
    readonly messageID: SessionMessage.ID
    readonly id: Tool.CallID
    input: unknown                            // 可变
  }
  "execute.after": {
    readonly tool: string
    readonly sessionID: Session.ID
    readonly agent: Agent.ID
    readonly messageID: SessionMessage.ID
    readonly id: Tool.CallID
    readonly input: unknown
  } & (
    | { readonly status: "completed"; result: Tool.Result }   // result 可变（整对象可替换）
    | { readonly status: "error";     error: Tool.Error }     // error 可变（整对象可替换）
  )
}

interface ToolFailures extends Record<keyof ToolHooks, unknown> {
  readonly "execute.before": Tool.Error
  readonly "execute.after": never              // 唯一不能失败的钩子
}
```

**[事实]** `Tool.Result` 定义在 `packages/schema/src/tool.ts:88-92`：

```ts
{ readonly output?: unknown; readonly content?: string | ReadonlyArray<Content>; readonly metadata?: Metadata }
Metadata = Readonly<Record<string, any>>     // schema/tool.ts:64
Content  = TextContent{type:"text",text} | FileContent{type:"file",uri,mime,name?}
```

**[事实]** `execute.after` 的可写面精确等于：`result` 整个对象替换（含 `output` / `content` / `metadata` 三项），或 `error` 整个对象替换。`tool` / `sessionID` / `agent` / `messageID` / `id` / `input` 都是 `readonly`——想改工具名或参数只能去 `execute.before`。

**[事实]** `execute.after` 的失败类型是 `never`，源码带注释（`plugin/src/effect/tool.ts:48`）：**"Only execute.before may fail: a Tool.Error rejects the call before the tool runs."** 即：想在工具跑之前阻断，就在 before 里 `Effect.fail(new Tool.Error(...))`；after 没有"合法失败通道"。

### 2.2 触发点与调用链（读的是哪一版对象）

**[事实]** `packages/core/src/tool.ts:113-156` `executeTool()` 是唯一触发点：

```ts
const execution = yield* execute(tool, input, context).pipe(
  Effect.map(v => ({ value: v })),
  Effect.catchTag("Tool.Error", f => Effect.succeed({ failure: f })),   // 工具错误不抛出，转成值
)
// 失败分支
const afterEvent = { ...base, status: "error", error: execution.failure }
yield* hooks.trigger("tool", "execute.after", afterEvent)
return yield* afterEvent.error                                     // ← 返回"钩子改过之后"的 error
// 成功分支
const afterEvent = { ...base, status: "completed",
  result: { ...(output===undefined?{}:{output}), content, ...(metadata===undefined?{}:{metadata}) } }
yield* hooks.trigger("tool", "execute.after", afterEvent)
const afterContent = yield* normalizeImages(normalizeContent(afterEvent.result.content, afterEvent.result.output))
return { ...(output===undefined?{}:{output: afterEvent.result.output}),
         content: afterContent,
         ...(metadata===undefined?{}:{metadata: afterEvent.result.metadata}) }
```

三个要点，全部是"钩子最后说了算"：

1. **同一个事件对象**。`packages/core/src/plugin/hooks.ts:75-90` 的 `trigger` 按注册顺序 `yield* entry.callback(event)` 后 `return event`——不拷贝、不合并返回值。多个插件依次看到并叠加彼此的修改。
2. **失败分支返回的是 `afterEvent.error`**（`tool.ts:138`），不是原始 failure。官方内置插件 `packages/core/src/plugin/plan.ts:52-63` 就是这么用的：命中 `Permission.BlockedError` 时把 `event.error` 换成自己的 `ToolFailure`，端到端生效。
3. **`normalizeContent` 在钩子之后**（`packages/core/src/tool/runtime.ts`）：`content` 是字符串→包成一个 `text` 项；是非空数组→原样拷贝；两者都不是→用 `JSON.stringify(output)` 兜底。所以钩子可以把 `content` 设成字符串、项数组、或干脆只改 `output` 让系统替你序列化。

**[事实]** `packages/core/test/tool-registry.test.ts:883-899` 是上游自己的证明：注册一个把 `event.result` 整体替换成 `{output, content, metadata}` 的 `execute.after`，断言最终结果是替换值。这比读文档更硬。

**[事实]** `execute.before` 的返回值另有用途：`tool.ts:104-112` 的 `beforeExecute` 返回事件对象，`Tool.execute`（`:255-275`）用 `event.tool` 和 `event.input` 继续路由——所以 before 可以改工具名（别名修复）与输入（`packages/core/src/plugin/tool-input-repair.ts:29-41` 是现成例子）。

### 2.3 钩子之后还发生了什么（不能想当然）

**[事实]** 钩子退出后，结果依次经过：

1. `normalizeContent`（见上）。
2. `normalizeImages`（`packages/core/src/tool.ts:70-100`）：只处理 `content` 里 `type:"file"` 且 `mime` 以 `image/` 开头的项，做 base64 尺寸归一；解码/尺寸失败会**替换成一条文本** `[N image(s) omitted: could not be decoded.]` / `[N image(s) omitted: could not be resized below the image size limit.]`。→ **钩子放进来的图片也会被再处理一次**。
3. `ToolOutput.truncate`（`packages/core/src/session/runner/step.ts:124-128`，见 §3.2）。
4. `publisher.toolExecution` → `SessionEvent.Tool.Success`。这里有个防御性 `Effect.die("Tool execution has no content: <id>")`（`packages/core/src/session/runner/publish-llm-event.ts:613-629`），但因为 `normalizeContent` 永远至少产出 1 项，**这条 die 在实践中打不到**——见 §2.5，这反而是个陷阱。
5. 投影进会话 DB：`packages/core/src/session/message-updater.ts:343-360` 把 `event.data.content` 原样写进 `ToolStateCompleted.content`。
6. 下次请求时 `packages/core/src/session/runner/to-llm-message.ts:127-152`：单条文本 → `{type:"text", value}`，多条 → `{type:"content", value: content}`；**`output` 字段从不发给 provider**。

→ **结论：`content` 是唯一对模型可见的载体；钩子改完的 `content` 既是模型看到的，也是会话库里存的。** 不存在"库里存原文、模型看压缩版"的双轨。

**[建议]** 由此推出两条硬性实现约束：① 绝不能把 `content` 置成空数组却不补文本：`output` 缺失时会变成字面 `undefined`，`output` 存在时会把结构化 output 序列化后原样送出（见 §2.5）；② 若同时改 `content` 与 `output`，两者语义要一致——`output` 是 Code Mode 脚本看到的程序数据，不应为了压缩而改它（见 §4.2）。

### 2.4 Effect API 与 Promise API

**[事实]** 两套并行，行为一致：

- Effect：`ctx.tool.hook(name, cb)` 返回 `Effect<Registration, never, Scope>`（`plugin/src/effect/registration.ts`）；注销靠 Scope 关闭或 `registration.dispose`。
- Promise：`packages/plugin/src/promise/tool.ts` 的 `ToolContext = Omit<Tool.Context,"progress"> & {signal: AbortSignal; progress: Promise}`；`promise/adapter.ts:477-518` 把回调包在 `Effect.promise` 里 → **after 钩子抛异常会变成 defect（同 Effect 版）**，不是被吞掉。
- `setup` 里注册的钩子绑定到插件 scope（`promise/plugin.ts:61` 支持 `Promise<Cleanup | void>`）。

**[建议]** 用 Effect 版：`ctx.tool.hook` 的 `Scope` 语义天然对应"插件卸载即摘钩"，Promise 版要靠 cleanup 函数手工兜。

### 2.5 陷阱：空 `content` 不会报错，而会回落到 `stringify(output)`

**[事实+实测]** `normalizeContent`（`packages/core/src/tool/runtime.ts:261-266`）的兜底分支：

```ts
export const normalizeContent = (value, output?) => {
  if (typeof value === "string") return [{ type: "text", text: value }]
  if (value !== undefined && value.length > 0) return [...value]
  return [{ type: "text", text: stringify(output) }]        // stringify(undefined) === "undefined"
}
```

把 `event.result.content` 设成 `[]` 或 `undefined` 时，`normalizeContent` 会回落到 `stringify(output)`：

- **`output` 也缺失** → `stringify(undefined)` 产生字面文本 **`undefined`**（本地验证：`normalizeContent([], undefined)` → `[{"type":"text","text":"undefined"}]`）。
- **`output` 存在** → 得到 `JSON.stringify(output)`（本地验证：`normalizeContent([], {a:1})` → `[{"type":"text","text":"{\"a\":1}"}]`），**不是** `undefined`。

→ **后果**：压缩逻辑一旦"把所有文本删了但忘了补提示行"，模型会看到一条 `undefined`（或一段未经压缩的 JSON）——**会话不失败、日志不报警**，比抛异常更难发现。对于带 `output` schema 的工具（MCP、自定义工具），这意味着"压了 content 却没补文本"会把**完整结构化 output 序列化后原样送给模型**，压缩彻底失效。

**[建议]** adapter 出口加断言，但**别写成只允许数组**：

```ts
// 仅适用于 Outrim 自己允诺的"数组协议"分支；若允许回写 string，需同时接受非空 string
if (content === undefined || (Array.isArray(content) && content.length === 0)) 保持原结果
```

并为"压缩后为空"的退化路径（含 `output` 存在与不存在两种）写单测。

---

## 3. 关键问题：`execute.after` 拿到的是原文还是已截断？

**答案：要分工具看。** 有内部截断的工具（`shell`/`read`/`grep`/`glob`/`webfetch`）在钩子前已处于截断态；MCP、插件自定义工具、`websearch`、`mcp-resource` 在钩子里仍可超过 50 KiB。通用的 50 KiB 截断发生在钩子**之后**。下面给出逐条源码证据。

### 3.1 通用后置截断（钩子之后）

**[事实]** `packages/core/src/tool-output.ts`（全文 140 行左右）：

- 常量 `:13-16`：`MAX_LINES = 2_000`、`MAX_BYTES = 50 * 1024`、`RETENTION = Duration.days(7)`、`DIRECTORY = "tool-output"`。
- `:68` 第一行就是短路：**`if (result.metadata?.truncated !== undefined) return result`** —— 任何工具（或插件）只要在 `metadata` 里放了 `truncated` 键（哪怕是 `false`），通用截断**整个跳过**。
- `:69-76` 只统计 `text` 项（用换行符连接，尾部空行先 `pop`），用 `Buffer.byteLength(text, "utf-8")` 量字节；未超限直接返回 `{..., metadata:{..., truncated:false}}`，**不落盘**。
- `:78-85` 保留**头部**窗口：`for (const line of lines.slice(0, limits.maxLines))`，按字节预算累加。
- `:86-88` `const file = path.join(directory, fileName())` → `fs.ensureDir(directory)` → `fs.writeFileString(file, text)`：写的是**完整文本**（未裁的原串），位置 `global.data/tool-output/tool_<id>`。
- `:89-90` 提示串：`[showing ${kept.length > 0 ? "lines 1-N" : "0 lines"} of ${lines.length}; full output saved to ${file}]`。
- `:91-111` 逐项重建 `content`，用 `remaining` 字符预算逐项切片；`file` 项原样透传；marker 只插一次。
- `:113-116` 返回 `metadata: {...result.metadata, truncated: true, outputPath: file}`。
- `:38-58` + `:130-140` 清理：`cleanup()` 按保留期删 `tool_[0-9a-f]{12}*`，`Effect.repeat(Schedule.spaced(Duration.hours(1)))` 每小时一次。

**[事实]** 该文件可被配置放宽：`packages/core/src/config/plugin/tool-output.ts` 把 `opencode.json` 的 `tool_output.max_lines / max_bytes` 映射到 `ToolOutput.transform(editor.configure(...))`（文档 `config.mdx:307-313` 给了 `{"max_lines":2000,"max_bytes":51200}` 的例子）。**同一个配置值也决定了 shell 自己的截断阈值**（见 3.2），改它会同时影响两处。

### 3.2 钩子前是否已截断，逐工具区分（核心事实）

**关键区分**：`ToolOutput.truncate` 是在 `execute.after` **之后**才执行的（`step.ts:118-133`）。所以"钩子看到的量"取决于**工具自己有没有内部截断**，不能一概而论：

| 工具 | 工具内部截断（在钩子**之前**） | 内部阈值 | 钩子可见上限 | `metadata` | 原文出口 |
|---|---|---|---|---|---|
| `shell` | **尾部**（保留最后 N 行） | 2000 行 / 50 KiB（读 `tool_output` 配置） | 2000 行 / 50 KiB | `{status, truncated, exit?, signal?, timeout?, shellID?}` | `global.data/shell/<projectID>/<shellID>.out`（7 天 + 最近 25 条）+ `Shell.output(id,{cursor,limit})` |
| `read` | **头部**（保留前 N 行）+ 单行截断 | 2000 行 / 50 KiB，单行 2000 字符 | 2000 行 / 50 KiB | `{truncated}`（媒体文件恒 `false`） | **当前磁盘文件**，`offset`/`limit` 续读（≠ 当时的原文） |
| `grep` | 条数 | 默认 100 匹配行（`limit+1` 探测） | 100 条 | `{matches, truncated}` | **无**，只能重跑或收窄参数 |
| `glob` | 条数 | 默认 100 条 | 100 条 | `{count, truncated}` | **无**，重跑 |
| `webfetch` | 响应体字节 | 5 MiB | 5 MiB | `{contentType}` | **无**（工具内部上限，**不是**后置通用落盘） |
| `websearch` | **无内部截断** | — | 仅 provider 返回量 | `{provider}` | **无**（若超 50 KiB，溢出文件在钩子后才产生） |
| `mcp-resource` | **无内部截断** | — | 不受 50 KiB 限制 | 通用 `{truncated, outputPath}`（**钩子之后**才产生） | 仅后置溢出文件，且**压小后不会创建** |
| MCP server tool | **无内部截断** | — | 不受 50 KiB 限制（受 16 MiB stdio 帧上限） | 同上 | 同 |
| 插件自定义工具 | **无内部截断**（除非自己截） | — | 不受 50 KiB 限制 | 同上 | 同 |
| `edit`/`write`/`patch` | — | — | — | `{files: [...]}` | 产物即文件 |
| `question` | — | — | — | `{answers}` | — |
| `skill` | — | — | — | `{name, directory}` | — |
| `subagent`（后台） | 只回占位串 | — | — | `{sessionID, status}` | 子会话 |

（逐条依据：`packages/core/src/tool/plugin/{shell,read,grep,glob,webfetch,websearch,edit,write,patch,question,skill,subagent,mcp-resource}.ts`，`packages/core/src/tool/mcp.ts`，`packages/core/src/tool/read-filesystem.ts`，`packages/core/src/shell.ts`，`packages/core/src/tool-output.ts`。）

→ **"钩子上限 = 50 KiB" 是错的**。对 `shell`/`read`/`grep`/`glob`/`webfetch` 成立（它们自己截），对 MCP/自定义/`websearch`/`mcp-resource` **不成立**：这些工具的 `content` 在钩子里可以是任意大小，50 KiB 是它们**经过**钩子之后才被通用截断的。

#### `shell`（最重要的一条）

**[事实]** 采集逻辑在 `packages/core/src/shell.ts:228-252`（tag `843a6c5` 行号；HEAD `501b15a` 为 `:236-260`，代码逐字相同）：

```ts
const limits = Config.latest(config.entries(), "tool_output")
const maxLines = limits?.max_lines ?? ToolOutput.MAX_LINES
const maxBytes = limits?.max_bytes ?? ToolOutput.MAX_BYTES
const latest = yield* output(id, { cursor: Number.MAX_SAFE_INTEGER })     // 只为拿 size
const page   = yield* output(id, { cursor: Math.max(0, latest.size - maxBytes), limit: maxBytes })
// 去尾空行 → truncated = latest.size > maxBytes || lines.length > maxLines
// text = lines.length > maxLines ? lines.slice(-maxLines).join("\n") : page.output
// 末尾追加：`\n\n[showing lines ${total-shown+1}-${total} of ${total}; full output saved to ${info.file}]`
```

→ **钩子里的 `result.content` 是"最后 N 行 + 一段提示"**，不是完整输出；完整输出在 `global.data/shell/<projectID>/<shellID>.out`，路径**已经写在提示文本里**，也在 `metadata` 之外（`metadata` 里只有 `shellID`，没有 `file`）。

**[事实]** `Shell.output(id, {cursor, limit})`（`shell.ts` 同文件）是字节分页接口，默认 `limit = 65536`；`Shell.result()` 自己就用它做尾部取窗，说明"读全量"是官方支持的路径。

**[事实]** 工具层再包一层（`packages/core/src/tool/plugin/shell.ts:60-105`）：`resultMessages(output)` = `[...(output.output ? [output.output] : []), ...(notice ? [notice] : [])]`，`notice` 由 `ShellResult.notice` 生成（`Timed out before completion` / `Killed by <signal>` / `Exited with code <n>`），最后 `join("\n\n")`。元数据合并成 `{status, ...ShellResult.metadata(output), shellID?}`。

**[事实]** 超时（`shell.ts:60-105` + `:200-270`）：`DEFAULT_TIMEOUT_MS = 2 * 60 * 1_000`；超时后内容追加 `\n\nCommand exceeded timeout of <ms> ms. Retry with a larger timeout if the command is expected to take longer.`；`timeout: 0` 关闭超时。

**[事实]** 后台执行**绕开钩子**：`input.background === true` 时立即返回 `Command moved to the background (shell ID: <id>).\nOutput is streaming to: <file>`（`truncated:false, status:"running"`），完成后由 `notifyWhenDone` 调 `sessions.synthetic({...})` 把 `<shell id="…" state="…" command="…">` 形式的合成消息塞进会话（`tool/plugin/shell.ts:170-200`）。**这条路径没有 `execute.after`。** 同理，前台命令被 `jobs.background(job.id)` 降级为后台时也一样。

#### `read`

**[事实]** `packages/core/src/tool/plugin/read.ts:60-140`：

- `toModelContent` 生成 `Read file <path>, lines a-b`（或 `Read directory <path>, entries a-b`），正文每行 `<n>: <line>`，被截断时补 `[Output truncated. Continue reading with offset: N]`。工具描述已明说"Each text line is prefixed by its 1-based line number as `<line>: <content>`"。
- 媒体（图片 / PDF）走 `[{type:"text", text:"Image read successfully" | "PDF read successfully"}, {type:"file", uri:"data:<mime>;base64,…", mime, name: path}]`，`metadata.truncated = false`。
- 错误是 `ToolFailure`：`Unable to read <path>`、`File not found: <path>`（附 `Did you mean one of these?`）、`Cannot read binary file: …`、`Media exceeds <n> byte ingestion limit: …`、`Offset <n> is out of range for this file (…)`、`Path is not <kind>: …`。
- 成功后还会向上找 `AGENTS.md` 并加载为会话指令（`sessionInstructions.load`），外部目录跳过，失败静默——**这是与内容改写无关的副作用，压缩不影响它**。

**[事实]** 分页常量在 `packages/core/src/tool/read-filesystem.ts:14-22`：`MAX_READ_LINES = 2_000`、`MAX_READ_BYTES = 50 * 1024`、`MAX_MEDIA_INGEST_BYTES = 20 * 1024 * 1024`、`MAX_LINE_LENGTH = 2_000`（`MAX_LINE_SUFFIX = "... (line truncated to 2000 chars)"`）。

#### `grep` / `glob`

**[事实]** `packages/core/src/tool/plugin/grep.ts`：`limit` 默认 `FileSystem.DEFAULT_SEARCH_LIMIT`（100），内部请求 `limit + 1` 探测是否还有更多；输出 `toModelContent` 形如 `Found N matches` / `<path>:` / `  Line <n>: <text>`，截断时附 `(Results are truncated: showing first N results. Consider using a more specific path or pattern.)`；`metadata: {matches, truncated}`；`output` 是 `Match[]` 结构化数组。超时 → `Search timed out after 30 seconds. …`；非法正则 → `Invalid regex pattern: …`。

**[事实]** `glob.ts:127` 同构：`metadata: {count, truncated}`。

→ **这两个工具的超限内容在进程内没有任何原文副本**（既无文件也无结构化全量），唯一恢复手段是带着更窄的 `path`/`include`/`pattern` 重跑。

#### `webfetch` / `websearch`

**[事实]** 两者的"无出口"性质不同：`webfetch` 有**工具内部**的 5 MiB 响应体上限（`packages/core/src/tool/html-markdown.ts:50` 的 `MAX_MARKDOWN_BYTES`，`webfetch.ts:13` 的 `MAX_RESPONSE_BYTES` 引用同一值）；`websearch` **没有工具内部上限**，完全取决于 provider 返回多少。两者都**没有**原文落盘——但若是它们超过 50 KiB，那是 `ToolOutput.truncate` **过了钩子之后**才写的溢出文件，不在钩子视野内。

### 3.3 v1 对照（结论相同、提示更啰嗦）

**[事实]** v1 `packages/opencode/src/session/tools.ts:96-125`：`plugin.trigger("tool.execute.before", …)` → `item.execute(args, ctx)` → 给 attachments 补 id → `output = {...result}` → `plugin.trigger("tool.execute.after", …, output)` → abort 时仍 `completeToolCall` 记录结果。

**[事实]** 而 `item.execute` 内部（`packages/opencode/src/tool/tool.ts:90-150` `wrap()`）已经跑过 `Truncate.output(...)`。v1 的 `truncate.ts` 常量同为 2000 行 / 50 KiB，落盘目录 `packages/opencode/src/tool/truncation-dir.ts:4` = `Global.Path.data + "/tool-output"`，并且**提示文本很长**：

> `The tool call succeeded but the output was truncated. Full output saved to: <file>`
> `Use the Task tool to have explore agent process this file with Grep and Read (with offset/limit). Do NOT read the full file yourself - delegate to save context.`

→ 这就是用户明确否掉的"免责声明式提示"。**Outrim 的提示应摒弃这类免责声明**（具体长度未测，本报告不给倍数估算）。

### 3.4 由此得出的四条设计事实

1. **工具内部截断是硬上限，自己缓存 after 看到的输入不能突破它**。对 `shell`/`read`/`grep`/`glob`/`webfetch` 而言，钩子看到的已经是工具裁过的；把这份（已截）内容写进 Outrim 缓存，存到的也只是同一份截断文本。想拿更多只有两条路：自己接管该工具的 `execute`（但**接管同样拿不到该工具内部未截断的中间值**，除非重写其采集逻辑），或改宿主配置把内部阈值调大（`tool_output.max_lines/max_bytes` 只影响 `shell` 与通用截断，不影响 `grep`/`glob`/`webfetch` 的常量）。
2. **对上表后半段（MCP、自定义工具、`websearch`、`mcp-resource`）自己落盘钩子看到的 `content` 才有价值**：那是这些工具唯一一次原文经过的位置。
3. **`metadata.truncated` 是通用截断的总开关**（`:68` 第一行 `if (result.metadata?.truncated !== undefined) return result`）——设任何值（含 `false`）都会跳过通用截断，这是源码事实。**但不要把它列为无条件做法**：首要原则是**保留工具原有的 `metadata` 语义**（工具原本没这个键就不要凭空加）；只有在压缩后输出确实已落在安全尺寸内、且不改变原有截断语义时才可能涉及，**绝不能用来绕过宿主的安全上限**（例如把超大 MCP 结果原样放行）。
4. **`shell` 的 `.out` 文件与 `read` 的磁盘文件都存在，但都不是"钩子那一刻的原文"的直接句柄**：`.out` 是 shell 原始输出的可靠副本（只受 7 天/25 条寿命约束）；`read` 的磁盘文件随时会被后续编辑改写，且 `read` 自身的 2000 行/50 KiB 窗口与单行 2000 字符截断已丢失信息（见 §5.2）。

---

## 4. 覆盖范围：一个钩子能盖住多少

### 4.1 内置工具即插件

**[事实]** v2 里内置工具本身就是插件：`packages/core/src/plugin/internal.ts:71-110` 导入 `ReadTool.Plugin` / `ShellTool.Plugin` / `GrepTool.Plugin` / …，每个 `tool/plugin/*.ts` 导出 `{ id: "opencode.tool.read", effect }` 并用 `ctx.tool.transform(editor => editor.add({...}))` 注册。

→ 两点收益：① `execute.after` 对内置工具与第三方工具**完全同构**，不需要逐工具适配；② `editor.update("read", fn)` 能统一包装任意内置工具的 `execute`，是"拿原文"的统一入口（代价见 §8）。

### 4.2 Code Mode（`execute`）

**[事实]** `packages/core/src/tool.ts:255-275` 的路由顺序：`beforeExecute` → 若 `name === "execute"` 且 codemode 可用则走 `CodeModeTool` → 否则 `direct.get(name)` → 否则 `Tool.Error("No tool named \"<name>\" is currently available…")`。

**[事实]** `packages/core/src/codemode/tool.ts:94-108`：`execute` 的 `execute()` 通过注入的 `executeTool(name, tool, input, context)` 逐个调用子工具，**每个子调用都完整走一遍 `execute.before` / `execute.after`**（`event.tool` 是子工具名，`event.id` 是同一个 `Tool.CallID`）；子结果的处理是：

```ts
const content = typeof executed.content === "string"
  ? [{ type: "text", text: executed.content }]
  : (executed.content ?? [])
const outputFileParts = outputFiles(content)          // file 项抽出来，不进模型正文
if (executed.output !== undefined) return executed.output        // ← 关键：先用 output，不看 content
const text = content.flatMap(p => p.type === "text" ? [p.text] : []).join("\n")
return text === "" ? null : text
```

最终 `content = [{type:"text", text: formatResult(result)}, ...files as data: URL]`（`:120-130`），`metadata = {toolCalls, error?}`。`formatResult`（`:310-322`）= `formatValue(result.value)` + `Warnings:` + `Logs:`，而 `formatValue` 对非字符串直接 `JSON.stringify(value, null, 2)`。

→ **重大语义：只压子工具的 `content` 不保证能省模型 token。** 因为 `if (executed.output !== undefined) return executed.output` 在 `content` **之前**：带 `output` schema 的工具（MCP 服务器给了 `outputSchema` 的、插件自己声明了 `output` 的）会把**未修改的结构化 `output`** 交给 JS；JS 若把该值 `return` 出去，外层 `formatValue` 会 `JSON.stringify` 出完整原文放进外层 `execute` 的 `content`。此时子调用被压小了，模型看到的外层结果仍是大的。

→ **反过来压或删 `output` 也不行**：那是脚本内的程序数据，改它等于篡改工具返回值，会破坏依赖该结构的脚本逻辑与 `outputSchema` 语义。**不应以压缩为目的修改 `output`。**

→ **正确的边界候选**（**需验证，本报告不下结论**）：保留 `output` 原值（程序数据不动），只压子调用 `content`；真正想省 token 得在**外层 `execute` 的 `content`** 上动手（那一层才是模型可见的最终输出），但需要先验证：① 子调用的 `execute.after` 是否真的会在会话里产生独立的模型可见消息（本报告**未验证**；可观测的是 `event.id` 相同）；② 同一 `CallID` 下多次子调用是否都各自进 LLM（**不能默认成立**）。

**[事实]** 哪些工具走 Code Mode：`read/shell/grep/glob/webfetch/websearch/edit/write/patch/question/skill/subagent` 都是 `options: {codemode: false}`（直连）；`opencode.*` 元工具与 `mcp-resource` 是 `{namespace:"opencode", codemode:true}`。MCP 工具默认 codemode（`packages/core/src/mcp/index.ts:305-310` 把 server 配置的 `codemode` 传下来；`plugin/mcp-codemode-defaults.ts` 对 executor.sh 等强制 `false`）。文档 `tools.mdx:216-220` 亦确认："Denying `execute` with resource `*` removes Code Mode and every tool reached only through it, including MCP tools from servers that do not set `codemode: false`."

→ **对 Outrim 的含义**：如果用户或策略禁掉了 `execute`，所有 codemode MCP 工具一起消失；Outrim 的覆盖不受影响（它们本来也不会跑）。但若想"只在入口压一次"，子调用与外层 `execute` 是**两层**，且外层才是模型可见的最终文本（§4.2）。

**其他注意点**：① 子工具的 `file` 项会被提升到外层 `files`，不属于文本压缩面；② 外层 `execute` 自己也会触发一次 `execute.after`（`event.tool === "execute"`），要防重复处理，但**不要求**代数/幂等性设计——按 key 标记已处理即可，需同时防递归（对已处理过的内容再压）。③ 同一 `Tool.CallID` 下可能有多个子调用，用 `(callID, tool, 序号)` 做缓存键可避开碰撞，但**不要**假定每个子调用都会独立进入模型。

### 4.3 MCP

**[事实]** `packages/core/src/tool/mcp.ts:40-152`：每个 MCP tool 注册成一等 `Tool.Info`：

```ts
editor.add({ name: tool.name,
  options: { namespace: sanitize(tool.server), codemode: tool.codemode !== false },
  input:  tool.inputSchema  ?? { type:"object", properties:{} },
  output: tool.outputSchema ?? {},
  execute })
```

- `name` = `<server>_<tool>`（非法字符替 `_`）；权限 action 同名，`resources:["*"]`。
- 成功：`output = structured ?? (outputSchema === undefined && text 以 {/[ 开头 ? JSON.parse(text) : text)`；`content` = 文本项 + `{type:"file", uri:"data:<mime>;base64,…", mime}`；空内容时**省略 `content` 键**（于是落到通用截断/`normalizeContent`）。
- 失败：`result.isError` → `ToolFailure`（拼接文本或 `MCP tool returned an error`）→ 走 `status:"error"` 的 after 事件；`MCP.NotFoundError` → `MCP server "<x>" is not available`；其它 → `Unable to execute <server>_<tool>`。
- 重新注册：`McpEvent.ToolsChanged` 上 `lock.withPermit` + `Stream.debounce("100 millis")` → `tools.reload()`。
- 传输上限：`packages/core/src/mcp/stdio.ts:15` `MAX_FRAME_BYTES = 16 MiB`（错误 `MCP stdio frame exceeded 16 MiB`）；客户端超时 `startup 30s / catalog 30s / execution 12h`（`mcp/client.ts:34-36`）。

→ **v2 没有 20 KiB MCP 上限**（那是 v1 与 Pi 的特征）。MCP 工具**自己不做截断**，所以钩子里的 `content` 可以远大于 50 KiB（只受 16 MiB stdio 帧与 12 h 执行超时约束）；50 KiB 是它**过了钩子之后**才被 `ToolOutput.truncate` 施加的，且那时才写溢出文件——**若 Outrim 先把内容压小，溢出文件根本不会产生**。

### 4.4 自定义工具

**[事实]** v2 已无 v1 的 `{tool,tools}/*.ts` 目录约定（`migrate-v1.mdx:220-262` 只给 `ctx.tool.transform` 一种迁移路径；`packages/core/src/config/plugin/` 下没有 tool 目录扫描器）。因此第三方工具 = 插件工具，天然经过同一钩子。

**[事实]** 工具名会规范化：`packages/core/src/tool/runtime.ts` 的 `normalizedName = name.replace(/[^a-zA-Z0-9_-]/g,"_")`，`effectiveName` = `namespace.replaceAll(".","_") + "_" + name`（`tool.ts:300-340` 的注册校验也拒绝非法名）。→ Outrim 匹配工具名要按**规范化后**的名字比对（如 MCP 的 `github_search_issues`）。

### 4.5 覆盖缺口（必须写进产品约束）

**[事实]** 以下路径**不经过** `execute.after`：

1. **后台 shell**：`tool/plugin/shell.ts:158-186` 的 `notifyWhenDone` 在命令结束时调 `sessions.synthetic({sessionID, description: command, ...ShellResult.notification({...})})`，把完成结果作为 **`Session.Message.Shell` 类型的 inbox 项**注入（schema 见 `packages/schema/src/session-message.ts:111-124`，字段 `{command, shellID, status, output?}`），随后 `to-llm-message.ts:301-310` 把它渲染成 `The following shell command was executed by the user: …` 交给模型。**这条路径既不过 `execute.after`，也不过 `session.hook("prompt")`**——`sessions.synthetic`（`packages/core/src/session/session.ts:273-315`）直接 `admission.admit(...)`，而 `session.hook("prompt")` 只在 `SessionPrompt.prepare`（`packages/core/src/session/prompt.ts:40`）里触发，两者是并行入口。
2. **后台 subagent**：同样只回占位串，完成时以合成消息注入（`tool/plugin/subagent.ts:264` 的 `metadata:{sessionID,status}`）。
3. **provider hosted 工具**（provider 端执行、结果随流回传）：`step.ts:104` 里 `if (event.type !== "tool-call" || event.providerExecuted) return` 直接跳过本地执行；结果走 `publish-llm-event.ts` 的 `tool-result` 分支 + `hostedContent()`。
4. **根本没有进入工具执行的失败**：这类失败连 `execute.after` 都不触发。三处：① `toolChoice === "none"` 时 `SessionStep.executeTool` 直接 `Effect.fail(new Tool.Error({message:"Tools are disabled after the maximum agent steps"}))`（`step.ts:91-100`），不调用 `snapshot.execute`；② 工具未注册 / 被会话上下文移除：`Tool.snapshot.execute` 在 `beforeExecute` 之后直接返回 `Tool.Error("Tool is not available for this request: …")`（`tool.ts:275`）或 `"No tool named …"`（`tool.ts:281-283`），不进入 `executeTool`；③ `execute.before` 里主动 `Effect.fail(new Tool.Error(...))` 的阻断。共同点：`execute.before` 可能触发（③ 必触发），`execute.after` 一定不触发，事件流里只出现 `Tool.Failed`。

**[事实]** 反向的边界同样要记住：`Tool.execute` **内部**的失败（入参解码失败、输出 schema 校验失败、工具自己返回 `Tool.Error`）会触发 `execute.after`（因为 `tool/runtime.ts` 的 `execute()` 把它们收敛成 `Tool.Error` 值），只有"没进 `Tool.execute`"的才不触发。

**[建议]** 只压"有内容价值"的结果：`status === "completed"` 且 `content` 含文本且估算 token 超阈值。`status === "error"` 一律放行（错误信息短且高信息密度，压了反而糟）。

---

## 5. 原文缓存与"能否复用"

### 5.1 现成的原文出口

| 来源 | 位置 | 寿命 | 可复用性（重要：都不是钩子内的句柄） |
|---|---|---|---|
| `shell` 完整输出 | `global.data/shell/<projectID>/<shellID>.out` | 7 天（`RETENTION`）+ 最近 25 条已退出（`EXITED_LIMIT`）——**有限寿命，不是保证** | **中高**：路径在 `content` 文本里 / 可用 `shellID` 拼；`Shell.output(id,{cursor,limit})` 可字节分页。但需验证路径推导与 override 兼容（§5.2） |
| 通用溢出 | `global.data/tool-output/tool_<id>` | 7 天，每小时清理 | **低**：**在钩子之后才创建**；只有 `metadata.outputPath` 指向它，仓库内无消费者。**若先压小，这个文件根本不存在**，不能当作 after 阶段的原文缓存 |
| `read` 的当前磁盘文件 | 项目内路径 | 直到下次编辑 | **不等价于原文**：文件可能已被编辑，且 `read` 自身的行窗口/单行 2000 字符截断已丢信息 |
| `grep`/`glob` | 无 | — | **无**：只能重跑 |
| `webfetch` | 无（5 MiB 工具内部上限） | — | **无** |
| `websearch` | 无（无内部上限） | — | **无** |
| MCP / 自定义工具 | 仅通用溢出 | 7 天 | **同上，钩子内不可得** |

→ **结论：钩子阶段唯一可靠的原文来源是自己把实际收到的输入落盘**，而不是指望宿主已有的文件。

**[事实]** `ToolOutput.truncate` 写文件用的是**完整文本**（`:95` 写 `text`，即所有 text 项 `\n` 拼接后的全文），但只统计 `text` 项——如果工具返回的是"结构化 `output` + 短 `content`"，落盘的是短文本，不是结构化全量。

**[事实] 全局路径解析（`packages/util/src/global-roots.ts:1-19` + `packages/util/src/global.ts:8-22`）**：`data = XDG_DATA_HOME || ~/.local/share`、`cache = XDG_CACHE_HOME || ~/.cache`、`config = XDG_CONFIG_HOME || ~/.config`、`state = XDG_STATE_HOME || ~/.local/state`、`tmp = os.tmpdir()`，再各自拼 `/opencode`。**Windows 上也一样用 `~/.local/share/opencode`**（本机实测即是），不是 `%LOCALAPPDATA%`；`bin = <cache>/bin`（ripgrep 二进制就落在这里）。`home` 可用 `OPENCODE_TEST_HOME` 覆盖。

**[事实]** 这两处目录都属于"项目外"：`packages/core/src/file-access.ts:96-123` 判定 internal 的条件是"在 `location.directory` 内或在 `location.project.directory` 内"，否则构造 `externalDirectory` 检查。

**[事实]** 关于 `external_directory` 的默认效果，**读源码与测试后确认是 allow**：`packages/core/src/permission.ts:86-100` 的 `evaluate()` 用 `findLast` 在全部规则里找通配匹配，找不到才回落到 `{action, resource:"*", effect:"ask"}`；而默认 agent 规则集带 `{action:"*",resource:"*",effect:"allow"}`（`packages/schema/src/agent.ts:41-49`），所以 `external_directory` 命中 allow。`packages/core/test/agent.test.ts:151-162,198-205` 直接断言了 `global.data/shell/*/*`、`global.data/tool-output/*`、`global.config/*`、`/outside/*` 的 `external_directory` 都是 `allow`。文档 `permissions.mdx:143-144` 与 `migrate-v1.mdx:126-129` 也一致。唯一例外是用户显式加了 `{action:"external_directory",resource:"*",effect:"ask"}`（文档把它当作"恢复 v1 行为"的推荐做法）。

→ **这只能说明"读项目外的文件默认不弹权限"，不能说项目内缓存就"免掉一切权限分支"**：`read` 自身还有 `read` 动作的权限评估（`authorizeRead`），且 `external_directory` 可被用户策略改成 `ask`/`deny`。

### 5.2 Outrim 自己的缓存（建议，待定权衡）

**[建议] 原文保存是必需的**：既然宿主已有的文件都不在钩子内可得（§5.1），Outrim 必须把钩子实际收到的 `content` 自己落盘，否则没有可指向的东西。缓存位置是**权衡题，本报告不裁定**：

| 位置 | 优点 | 风险 / 代价 |
|---|---|---|
| 项目内（如 `.outrim/cache/`） | 若目录可写，`read` 命中的是 internal 分支（仍要过 `read` 权限评估）；删仓即带走 | **不是既定方案**：可能被 `git add`/索引/备份/`grep` 扫入，必须显式 `.gitignore` 或等效排除；多仓/只读仓会失败；**需自己的容量上限与清理**（不会"自然回收"） |
| 系统 tmp（`os.tmpdir()`） | 不污染仓库；重启可清 | 可能被系统清理（寿命不可控）；Windows 上 `%TEMP%` 可被其他进程读到；路径不稳定 |
| 宿主 data（`~/.local/share/opencode/...`） | 与 OpenCode 自己的生命周期/清理习惯一致；`external_directory` 默认 allow | 依赖未暴露的 `global.data`，需自己按 XDG 规则拼；受 `external_directory` 策略变影响 |

**[事实]** 无论选哪里，**两件事不能当作保证**：① `.outrim/cache/` 不会被 git 扫入（必须显式排除）；② 项目生命周期等回收等于不漏盘（需 set 上限/定时清理，用户可能用 `git clean` 但不会自动清 `~/.local/share`）。

**[事实] 权限：`0600` 在 Windows 上不是 ACL 保证**。`writePluginConfig` 的 `mode: 0o600` 是 POSIX 语义；Windows 上 Node 只把它当只读标志处理，**不等于"仅本用户可读"**。若原文含敏感内容（env、密钥、客户数据），缓存目录的访问控制需要单独设计（或至少说明未做）。

**[事实] 自己拼 `shell` 路径不是无风险的**：`path.join(global.data, "shell", location.project.id, shellID + ".out")`（tag `843a6c5` 的 `packages/core/src/shell.ts:136` `outputDir`，HEAD `501b15a` 为 `:144`）能算出来，但依赖 ① XDG/`OPENCODE_TEST_HOME` override 与 XDG 规则在其他平台/包装器上一致；② 目录名 `"shell"`（`DIRECTORY`）+ `${id}.out`（tag `843a6c5` 的 `:279`，HEAD `501b15a` 的 `:287`）不变；③ **文件存在不能证明它对应的就是这次钩子看到的输出**——只说明同 ID 的文件在。→ **建议把它作为候选 fallback（以尺寸/时间戳/内容前缀交叉校验），而不是必然路径**；真正可靠的做法仍是自己存一份。

**[建议] 提示行要区分三种语义**（这直接影响"恢复"是否真能找回当初的内容）：

1. **能指回当初原文**（Outrim 自己存的完整输入）：可以说"原文在 X"。
2. **只能指回当前磁盘文件**（`read`）：**不能**暗示它就是当时的原文——文件可能已经变了，且当时的行窗口/单行截断信息已丢。提示行应只说位置与行号区间，不说"完整"。
3. **只能指回宿主自己的临时副本**（`shell` 的 `.out`）：寿命 7 天 / 最近 25 条，**不是永久保证**；用户 resume/fork 一个旧会话时指针可能已失效。

**[事实]** 用户 resume / fork 旧会话会让指针失效（`.out` 被 7 天清理、`tool-output` 被 7 天清理、`read` 指向的文件已改）→ **指针的长期有效性需要单独设计**（例如在提示行内带上校验信息，或缓存在项目内并纳入仓库生命周期）。本报告**不**给出设计答案。

**[建议]** 缓存键至少包含 `sessionID + callID + tool`（Code Mode 下同 callID 可能有多个子调用，加序号可避碰撞）；写入用 `wx`（已存在即失败）+ 失败即放弃压缩、原样放行。`0600` 在 POSIX 上可给，Windows 上不要当作安全保证。

**[建议] `read` 例外可以不缓存，但那是省存储的取舍，不是正确性保证**：`read` 的原文仍可由 `offset`/`limit` 续读，但那只对"当前磁盘文件"成立；文件被编辑、`read` 窗口与单行 2000 字符截断都已丢失的信息不会因续读而回来。若提示行要靠它，必须按 §5.2 第 2 条措辞（只给位置与行号区间，不说"完整"）。

---

## 6. 错误、取消、并发、附件

### 6.1 钩子所处的位置

**[事实]** `packages/core/src/session/runner/step.ts:118-133`：

```ts
toolRuns.push({ call: event, fiber: yield* Effect.uninterruptibleMask((restore) =>
  restore(executeTool(event)).pipe(
    Effect.flatMap(toolOutput.truncate),
    Effect.flatMap(outcome => publisher.toolExecution(event.id, event.name, outcome)),
    Effect.catchTag("Tool.Error", error => publisher.failTool(...).pipe(Effect.asVoid)),
  )).pipe(Effect.forkScoped) })
```

三点推论：

1. **`execute.after` 在 `uninterruptibleMask` 的 `restore` 区内**——可以被中断，但 `truncate` 与 `toolExecution` 在 restore 外，属于不可中断段。→ 压缩最好在钩子里做完，别指望"在 truncate 阶段再补一次"。
2. **每个 tool call 一条独立 fiber**（`forkScoped`），所以并行调用之间**不互等**；但同一调用内钩子耗时**直接叠加**在该次 `Tool.Success` 的发布时间上（模型要等它）。**"不互等"不等于"互不影响"**：它们仍共享同一进程的 CPU、事件循环与 Rust 侧队列，见 §6.3。
3. `Tool.Error` 在同一 fiber 内被 `catchTag` 吞掉并转成 `failTool`，不会冒泡成步骤失败。

**[事实]** 钩子抛异常（defect）的后果：`classifyToolExits`（`step.ts:296-325`）把每次 fiber 的 `Cause` 分两类——typed failure（用户拒绝）与 die 原因；**die 原因会被记为 `tools.failure`**，最终映射成该轮 step 失败。所以"钩子出错 = 用户看到失败"，**不是静默降级**。

**[建议]** 钩子必须 **fail-open**：Rust 调用失败、超时、解析错误都不应外泄 defect。但**不要用 `catchAllCause` 无脑吞掉一切**——`Cause` 里包含**用户取消/中断**（§6.2 的 `interruptTools`），吞掉它等于让压缩掩盖 abort。正确形态是：只捕获工具/序列化/自身超时这类可预期失败，**interrupt 继续传播**：

```ts
// 示意：catch 可预期失败，但不吞 interrupt
Effect.catchTag("...", () => Effect.void)  // 或 Effect.catchIf(cause => !Cause.isInterrupted(cause), () => Effect.void)
```

自身超时的具体数值是**实验参数而非承诺**（本报告未测），超时即放行。超时后的清理/子进程回收也**需实测验证**（不能假定 `Scope` 会自动杀掉外部进程）。

### 6.2 取消与中断

**[事实]** 流中断 / 用户中止时：

- `step.ts:200-220`：`if (streamInterrupted) yield* interruptTools`（`Fiber.interruptAll`），随后 `Fiber.awaitAll(toolRuns)`。
- `TOOLS_INTERRUPTED = {type:"aborted", message:"Tool execution interrupted"}`（`step.ts:62`）、`STEP_INTERRUPTED`、`RESULT_MISSING = {type:"tool.result-missing", message:"Provider did not return a tool result"}`。
- 未结算的工具调用被 `failUnsettledTools` 补成 `Tool.Failed`；残留的 `streaming`/`running` 状态在 `llm.ts:367-384 settleStaleToolCalls` 里补成 `{type:"aborted", message:"Tool execution interrupted: <name>"}`。
- 权限拒绝是**类型化失败**（`Permission.DeclinedError`），且被有意做成 defect 隧道穿越掩码，最终在 `SessionModelRequest.executeTool` 边界重新变回 typed failure（`permission.ts` 注释原文）。用户选择"拒绝并给反馈"则是 `CorrectedError{feedback}`，保持 typed。

→ **含义**：钩子若正在 await Rust，可能被中断；Rust 子进程必须能被杀死且不留下半写文件（先写 `.tmp` 再 `rename`）。

### 6.3 并发

**[事实]** 并行工具调用是独立 fiber（见 6.1），但**同一调用内的 `execute.after` 链是串行的**（`hooks.trigger` 顺序执行）；多个插件之间也是串行、后看到前者的修改。

**[事实]** Code Mode 内的子调用：`Semaphore.makeUnsafe(1)` 只保护 `calls` 数组与 `context.progress({toolCalls})`（`codemode/tool.ts:80-100`），子工具本身可以并发（`Promise.allSettled`）。→ **Outrim 的钩子会被并发调用**，Rust 侧必须能处理并发或做本地排队；缓存写盘按 callID 分文件，不要共享可变句柄。

**[事实]** 并发只是"逻辑上不互等"：所有钩子共用同一个 Node/OpenCode 进程的 CPU 与事件循环，也共用 Rust 侧的进程/队列。若 Rust 侧串行处理，则每次钩子的实际延迟 = 排队等待 + 处理时间，高峰期可能显著超过单次处理耗时。**这一点未测**，但不应写成"互不阻塞"。

**[事实]** v2 没有 v1 那种 `toolExecution: "sequential" | "parallel"` 配置（两处 grep 均无命中）；并行是默认行为。

### 6.4 附件 / 媒体

**[事实]** 工具返回的媒体是 `{type:"file", uri:"data:<mime>;base64,…", mime, name?}`（`read` 的图片/PDF、MCP 的 blob、Code Mode 提升的 `files`）。

**[事实]** 钩子之后 `normalizeImages` 会再处理一遍图片（缩放、超限则替换成 `[... omitted: …]` 文本）。

**[事实]** 请求发出前还有一层：`packages/core/src/session/model-request.ts:235-300` 的 `prepare()` 在 `session.hook("context")` 之后跑 `boundImages(unsupportedParts(...))`（`:297`），阈值 `IMAGE_BYTES_TRIGGER = 25 MiB`、`IMAGE_BYTES_TARGET = 15 MiB`，不支持多模态的模型会得到 `ERROR: Cannot read "<name>" (this model does not support <modality> input). Inform the user.`。

**[事实]** 会话级附件（用户贴的文件）走另一条路：`attachments.mdx:30-60` —— `uri` + 可选 `name`/`description`，`file://` 支持 `?start=&end=` 行区间，`data:` 内联，单附件解码上限 20 MiB，HTTP(S) 不支持。

**[建议]** Outrim **完全不要碰 `file` 项**（`type === "file"` 原样透传）：图片/PDF 是模型直接消费的模态数据，改写即失效。压缩只作用于 `text` 项；若文本全被压掉必须自己补一条非空文本（否则按 §2.5：`output` 缺失时静默变成字面 `undefined`，`output` 存在时变成未压缩的 JSON）。

---

## 7. 自动安装与全局配置（"一次配置后自动"）

### 7.1 插件从哪来

**[事实]** `packages/core/src/config/plugin/source.ts`（197 行）：

- config 里的 `plugins` 数组既可写包 spec（`superpowers` / `pkg@1.2.3` / `git+…`），也可写相对/绝对路径；`-target` 表示移除（`Plugin remove operation requires a target`）。
- **`plugins` 数组里写"绝对文件路径"会被静默丢弃**，只留一条 `Effect.logWarning("configured plugin path must be a directory")`；相对文件路径（`./x.ts`）反而被接受并转成绝对路径（`path.resolve(dirname(entry.path), target)`），`file://` 走 `fileURLToPath`。
- 但是**目录发现出来的 `.ts` 文件不会走那条校验**：`discovered` 分支（`PluginSourceDirectory.discover`）直接产出文件路径，只有 `configured`（`plugins[]`）里的绝对路径才被 `fs.isFile` 拦下。而 `PluginModule.load()`（`packages/core/src/plugin/module.ts:110-116`）对"绝对路径且 `stat().isFile()`"是支持的：`{ server: pathToFileURL(operation.target).href }`，注释写着 **"Legacy auto-discovery still admits standalone server sources."** → **把单个 `.ts` 文件放进 `~/.config/opencode/plugins/` 是官方支持的路径，把同一个文件的绝对路径写进 `plugins: [...]` 反而不行。** 这是最容易踩的一处不一致。
- 目录目标走 `Host.resolve({directory})`，入口必须在真实根目录内（`FSUtil.contains(root, server)`，否则该 operation 被丢弃），`mtime` 取所有入口与 `package.json` 的较大值 → 变更检测。
- "Explicit config is applied last so it can remove auto-discovered packages."（显式配置优先级最高）。
- `watchConfiguredSources` 只监听绝对路径的 add-target（且排除 `{plugin,plugins}` 目录内部，那由目录 discover 自己管）。

**[事实]** 目录发现：`packages/core/src/plugin/source-directory.ts` 的 `names = ["plugin","plugins"]`，接受 `.ts`/`.js` 文件、目录、以及指向二者的 symlink。该逻辑对 **每个 config 目录条目**生效，包括 `global.config`（`~/.config/opencode`）、系统目录（Windows `%ProgramData%\opencode`）、以及项目里每个祖先目录的 `.opencode`（`packages/core/src/config/discovery.ts:59-144`）。

**[事实]** `migrate-v1.mdx:553`：v2 同时发现 `.opencode/plugin/` 与 `.opencode/plugins/`；`plugins.mdx:1-58` 补充：**项目根目录 `opencode.json(c)` 旁边的 `plugins/` 目录不会被发现**（必须是 `.opencode/plugins/`）。

**[事实]** 加载契约 `packages/core/src/plugin/module.ts`：`Module = {default: {id, effect} | {id, setup}}`，错误文案 `"Plugin must export a default definition with an id and an effect or setup function."`；ManagedPolicy `integration.use` 或 `plugin:<target>` 命中 deny → `{blocked: true}`；`install: false` 时只 `npm.resolve` 不装，解析不到就 `{pending: true}`；找不到入口 → `LoadError("Plugin entrypoint not found: <target>")`。

**[事实]** 运行时隔离 `packages/plugin/src/runtime.ts`：Bun 插件拦截插件依赖图里对 `node_modules/@opencode/plugin` 或 `node_modules/effect` 的加载，抛出

> `Cannot load "<target>" from plugin node_modules: "<target>" is not provided by OpenCode; plugins must use the host's "effect" and "@opencode/plugin" modules.`

→ **插件必须把 `effect` / `@opencode/plugin` 声明为 peerDependency 且不打包**（文档 `build/plugins/effect.mdx:5-13` 明确写了 `bun add --peer @opencode/plugin effect`）。注意该守卫在 `typeof Bun === "undefined"` 时 no-op，即 Node 版二进制没有这条保护。

### 7.2 CLI 自动安装

**[事实]** `packages/cli/src/commands/handlers/plugin/add.ts`（81 行，全文读过）：

1. `Npm.isInstallablePackage`（`packages/util/src/npm.ts`，基于 `npm-package-arg`）校验；不通过 → `Plugin target must be an npm registry package or Git package specifier`。**只接受 registry spec 与 git spec，本地目录/文件不行。**
2. `npm.add(installed)`（reify 到 `global.cache/npm/<key>`，flock 保护）。
3. `Host.resolve(installed)` 找 `server`/`tui`/`rpc` 入口；server 优先。
4. server 分支 → `resolveConfigPath(global.config)` + `writePluginConfig(configPath, spec)`：
   - 读文件（ENOENT 用 `"{}"`）→ `parse(text, errors, {allowTrailingComma:true})` → 校验为对象且 `plugins` 为数组；
   - `modify(text, ["plugins"], [...(plugins ?? []), spec], {formattingOptions:{tabSize:2,insertSpaces:true}})` + `applyEdits`（**保留注释**）；
   - `mkdir -p` + 写 `configPath + ".tmp"`（`mode: 0o600`）+ `rename`（原子）；
   - 已存在则返回 `false`，打印 `Plugin "<x>" is already configured in <configPath>`，否则 `Plugin "<x>" installed and added to <configPath>`。
5. TUI 分支 → `config.update(draft => draft.plugins = [...])`。

**[事实]** 命令注册 `packages/cli/src/commands/commands.ts:261-299`：`plugin list --builtin` / `plugin add <spec>`（参数描述 "npm registry or Git package specifier"）/ `plugin check [target]` / `plugin update [target]` / `plugin remove <package>`。**v2 没有 `--global` / `--force` 开关**（server 插件无条件写全局配置）。

**[事实]** 官方插件的分发方式印证了上述约束：`packages/plugins/README.md` 写 **"Plugins are currently installed from Git URLs, not npm."**，`packages/plugins/figma/README.md` 给的安装命令是

```bash
opencode plugin add 'git+https://github.com/anomalyco/opencode.git#path:packages/plugins/figma'
```

本地开发用 `"plugins": ["./packages/plugins/figma"]`，入口直接是 `index.ts`（无构建步骤），`peerDependencies: {"@opencode/plugin": "^2.0.27 || ^0.0.0-dev"}`。

**[事实]** 更新与热载：`packages/core/src/plugin/supervisor.ts:19-140` 用 `matches(selector,target)`（`*`、`prefix.*`、精确）解析；`opencode.` 前缀可选中内置；重复 id → `Duplicate plugin ID: <id>`（保留首个）；失败的新版本不替换正在跑的旧版本（"The new revision never became a generation, so the one already running keeps its place."）。`plugin/update.ts` 每 24h 检查一次未固定版本的 target。

**[事实]** 生效时机：`packages/core/src/plugin/service.ts:1-34` 注释——"Session entry points wait here before reading plugin-provided state…"（`awaitActivation`），即**插件在会话入口前完成激活**，用户无需任何手动步骤。配置目录由 `config/watch.ts` 监听，`config.ts:300-360` 以 100 ms debounce 触发 reload。

### 7.3 "一次配置后自动"的三条可行路径（不裁决）

| 路径 | 用户操作 | 生效范围 | 备注 |
|---|---|---|---|
| A. `opencode plugin add <git/npm spec>` | 一条命令 | 全局（`~/.config/opencode/opencode.jsonc`） | 与官方 figma 插件同款；需 Outrim 有可安装的 git/npm 包 |
| B. 写 `~/.config/opencode/plugins/outrim.ts`（或目录） | 复制/安装一个文件 | 全局 | 不经 npm，最轻；`sources.read()` 按 sha256 缓存模块图、文件变更自动重载；但**不受 `plugins` 数组的版本/更新管理**，且要自己处理 `effect` peer 依赖（位于 host 目录内，走 host 模块解析） |
| C. 项目内 `.opencode/plugins/outrim.ts` | 每仓一次 | 单项目 | 与"一次配置后自动"目标冲突，除非安装器自动生成 |

**[事实]** 路径 B 的可行性有上游测试背书：`packages/core/test/plugin.test.ts:91-109` 直接用 `modules.load({type:"add", target: path.join(import.meta.dir, "plugin/fixtures/greeting.ts"), options:{...}})` 加载单个本地 `.ts` 文件并断言其注册的 command 生效。

**[建议]** 首版走 A 或 B 的**全局**变体，绝不要 C。A 的前提是 Outrim 能产出一个可安装包（含 `server` 导出、`peerDependencies` 声明）；B 更轻但要自己保证 host 模块解析可用。

**[事实] 直接改 jsonc 只对某些形态是必需的**：`opencode plugin add`（路径 A）已经把"先装包、再写进 `plugins[]`"全做了（`writePluginConfig`），**不需要自己实现**。只有当分发形态是"本地目录/文件"（路径 B/C）时，`Npm.isInstallablePackage` 才会拒绝、才必须自己写配置——那时可照抄 `writePluginConfig`：`parse` → `modify(text,["plugins"],…)` → `applyEdits` → tmp+rename。

**[事实]** 我查到的公开配置 API 里没有写 `plugins[]` 的接口：`Config.update` 的 `Patch` 只含 `shell` 一个字段（`packages/schema/src/config.ts:116-119`、`packages/core/src/config.ts:383-400`，实现是 `modify(text, ["shell"], patch.shell)`）。**这不等于"插件机制没有这个能力"**——只说明**这些公开 API 不提供它**，不代表全部插件能力都不可达（例如 `ctx` 暴露的其他域、事件订阅等本报告未穷举）。

**[事实]** 关于"临时禁用"：v2 CLI help 里**没有** `--pure`（本机 v2.0.26 `--help` 已核实），`cli.mdx:668` 仍列 `--pure  Run without external plugins` 属遗留页面。但"没有 `--pure`"**不等于"没有任何禁用方式"**：至少还有 ① 配置里 `-outrim` 移除该插件；② ManagedPolicy（`integration.use` / `plugin:<target>`）阻断；③ 权限策略影响其行为。**未找到等效的"一次性临时开关"**——这只是检索结论，不是存在性证明。

---

## 8. 薄 adapter → Rust 的边界

### 8.1 分层与职责

**[事实]** 插件与 OpenCode **同进程**，拥有完整 FS / env / 进程权限（插件是 in-process Effect 插件，`packages/core/src/plugin.ts:20-120` 为每个插件 fork 一个 `Scope` 并注入 host 服务）。`packages/plugin/src` 与 `packages/plugins/*` 里**没有任何 `child_process`/`Bun.spawn` 调用**（`grep -rn 'child_process\|spawn('` 在这两个目录零命中），而上游核心有大量先例（`packages/core/src/{shell,ripgrep,mcp/stdio,pty,persistent-pty/daemon}.ts`、`packages/util/src/{process,cross-spawn-spawner}.ts`）→ 插件里起子进程**技术上可行，但没有上游插件这样做的先例**；`ripgrep/binary.ts` 证明"随包附带一个预编译二进制并按平台 spawn"是 OpenCode 自己就在用的分发方式。

**[建议]** 分层：

| 层 | 职责 | 规模 |
|---|---|---|
| TS 薄壳（OpenCode 插件） | 注册 `execute.after`；挑选需压缩的结果；构造请求（NDJSON）；把返回文本回填 `event.result.content`；可预期失败 fail-open（不吞中断）；原文自己落盘并生成一行位置提示 | 数百行 |
| Rust 核心（单一二进制） | 压缩策略、缓存写盘、提示行生成、并发/超时自保 | 主体 |

**[事实]** 插件自带一个持久化 KV：`ctx.storage.{get,set,remove,scan}`（`packages/plugin/src/effect/storage.ts`、`packages/plugin/src/storage.ts`），底层是 SQLite `kv` 表，命名空间由插件 id 逐字符 hex 编码成 `plugin:<hex>:`（`packages/core/src/plugin/host.ts:623-650`），值必须是 `Schema.Json`。→ **"记住已压过的 callID"这类小状态不需要自己落盘**；但送不进大块原文（JSON + SQLite，不适合当 tool output 缓存）。

**[事实]** 插件拿不到 `global.data` 常量（`packages/plugin/src` 与 `packages/plugins/*` 里 `Global.Path` 零命中）——想要 OpenCode 的全局目录只能自己按 XDG 规则拼，或直接向 OpenCode 询问 `ctx.location.directory` / `ctx.location.project.directory`（`packages/plugin/src/effect/plugin.ts:28`，`Location.Info` 含 `{directory, workspaceID?, project:{id,directory,canonical}}`）。

### 8.2 传输与生命周期

**[建议]** 用子进程 stdin/stdout 传 NDJSON，**不要**用 argv（Windows 命令行上限约 32 KiB，而 `execute.after` 的 `content` 可达 50 KiB 以上——MCP/自定义工具没有内部上限，见 §3.2）。进程创建建议走 `effect/process` 的 `ChildProcessSpawner`（`stdin: "pipe"`），以便挂在插件 scope 上回收；回收语义**需实测**，不能假定 scope 关闭会杀掉外部进程。请求字段建议镜像 OpenCode 的事件：

```jsonc
{ "id": "<callID>:<tool>:<n>", "tool": "<effective name>", "agent": "<agent id>",
  "status": "completed", "input": { /* 可选，注意可能含大 payload */ },
  "content": [ { "type": "text", "text": "…" } ],
  "metadata": { "truncated": true, "outputPath": "…", "shellID": "…" },
  "limits": { "maxLines": 2000, "maxBytes": 51200 } }
```

响应：`{ "content": [ … ], "metadata": {…}, "pointer": "<一行位置提示>" }`（`pointer` 仅用于日志，正文由 Outrim 自己拼进 `content`）。

**[建议]** 进程模型二选一，取决于后续实测：① 每次调用 spawn-一次-退出（最简单，冷启动开销未测）；② 插件 scope 内常驻一个 sidecar，`Effect.acquireRelease` 尝试在插件卸载时回收（**回收是否生效需实测**）。**不要**在插件工厂里起定时器/常驻 socket 而不挂在 Scope 上——OpenCode 会 fork/关闭插件 scope（`plugin.ts` 的 `Scope.fork`/`Scope.close`），未托管的句柄会泄漏。

**[事实] 上游现成的"附带原生二进制"范式**（`packages/core/src/ripgrep/binary.ts`，可直接照抄）：按 `` `${process.arch}-${process.platform}` `` 选平台包 → 优先用 PATH 上的同名可执行文件 → 否则落到 `global.bin/<name>[.exe]` 并 `Effect.cached` 缓存路径 → 下载/解压（**Windows 走 `%SystemRoot%\System32\tar.exe`**，注释明确说明"不用 PowerShell，因为策略可能封禁；也不依赖 PATH 上的 GNU tar，因为读不了 zip；且 bsdtar 用 ANSI 代码页解码参数，所以先 cd 到临时目录只让它看到 ASCII 文件名"）→ 非 win32 上 `chmod 0o755` → `spawner.spawn(ChildProcess.make(command, args, {cwd, extendEnv: true, stdin: "ignore"}))` 并并发收集 stdout/stderr/exitCode。

→ **对 Outrim 的启示（建议）**：Rust 二进制照这个范式分发（`effect/process` 的 `ChildProcessSpawner` + `global.bin` 落点 + 平台矩阵），能直接复用 OpenCode 已经趟过的 Windows 坑（`tar.exe` 按绝对路径调用、临时目录隔离、`0o755`）；传输用 `stdin: "pipe"` 而非 `"ignore"`。

### 8.3 Rust 侧做不到 / 不该做的事

**[事实]** 对有内部截断的工具（`shell`/`read`/`grep`/`glob`/`webfetch`），钩子与接管 `execute` 都**看不到**超出其内部上限的内容：包装现有 `execute`（`editor.update("grep", t => ({...t, execute: ...}))`）拿到的仍是它自己裁过的返回值。要更多只有重写该工具的采集逻辑本身（不是"包装"）。

**[事实] 包装不必复刻描述/schema**：`editor.update(id, fn)` 可以在保留原 `description`/`input`/`output` 的前提下只替换 `execute`（原样透传 `input`、对返回值做后处理）。只有当你需要改变"对模型暴露的契约"（换描述、改 schema）时才涉及复刻——**那是另一类改动，不应和"为了拿原文"混为一谈**。

**[事实]** 改写会同时改变 TUI 显示：`packages/session-ui/src/message/current-tool-state.ts:20-31` 的 `currentToolOutput()` 就是取 `content` 里的文本（running 状态取 `metadata.output`），**没有"查看原文"入口**。而 `packages/session-ui/src/tools/tool-renderer.tsx:1960-2005` 在 `metadata.shellID` 存在时优先显示实时输出流——若 Outrim 把 shell 的 `content` 压小但保留 `shellID`，UI 与模型看到的内容会不一致（视觉上不算错，但会让人以为"模型看到的就是这些"）。

**[建议]** 压缩 shell 结果时，要么保留 `shellID`（UI 流仍在，模型看压缩文本），要么在 `metadata` 里加一个 Outrim 自己的键说明已压缩——**不要删 `shellID`**，那会让 UI 失去实时能力。

**[事实]** 历史改写不适用（本产品明确不做）；且 v2 的会话事件是 durable event + projection（`packages/core/src/bus.ts`、`session/projector.ts:724-726`），改动历史需要重放事件，成本与风险都远超入口压缩。

### 8.4 延迟预算（事实基础 + 建议）

**[事实]** 钩子在工具调用 fiber 内的 `restore` 区执行（§6.1），其耗时直接推迟该次 `Tool.Success`；并行调用之间不互等，但共享同一进程资源（§6.3），实际延迟会互相影响。

**[建议]** 迟延预算**尚未由用户定为目标**；若要做实验，可先按 p95 ≤ 200 ms / 超时 200–500 ms 这类**可选设定**去测，但**本报告不承诺任何数值已满足**。原则只有一条：压缩是附加价值，不能变成可靠性风险点，超时即放行。

---

## 9. 可选集成形态对比（不裁决）

| 形态 | 覆盖 | 钩子内可见量 | 每工具额外延迟 | 用户配置成本 | 主要风险 |
|---|---|---|---|---|---|
| **A. 插件 + `execute.after`** | 内置 + MCP + 第三方工具 + Code Mode 子调用；**不含**后台 shell/subagent/hosted | **按工具而异**：有内部截断的 5 个工具 = 其上限；MCP/自定义/`websearch` = 不受 50 KiB 限制 | 一次进程往返（与并发的其他钩子共享 CPU/队列） | 一条 `plugins` 项 / 一个文件 | 钩子 defect 会记为该轮失败；需 fail-open 且不吞中断 |
| **B. 插件 + `editor.update(id, …)` 接管工具** | 仅被接管的工具 | **不自动更多**：包装其 `execute` 拿到的仍是它自己的返回值；要更多必须重写该工具的采集逻辑 | 同 A + 包一层调用的开销 | 同 A | 只需按实际函数边界包装（**不必**复刻描述/schema，除非要改变对模型暴露的契约）；上游改版需跟进 |
| **C. 插件 + `session.hook("context")`** | 请求级（能拿到全量消息） | 同请求体里的内容（已是截断后） | 每次请求 | 同 A | **能不能"只改首次未发送项"未研究**；若退化成回改历史则违反产品约束 |
| **D. 代理 / CLI 包装**（拦截模型请求） | 取决于是否改写请求体 | **请求体里已是截断后的工具结果**，不是原始工具输出 | 每个请求 | 高（要接管启动方式） | 破坏 OpenCode 自身的 provider 逻辑与流式；与本产品"轻量、少 prompt"目标冲突 |
| **E. MCP server 形态**（Outrim 作为 MCP 工具） | 只覆盖显式调用 | 不适用 | — | 中 | 需要模型主动调用，不是"自动入口" |

**[事实]** C 形态技术上确有入口：`packages/core/src/session/model-request.ts:422-426` 的 `agentHook` 在 `prepare()` 里触发 `session.hook("context"|"compaction"|"generate")`，把完整 `{system, messages, tools}` 交给插件（可删工具、可改描述，`plugin/optimize.ts` 就是这么干的）。

**[事实+边界]** **"看得到全部历史"不等于"必须改写全部历史"**：该钩子拿到的是完整消息列表，插件完全可以只挑"首次尚未发送的工具结果"改写并固定表示、其余原样返回。**禁止的是对历史条目的回改（已在上下文里的内容再变），不是名字里带 context 的 API 本身。** 这条"选择性首入口"路线**本报告未研究**其可行实现——难点在持久化与重试（改写后的表示需与会话库一致，且不能在同一请求的多次尝试间漂移），需单独验证。

**[建议]** A 作为默认候选（配置成本最低、覆盖最广、与 OpenCode 官方 API 同构）；B 按工具增量启用只在**该工具的内部采集逻辑确实可被替换/重写**时才有意义——单纯包装 `execute` 不会多拿到原文（§8.3）。**两条都未实测，本报告不选优。****不在本报告里裁决 A/B 谁是最终方案**——需要先有压缩效果与原文可得性的实测数据。

**[事实]** 形态 D（proxy / CLI 包装）"能看到完整请求"**不等于能看到完整的原始工具输出**：请求体里已经是经过 `read`/`shell`/`grep` 内部截断与（若溢出）通用截断之后的内容，代理拿到的只是更大的**上下文集**，不是工具的真原文。要拿真原文仍需回到工具本身或自建采集。

---

## 10. 未核实 / 待验证清单

1. **本机 v2.0.26 与快照 v2.0.32 的差异**：本报告全部结论基于 v2.0.32 的快照（tag `843a6c5`；引用到的关键文件在 tag→HEAD `501b15a` 间 `git diff --quiet` 均无差异，仅 `packages/core/src/shell.ts` 行号不同）。本机只验证了 `opencode --version` 与 `--help`，**未做 26↔32 的逐 commit diff**，因此**不对 26 的兼容性下任何结论**（不据 CHANGELOG 推断）。
2. **插件内起子进程的实测**：没有上游插件先例，也没有文档禁止；需实测 OpenCode 关闭插件 scope 时子进程是否随之退出（`Scope` 能否可靠回收 `child_process`）。上游在核心层用的是 `effect/process` 的 `ChildProcessSpawner`（`ripgrep/binary.ts`），它把子进程挂在 Effect scope 上——**可以照抄这条路径而不是裸 `child_process.spawn`**，但它是否真能把"插件 scope 关闭 → 子进程退出"打通，**仍需实测**，不能假设有此保证。
3. **`shell` 输出目录在 Windows 上的实测路径**：按 `global-roots.ts` 推导应为 `~/.local/share/opencode/shell/<projectID>/<shellID>.out`（Windows 同样走 XDG 默认值），本机 `~/.local/share/opencode/shell` 目录存在但当前为空（没跑过后台/前台命令），**未实测过落盘文件名**。若实际有出入，§5.2 的"自己拼路径"策略需要调整。
4. **`ToolOutput` 的 7 天保留是否也在配置里**（只确认了 `RETENTION = Duration.days(7)` 为常量，未见 config 覆盖项）。
5. **插件禁用**：快照与 v2.0.26 `--help` 都没有 `--pure`（`cli.mdx:668` 仍列 `--pure  Run without external plugins`，属遗留页面）。已确认存在的禁用途径有：配置里 `-"<target>"` 移除、ManagedPolicy 的 `integration.use` / `plugin:<target>` 阻断。**未找到等效的“一次性临时开关”**——这是检索结论，不是存在性证明。
6. **`sessions.synthetic()` 的注入形态能否被插件改写**（后台 shell/subagent 缺口的补救点）：已确认它**不经过** `session.hook("prompt")`（`synthetic` 直接 `admission.admit`，`prompt` 钩子在 `SessionPrompt.prepare` 里），**目前没有“只在这一条合成消息入口”的钩子**。剩下能看到的只有请求级 `session.hook("context")`——那个钩子并不强制改写全部历史（可只挑首次未发送项改写、其余原样返回），但“选择性首入口”的**持久化与重试一致性本报告未研究**，不能视为已可用。待验证：`ctx.event.subscribe` 能否在 admit 之前干预算（未实测，且即便能也属事件旁路，稳定性不明）。
7. **`tool_output.max_lines/max_bytes` 调大后的实际内存/延迟影响**（理论上可以让 `shell`/`read` 上限更大，从而让钩子看到更多原文；但也会让模型上下文膨胀）。
8. **`read` 的 `siblingOf` 外部校验复用**（`file-access.ts:143-161`）在多工具并发时是否仍成立（只读了代码与 `file-access.test.ts`，未做并发实测）。

---

## 11. 引用

### 11.1 v2 `tool` 契约

- `packages/plugin/src/effect/tool.ts:19-57` — [`ToolHooks` / `ToolFailures` / `ToolDomain`](https://github.com/anomalyco/opencode/blob/843a6c514e606b436a7fe713422ace0a93ad3d61/packages/plugin/src/effect/tool.ts#L19-L57)
- `packages/schema/src/tool.ts:40-92` — [`Options` / `Error` / `Content` / `Result`](https://github.com/anomalyco/opencode/blob/843a6c514e606b436a7fe713422ace0a93ad3d61/packages/schema/src/tool.ts#L40-L92)
- `packages/core/src/tool.ts:104-156` — [`beforeExecute` / `executeTool` / 钩子后归一](https://github.com/anomalyco/opencode/blob/843a6c514e606b436a7fe713422ace0a93ad3d61/packages/core/src/tool.ts#L104-L156)
- `packages/core/src/tool/runtime.ts` — `definition` / `execute` / `normalizeContent` / `normalizedName` / `effectiveName`
- `packages/core/src/plugin/hooks.ts:60-108` — 触发顺序与"同一对象"语义
- `packages/plugin/src/promise/tool.ts`、`packages/plugin/src/promise/adapter.ts:477-518` — Promise 镜像与异常→defect
- `packages/core/test/tool-registry.test.ts:883-899` — 上游对"整体替换 `result`"的测试
- `packages/core/src/plugin/plan.ts:52-63` — 官方"替换 `event.error`"的先例（文件共 120 行）
- `packages/core/src/plugin/tool-input-repair.ts:29-41` — 官方"改 `event.input`"的先例

### 11.2 截断与原文

- `packages/core/src/tool-output.ts:13-140` — [`MAX_LINES`/`MAX_BYTES`/`truncate`/落盘/清理](https://github.com/anomalyco/opencode/blob/843a6c514e606b436a7fe713422ace0a93ad3d61/packages/core/src/tool-output.ts#L13-L140)
- `packages/core/src/config/plugin/tool-output.ts` + `services/www/src/docs/content/config.mdx:307-313` — `tool_output` 配置
- `packages/core/src/shell.ts:228-252`（tag `843a6c5`，文件共 455 行；`outputDir` 在 `:136`、`${id}.out` 在 `:279`）→ HEAD `501b15a` 因超时实现重写整体下移 8 行（`:236-260`、`:144`、`:287`，文件 469 行），代码相同 — [`Shell.result()` 尾部截断 + `output()` 分页](https://github.com/anomalyco/opencode/blob/843a6c514e606b436a7fe713422ace0a93ad3d61/packages/core/src/shell.ts#L228-L252)
- `packages/core/src/shell/result.ts` — `notice` / `metadata` / `notification`
- `packages/core/src/tool/plugin/shell.ts:20-105,108-293`（文件共 293 行）— 描述、`resultMessages`、`prepare`、后台降级与 `notifyWhenDone`
- `packages/core/src/tool/plugin/read.ts:19-140` — `toModelContent` / 媒体项 / 错误文案 / AGENTS.md 副作用
- `packages/core/src/tool/read-filesystem.ts:14-22` — `MAX_READ_LINES` / `MAX_READ_BYTES` / `MAX_LINE_LENGTH`
- `packages/core/src/tool/plugin/grep.ts`、`glob.ts:127` — 条数截断与提示
- `packages/core/src/tool/plugin/webfetch.ts:120-162` — 5 MiB 上限、`content` 为字符串
- `packages/core/src/tool/mcp.ts:40-152` — MCP 工具注册、`isError`、`output` 推导、reload debounce
- `packages/core/src/tool/plugin/mcp-resource.ts:30-101` — 两个工具（`list_mcp_resources` / `read_mcp_resource`）、`opencode_*` 权限动作、`:53` 溢出提示措辞、blob→`file` 项
- `packages/core/src/mcp/stdio.ts:15`、`packages/core/src/mcp/client.ts:34-36` — 帧上限与超时

### 11.3 流水线、持久化、并发

- `packages/core/src/session/runner/step.ts:62-133,200-325` — [fork/mask/truncate/publish 与中断分类](https://github.com/anomalyco/opencode/blob/843a6c514e606b436a7fe713422ace0a93ad3d61/packages/core/src/session/runner/step.ts#L62-L133)
- `packages/core/src/session/runner/publish-llm-event.ts:117-121,606-633` — `failureSnapshot`、空内容 die
- `packages/core/src/session/runner/to-llm-message.ts:127-152` — [模型只看 `content`](https://github.com/anomalyco/opencode/blob/843a6c514e606b436a7fe713422ace0a93ad3d61/packages/core/src/session/runner/to-llm-message.ts#L127-L152)
- `packages/core/src/session/model-request.ts:235-300,405-430` — `prepare()`、`tools.execute({...call, definitions: hooked})`、`session.hook("context")`、`boundImages`/`unsupportedParts`
- `packages/core/src/session/message-updater.ts:343-378` — `ToolStateCompleted.content`
- `packages/core/src/session/projector.ts:724-726`、`packages/core/src/bus.ts` — durable event → projection
- `packages/core/src/session/compaction.ts:100,813-826` — 压缩用独立上限 `TOOL_OUTPUT_MAX_CHARS = 1_250`，`truncateToolOutput` 按**码点**截断（避免劈开代理对）后追加 `\n[truncated]`
- `packages/core/src/session/runner/llm.ts:367-384` — `settleStaleToolCalls`

### 11.4 Code Mode / MCP / 权限 / 文件访问

- `packages/core/src/codemode/tool.ts:55-140,310-327` — `execute` 语义、子调用包装、`formatResult`
- `packages/core/src/codemode/catalog.ts:48-90` — 目录描述截断与 `pinned`
- `packages/core/src/mcp/index.ts:305-310`、`packages/core/src/plugin/mcp-codemode-defaults.ts` — codemode 传播
- `packages/core/src/permission.ts:19,86-100,158-190,233-265` — 规则匹配、`ask` 回落、拒绝语义
- `packages/schema/src/agent.ts:38-51`、`packages/core/src/plugin/agent.ts:83-143` — 默认 agent 权限集
- `packages/core/src/file-access.ts:83,96-161` — internal/external 判定、`authorizeRead`
- `packages/core/test/agent.test.ts:151-162,198-205` — `external_directory` 默认 allow 的测试证据
- `services/www/src/docs/content/{tools,permissions,config,plugins,migrate-v1,attachments}.mdx`

### 11.5 插件安装与运行

- `packages/core/src/config/plugin/source.ts`（全 197 行）— 发现/解析/绝对路径警告/watch
- `packages/core/src/plugin/source-directory.ts`、`module.ts`、`plugin.ts:20-120`、`supervisor.ts:19-140`、`service.ts:1-40`、`update.ts`
- `packages/core/src/config.ts:190-360`、`config/discovery.ts:59-144`、`config/watch.ts`
- `packages/cli/src/commands/handlers/plugin/add.ts`（81 行）、`packages/cli/src/commands/commands.ts:261-299`、`packages/util/src/npm.ts`
- `packages/plugin/src/runtime.ts`、`runtime-modules.ts`、`source.ts`、`source.node.ts`、`packages/util/src/runtime/import.node.ts`
- `packages/plugins/README.md`、`packages/plugins/figma/README.md`、`packages/plugins/figma/package.json`
- `services/www/src/docs/content/plugins.mdx:1-139`、`build/plugins/{index,effect,migrate-v1}.mdx`

### 11.6 TUI 影响

- `packages/session-ui/src/message/current-tool-state.ts:20-31` — 内容取自 `content` 文本
- `packages/session-ui/src/tools/tool-renderer.tsx:1960-2005` — `metadata.shellID` 存在时优先实时流

### 11.7 原生二进制分发（可照抄的范式）

- `packages/core/src/ripgrep/binary.ts:1-120` — 平台矩阵（`arm64/x64/ia32 × darwin/linux/win32`）、`global.bin` 落点、`Effect.cached` 缓存路径、优先 PATH、Windows 用 `%SystemRoot%\System32\tar.exe` 解 zip、非 win32 `chmod(0o755)`、`spawner.spawn(ChildProcess.make(..., {cwd, extendEnv:true, stdin:"ignore"}))` 并发收集 stdout/stderr/exitCode
- `packages/util/src/cross-spawn-spawner.ts`、`packages/util/src/process.ts` — spawner 实现

### 11.8 v1 对照（commit `e3c4eb2`）

- `packages/opencode/src/session/tools.ts:96-125` — [before/after 触发点与可变 `output`](https://github.com/anomalyco/opencode/blob/e3c4eb242585dcd77be1a867391b7c25fd47ce52/packages/opencode/src/session/tools.ts#L96-L125)
- `packages/opencode/src/tool/tool.ts:90-150` — `wrap()` 内截断（after 看到的是截断后）
- `packages/opencode/src/tool/truncate.ts`、`truncation-dir.ts:4` — 2000 行 / 50 KiB / `tool-output` 目录 / 长免责提示
- `packages/opencode/src/tool/registry.ts` — v1 自定义工具目录约定
- `packages/plugin/src/index.ts` — v1 插件 hook 联合类型（含 `shell.env`、`tool.definition`）
