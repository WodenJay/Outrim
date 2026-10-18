# 原文恢复与评估：入口指针、原文缓存与真实任务记账

研究票：<https://github.com/WodenJay/Outrim/issues/3>（不改 issue）
范围：**入口**压缩之后的"原文在哪里/怎么取回/要不要取回"，以及恢复的成本与评估方法。不重复结构算法（另一分工）、不重复语义论文（另一分工）、不重复宿主 API 全面调研（`research/pi-integration` 已完成），也不做 go/no-go 立项判断。

所有数字都绑定到具体的脚本与数据路径；**[事实]** 表示在源码/文档里能指到行，**[测量]** 表示本机复算结果，**[估算]** 表示用历史 usage 代入成本模型算出的**反事实**数字（不是开启压缩后实测的收益），**[建议]** 是方案取向，**[未知]** 是没测出来、不应擅自定阈值的地方。

本文没有任何"开启压缩后的实测节省"或"成功率影响"数据：这两类结论都需要 §7 的实验，文中出现的 `r*`、净节省、回本率均为估算。

---

## 0. 结论摘要

1. **"Headroom/CCR" 指 Compress-Cache-Retrieve**，不是"Cache-Compress-Retrieve"之类的营销词：压缩后的占位符带一个 hash，原文进了本地 store，模型靠 `headroom_retrieve` 工具（或 MCP 工具）拿回来。真实代码里有三个值得对齐的细节：**sticky 注入**（工具一旦出现就不能再消失，否则工具数组字节翻转破坏前缀）、**TTL 是滑动空闲窗**（默认 30 分钟 + 8× 硬上限）、**miss 消息本身可操作**（告诉模型重新 read/重跑）。
2. **Condensed Milk 里值得迁移的是入口侧机制**；它的静态 cutoff 历史掩码在本样本的**纯 cache 读写价差反事实模型**里不利：300 个真实 Pi 会话中，模型判定"能在会话/压缩结束前付清重写前缀代价"的候选只占 **0.81%**（Anthropic 定价形状）/ **1.40%**（无 write 溢价）。**这是估算，不含质量影响，也不构成对历史方案的判决**；它只说明"入口比历史更值得先做"，而历史本就不在本产品范围内。
3. **指针文字本身很短，但具体长度依赖环境**：在 cl100k 下，路径样式的一行计数约 **20–23 token**，Headroom 的 `headroom_retrieve` 工具定义 **100 token**、Condensed Milk 的系统提示解释 **56 token**、啰嗦指针 **66 token**（§4.1 会说明这为什么**不是**生产线上的固定值）。**专用检索工具既不是免费，也不是天然昂贵**：它的全前缀失效代价（§3.3 的 105,176 units）**只适用于“会话中途才新增该工具”这一种接入方式**；若从会话开始就稳定注册、全程不变，就不支付这次中途 rewrite。
4. **cached/uncached 记账必须按 provider 形状分开算，不能用一个"5×"拍死**。同一份 SWE 轨迹（Qwen3-32B，Anthropic 价）：output 占 **74.4%**，可归因于 observation 字节的上限只有 **6.4%**；同一台机器上的 300 个 Pi 会话：cache read 占账单 **69.9%**、output 只占 **13.6%**，可归因于 tool output 的上限 **63.8%**。"压缩 tool output 能省多少"这个问题的答案完全取决于 agent 的形状，不能跨形态外推。
5. **"首次压缩后历史保持稳定"不等于"历史被重写"**。前向压缩（内容第一次进上下文时就已经是短的）在之后的每个 turn 都前缀稳定；只有**改已经发出去的历史**才需要付一次 rewrite premium。官方文档支持这一点，但没有任何一家说"压缩必然破坏 cache"。
6. **恢复的估算成本主要是“多一轮 LLM 往返”，不是 pointer token**。在 §5.3 的反事实模型里，中位 prompt 91.5k token 时一次恢复往返估价 ≈ **9,988 units ≈ 3.7 个中位 tool result**。**这是模型估算，不是在开启压缩的真实会话里测得的节省**；它依赖“恢复后原文会继续携带”“未合批”“模型输出量取中位”等假设，且不包含“质量下降导致额外轮次”。因此 §5.3 给出的 `r*` 只是**在该成本假设下的临界恢复率**，不是质量判据，也不是实测的真实恢复率。
7. **收益高度依赖尺寸门槛，而门槛本身是待定参数**：门槛 0 时模型保留 100% 的 tool token、单条 `r*` 中位 18.5%；门槛 2000 token 时保留 55.6% 的量、`r*` 中位 84.5%。**`r*` 高只说明“在该成本模型下更容易划算”，它既不测质量、也不测真实恢复率**；门槛高也意味着大量原文没有压缩（收益本身变小），所以“高 `r*`”不是“更安全”，更不是验收阈值。
8. **字节完全相同的重复折叠在“原文内容”层面不额外丢失信息，但风险未清**：内容完全相同的重复 tool result 占 tool-output token 的 **8.80%**（同工具同参数且文件未变：**7.42%**）。它仍保留两处未验证风险：**可访问性**（模型能否/会不会去引旧副本，尤其旧副本被 compaction 或分支操作遮住时）与**任务行为**（旧副本可能正是任务需要的历史状态）。Headroom 在它们自己的流量上测到 byte-identical 重复 Read 只占 **0.1%**——两个数差 70 倍，说明“重复率”是流量形状属性，上线前必须在**目标用户流量**上测。
9. **本样本里没有观察到输入长度与延迟的明显关系**：本机进程启动地板 26.7/25.2/37.0 ms（`cmd`/`python`/`node` 各 15 次中位），混合 provider 下 turn 中位时长 8.2–9.6 s，按 prompt 分桶后中位时长基本持平（§5.4）。**这只是本样本的相关现象，不是因果结论，也没有测完整压缩路径**，所以不能据此断言“任何算法开销都是毫秒级无感”，也不能给出“不要为了延迟做压缩”这类普遍指令。
10. **质量无法从日志测**：没有任何真实数据能回答“少了这段原文，任务成功率掉多少”。可先做小样本**配对**任务以发现失效模式（不追求统计功效）；若要做成功率的两比例统计结论，那是另一件事，按 α=.05/power=.80 在 p≈.5 下检 5 个百分点约需 **1,568/臂**（该数字只成立于独立两比例假设，不是 MVP 门槛）。

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

