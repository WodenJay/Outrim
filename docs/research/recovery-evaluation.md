# 原文恢复与评估：入口指针、原文缓存与真实任务记账

研究票：<https://github.com/WodenJay/Outrim/issues/3>（不改 issue）
范围：**入口**压缩之后的"原文在哪里/怎么取回/要不要取回"，以及恢复的成本与评估方法。不重复结构算法（另一分工）、不重复语义论文（另一分工）、不重复宿主 API 全面调研（`research/pi-integration` 已完成），也不做 go/no-go 立项判断。

所有数字都绑定到具体的脚本与数据路径；**[事实]** 表示在源码/文档里能指到行，**[测量]** 表示本机复算结果，**[建议]** 是方案取向，**[未知]** 是没测出来、不应擅自定阈值的地方。

---

## 0. 结论摘要

1. **"Headroom/CCR" 指 Compress-Cache-Retrieve**，不是"Cache-Compress-Retrieve"之类的营销词：压缩后的占位符带一个 hash，原文进了本地 store，模型靠 `headroom_retrieve` 工具（或 MCP 工具）拿回来。真实代码里这套机制有三个必须抄对的细节：**sticky 注入**（工具一旦出现就不能再消失，否则工具数组字节翻转破坏前缀）、**TTL 是滑动空闲窗**（默认 30 分钟 + 8× 硬上限）、**miss 消息本身可操作**（告诉模型重新 read/重跑）。
2. **Condensed Milk 唯一值得迁移的是入口侧机制**，它的静态 cutoff 历史掩码在我们的宿主数据上**算不过来**：在 300 个真实 Pi 会话里，回改历史能付清"重写前缀"代价的候选只占 **0.81%**（Anthropic 定价形状）/ **1.40%**（无 write 溢价）。这条与其说是"另一个仓库的选择"，不如说是我方"只入口不历史"的产品决定有独立的经济学支撑。
3. **原文指针极短是可行的**：路径一行约 **20–23 token**（cl100k 实测），而 Headroom 的 `headroom_retrieve` 工具定义 **100 token**、Condensed Milk 的系统提示解释 **56 token**、啰嗦指针 **66 token**。复用内置 `read` 读指针路径，**不需要新增工具**——这一点在钱上有量级差：新增工具会在第一次压缩时改工具数组，等于**一次全前缀失效 = 105,176 units ≈ 38.8 个中位 tool result**；而工具定义本身长期携带只值 **0.54 个中位 tool result**。
4. **cached/uncached 记账必须按 provider 形状分开算，不能用一个"5×"拍死**。同一份 SWE 轨迹（Qwen3-32B，Anthropic 价）：output 占 **74.4%**，可归因于 observation 字节的上限只有 **6.4%**；同一台机器上的 300 个 Pi 会话：cache read 占账单 **69.9%**、output 只占 **13.6%**，可归因于 tool output 的上限 **63.8%**。"压缩 tool output 能省多少"这个问题的答案完全取决于 agent 的形状，不能跨形态外推。
5. **"首次压缩后历史保持稳定"不等于"历史被重写"**。前向压缩（内容第一次进上下文时就已经是短的）在之后的每个 turn 都前缀稳定；只有**改已经发出去的历史**才需要付一次 rewrite premium。官方文档支持这一点，但没有任何一家说"压缩必然破坏 cache"。
6. **恢复的真正成本是"多一轮 LLM 往返"，不是那点 pointer token**。中位 prompt 91.5k token 时，一次恢复往返 ≈ **9,988 units ≈ 3.7 个中位 tool result** 的价值。因此判定规则是**逐条**的：单条 `r* = save/(save+round-trip)`，而不是全局"要不要恢复"。
7. **入口指针的收益高度依赖尺寸门槛**：门槛 0 时保留 100% 的 tool token、单条 `r*` 中位 18.5%；门槛 2000 token 时保留 55.6% 的量、`r*` 中位 84.5%（几乎不会误伤）。这不是验收阈值，是在给定语料上的**测量膝点**。
8. **重复内容折叠是"零风险但小"**：内容完全相同的重复 tool result 占 tool-output token 的 **8.80%**（同工具同参数且文件未变：**7.42%**）。但 Headroom 在它们自己的流量上测到 byte-identical 重复 Read 只占 **0.1%**——两个数差 70 倍，说明"重复率"是流量形状属性，上线前必须在**目标用户流量**上测，不能拍。
9. **运行时开销可以忽略，但不能无限**：本机进程启动地板 25–37 ms，混合 provider 下 turn 中位时长 8.2–9.6 s。钩子加 1–5 ms 无感；把整个 tool output 走一次外部进程 + 序列化也只在毫秒级。真正的时间成本是**恢复多一轮**（数秒到数十秒）。
10. **质量无法从日志测**：没有任何真实数据能回答"少了这段原文，任务成功率掉多少"。只能设计双臂实验，样本量按 5 个百分点差异约 **1,568/臂**（α=.05, power=.80, p≈.5）。

---

## 1. 术语与真实代码：Headroom / CCR 到底是什么

### 1.1 命名

**[事实]** `headroom/ccr/__init__.py:1` 与 `headroom/cache/compression_store.py:1` 都写着同一个展开：**CCR = Compress-Cache-Retrieve**；`benchmarks/ccr_regression_benchmark.py:5` 亦同。仓库 README 把它叫 "Reversible (CCR) — originals are cached locally and retrieved on demand"。

一手来源（local clone，commit `fbf7bc3278c11975299b9bd0a2ecf200dfd25a05`，2026-10-17）：
<https://github.com/headroomlabs-ai/headroom/tree/fbf7bc3278c11975299b9bd0a2ecf200dfd25a05>

### 1.2 标记形态（决定了"极短指针"能短到什么程度）

**[事实]** 正则与注释在 `headroom/ccr/marker_resolution.py:30-34` 与 `headroom/ccr/tool_injection.py:219-262`。真实存在四种记忆形态，且每一种都是**为兼容旧版本而叠加**的：