[测量] 用 cl100k 实测算这些**形态**的长度（`%TMP%` 为显示占位，不是真实路径）：`ccr marker` 35 token，`ccr bracket` 38 token，"`[outrim: 412 lines, full text at outrim:7f3a9c]`" 21 token，"`raw: %TMP%\outrim\7f3a9c2e.log`" 20 token。

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

**这一段对我们是两条可直接参考的规则**：滑动空闲 TTL + 硬寿命上限（默认 1800s / 8×）。**它们是 Headroom 的选择，不是“现成正确答案”**：对 Outrim 而言，生命周期还要与 resume/fork、历史指针重放、操作系统临时目录不保证及时清理这些因素一起决（见 §4.4）。

### 1.5 read_lifecycle：他们自己也承认"重复 Read 去重不值钱"

**[事实]** `headroom/transforms/read_lifecycle.py:1-21` 的模块 docstring（`:11-13`、`:15-19`）：

> "Real-world data shows 75% of Read output bytes fall into these two categories: 67% stale (file edited after Read), 12% superseded (file re-Read later), Only 20% are fresh"
>
> "NOTE: a first-sight repeat-Read dedup mechanism (DEDUP_REPEAT) was prototyped here and removed — `headroom audit-reads` measured byte-identical repeats at 0.1% of Read bytes on real traffic."

**这是一个有价值的反面证据**：他们把“重复”细分成 **stale（文件后来被改，旧内容已经错了）** 与 **superseded（同文件被重读，内容冗余）**，只对这两种做替换。需要准确理解这个“安全”的边界：**它只说明“被替掉的字节不再代表当前文件状态”**（就这一点而言旧副本已无可引用价值），**不说明“旧观察对任务无用”**：旧内容仍可能是任务所需的历史快照（例如“改之前它长什么样”、“当时这条命令报了什么错”）。因此对我们而言，这只能作为“字节一致折叠不额外丢失原文内容”的旁证，**不能推出可证明的任务安全性**。

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

### 2.5 历史掩码：在本样本的回本估算里不利（仅作对照）

> 说明：历史改写不在本产品范围内。本节只作为"入口与历史在经济上不是同一件事"的对照，**不用来重新讨论是否要做历史方案**。

**[测量/估算]** 数据来自真实 Pi 会话（`C:/Users/wudon/.pi/agent/sessions/**/*.jsonl`，>20KB 的最大的 300 个；取数脚本 `corpus/breakpi.py`、`corpus/meas.py`），但下面的"回本"是一个**反事实模型**：把第 i 条 tool result 换成 pointer 后，从该点之后的全部内容要重写一次（write 1.25×）而不再是读（0.10×）。回本条件：

```
m·R·L  >  (m + rest)·(W − R)      ⇒      L > 11.5·(m+rest)/m
```

（W=1.25, R=0.10；若 provider 无 write 溢价则系数变 9.0）

| 量 | 值 |
|---|---|
| 候选数（有后续 turn 的 tool result） | 111,047 |
| 需要的复用轮数 | p10 388 / **p50 4,567** / p90 80,690 |
| 模型判定"能在会话/压缩结束前回本"的 | **898 = 0.81%** |
| 无 write 溢价时（W=1.0） | **1.40%** |
| 被重写的后缀 / 被掩掉的质量 | **p50 396×** |

这个模型只计 cache 读写价差，**不含**改写带来的质量影响（历史快照丢失、模型行为变化），所以"不利"只就纯读写价差这一维度成立。

> 同一套公式用在 SWE-agent 轨迹上结论相反（`corpus/fold.py`：masked:rewritten 比中位 **15.8**，回本只需 **0.7 个缓存 turn**，**82.9%** 的 turn 落在该模型的划算区）。差别在于那批轨迹的 observation 相对后续上下文极大。**结论：回改历史的回本程度是轨迹形状函数**，本样本这种长会话形状在纯读写价差维度上不利——这只是"入口比历史更值得先做"的一个弱证据，不是对历史方案的判决。

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

1. **入口压缩本身不改变前缀。** 如果 tool result 在**第一次进上下文时**就已经是短指针，那么之后每个 turn 的前缀都是同一串字节——前缀稳定性是这个设计的**结构性结果**（前提是工具数组、system 等前缀构成也不变；一旦其中任一在会话中途变化，仍会从改动点起失效）。
2. **"首次压缩后历史保持稳定" ≠ "历史被重写"。** 前者说的是"从这次之后前缀不再变"，后者说的是"把已经写进缓存的内容改掉"。OpenAI 文档点名的是后者（"summarization, compaction, or context truncation can change the prefix"），不是前者。
3. **不能泛称"压缩必然破坏 cache"。** 破坏与否取决于**改动位置**：改 `tools`/`system` 或已写入的前缀 → 从改动点起全部失效；只在尾部追加 → 前缀照旧命中。回改历史之所以贵，是因为它把"尾部追加"变成了"中段改写"。

**[测量]** 把第 2 点里的"中段改动"量化：在我方语料里，若一次工具数组变化（**会话进行到中途**才新增检索工具）发生在中位 prompt 91,457 token 时，代价 = `91,457 × (1.25 − 0.10)` = **105,176 units**，等于 **38.8 个中位 tool result** 的价值。这个数字**只属于"中途新增"这一种接入方式**；从会话开始就稳定注册的工具数组不支付这次 rewrite（它只支付定义本身的常驻成本，见 §4.2）。

---

## 4. 原文缓存与极短位置指针

### 4.1 指针能有多短（cl100k 实测）

| 候选 | token | 说明 |
|---|---|---|
| `raw: %TMP%\outrim\7f3a9c2e.log` | **20** | 纯路径（含标签）；`%TMP%` 是显示占位，非真实绝对路径，见下文 |
| `[outrim] raw: %TMP%\outrim\7f3a9c2e.log` | 22 | 加产品名 |
| `[outrim: 412 lines, full text at outrim:7f3a9c]` | 21 | 无路径、需自己解析 id |
| `[cm-masked read] {path} ({n} lines, {size})` | 23 | Condensed Milk 的 read 形态 |
| `[outrim] `git status` (3.1KB -> 0.2KB) raw: …` | 36 | 加了来源命令与原/新尺寸 |
| 啰嗦指针（含"可能遗漏细节，请核实"） | **66** | 3× 于最简 |
| Condensed Milk 系统提示解释 | 56 | 每个 session 一次 |
| `headroom_retrieve` 工具定义 | 100 | 进 tools 数组 |