| 形态 | 例 | 来源 |
|---|---|---|
| `<<ccr:HASH,KIND,SIZE>>` | `<<ccr:9f2a…,blob,1200>>` | SmartCrusher 不透明块 |
| `<<ccr:HASH N_rows_offloaded>>` | `<<ccr:9f2a… 1200_rows_offloaded>>` | 行丢弃摘要 |
| `[N items compressed to M. Retrieve more: hash=…]` | 24 hex 字符（SHA-256 前 96 bit） | 旧 TextCompressor |
| `[Read content stale/superseded: … Retrieve original: hash=…]` | 12–24 hex | read_lifecycle |

[测量] 用 cl100k 实测这些形态的长度：`ccr marker` 35 token，`ccr bracket` 38 token，"`[outrim: 412 lines, full text at outrim:7f3a9c]`" 21 token，"`raw: %TMP%\outrim\7f3a9c2e.log`" 20 token。

**可迁移的教训**：它们的标记长是因为它同时要编码 **hash + kind + size + 人可读语义**，并且要为 4 种历史格式做正则兼容。我们没有历史包袱，可以只留**一行路径**。

### 1.3 检索工具：sticky 注入是硬约束

**[事实]** `headroom/ccr/tool_injection.py:415-467` `inject_tool_definition()`：默认只在本请求检测到 marker 时才注入 `headroom_retrieve`；但调用方可以传 `session_has_done_ccr=True` 走 **sticky-on**。注释写得很直白（`:423-429`）：

> "once a session has done CCR, the `headroom_retrieve` tool must stay in `body["tools"]` for every subsequent request, otherwise the tool list bytes flip on/off mid-session and bust the prompt cache."

**[事实]** 工具定义本体在 `:37-74`（`create_ccr_tool_definition`），描述里显式教模型"hash 就在 marker 里"。**[测量]** 该定义 100 token（cl100k，Anthropic 形状）。

**这直接给出我方的取舍**：如果我们**不加工具**（复用内置 `read`），就不会有"工具数组翻转"这一类 bug，也不需要 sticky 机制；代价是模型必须用 `read` 而非专用检索调用——而这正好符合"重新 read 那个文件"的自然行为。

### 1.4 存储：TTL、容量、hash、落盘

**[事实]** 参数与语义（`crates/headroom-core/src/ccr/mod.rs:59-79`，Python 侧同值）：

- 容量 `DEFAULT_CAPACITY = 1000`，LRU 淘汰；
- `DEFAULT_TTL = 1800s`；
- **TTL 是滑动空闲窗**：每次成功 `get` 重新计时；但绝对寿命上限 `DEFAULT_MAX_LIFETIME_MULTIPLIER = 8`（即 `8×ttl`，默认 4 小时）防止热点条目永久驻留；
- hash = `sha256(original)[:24]`（`headroom/cache/compression_store.py:361`）；
- miss 消息是**可操作**的（`compression_store.py:150-159`）：

> "Entry not found or expired. To recover: if the compression marker references a file Read, re-read that file (the path is in the marker; disk is the source of truth). If it was command output, re-run the command."

**[事实]** 落盘默认在 `workspace_dir()/ccr_store.db`（SQLite，`headroom/cache/backends/sqlite.py:69`），创建时 **先 chmod 0600 再给 sqlite 打开，失败即 fail-closed**（`:99`、`:136`）；含 `--stateless` / `HEADROOM_STATELESS` 时统一走 `persistence_allowed()`（`headroom/paths.py:133-148`）拒绝任何落盘。

**这一段对我们是三条可直接搬的规则**：滑动 TTL（会话级，不是 5 分钟）、硬寿命上限、**fail-closed 的权限设置**。

### 1.5 read_lifecycle：他们自己也承认"重复 Read 去重不值钱"

**[事实]** `headroom/transforms/read_lifecycle.py:1-21` 的模块 docstring（`:11-13`、`:15-19`）：

> "Real-world data shows 75% of Read output bytes fall into these two categories: 67% stale (file edited after Read), 12% superseded (file re-Read later), Only 20% are fresh"
>
> "NOTE: a first-sight repeat-Read dedup mechanism (DEDUP_REPEAT) was prototyped here and removed — `headroom audit-reads` measured byte-identical repeats at 0.1% of Read bytes on real traffic."

**这是一个有价值的反面证据**：他们把"重复"细分成 **stale（文件后来被改，旧内容已经错了）** 与 **superseded（同文件被重读，内容冗余）**，只为这两种做替换——因为只有这两种是**可证明安全**的。这比"一律去重"严格得多，也解释了为什么他们敢做历史替换而我们不做。

### 1.6 定价表：他们自己也知道 read/write 乘数因 provider 而异

**[事实]** `headroom/cache_economics.py` 共 32 行，内容只有一张 fallback 表（表外无其它逻辑）：

| provider | read_multiplier | write_multiplier | 备注 |
|---|---|---|---|
| anthropic | 0.1 | 1.25 | 显式断点，5 分钟 TTL |
| openai | **0.5** | **1.0** | 自动，无 TTL 控制 |
| gemini | 0.1 | 1.0 | 显式 cachedContent |
| bedrock | 0.1 | 1.25 | 同 Anthropic |
| xai | 0.16 | 1.0 | 自动 |

注意 openai 行的 `read_multiplier: 0.5` 与官方文档的 **0.1×**（最多 95% 折扣）不一致——**[建议]** 我们不抄这张表，乘数直接取官方文档（见 §3），并把 provider 形状当作显式输入而不是常量。

---

## 2. Condensed Milk：哪些入口机制值得迁移

一手来源：`C:/Users/wudon/AppData/Local/Temp/cm`，commit `71f9e396951c42687f0c3456727b2b5c8c625da1`（v1.10.0）。
<https://github.com/tomooshi/condensed-milk-pi/tree/71f9e396951c42687f0c3456727b2b5c8c625da1>

### 2.1 它靠哪几个钩子完成"入口 + 历史"

**[事实]** `index.ts` 里注册了 6 个事件 + 4 个命令：

| 钩子 | 位置 | 干什么 |
|---|---|---|
| `before_agent_start` | `index.ts:301-303` | 把常量 `CM_EXPLAINER` 拼到 system prompt |
| `session_start` | `index.ts:335` | 复位计数器 |
| `tool_result` | `index.ts:484` | **入口**：只对 `bash` 做语义压缩（`read` 直接 return，不改） |
| `context` | `index.ts:580` | **历史**：静态 cutoff 掩码 |
| `session_compact` | `index.ts:405` | 复位 cutoff 与掩码跟踪表 |
| `session_shutdown` | `index.ts:430` | 遥测收尾 |