**这些数字是 cl100k 分词器下的计数示例，不是可直接 `read` 的绝对路径。** `%TMP%` 只是我为压缩显示而写的占位：内置 `read` 是否展开 `%TMP%` **未经验证**，也不应依赖；实际实现必须写**展开后的绝对路径**（如 `C:/Users/<u>/AppData/Local/Temp/outrim-<sid>/7f3a9c2e.log`），而这类路径的 token 数**随用户名长度、盘符样式（`C:/` vs `C:\`）、分词器版本变化**，所以不能宣称"生产线固定 20–23 token"——**20–23 只表示"路径样式的一行在这个分词器下大约这个量级"**，比 66 的啰嗦指针低一个数量级这个量级关系才是要用的结论。

**[建议]** 用户已定"一短行告诉原文在哪里、拒绝长警告"，那么就写成**一行可直接 `read` 的绝对路径**（外加一个极短标签）。不要放免责声明（66 档），也不要放"可能不完整"的警告——那是在替模型做它自己该做的判断，而且贵约 3 倍。

### 4.2 复用已有 read vs 极少新工具

| 方案 | 一次性代价 | 长期代价 | 额外风险 |
|---|---|---|---|
| **A. 复用内置 `read`**（指针给绝对路径） | 0 | 0（path token 已在指针里） | 需保证本产品不去压缩"读自己缓存目录"的 read，否则自噬 |
| **B. 专用检索工具** | **取决于接入方式**：会话中途新增才付"工具数组变化"价（§3.3 的 105,176 units ≈ 38.8 中位条目）；从会话开始稳定注册则无此项 | 定义常驻：`100×1.25 + 100×0.1×135` = **1,475 units ≈ 0.54 中位条目**（估算，L=135 取中位） | 必须保证工具数组全程稳定（中途翻转会反复触发前缀失效，`tool_injection.py:423-429` 的坑） |
| C. MCP 工具 | 同 B + 多一层 server | 同 B | 更多活动部件 |

**[事实]** Pi 的 `read` 对指针路径没有沙箱限制：`read` 的路径解析走 `resolveReadPathAsync(path, ctx?.cwd || cwd)`（`dist/core/tools/read.js:73`）→ `resolveToCwd`（`path-utils.js:42-44`，只做 `~` 展开与 `@` 前缀处理）→ `resolvePath(input, baseDir)`，`isAbsolute` 时直接 `nodeResolvePath`（`dist/utils/paths.js:83-87`）。因此指针指向 `tmpdir` 或我们自己的缓存目录都能被打开，受同样的 2000 行 / 50KB 与 `offset` 分页约束（`dist/core/tools/truncate.js:10-11`；`read.js:134-163`）。

**[建议]** 首版走 A。理由不是"省 token"（B 的长期代价只有约 0.54 个条目，估算），而是**A 去掉了整整一类 bug**：工具数组翻转、sticky 状态、工具描述与实际能力不一致。B 的唯一优势是能在指针里放一个短 id 而不是长路径——不值这个复杂度。

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

**生命周期：目前是待决选项，不是已定答案**

**[事实]** Headroom 的两条参数是滑动空闲 TTL（默认 1800s）+ 绝对寿命 8×（`crates/headroom-core/src/ccr/mod.rs:59-79`）。这是**他们在自己的宿主与流量上的选择**，可以直接看，但不能当成对我们的"现成正确答案"，因为约束集不同。

**[测量]** 一条 tool result 之后**中位还有 135 个 assistant turn**（p75 275、p90 455，`meas.py` §6 的候选统计）。这个数字说明"一次压缩后的多轮之后仍可能想看原文"，但它**不能**换算成"5 分钟绝对不够"或"30 分钟必定覆盖会话"：turn 间隔与总会话跨度在这次统计里没测，单靠轮数无法推出墙钟时间。任何 TTL 都需要实测会话时间分布才能定。

**[待决]** 至少要在这几项里做选择，每项都有真实冲突，本文不给结论：

| 选项 | 好处 | 冲突/代价 |
|---|---|---|
| 会话结束（shutdown）时删除 | 隐私最小面、实现最简 | 与 resume / fork 冲突：会话被恢复后，历史消息里的指针仍在，但原文已删 → 指针变成死链。fork 出的分支也会继承同一批指针 |
| 滑动空闲 TTL（如 30 min） | 覆盖"离开一会儿再回来" | 需要在 resume 路径上校验并可能"复活"或降级为死链；TTL 值与真实间隔分布强相关，未测 |
| 按会话 id 分目录 + 显式世代号 | resume 时能判断"这是旧会话的原文"并如实报告不可用 | 需要跨会话持久记录，磁盘占用与隐私面都变大 |
| 一直保留到用户手动清理 | 指针永不失效 | 残留面最大，且与"不做隐藏持久存储"的产品态度冲突 |

**能确定的只有两条**：(a) 指针指不到原文时**必须显式说明失效**，不能静默返回空或旧内容（否则等于把"可恢复"变成谎言）；(b) 不能靠"会话结束删除"这一句话掩盖 resume/fork 的语义——那需要单独的设计决定，属于**未决**项。

**[事实/未知] 关于"交给 OS 清理"**：把残留放在 OS 临时目录**不等于**会被及时清理——操作系统不保证按时回收 tmp（清理策略依赖平台/发行版/磁盘压力，Windows 更是不保证），所以"崩溃残留由系统兜底"只能作为**残余风险的接受**，不能当作生命周期机制。

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
- 默认落在 OS `tmpdir`（不是仓库、不是 home 目录的隐藏文件夹）——与 Pi 的 output-files 同一约定，用户的清理工具至少会覆盖到同一目录（但不保证及时，见上）；
- 不提供"默认永久保存原文"；具体删除时机是上表里**未决**的选项之一；
- 不做任何遥测（Headroom 默认开启的 beacon 是他们的商业选择，不是我们该抄的部分）。

**关于权限位**：用 `0o600` / `flag "wx"` 与 Pi 的 output-files 对齐是合理的**降低暴露面**做法，但**不要把它当成安全等价**：
- **Windows 上 0600 不是 ACL 保证**：Node 在 Windows 上对 `mode` 基本不生效（权限由 ACL 决定），同用户其他进程、以及某些继承 ACL 的子目录都可能读到。
- 因此"tmpdir + 0600 + 会话结束删除"**不能**被描述为"与 X 等价"。按本轮已标为未知的权限问题处理：实现时需要在两个宿主上实测有效权限，结论前不要写安全性声明。

**为什么不加密**：加密需要密钥管理，而密钥只能放本机，收益有限。可以确定的只是：**在这一威胁模型下，落盘原文的可读面由 OS 权限决定，我们不宣称它被保护过**。

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

**注意这是反事实估算，不是实测节省**：以上 `P=91,457`、`O=170`、以及乘数（W=1.25 / R=0.10 / output 5.0）都是把历史 usage 代入价格表得到的模型值，**没有任何会话在真实开启压缩的情况下测过节省**。

由此得到的判定式（同样只是模型）：

```
单条划算  ⟺  r < r* = (n − P)·(W + R·L) / [ (n − P)·(W + R·L) + round_trip ]
```

其中 `n` 是原文 token、`P` 是指针 token、`L` 是之后的 turn 数。`r*` 的含义严格限于此式：**在给定成本模型下、假设恢复只发生一次且立即**的临界恢复概率。它至少漏掉四件事，且方向不一致（所以不能靠"偏保守"来自我安慰）：

1. **恢复之后原文继续携带**（这一项使真实成本**高于**模型）：拿回来的 `n` token 会留在后面每一轮的 prompt 里，按 read 价反复计费——上述式子只算了恢复当轮。若还剩 `L` 轮，成本接近 `round_trip + n·R·L`，`r*` 会明显变小。
2. **原文是否本来就在 cached prefix 里**（这一项使模型**高估收益**）：若已经缓存，压缩省下的不是 `W` 而是 `R` 级别的差，式中按 `W` 计的省额会被高估，`r*` 也偏大。
3. **合批恢复**（这一项使真实成本**低于**模型）：模型可能用一个 turn 同时恢复多条原文，round_trip 被摊薄，真实 `r*` 会更大，式中没有这一项。
4. **质量导致的额外轮次**（方向偏**贵**，且完全在估计之外）：缺原文导致判断错误 → 多跑几轮，这部分成本与 §5.4 的质量未知同源。

所以 `r*` 只能读作"**近似示意**：这个量级下判定必须逐条而不是一刀切"，**不能**读作"这条就该压"。要拿到真实值，只能做 §7 的实验。

### 5.4 质量与运行时

**质量**：**[未知]**。日志里没有任务成功率的标签。唯一可对照的是外部实验（Lindbauer et al. 2025，arXiv 2508.21433）：observation masking 在 Qwen3-Coder 480B 上 54.8% vs LLM-Summary 53.8%，比 Raw 便宜 52% 且成功率 +2.6pp。但那是**历史掩码**，不是入口指针；不能直接搬。指针化引入的新风险是"模型为了拿原文多花一轮"——这在成功率上可能是 0 也可能是负的，必须实验。

**运行时**：

**[测量]** 本机进程启动地板（各 15 次，`cmd /c exit` 中位 26.7ms、`python -c pass` 25.2ms、`node -e 0` 37.0ms）。混合 provider 的 turn 时长中位 **8.2–9.6 s**；按 prompt 长度分桶后中位时长未见单调上升：

| prompt 区间 | n | 时长 p50 | p90 |
|---|---|---|---|
| 0–20k | 201 | 8.2 s | 17.3 s |
| 20–50k | 1,132 | 9.6 s | 27.4 s |
| 50–100k | 2,625 | 9.3 s | 39.9 s |
| 100–200k | 691 | 8.9 s | 36.6 s |

`pearson r(output token, 时长) = 0.73`（n=4,649）。

**这些只是本样本的相关现象，局限必须说清**：
- provider 混合（不同模型/服务），分桶只按 prompt 长度，**没有控制输出长度、模型、网络、服务端负载**——所以"输入长度与时长基本无关"不能读成因果，也不能推广到单个 provider 的延迟曲线。
- **这只是 turn 级观测，没有测完整压缩路径**（钩子执行 + 落盘 + 输出改写）。所以本文**不给出任何算法的毫秒开销结论**。
- 由此**不能**推出"不要为了延迟做压缩"这类普遍指令；也不能断言 prompt 缩短无助于延迟——只能说在这个混合样本里看不到明显关系。

可以保留的弱结论：相对 8–10 s 的 turn 量级，一个纯本地、无同步网络调用的钩子不太可能成为主导项——这仍需实测，不是从上面数据推出的。真正可预期的时间成本是**恢复多一轮**（数秒到数十秒）。

---

## 6. 最小有用算法对比（不做框架）

三个候选，按"先做的成本"排序。都只发生在 `tool_result` 上（入口），不碰历史。

### 6.1 A：入口指针（指针 + 原文本地落地）

```
if 内容 token ≥ G 且 不是错误输出 且 不是对 Outrim 缓存目录的 read:
    写原文到 <tmpdir>/outrim-<session>/<rand>   (0o600, wx；Windows 上 0600 非 ACL 保证，见 §4.4)
    用一行 "raw: <abs path>" 替换（~20 token）
```

**值多少（反事实模型估算，非实测）**：`corpus/calib.py` 把线性模型按"记录的 read 质量 / 模型推的 carried 质量"标定，标定系数 **0.612**（线性模型高估 carried 质量约 **1.63×**）。下表所有"净节省"都是**把历史 usage 代入价格模型算出来的**，不是在开启压缩的真实会话里测到的；它同时继承了 §5.3 列出的四个缺口（恢复后继续携带、是否已含在 cached prefix、合批、质量引起的额外轮次）。读表时把它当"不同门槛下的相对排序"，不要当收益预测。

| 门槛 G | 保留的 tool token 占比 | 单条 r* p25 | p50 | p75 | 净节省占全账单（r=0） | r=5% | r=20% |
|---|---|---|---|---|---|---|---|
| 0（全压） | 100% | 2.7% | **18.5%** | 51.7% | 59.7% | 57.7% | 51.6% |
| 200 | 95.3% | 30.0% | 52.3% | 73.9% | 57.7% | 56.7% | 53.8% |
| **500** | **87.4%** | 45.6% | **66.5%** | 81.6% | **53.2%** | 52.6% | 50.7% |
| 1000 | 74.1% | 60.0% | 76.7% | 87.4% | 45.4% | 45.0% | 43.9% |
| 2000 | 55.6% | 71.9% | **84.5%** | 91.8% | 33.8% | 33.6% | 33.1% |
| 5000 | 25.8% | 84.0% | 92.3% | 96.1% | 15.7% | 15.6% | 15.5% |

读法：
- `r*` 是"这条被压缩后，模型有多大概率需要拿回原文"的盈亏平衡点，**但只在这个成本模型里成立**（§5.3 列了四个被漏掉的项：恢复后继续携带、是否已含在缓存、合批、质量引起的额外轮次）。因此这张表是"**相对量级示意**"，不是逐条判决，更不能当验收阈值。
- 门槛 0 / r=0 的 59.7% 已经顶到 §5.1 的 63.8% 上限，说明模型没有凭空放大收益（自洽性检查）。
- **门槛抬高会让"少压"和"压得准"互相拉扯**：G=2000 时只保留 55.6% 的量、`r*` 中位 84.5%；G=500 时保留 87.4% 的量、`r*` 中位 66.5%。注意 `r*` 高只表示"在这个成本模型下更不容易亏"，**它不测质量，也不测真实 `r`**；而且门槛越高，"没被压缩的原文"越多，可省的总额越小。
- **[建议]** 若要选一个起点，本样本的相对排序指向 **G ≈ 500–1000 token**；但这是模型内的排序，不是实测最优，也不该现在定成验收阈值。把 `r` 作为**线上观测量**（见 §6.4）后再调。

### 6.2 B：字节完全相同的重复折叠（不额外丢失原文内容，风险仍需验证）

**[测量]** 同一会话内、同一工具、内容**完全一致**且前一份还在上下文里的 tool result：**6,995,990 / 79,533,565 token = 8.80%**，per-session p10 0% / p50 3.5% / p90 18.1%（`idread2.py`）。
**[测量]** 收窄到"同工具同参数 + 文件在两份之间没有被改"：**5,901,731 token = 7.42%**（`meas.py` §2）；文件被改过（旧内容已错）只有 0.23%。
**[测量]** 只算 `read`：args-identical 重复占 read 字节 **19.5%**，byte-identical **23.3%**（`identread.py`）。

**这与 Headroom 的 0.1% 差 70 倍**（§1.5）。差别在语料：我的样本里 `read` 占 tool token **52.9%**，且 74% 的 read 带 `offset/limit`（分页读同一文件），重复率高。**[建议]** 把 B 保留为独立规则：它的收益不需要概率假设（只有内容逐字节相同才折叠），所以**被替掉的字节在"原文内容"意义上不额外丢失**。但它**不是零风险**：折叠后旧副本是否还能被引用（尤其旧副本落入 compaction 摘要或被分支操作遮住时）、旧副本是否正是任务需要的历史状态，这两点都未验证。**上线前在目标流量上测 `r` 与折叠后的行为**——Headroom 敢说"不值得"是因为他们测到 0.1%，我们测到 8.8%，两个都可能对。

### 6.3 C：静态 cutoff 历史掩码 —— 不做

理由见 §2.5：在纯读写价差反事实模型下，Pi 真实会话里能回本的候选 0.81%（Anthropic 形状）/ 1.40%（无 write 溢价）；该模型不含质量、不含"旧快照本身有价值"，所以只能作为弱证据。**不是"CM 做错了"**——他们的目标是"上下文压力"，我们的目标是"成本"，在本样本的长会话形状上两者指向不同算法。

（历史方案是否在"上下文墙逼近"时作为兜底有价值，属于本产品范围之外的问题，本文不讨论。）

### 6.4 建议的观测量（不是验收阈值）

记这几个数，就能事后判断算法对不对：

| 观测量 | 怎么得到 | 用来判什么 |
|---|---|---|
| 每次压缩的 `n`（原文 token）、`L`（后续 turn） | `n` 在钩子里可得；`L` **不是钩子里的现成值**——只能事后统计（压缩点之后的 turn 数）或事前用历史分布预测 | 事后校准 / 事前预测该不该压 |
| **`r`（实际恢复率，按唯一条目）** | 至少被恢复过一次的条目数 / 被压缩的唯一条目数 | 事后校准 `r*`；CM 用"mask 后再 read / 再跑"做同一件事（`index.ts:484-520`），Headroom 用 retrieve 调用计数 |
| **额外读次数** | 对缓存目录的 `read` 调用总数 | 与 `r` 分开记：一次恢复可含多次/分段读取，`r` **不等于**读次数，也不等于压缩次数 |
| 每次恢复后的**第一轮输出 token** | Pi session `usage.output` | 恢复的隐性成本 |
| 每次恢复**是否导致了额外轮次** | turn 计数 | 检验 §5.3 的 3.7× 估算是否属实 |

CM 的 re-read 遥测（`CHANGELOG.md:592-621`）是这套观测的先例，但需要注意它的分母陷阱：CHANGELOG 里专门修过一次"`reReadCount / contextMasksTotal` 用错了分母"的 bug（`:538-556`）。我们也别用"掩码次数"当分母，而要用"至少有一次机会被恢复的压缩次数"。

---

## 7. 任务实验建议（最小、可判定）

不做复杂框架，两件事就能闭合。

### 7.1 离线 replay（已可跑，不需要 API）

数据与脚本已有：
- 500 条 SWE 轨迹（真实 per-turn `raw_input / cached_input / output`），`corpus/account.py`、`corpus/ceiling.py`、`corpus/replay.py`；
- 300 个 Pi 会话（真实 per-turn usage），`corpus/meas.py`、`corpus/calib.py`、`corpus/breakpi.py`。

**必须声明的模型局限**（否则数字会被误读）：
- Pi 的 prompt 无法从消息字节重建成整数 token：`head.py` 得到"重建 body / 记录 prompt"的中位比 **0.28**。**这个数目前没有可核实的解释，标为未知。** 一个**不适用的**解释要先排除："记录值里包含我们看不到的 system/tool 定义"——这个方向只会让重建值更小、记录值更大，差值为正；它**不能**解释"隐含 head 为负"。真实原因（provider 侧规范化、记录口径、缓存计数、某类消息未落盘……）未查证。实际影响不受解释影响：Pi 侧只能做**比例归因 + 标定**（§6.1 的 0.612 同样是拟合系数，不是因果量），不能给绝对 token 预测。
- SWE 侧可以直接用记录值，但那批轨迹的 compaction/截断行为不知道，`replay.py` 的重建 total prompt 比记录高 **71.6%**，同样只能做相对比较。

### 7.2 小样本双臂在线实验

**两层目标，别混**：

1. **小样本配对实验（先做）**：覆盖真实任务形态（长输出、分页读、报错排查、跨文件改），目的是**发现失效模式**——哪些原文一被指针化模型就得多跑一轮、哪些它根本不去读。这一层不追求统计功效，10–30 个任务足够产生清单；它回答"我们要不要继续/改规则"，不回答"成功率掉了几个点"。**不要说 20 个任务没用**——它在它的目标上是有效的。
2. **成功率对比（另做，若需要）**：要给出"入口指针使任务成功率下降不超过 X"的**统计**结论，才需要下面的样本量（独立两比例、α=.05、power=.80、基线 p≈0.5）：

| 想检测的差异 | 每臂样本 |
|---|---|
| 3 个百分点 | 4,356 |
| 5 个百分点 | 1,568 |
| 10 个百分点 | 392 |

**这个 1,568 只成立于"两独立比例 + 上述检验假设"**，配对设计/更小方差/单侧检验都会明显降低所需样本；它是"若要把成功率当验收指标"的量级参考，**不是 MVP 门槛**。

**度量**：净成本（按 provider 乘数分别记账的 input/write/read/output 四元组）、turn 数、按唯一条目计的恢复率 `r` 与额外读次数、任务是否完成（人工或测试判定）。

**不要做的事**：不要用"token 降了多少"当成功判据（本样本里 prompt 长度与时长无明显关系，见 §5.4 的限定）；也不要用 §5.3 的 `r*` 当质量依据。

---

## 8. 剩余未知（不要擅自填）

1. **恢复率 `r` 的真实值**：本次只能给出 `r*`（门槛），没有 `r`。要上线才知道。CM 的历史数据（read 33%、bash 0%，n=9/52）样本太小，不能当先验。
2. **质量影响**：入口指针是否降低任务成功率，完全未知。外部 masking 论文不能外推。
3. **Pi 的 compaction 与 resume/fork 对我们指针的影响**：`compaction` 会把之前的消息折成摘要（`docs/session-format.md:225-245`），此时指针指向的原文可能仍在磁盘、但消息已被摘要覆盖。路径不变意味着**文件仍可读**，但"摘要之后模型是否还会/还能用旧指针"未实测。resume 与 fork 则是另一层：它们会让"会话结束就删原文"的策略产生死链，且两处都未测。**两项都标未知**，不要用"路径没变所以没事"打发。
4. **`read` 的 `offset/limit` 分页读与"原文完整性"**：74% 的 read 是分页的（`identread.py`），一条指针指回"整个文件"与"当初那 200 行"不是一回事。**[未知]** 模型拿到"整个文件"是否比"当初那段"更好或更差。
5. **错误输出的压缩价值**：本次没有单独测（错误输出通常很短）。CM 明确"Don't skip errors — traceback filter specifically targets error output"（`index.ts:502`），但那是语义过滤，不是指针化。
6. **OpenCode 侧的唯一入口**：本次只在 Pi 上取数；OpenCode 的钩子是否给出"原文出口"未验证（属于另一分工，但"原文从哪里来"这个结论需要双宿主都成立才算完整）。
7. **并发写与 Windows 上 `wx` 的语义**：只在本机做过一次 25–37 ms 的 spawn 地板测量，没有做并发压测。
8. **`G` 的最优值**：§6.1 的区间（500–1000）只是本机 300 个会话在反事实模型下的相对排序；目标用户流量可能不同，且模型缺口（§5.3）会移动这个区间。
9. **原文缓存的生命周期选择**：§4.4 的四选一尚未决定，取决于 resume/fork 的语义与实测会话时间分布。
10. **恢复后原文的携带成本**：§5.3 的判定式把它漏掉了，需要实验测"恢复一次总共多花多少"（含后续轮次）。
11. **`r` 与"额外读次数"的真实联合分布**：两者分离记账，但都只能上线后测；CM 的历史数据（n=9/52）不能当先验。

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
| 只入口、不改历史 | §3.3：在首次进上下文时就是短指针，则之后前缀稳定（"首次压缩后历史稳定"≠"历史被重写"）；§2.5：纯价差模型下历史改写回本率 0.81%/1.40%（估算，非判决） |
| 一短行告诉原文在哪里、不要长警告 | §4.1：一行绝对路径在这个分词器下量级 20–23 token（随环境变），啰嗦指针 66 token、约贵 3 倍 |
| 拒绝长免责声明 | §4.1：把"怎么拿回来"编码进那行路径，省掉 56 token 的 system prompt 解释 |
| Rust 轻量、少 tool / 少 prompt | §4.2：复用 `read` 少一类工具数组/注入状态的风险（若在会话中途新增工具，还会付 §3.3 的 105,176 units 中段 rewrite；稳定注册则不付）；§5.4：本样本未见明显延迟敏感 |
| 初始 Pi + OpenCode 自动一次配置、不人工提醒 | §6.4：`r` 由对缓存目录的 `read` 自动观测，不需要用户动作 |