**[事实]** `tool_result` 里的分支值得注意：read 结果完全不动（`index.ts:499` 直接 `return`），只有 bash 走 `dispatch(command, stdout)` 的语义 filter。也就是说**它的入口压缩只覆盖 bash**。

### 2.2 静态 cutoff（历史掩码）的算法

**[事实]** 设计意图写在 `index.ts:53-55`：

> "v1.2.0 (ADR-018): static-cutoff algorithm replaces rolling window. Cutoff advances only on pressure threshold crossings — bytes before the cutoff stay identical turn-over-turn — cache prefix stable."

**[事实]** 默认参数 `index.ts:63-79`：`thresholds = [0.30, 0.45, 0.60]`（上下文使用率），`coverage = [0.60, 0.80, 0.95]`（每次跨阈值掩掉多少比例的消息）。`decideCutoff` 在 `filters/context-compress.ts:328`，v1.2.1 起 **cutoff 在进区时冻结，不再从 messages.length 重算**，注释（`index.ts:623-625`）说这是为了消除"drift-write pattern"。

**这个算法本身是对的**：把"每次 turn 都改历史"换成"少数几次改历史"，把 rewrite 次数从 O(turns) 压到 O(zones)。它想解决的问题是真实的。

### 2.3 占位符与门槛（可直接对标的短指针）

**[事实]** `filters/context-compress.ts:57` `MIN_MASK_LENGTH = 120`（字符，低于 120 不掩）；`:393`、`:422` 是两道 gate（长度 + 是否是 reference 文件 + 是否在 cutoff 之前）。`filters/profiles.ts:73-74` 的默认占位符：

```
bash: "[cm-masked bash] {cmd}"
read: "[cm-masked read] {path} ({n} lines, {size})"
```

**[测量]** cl100k：`[cm-masked bash] git status` = **8 token**；`[cm-masked read] astropy/modeling/separable.py (412 lines, 14.8KB)` = **23 token**。

**[事实]** 为什么 read 占位符比 bash 长？`CHANGELOG.md:560-575`（v1.4.0）给了实测理由：

> "v1.3.0 real-session data showed reads had 33% re-read rate (3/9) while bashes had 0% (0/52) — the read placeholder `[masked read] /path` was semantically lossy. […] **New read placeholder:** `[masked read] /path (N lines, SIZE)` […] Derived purely from the original content → byte-deterministic per message → cache prefix stays stable."

这两句都要抄：**(a) 占位符的信息量要按工具类型分级**（bash 的命令名自带语义，read 的裸路径不够）；**(b) 占位符必须由原文内容确定性派生**，否则每次算出来的字节不同，前缀就不稳定。

### 2.4 系统提示解释 = 56 token，他们要，我们可以不要

**[事实]** `index.ts:270-279` 的 `CM_EXPLAINER` 是一段常量：

```
condensed-milk: tool results older than the current cutoff appear as
`[cm-masked bash|read] …` to keep the prompt cache stable. Re-run the
command or re-read the file to get current content. `/compress-stats`
for details.
```

**[测量]** 56 token。注释（`index.ts:272-277`）强调"bytes MUST be deterministic turn-over-turn … Any future edit here is a deliberate cache bust and MUST be ADR'd"。

**它不是免责声明**，是操作说明（"怎么拿回来"）。用户的取向是"一短行告诉原文在哪里，拒绝长警告"——那么结论是：**把"怎么拿回来"编码进那一行本身**（给的是可 `read` 的路径），就不用额外的 system prompt 解释。代价是模型需要知道 `read` 一个路径是合法的——而这本来就是内置工具，不需要教。

### 2.5 历史掩码：在我们的宿主上算不过来

**[测量]** 用真实 Pi 会话（`C:/Users/wudon/.pi/agent/sessions/**/*.jsonl`，取 >20KB 的最大的 300 个，脚本 `C:/Users/wudon/AppData/Local/Temp/corpus/breakpi.py` 与 `meas.py` §6）复算"回改一条历史要多久回本"：

一次回改把第 i 条 tool result 换成 pointer 后，**从该点之后的所有内容都要重写一次**（write 1.25×）而不再是读（0.10×）。回本条件：

```
m·R·L  >  (m + rest)·(W − R)      ⇒      L > 11.5·(m+rest)/m
```

（W=1.25, R=0.10；若 provider 无 write 溢价则系数变 9.0）

| 量 | 值 |
|---|---|
| 候选数（有后续 turn 的 tool result） | 111,047 |
| 需要的复用轮数 | p10 388 / **p50 4,567** / p90 80,690 |
| **能在会话/压缩结束前回本的** | **898 = 0.81%** |
| 无 write 溢价时（W=1.0） | **1.40%** |
| 被重写的后缀 / 被掩掉的质量 | **p50 396×** |

原因是"中位 tool result 188 token"对"其后剩余上下文"的比值太极端：**你省下的那 2,712 units 要用后面 91k 的 token 重新写一次来换**。

> 注意不要把这个结论外推到所有数据集：同一套公式用在 SWE-agent 轨迹上是相反的（`corpus/fold.py`：masked:rewritten 比中位 **15.8**，回本只需 **0.7 个缓存 turn**，**82.9%** 的 turn 属于"明显划算"）。差别在于那些轨迹的 observation 相对后续上下文极大。**结论是"回改历史的划算程度是轨迹形状函数"，而在 Pi 这种长会话形状上它不划算。**

这正是"只入口不历史"的独立依据：不是因为产品口味，而是因为在我们宿主的真实形状上，历史改写的 rewrite premium 比它能省的多两个数量级。

---

## 3. provider cache：官方文档核实（避免"压缩必然破坏 cache"的误断言）

### 3.1 Anthropic

来源：<https://platform.claude.com/docs/en/build-with-claude/prompt-caching>

**[事实]** 原文引用：

- "The system checks if a prompt prefix, up to a specified cache breakpoint, is already cached from a recent query." —— **前缀**匹配，不是全文匹配。
- "Prompt caching references the entire prompt: `tools`, `system`, and `messages` (in that order), up to and including the block designated with `cache_control`."
- "Because the hash is cumulative, covering everything up to and including the breakpoint, changing any block at or before the breakpoint produces a different hash on the next request."
- "Cache hits require 100% identical prompt segments, including all text and images up to and including the block marked with cache control." —— **exact matching**。
- 断点上限 **4** 个；"The lookback window is 20 blocks."（每个断点最多向前找 20 个位置）
- 计价："5-minute cache write tokens are 1.25 times the base input tokens price"、"1-hour cache write tokens are 2 times"、"Cache read tokens are 0.1 times the base input tokens price"（Fable/Mythos 5.1 与 Opus/Sonnet 5.5 有 0.025×/0.05× 例外）。
- 最小可缓存长度按模型：**512 / 1,024 / 2,048 / 4,096** token 不等（Haiku 4.5 与 Opus 4.5/4.6 是 4,096）。低于阈值"will be processed without caching, and no error is returned"。
- TTL 默认 5 分钟，"refreshed for no additional cost each time the cached content is used"。

### 3.2 OpenAI

来源：<https://developers.openai.com/api/docs/guides/prompt-caching>

**[事实]** 原文引用：

- "Prompt caching reuses work when requests share the same prompt prefix."
- "Cache reuse requires the entire rendered prefix to match. If content or a relevant setting changes before a breakpoint, the prefix after that change cannot match the existing cache entry."
- "Changing a request does not necessarily discard an existing cache entry. What matters is whether a subsequent request has the same prefix and can find an eligible matching breakpoint."
- **"Summarization, compaction, or context truncation can change the prefix and reset cache reuse."**
- 计价：GPT-5.6+ "cache writes cost 1.25× the standard, uncached input-token rate. Subsequent reads cost 0.1× that rate on most of these models and 0.05× on GPT-6.1 Sol"；更早模型无 write 费。折扣"up to 95%"。
- 最小可缓存 1,024 token（5.6+）；TTL 默认 `30m`（5.6+）；更早模型 `in_memory` 约 5–10 分钟。
- 会影响前缀的设置项：`model`、`tools`、`parallel_tool_calls`、`text.format`、`reasoning.effort`、`text.verbosity`、`context_management`。

### 3.3 精确表述（这三句话必须分清）

1. **入口压缩不破坏 cache。** 如果 tool result 在**第一次进上下文时**就已经是短指针，那么之后每个 turn 的前缀都是同一串字节，前缀稳定性是这个设计的**结论**，不是运气。
2. **"首次压缩后历史保持稳定" ≠ "历史被重写"。** 前者说的是"从这次之后前缀不再变"，后者说的是"把已经写进缓存的内容改掉"。OpenAI 文档点名的是后者（"summarization, compaction, or context truncation can change the prefix"），不是前者。
3. **不能泛称"压缩必然破坏 cache"。** 破坏与否取决于**改动位置**：改 `tools`/`system` 或已写入的前缀 → 从改动点起全部失效；只在尾部追加 → 前缀照旧命中。回改历史之所以贵，是因为它把"尾部追加"变成了"中段改写"。

**[测量]** 把第 2 点量化：在我方语料里，一次工具数组变化（新增检索工具）发生在中位 prompt 91,457 token 时，代价 = `91,457 × (1.25 − 0.10)` = **105,176 units**，等于 **38.8 个中位 tool result** 的价值。

---

## 4. 原文缓存与极短位置指针

### 4.1 指针能有多短（cl100k 实测）

| 候选 | token | 说明 |
|---|---|---|
| `raw: %TMP%\outrim\7f3a9c2e.log` | **20** | 纯路径（含标签） |
| `[outrim] raw: %TMP%\outrim\7f3a9c2e.log` | 22 | 加产品名 |
| `[outrim: 412 lines, full text at outrim:7f3a9c]` | 21 | 无路径、需自己解析 id |
| `[cm-masked read] {path} ({n} lines, {size})` | 23 | Condensed Milk 的 read 形态 |
| `[outrim] `git status` (3.1KB -> 0.2KB) raw: …` | 36 | 加了来源命令与原/新尺寸 |
| 啰嗦指针（含"可能遗漏细节，请核实"） | **66** | 3× 于最简 |
| Condensed Milk 系统提示解释 | 56 | 每个 session 一次 |
| `headroom_retrieve` 工具定义 | 100 | 进 tools 数组 |

**[建议]** 用户已定"一短行告诉原文在哪里、拒绝长警告"，那么取 **20–23 token** 档：一行里只放**能 `read` 的路径**。不要放免责声明（66 档），也不要放"可能不完整"的警告——那是在替模型做它自己该做的判断，而且贵 3 倍。

### 4.2 复用已有 read vs 极少新工具

| 方案 | 一次性代价 | 长期代价 | 额外风险 |
|---|---|---|---|
| **A. 复用内置 `read`**（指针给绝对路径） | 0 | 0（path token 已在指针里） | 需保证本产品不去压缩"读自己缓存目录"的 read，否则自噬 |
| **B. 专用检索工具** | 首次压缩时工具数组变化 = **105,176 units ≈ 38.8 中位条目** | 定义常驻：`100×1.25 + 100×0.1×135` = **1,475 units ≈ 0.54 中位条目** | 必须做 sticky 注入（`tool_injection.py:423-429` 的坑），否则中途翻转会反复失效 |
| C. MCP 工具 | 同 B + 多一层 server | 同 B | 更多活动部件 |

**[事实]** Pi 的 `read` 对指针路径没有沙箱限制：`read` 的路径解析走 `resolveReadPathAsync(path, ctx?.cwd || cwd)`（`dist/core/tools/read.js:73`）→ `resolveToCwd`（`path-utils.js:42-44`，只做 `~` 展开与 `@` 前缀处理）→ `resolvePath(input, baseDir)`，`isAbsolute` 时直接 `nodeResolvePath`（`dist/utils/paths.js:83-87`）。因此指针指向 `tmpdir` 或我们自己的缓存目录都能被打开，受同样的 2000 行 / 50KB 与 `offset` 分页约束（`dist/core/tools/truncate.js:10-11`；`read.js:134-163`）。

**[建议]** 首版走 A。理由不是"省 token"（B 的长期代价只有 0.54 个条目），而是**A 去掉了整整一类 bug**：工具数组翻转、sticky 状态、工具描述与实际能力不一致。B 的唯一优势是能在指针里放一个短 id 而不是长路径——不值这个复杂度。

### 4.3 原文从哪来：Pi 已经给了一半

**[事实]** 三条原文出口（与 `research/pi-integration` 一致，本节只补"够不够用"）：

| 工具 | 入口能看到的原文 | 出路 |
|---|---|---|
| `bash` / `powershell` | 已截断的尾部（`truncateTail`，`dist/core/bash-executor.js:88-98`） | `details.fullOutputPath` 指向 OS 临时文件；只有 >50KB 才创建（`bash-executor.js:26-30`、`bash.js:238-249`） |
| `read` | 已截断的头部 | 文件本身还在，`offset/limit` 续读 |
| `grep` / `find` / `ls` / MCP | 已截断 | grep/find/ls 无出口（只能重跑）；MCP 20KB 后写临时文件 |

**[测量]** 这些临时文件是 `mode 0o600` + `flag "wx"`（`dist/utils/output-files.js:10-31`），注释写明"Output can carry private data, so only the user may read the files"、路径 `<tmpdir>/<prefix>-<16hex><ext>`——**Pi 已经有一套和我们需求完全同构的原文落盘约定，可以直接对齐格式而不必发明新的**。

**[测量]** 但截断出口的覆盖率很低：本次统计的 110,712 条 tool result 里，>50KB 只有 **63 条（0.06%）**，>16KB **3.23%**，>8KB **8.78%**（`outcost.py`，字节计数）。也就是说"靠 Pi 自己的 fullOutputPath 兜底"只覆盖万分之六的调用——**必须自己落盘**，否则入口压缩就没有原文可指。

### 4.4 原文缓存的生命周期、隔离、隐私

**生命周期**

**[事实]** Headroom 的两条参数是现成的正确答案：滑动空闲 TTL（默认 1800s）+ 绝对寿命 8×（`crates/headroom-core/src/ccr/mod.rs:59-79`）。他们注释里解释了为什么不是 5 分钟：

> "Session-scale: agentic sessions routinely outlive the old 5-minute default, and an expired entry silently converts 'lossless with retrieval' into 'lossy'."

**[测量]** 我们的会话形状支持这个判断：一条 tool result 之后**中位还有 135 个 assistant turn**（p75 275、p90 455，`meas.py` §6 的候选统计）。所以"5 分钟"绝对不够，**至少要覆盖整个会话**。

**[建议]** 三档组合，都便宜：
1. 会话级目录（`<tmpdir>/outrim-<session>/`），`session_shutdown` 时整目录删除；
2. 目录内每条 0o600、随机名（不含源路径，避免文件名泄漏路径）；
3. 进程被 kill / 崩溃留下的残留在 OS 临时目录，交给系统清理（与 Pi 现状一致），但要保证**残留不会跨会话被引用**——指针里带 session id 前缀就够。

**隔离**

- **自噬防护**：一定不要压缩"对 Outrim 自己缓存目录的 `read`"。否则模型为了拿原文去 read 指针，结果这个 read 又被压缩成指针，形成循环。这是一条必须写进代码的 invariant（而非配置项）。
- **命名空间**：指针路径必须是绝对的、且不受 cwd 影响的，否则 `read` 的相对路径解析会随 cwd 漂移（`resolvePath(input, baseDir)` 在非绝对时拼 cwd）。
- **并发**：并行工具调用会同时触发钩子（`docs/extensions.md:124`"Tool calls from one assistant message can run in parallel"），写入必须是原子的（`wx` 语义，同 Pi）。

**隐私**

**[事实]** Headroom 的选择是**本地明文 SQLite、不加密**，并在 SECURITY.md 里明说（`SECURITY.md:89-92`）：

> "Cached request content is written to disk. CCR stores pre-compression originals — tool outputs, file contents, retrieved chunks — in a local SQLite database (`~/.headroom/ccr_store.db`, `0600`). Entries are plaintext and are not encrypted at rest. Set `HEADROOM_CCR_BACKEND=memory` if your deployment cannot accept disk persistence."

并且他们有 `persistence_allowed()`（`headroom/paths.py:133-148`）作为**唯一**的落盘裁决点，"the stateless guarantee ('writes nothing to the workspace') is enforced in one place"。

**[建议]** 照抄这个结构而不是照抄这个选择：
- 单一裁决函数（`can_persist()`），所有写盘路径都过它；
- 默认落在 OS `tmpdir`（不是仓库、不是 home 目录的隐藏文件夹）——Pi 自己就是这么做的，用户的清理工具会覆盖到；
- 会话结束删除；不提供"永久保存原文"的默认；
- 不做任何遥测（Headroom 默认开启的 beacon 是他们的商业选择，不是我们该抄的部分）。

**为什么不加密**：加密需要密钥管理，而密钥只能放本机，收益有限；把文件放 tmpdir + 0600 + 会话结束删除，在威胁模型上等价于"任何同用户进程都能读"，这一点必须承认而不是假装解决了。

---

## 5. 真实 Agent 任务的净 token：cached / uncached / read / write 记账

### 5.1 两个形态，两个完全不同的答案

**形态 A：SWE-agent 轨迹（Qwen3-32B，500 实例，10,663 turn）**

数据：`C:/Users/wudon/AppData/Local/Temp/jb/trajectories/lindenbauer/main_experiments/local-Qwen3-32B_thinking-baseline_N_1_M_10-collect_reasoning.0-verified-500`（Lindbauer et al. 2025 复现包的 Qwen3-32B thinking / M=10 臂，500 实例）。
脚本：`corpus/account.py`、`corpus/ceiling.py`。

**[未知]** 该目录名里的 `M_10` 指 SWE-agent 的 observation 窗口参数，但我没有逐条核对该臂在采样时是否已经启用了某种历史处理（复现包的 `config/` 里有大量 `N=`/`M=` 变体）。本文所有“无压缩基线”说法均指**该臂记录下来的 per-turn usage**，不是“模型原始上下文”。

| 项 | 值 |
|---|---|
| billed prompt | 76,519,213（write 31,835,837 / read 44,683,376，read 占比 58.4%） |
| output（含 internal reasoning） | 21,098,875，其中 reasoning **20,599,792** |
| Anthropic 价格下账单构成 | write **22.5%** + read **3.2%** + output **74.4%** |
| 可归因于 observation 字节的成本 | **$27.23 / $425.40 = 6.4%** |
| ⇒ 只压缩 observation 的**硬上限** | **6.4%**（OpenAI 价格下 7.5%） |

**形态 B：本机 300 个 Pi 会话**

数据：`C:/Users/wudon/.pi/agent/sessions/**/*.jsonl`（>20KB 的最大的 300 个，共 110,729 条 tool result）。
脚本：`corpus/meas.py`（每个数字都来自这一次运行）。

| 项 | 值 |
|---|---|
| billed prompt | 12,018,869,770（input 277,136,799 / cacheWrite **202,938** / cacheRead 11,741,530,033） |
| output | 45,726,600 |
| 账单构成（1 / 1.25 / 0.10 / 5 相对价） | input **16.5%** + write **0.0%** + read **69.9%** + output **13.6%** |
| cache read 占 billed prompt | **97.7%** |
| tool output 占 read token | **83.0%** |
| ⇒ read 侧可归因 | **58.0%** |
| ⇒ 入口侧（uncached）可归因 | **5.8%** |
| ⇒ **合并上限** | **63.8%** |

**这两个数字（6.4% vs 63.8%）不是矛盾，是形态差异**：
- 形态 A 是**长 CoT 的短会话**——reasoning token 把 output 推到 74%，observation 只占 8.5% 的 prompt。
- 形态 B 是**长上下文重读**——97.7% 的 prompt token 是 cache read，其中 83% 是 tool output。

**[建议]** 任何"我们能省 X%"的说法必须绑定形态。对 Outrim 来说，形态 B 才是目标形态（我们装在交互式 agent 上，不是跑 benchmark）。

### 5.2 一个必须写进文档的记账警告

**[测量]** 形态 B 里 `cacheWrite` 只有 **202,938 token**（占 prompt 的 0.0%）。原因是这些会话的 provider 大多是 OpenAI 形状或第三方（`opencode-g/deepseek-v4-flash` 22,059 turn、`openai-codex/gpt-5.6-luna` 18,356 turn、`openai-codex/gpt-6-luna` 16,336 turn……），Pi 记录的 `cacheWrite` 为 0（`prov.py`）。

**含义**：
- 用单一乘数表算 savings 会系统性错：把 1.25 当常量 → 在 OpenAI 形状上**高估**入口压缩收益（实际没有 write 溢价，省下的写价只有 1.0×）；把 0 当常量 → 在 Anthropic 形状上**低估**（实际是 1.25×）。
- **[建议]** 记账时把 `(input, cacheWrite, cacheRead, output)` 四个量当作原始观测量，价格乘数是**配置输入**，不是硬编码常量。这与 Pi 的 session 记录格式一致：`docs/message-types.md` 的 `Usage` 就分开记 `input / output / cacheRead / cacheWrite / cacheWrite1h / reasoning`，且明确"reasoning is already included in output; do not add it again"。

### 5.3 "恢复"到底多贵

**[测量]** 一次恢复 = 一次额外 LLM 往返 = 重新发送当前 prompt（按 read 价）+ 一次输出（按 output 价）：

```
round_trip = P × R + O × W_out
           = 91,457 × 0.10 + 170 × 5.0
           = 9,988 units
```

其中 P 是中位 prompt（91,457 token，`meas.py`），O 是 tool-call-only 轮次的输出中位数（**98 token**，p90 267；`meas.py` §3 的 `n=21,857`）。取 170 是保守值（含少量文本输出）。

| 输出假设 | round trip | 折合中位 tool result 个数 |
|---|---|---|
| 98（实测 p50） | 9,628 | 3.55 |
| 170（保守） | 9,988 | **3.68** |
| 267（实测 p90） | 10,473 | 3.86 |

而**被保留**的中位 tool result 只值 **2,711 units**。

**结论**：恢复一次的代价 ≈ **3.7 个中位 tool result**。这就是为什么判定必须逐条，而不是全局：

```
单条划算  ⟺  r < r* = (n − P)·(W + R·L) / [ (n − P)·(W + R·L) + round_trip ]
```

其中 `n` 是原文 token、`P` 是指针 token、`L` 是之后的 turn 数。

### 5.4 质量与运行时

**质量**：**[未知]**。日志里没有任务成功率的标签。唯一可对照的是外部实验（Lindbauer et al. 2025，arXiv 2508.21433）：observation masking 在 Qwen3-Coder 480B 上 54.8% vs LLM-Summary 53.8%，比 Raw 便宜 52% 且成功率 +2.6pp。但那是**历史掩码**，不是入口指针；不能直接搬。指针化引入的新风险是"模型为了拿原文多花一轮"——这在成功率上可能是 0 也可能是负的，必须实验。

**运行时**：

**[测量]** 本机进程启动地板（各 15 次，`cmd /c exit` 中位 26.7ms、`python -c pass` 25.2ms、`node -e 0` 37.0ms）。混合 provider 的 turn 时长中位 **8.2–9.6 s**，且与 prompt 长度**基本无关**：

| prompt 区间 | n | 时长 p50 | p90 |
|---|---|---|---|
| 0–20k | 201 | 8.2 s | 17.3 s |
| 20–50k | 1,132 | 9.6 s | 27.4 s |
| 50–100k | 2,625 | 9.3 s | 39.9 s |
| 100–200k | 691 | 8.9 s | 36.6 s |

`pearson r(output token, 时长) = 0.73`（n=4,649）——**时长主要由输出 token 决定，不是输入长度**。

**[建议]** 由此得两条：
1. 入口钩子的毫秒级开销（写入 + 生成一行）相对 9 s 无感，不需要为性能做异步/缓存/预热；
2. **不要为了"缩短 prompt 让模型更快"做压缩**——在这个样本里 prompt 从 20k 涨到 200k，时长不涨。省 token 是为了省钱和质量，不是为了延迟。真正影响延迟的是**恢复多一轮**（+数秒），这也是 §5.3 那个 3.7× 的主要构成之一（其输出部分）。

---

## 6. 最小有用算法对比（不做框架）

三个候选，按"先做的成本"排序。都只发生在 `tool_result` 上（入口），不碰历史。

### 6.1 A：入口指针（指针 + 原文本地落地）

```
if 内容 token ≥ G 且 不是错误输出 且 不是对 Outrim 缓存目录的 read:
    写原文到 <tmpdir>/outrim-<session>/<rand>   (0o600, wx)
    用一行 "raw: <abs path>" 替换（~20 token）
```

**值多少**（`corpus/calib.py`，把线性模型按"记录的 read 质量 / 模型推的 carried 质量"标定，标定系数 **0.612**，即线性模型高估 carried 质量 **1.63×**）：

| 门槛 G | 保留的 tool token 占比 | 单条 r* p25 | p50 | p75 | 净节省占全账单（r=0） | r=5% | r=20% |
|---|---|---|---|---|---|---|---|
| 0（全压） | 100% | 2.7% | **18.5%** | 51.7% | 59.7% | 57.7% | 51.6% |
| 200 | 95.3% | 30.0% | 52.3% | 73.9% | 57.7% | 56.7% | 53.8% |
| **500** | **87.4%** | 45.6% | **66.5%** | 81.6% | **53.2%** | 52.6% | 50.7% |
| 1000 | 74.1% | 60.0% | 76.7% | 87.4% | 45.4% | 45.0% | 43.9% |
| 2000 | 55.6% | 71.9% | **84.5%** | 91.8% | 33.8% | 33.6% | 33.1% |
| 5000 | 25.8% | 84.0% | 92.3% | 96.1% | 15.7% | 15.6% | 15.5% |

读法：
- `r*` 是"这条被压缩后，模型有多大概率需要拿回原文"的**盈亏平衡点**。`r < r*` 这条就该压。
- 门槛 0 / r=0 的 59.7% 已经顶到 §5.1 的 63.8% 上限，说明模型没有凭空放大收益（自洽性检查）。
- **门槛抬高会让"少压"和"压得准"互相拉扯**：G=2000 时只保留 55.6% 的量却拿到 84.5% 的 r*；G=500 时保留 87.4% 的量、r* 中位 66.5%。
- **[建议]** 起步取 **G ≈ 500–1000 token**，把 `r` 作为**线上观测量**（见 §6.4），不要现在就把 G 定成验收阈值。

### 6.2 B：字节完全相同的重复折叠（零风险）

**[测量]** 同一会话内、同一工具、内容**完全一致**且前一份还在上下文里的 tool result：**6,995,990 / 79,533,565 token = 8.80%**，per-session p10 0% / p50 3.5% / p90 18.1%（`idread2.py`）。
**[测量]** 收窄到"同工具同参数 + 文件在两份之间没有被改"：**5,901,731 token = 7.42%**（`meas.py` §2）；文件被改过（旧内容已错）只有 0.23%。
**[测量]** 只算 `read`：args-identical 重复占 read 字节 **19.5%**，byte-identical **23.3%**（`identread.py`）。

**这与 Headroom 的 0.1% 差 70 倍**（§1.5）。差别在语料：我的样本里 `read` 占 tool token **52.9%**，且 74% 的 read 带 `offset/limit`（分页读同一文件），重复率高。**[建议]** 把 B 保留为独立规则（它的正确性不需要概率假设：内容逐字节相同才折叠），但**上线前在目标流量上测 `r`**——Headroom 敢说"不值得"是因为他们测到 0.1%，我们测到 8.8%，两个都可能对。

### 6.3 C：静态 cutoff 历史掩码 —— 不做

理由见 §2.5：Pi 真实会话里能回本的候选 0.81%（Anthropic 形状）/ 1.40%（无 write 溢价）。**不是"CM 做错了"，而是他们的目标是"上下文压力"，我们的目标是"成本"，在长会话形状上这两者指向不同算法。**

同时要承认 C 的价值：它是**唯一**在上下文墙逼近时还能继续工作的机制（我们的 A/B 都不减少已发送的 prompt，只减少未来的增量）。**[未知]** 我方是否需要它作为"上下文墙兜底"——这取决于 Pi 自身 compaction 的触发频率，本次没有测。

### 6.4 建议的观测量（不是验收阈值）

只记四个数，就能事后判断算法对不对：

| 观测量 | 怎么得到 | 用来判什么 |
|---|---|---|
| 每次压缩的 `n`（原文 token）、`L`（后续 turn） | 钩子里现成 | 事前判定该不该压 |
| **`r`（实际恢复率）** | 观察对缓存目录的 `read` 次数 / 压缩次数 | 事后校准 `r*`；CM 用"mask 后再 read / 再跑"做同一件事（`index.ts:484-520`），Headroom 用 retrieve 调用计数 |
| 每次恢复后的**第一轮输出 token** | Pi session `usage.output` | 恢复的隐性成本 |
| 每次恢复**是否导致了额外轮次** | turn 计数 | 3.7× 的实证值 |

CM 的 re-read 遥测（`CHANGELOG.md:592-621`）是这套观测的先例，但需要注意它的分母陷阱：CHANGELOG 里专门修过一次"`reReadCount / contextMasksTotal` 用错了分母"的 bug（`:538-556`）。我们也别用"掩码次数"当分母，而要用"至少有一次机会被恢复的压缩次数"。

---

## 7. 任务实验建议（最小、可判定）

不做复杂框架，两件事就能闭合。

### 7.1 离线 replay（已可跑，不需要 API）

数据与脚本已有：
- 500 条 SWE 轨迹（真实 per-turn `raw_input / cached_input / output`），`corpus/account.py`、`corpus/ceiling.py`、`corpus/replay.py`；
- 300 个 Pi 会话（真实 per-turn usage），`corpus/meas.py`、`corpus/calib.py`、`corpus/breakpi.py`。

**必须声明的模型局限**（否则数字会被误读）：
- Pi 的 prompt 无法从消息字节重建成整数 token（`head.py`：重建 body / 记录 prompt 的中位比 0.28，隐含 head 为负——因为记录值包含我们看不到的 system/tool 定义与 provider 侧规范化）。所以 Pi 侧只能做**比例归因 + 标定**（§6.1 的 0.612），不能给绝对 token 预测。
- SWE 侧可以直接用记录值，但那批轨迹的 compaction/截断行为不知道，`replay.py` 的重建 total prompt 比记录高 **71.6%**，同样只能做相对比较。

### 7.2 小样本双臂在线实验

**度量**：净成本（按 provider 乘数分别记账的 input/write/read/output 四元组）、turn 数、恢复次数、任务是否完成（人工或测试判定）。
**样本量**（两比例、α=.05、power=.80、基线 p≈0.5）：

| 想检测的差异 | 每臂样本 |
|---|---|
| 3 个百分点 | 4,356 |
| 5 个百分点 | 1,568 |
| 10 个百分点 | 392 |

**不要做的事**：不要用"token 降了多少"当成功判据（§5.4：prompt 长度对时长几乎无影响）；不要一次只跑 20 个任务就下结论（连 10pp 都要 392/臂）。

---

## 8. 剩余未知（不要擅自填）

1. **恢复率 `r` 的真实值**：本次只能给出 `r*`（门槛），没有 `r`。要上线才知道。CM 的历史数据（read 33%、bash 0%，n=9/52）样本太小，不能当先验。
2. **质量影响**：入口指针是否降低任务成功率，完全未知。外部 masking 论文不能外推。
3. **Pi 的 compaction 频率与它对我们指针的破坏**：`compaction` 会把之前的消息折成摘要（`docs/session-format.md:225-245`），此时指针里指向的原文可能仍在磁盘但消息已被摘要覆盖。**我方缓存应在 compaction 后仍然有效**（因为路径没变），但这需要实测。
4. **`read` 的 `offset/limit` 分页读与"原文完整性"**：74% 的 read 是分页的（`identread.py`），一条指针指回"整个文件"与"当初那 200 行"不是一回事。**[未知]** 模型拿到"整个文件"是否比"当初那段"更好或更差。
5. **错误输出的压缩价值**：本次没有单独测（错误输出通常很短）。CM 明确"Don't skip errors — traceback filter specifically targets error output"（`index.ts:502`），但那是语义过滤，不是指针化。
6. **OpenCode 侧的唯一入口**：本次只在 Pi 上取数；OpenCode 的钩子是否给出"原文出口"未验证（属于另一分工，但"原文从哪里来"这个结论需要双宿主都成立才算完整）。
7. **并发写与 Windows 上 `wx` 的语义**：只在本机做过一次 25–37 ms 的 spawn 地板测量，没有做并发压测。
8. **`G` 的最优值**：§6.1 给了膝点区间（500–1000），但那是本机 300 个会话的形状；目标用户流量可能不同。

---

## 9. 一手来源与版本

**代码（本地 clone，行号均指这些 commit）**

| 项目 | commit | 日期 | URL |
|---|---|---|---|
| Headroom | `fbf7bc3278c11975299b9bd0a2ecf200dfd25a05` | 2026-10-17 | <https://github.com/headroomlabs-ai/headroom/tree/fbf7bc3278c11975299b9bd0a2ecf200dfd25a05> |
| Condensed Milk | `71f9e396951c42687f0c3456727b2b5c8c625da1` | 2026-05-23 | <https://github.com/tomooshi/condensed-milk-pi/tree/71f9e396951c42687f0c3456727b2b5c8c625da1> |
| SWE-agent（轨迹复现包） | `bf15b5fb7d279679035a007ac9a81084d6b9a89a` | 2025-11-18 | 本地 `C:/Users/wudon/AppData/Local/Temp/ct` |
| Pi 宿主 | npm `@earendil-works/pi-coding-agent@1.3.0` | — | 本地 `C:/Users/wudon/.pi/agent/install/releases/1.3.0/` |

**文档**

- Anthropic prompt caching：<https://platform.claude.com/docs/en/build-with-claude/prompt-caching>
- OpenAI prompt caching：<https://developers.openai.com/api/docs/guides/prompt-caching>
- Anthropic 上下文工程（just-in-time 与 lightweight identifiers，`file paths` 名列第一）：<https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents>
- The Complexity Trap（observation masking vs summarization，含 cache 讨论）：<https://arxiv.org/html/2508.21433v3>
- SWE-Pruner（另一个家族的对照，未采纳其成本口径）：<https://arxiv.org/abs/2601.16746>

**本报告的测量脚本（不入库，路径备查）**

`C:/Users/wudon/AppData/Local/Temp/corpus/`

| 脚本 | 产出 |
|---|---|
| `meas.py` | §5.1 形态 B 全部数字、§5.3 round trip、§2.5 回本率、§5.4 时延 |
| `calib.py` | §6.1 标定后的净节省表（含 0.612 标定系数） |
| `account.py` / `ceiling.py` | §5.1 形态 A 全部数字与 6.4% 上限 |
| `idread2.py` / `identread.py` | §6.2 重复率三组口径 |
| `prov.py` | §5.2 provider 分布与 `cacheWrite=0` 的事实 |
| `breakpi.py` | §2.5 的回改历史回本判据（Pi 语料） |
| `fold.py` | §2.5 的对照（SWE-agent 语料，同一公式结论相反） |
| `ptr.py` | §4.1 指针与工具定义 token 长度 |

---

## 10. 与产品已定决策的对应关系

| 已定决策 | 本报告的支持证据 |
|---|---|
| 只入口、不改历史 | §2.5：Pi 真实会话回本率 0.81%/1.40%；§3.3：入口压缩本身不破坏 cache |
| 一短行告诉原文在哪里、不要长警告 | §4.1：20–23 token 档；66 token 的啰嗦档贵 3× |
| 拒绝长免责声明 | §4.1：把"怎么拿回来"编码进路径那一行，省掉 56 token 的 system prompt 解释 |
| Rust 轻量、少 tool / 少 prompt | §4.2：不加工具省掉 105,176 units 的一次性前缀失效；§5.4：毫秒级开销相对 9 s 无感 |
| 初始 Pi + OpenCode 自动一次配置、不人工提醒 | §6.4：`r` 由对缓存目录的 `read` 自动观测，不需要用户动作 |
