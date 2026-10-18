# 入口工具输出压缩：确定性 / 内容结构感知类技术与可复用包

> 研究票：WodenJay/Outrim#3。本文覆盖**确定性、内容结构感知**这一类技术（切分归属见 §0.3）：
> 不覆盖语义/神经压缩论文、Headroom 与 CCR 的完整评估方法（下一批）。
> 前提已定：产品**会做**，Rust、轻量、CPU 友好、只处理**入口**（tool output 首次进入上下文之前）、不改写历史。
> 因此本文只回答"用哪个算法、什么形态、代价多少"，不回答"要不要做"，也不给优先级排序。

**证据约定**：所有数字都用 **cl100k_base 真实 BPE**（`tiktoken 0.14.0`）计数，并同时给出 `chars/4` 估算值做对照；
字符数/字节数会明写。环境：Windows x86_64、rustc/cargo 1.98.1、Python 3.14.3。
每条数字绑定到附录 A 的探针脚本；凡未复现的一律标注"未核"，不保留无法溯源的数值。

---

## 0. 结论先行

### 0.1 最强发现

**1. RTK 的压缩核心是 BUSL-1.1，不是 Apache-2.0；能安全复用的只有 139 行的 `rtk-text`。**

`engine/Cargo.toml:8` 是 `license = "BUSL-1.1"`，`engine/LICENSE` 的 Additional Use Grant 原文：

```
Licensor:             RTK AI Labs
Licensed Work:        rtk-engine
Additional Use Grant: You may use the Licensed Work in production, without
                      charge, including as a component of the `rtk`
                      command-line binary and the `rtk-proxy` daemon
                      distributed by RTK AI Labs. This grant does not permit
                      offering the Licensed Work, or a derivative of it, to
                      third parties as a paid or hosted product or service;
                      for that, contact contact@rtk-ai.app.
Change Date:          2030-09-09
Change License:       Apache License, Version 2.0
```

同文件另有一条生效条件："Effective on the Change Date, or the fourth anniversary of the first
publicly available distribution … whichever comes first"。

按条文本身：**允许**免费用在生产环境（含作为 rtk 二进制 / rtk-proxy 的组成部分）；
**不允许**把该作品或其衍生品作为付费或托管的产品/服务提供给第三方。
根 `Cargo.toml` 头部也写明 "the shipped `rtk` binary contains BUSL-1.1 code"，
并且 `--no-default-features` 可以把 `rtk-engine` 整个从依赖图里移除（`docs/guide/resources/editions.md:66-77`，
该处同时复述了 grant 的边界并指向 `contact@rtk-ai.app`）。
`text/` 是 `rtk-text`，`license = "Apache-2.0"`、`publish = false`、单依赖 `regex`、共 139 行。
**BUSL 不止 engine**：`proxy/Cargo.toml:17` 与 `plugin/Cargo.toml:8` 也都是 `BUSL-1.1`，
只有根 `Cargo.toml:30` 的 `rtk` CLI 与 `text/Cargo.toml:8` 是 Apache-2.0。

→ 结论：**读机制、重写实现**。这不只是许可洁癖：§4 里几乎所有技术点都要改（见 4.1、4.4），
照抄反而会照抄它们的缺陷。

**2. RTK 已经做完了"内容结构分类 + 按形状选策略"这一整套，但内置管线一条都没启用它。**

`engine/src/shape.rs:303` 用纯结构统计（缩进标准差、12 字符前缀重复率、唯一行率）把输入分成
`Nested / Repetitive / FlatUnique / Structured / Monolithic / Unknown`；`engine/src/strategy.rs:415` 再按形状选
`Structural / JsonSkeleton / CharWindow / HeadTail` 之一。

但 `grep -l 'shape\|\brle\b' engine/builtin/*.toml` 在六个 TOML（`default` / `posthook` / `proxy` / `terse` /
`verbatim-diffs` / `delegate-reads`）里**一次都没有命中**。`shape` 只在 proxy 的 CCR 路径上，
`proxy/src/pipeline/ccr/mod.rs:342 strategy_mode()` 默认 `"shape"`（唯一可切换值是 `"headtail"`）。
内置默认入口压缩是 `term` 链：`ansi → progress → dedup → base64-elide → cap`（`engine/src/methods/mod.rs:54`）。
另外 `minify-json` 也不在任何链里（`JSON_CHAIN = [base64-elide, toon]`，toon 失败时回退 minified）。

还有一条同样重要的分档：`floor_for_tool()`（`ccr/mod.rs:130`）给 `ReadFile` 的阈值是
`read_min_tokens() = usize::MAX`——**默认永不压缩 Read**，注释理由是"恢复成本"而非"体积"。
`Bulk`（Bash/WebFetch）默认 300，其他工具 1500。

**3. "头尾窗口"是这批工具里最危险的默认策略，而它在真实 agent 轨迹上确实会掉错误。**

两条独立证据。

(a) 在本地 500 条 SWE-agent 轨迹（10037 条非空 observation，649.6 万 token）上按 `cap(50,200)` 重放：

| 工具 | n | 触发 | tokens | cap 后 | 削减 | error 词元保留 | `path:line` 保留 |
|---|---:|---:|---:|---:|---:|---:|---:|
| edit | 3301 | 0 | 2,350,479 | 2,350,479 | 0.0% | 100% | 100% |
| open | 2125 | 0 | 1,588,656 | 1,588,656 | 0.0% | 100% | – |
| goto | 757 | 0 | 759,296 | 759,296 | 0.0% | 100% | 100% |
| insert | 663 | 0 | 556,315 | 556,315 | 0.0% | 100% | – |
| submit | 458 | 8 | 358,154 | 235,320 | 34.3% | 73% | – |
| search_dir | 384 | 4 | 206,536 | 82,928 | 59.8% | 35% | – |
| scroll_down | 121 | 0 | 118,295 | 118,295 | 0.0% | 100% | 100% |
| find_file | 545 | 5 | 103,920 | 67,599 | 35.0% | 57% | – |
| python | 340 | 1 | 95,765 | 74,458 | 22.2% | 73% | 100% |
| ls | 35 | 5 | 93,074 | 14,353 | 84.6% | 18% | – |
| search_file | 463 | 0 | 53,361 | 53,361 | 0.0% | 100% | – |
| find | 85 | 1 | 37,241 | 14,761 | 60.4% | 40% | – |
| **合计** | | | **6,321,092** | **5,915,821** | **6.4%** | **82.5%** | **100%** |

12 个工具里只有 9 个触发（`open`/`edit`/`insert` 从不触发——这批轨迹的 `open` 观测全部 ≤112 行，
p99 = 105 行（2125 条），远低于 `MAX_LINES = 300`，所以"读文件"这条最贵的路径根本进不了窗口）。
语料级只削 6.4%，因为窗口在 12 个工具的绝大多数调用上不生效。

**只看真正触发的那 29 条（附录 A.16）结论要重得多**：

| 工具 | n | tokens | cap 后 | 削减 | error 词元保留 | `path:line` 保留 |
|---|---:|---:|---:|---:|---:|---:|
| search_dir | 4 | 144,000 | 20,392 | 85.8% | **35%** | – |
| submit | 8 | 143,186 | 20,352 | 85.8% | **7%** | – |
| ls | 5 | 90,913 | 12,192 | 86.6% | **18%** | – |
| find_file | 5 | 49,608 | 13,287 | 73.2% | **43%** | – |
| sphinx-build | 1 | 25,504 | 2,577 | 89.9% | **0%** | 3% |
| python / find / python3 / make / cd / cat / pytest | 7 | 81,703 | 22,392 | 33%–89% | 5%–100% | 24%–100% |
| **合计** | 29 | **545,009** | **90,988** | **83.3%** | **32.5%** | **5.5%** |

→ 一旦触发，`cap(50,200)` 平均削掉 83% 的 token，同时**只剩 32.5% 的错误词元**、
`path:line` 只剩 5.5%。上表"合计 82.5%"是被那两个巨型工具的错误词元数拉高的；
按触发样本算才是真实代价。

(b) 用 `rtk-engine` 在真实语料上跑 `strategy::render`（附录 A.2）：

```
grep.txt     238366 ch 2000 行 → headtail   →   3293 ch (-98.6%)   58155→871 tok
find.txt      76379 ch 1709 行 → headtail   →   1137 ch (-98.5%)   18722→306 tok
ls.txt        41817 ch  800 行 → headtail   →   1516 ch (-96.4%)   19490→725 tok
traceback.txt 10660 ch  200 行 → headtail   →   1464 ch (-86.3%)    3240→453 tok
gitlog.txt   992659 ch 15631 行→ headtail   →   1447 ch (-99.9%)  252909→283 tok
bench.py       8648 ch  211 行 → structural →   2952 ch (-65.9%)    2602→897 tok
json.txt      42630 ch    1 行 → json-skeleton → 124 ch (-99.7%)   18160→50 tok
mcp.json     137697 ch    1 行 → json-skeleton → 112 ch (-99.9%)   30101→39 tok
install.txt    1101 ch   18 行 → render=None（never_worse 拦下）        417→417
```

（`ch` 为 Unicode 字符数，`tok` 为 cl100k 真实计数；`cut%` 用 token 算，`chars/4` 算出的削减率
与它相差 ≤0.35 个百分点（附录 A.2 逐行复算，最大是 `bench.py` 的 0.34pp），因为 `render()` 的 `floor` 与 `count` 内部就用的 `bytes/4`——见 §4.7。

10 份真实语料里 5 份被判 `repetitive`（grep/find/ls/gitlog/install）、3 份 `nested`
（`bench.py`、`failing.py`、`traceback.txt`），只有 `json.txt` 与 `mcp.json` 走到 `json-skeleton`。
`grep.txt` 的 `path:line` 保留率 **1%**、`gitlog.txt` 的 commit 号保留 **0%**、
`bench.py`（唯一走到 `structural` 的代码文件）的 error 词只留 55%、数字只留 42%。

→ 头尾窗口只能作为"已确认无信号"时的最后兜底，且必须由**确定性信号检测**开门（§4.8）。

**4. grep 分组是少见的"忠实性可判定"技术，两条路线要分清：`grep-group` 方法 vs `rtk grep` 打印器。**

方法 `engine/src/methods/grep_group.rs` 的门是硬的：`splitn(3, ':')` 且第 2 段能 `parse::<usize>()`，
**任何一行不成立就整块原样返回**（`:47`；另 `total == 0` 时 `:58` 同样放行）。
复算（cl100k_base，附录 A.1）：

| 输入 | tokens | 分组后 | 变化 | 字符变化 |
|---|---:|---:|---:|---:|
| 2000 条 / 40 文件 / 长路径 | 55010 | 29698 | **-46.0%** | -50.1% |
| 120 条 / 6 文件 | 3600 | 2149 | -40.3% | -45.2% |
| RTK 真实 fixture `grep_content_small.json`（43 条单文件） | 860 | 788 | -8.4% | -10.8% |
| RTK 真实 fixture `grep_content_large_truncated.json`（250 条） | 3410 | 2554 | -25.1% | -44.4% |
| 真实 fixture 正文 + 41 字符路径（只换路径） | 1247 | 797 | -36.1% | -39.5% |
| 43 条 / 单文件 / 41 字符路径 + 25 字符正文（合成） | 1032 | 539 | -47.8% | -54.6% |
| 39 条 / `a.rs:1:x` 极短行 | 233 | **247** | **+6.0%** | +12.1% |
| 1 条命中 | 26 | **37** | +42.3% | +43.1% |
| 5 条 + 1 行 `-C` 上下文 | 62 | 62 | 0.0%（整块放行） | 0.0% |
| 真实 `grep.txt`（2000 行，忠实复刻 gate） | 58155 | 43016 | -26.0% | -31.9% |

收益**几乎完全来自"去掉每行重复的路径"**：把同一份 43 命中正文的路径换掉（附录 A.15），
11 字符/3 tok → **-17.8%**，17 字符/5 tok → -27.1%，41 字符/12 tok → **-47.8%**，60 字符/17 tok → **-56.4%**；
分组输出的 token 几乎不变（同上四档路径下 530 → 533 → 539 → 544），
涨的那点就是表头里多出的那一份路径；所以削减率就是"每行路径预算 ÷ 每行总预算"。
真实 fixture 的 `haystack.rs` 只有 -8.4%，原因是它的命中行本身很短（每条 55–60 字符），
不是路径短——`m1.py` 里 41 字符路径 + 长正文的合成用例是 -47.8%，把**同一份 fixture 正文**的路径换成
41 字符后是 **-36.1%**（附录 A.15 `m35.py`，1247→797 tok）；差的那 11.7pp 全部来自正文长度。
`never_worse` 确实拦住了变大那两种（`chars/4` 也同向：95→107、18→26）。

**另一个硬限制**：绝对路径让整块 gate 失败（附录 A.15）。`C:/…` 形态中第一个 `:` 落在盘符后，
`splitn(3,':')` 的第 2 段是 `/vibe_project/…`，`parse::<usize>()` 失败 → `Outcome::Continue(原文)`。
**Windows 上 `grep -rn`/`rg` 的默认输出就是这种形态**，所以在 Windows 上这个方法几乎不触发。

**但 `grep_group` 不是"只改排版"**：`MethodInfo` 是 `lossy: true, recovery: Recall`，代码注释写明
"Trimming match lines drops their indentation -- which IS the semantics in Python or YAML -- and blank
matches vanish"；`content.trim()`（`:66`）与 `if line.trim().is_empty() { continue; }`（`:40`）是实际行为。
实测（附录 A.1）：YAML 缩进命中 41→44 tokens（缩进被抹掉，且变大），
含空行命中 25→34 tokens（空行消失，且变大）——两种都被 `never_worse` 拒了，但**这是运气，不是设计**。

另有一条独立的有损路径容易被混淆：`rtk grep` 命令自己按文件截断，
`config::limits().grep_max_per_file` 默认 **25**（`src/core/config.rs:238`，`search.rs:845` 处 `.take(per_file)`
再补 `+{remaining} more in {file}`）。方法层无上限，命令层有上限——引用"grep 分组无损"时必须指出是哪一层。

**5. TOON 的 "‑47.9%" 是字符比，不是 token 比；换成 token 是 ‑44.9%，而真实统一数组上 TOON 是**变大**的。**

用 `toon-format 0.7.0` 真实编码（附录 A.3）：

| 形状 | minified 字符 | TOON 字符 | 字符比 | minified token | TOON token | **token 比** |
|---|---:|---:|---:|---:|---:|---:|
| 1 行对象（`[{a,b}]`） | 39 | 33 | 0.846 | 18 | 17 | 0.944 |
| 500 行统一对象（id/name/email/active） | 34421 | 17948 | 0.521 | 10002 | 5509 | **0.551** |
| 10 行 × 20 个长键名 | 5121 | 856 | 0.167 | 1602 | 524 | 0.327 |
| 50 行嵌套 4 层 | 1341 | 257 | 0.192 | 553 | 212 | 0.383 |
| 10 行、字符串带逗号/换行 | 1831 | 1146 | 0.626 | 803 | 708 | 0.882 |
| 混合类型 | 516 | 273 | 0.529 | 228 | 205 | 0.899 |
| 10 行 × 1200 字符正文 | 10981 | 10883 | 0.991 | 2082 | 2066 | 0.992 |
| **GitHub issue 列表（40 条，带 labels/user 嵌套）** | 12697 | 13014 | **1.025** | 2413 | 2934 | **1.216** |

字符比与 token 比差距很大（嵌套 0.192→0.383，宽键名 0.167→0.327），因为 `{`/`,`/`"` 在
cl100k 里常与相邻词符合并，而 TOON 的表头 `[500]{id,name}` 未必更便宜。

真实语料（附录 A.3，`json.txt` = 400 条统一服务数组，`mcp.json` = 真实 MCP 响应）：

```
json.txt  37429 字符 minified → toon 1.075 字符 / 1.273 token   （原始 pretty 18160 tok）
mcp.json 137697 字符 minified → toon 1.139 字符 / 1.070 token   （原始 30101 tok，已是 minified）
```

→ **TOON 只在"键名相对值很长"的表格型数组上赢，且必须与 minified 比 token，不能比字符**；
单行数组上它只值 -5.6% token（上表第一行）——`is_uniform_flat_array` 要求 `len() >= 2` 正好把这个
无效区间挡在外面；真实服务数组上它是 +27% token。规范自己也把这一点列为非目标（`SPEC.md:100-103`：
"not intended to replace: JSON for non-uniform or deeply nested structures where repeated keys are not dominant"）。
RTK 钉的是 `toon-format 0.5.0`（`engine/Cargo.toml:17`）；我用 0.5.0 交叉验过同样两例（uniform 0.521、
github 1.021），结论在两个版本上一致。

**6. 三种实现都把 JSON "lossless" 用错了，而 `arbitrary_precision` 只能保住数值文本，保不住字节。**

`engine/src/methods/minify_json.rs:1` 文件头写 "lossless JSON compaction via serde_json re-serialization"，
但同一文件 `:28` 是 `lossy: true` + `Recovery::Recall`，理由正确：
"Same serde limits as toon: >i64 integers and duplicate keys do not survive"。
`toon.rs:24-27` 同样标 `lossy: true`。lean-ctx `json_crush.rs:1-16` 声称 lossless 模式
"exactly reconstructible via reconstruct"。

实测（附录 A.4）：

```
默认（不 feature）             {"big":123456789012345678901234567890} → {"big":1.2345678901234568e+29}
                              {"f":2.50} → {"f":2.5}      {"e":1e6} → {"e":1000000.0}
                              {"neg":-0} → {"neg":-0.0}   {"f":1.0} → {"f":1.0}（小数点保留）
                              {"u":"\u00e9"} → {"u":"é"}  {"esc":"a\/b"} → {"esc":"a/b"}
                              {"dup":1,"dup":2} → {"dup":2}；空白全删；键序＝出现序
+ arbitrary_precision         大整数逐字保留；1.0 保持 1.0；2.50 保持 2.50；-0 保持 -0
+ preserve_order              但 {"e":1e6} → {"e":1e+6}（指数写法被重写）
                             {"dup":1,"dup":2} 仍然塌成 {"dup":2}
                             {"u":"\u00e9"} → {"u":"é"}、{"esc":"a\/b"} → {"esc":"a/b"} 照旧
```

→ **`arbitrary_precision` 给的是数值文本保真，不是字节保真**：重复键仍会塌、`1e6` 变 `1e+6`、
`\u00e9` 仍变 `é`、`a\/b` 仍变 `a/b`、空白消失。
RTK 也**没开**这个 feature（`engine/Cargo.toml:15` 只开了 `preserve_order`），
所以它标 `lossy: true` 是对的。lean-ctx 的 lossless 模式实测 `reconstruct` 结果与原文
**Value 相等但字节不等**（键被提进 `_defaults` 后顺序变了，附录 A.4 的 `jc` 探针输出 first diff at byte 12）。
"lossless" 必须限定到某一层比较器（§5.1）。

**7. 日志模板化的真实行为与"把变量抹掉"正好相反：squeez 的槽位门是 token 形状，不是基数。**

squeez `src/strategies/log_template.rs:98 looks_like_id()` 要求整个 token 是
纯数字/时间/ip/浮点（`numeric_shaped`）、数字+单位（`1ms`/`3.2KB`/`50%`）、≥8 位且含 ≥2 数字的 hex、
或带 `-`/`_` 的复合 id。**键名与值的写法决定它是否折叠**，于是：

```
F  "ERROR code={n} at handler_{i}: failed validation on field f_{k}" ×60  →  0 折叠 (+0.0%)  ← 基准
J1 去掉 handler_{i} 后的冒号                                             →  1 行 (-96.5%)
J3 冒号前加空格（"handler_{i} :"）                                       →  1 行 (-96.5%)
J4 每行再变一个英文词（validation/parsing/timeout/overflow）              →  4 行 (-85.9%)
J5 "a.rs:{n}:3 failed to parse" ×60                                      →  0 折叠 (+0.0%)
```

原因是 `is_variable()` 剥的是 `,;()[]{}"'<>`，**不含 `:`**，所以 `handler_0:` 的 core 是 `handler_0:`，
`-`/`_` 复合判断要求全部字符是字母数字或 `-_.`，冒号破坏了它；空格反倒把冒号切掉，于是 J3 折叠。
J4 说明变化词的位置也会进模板（每个不同的词都不像 id，于是模板不重复）。
**一行之差决定折叠与否——这不是"基数门"，是形状意外。**

折叠确实发生时非常激进（同上探针，字符口径）：

```
B ISO-空格时间戳  60→1 行 -96.6%    C 裸时钟   -96.1%    D 方括号时间戳 -96.2%
E kv id/status/ms  -95.9%           G 裸 hex   -96.0%    H 裸 epoch     -96.2%
I 小数+单位        -95.2%           A ISO-Z 时间戳（带 T）60→60 行 0.0%
```

A 是反面：`2024-01-01T00:00:12Z` 里同时有 `T` 与 `Z`，超出 `numeric_shaped` 允许的 `.:-/T+` 字符集。

真实语料上（附录 A.5，直接编译 squeez 的 `log_template.rs`）：

```
gitlog.txt      252909 → 247412 tok (-2.2%)   chars -1.6%
install.txt        417 →    304 tok (-27.1%)
ls.txt           19490 →  15460 tok (-20.7%)   ← 但 475 个不同文件名里丢了 21 个（4.4%）
                                              ← 73 个目录列表被折成 "[×73] total {}"
traceback.txt     3240 →   1295 tok (-60.0%)   ← 20 条相同 traceback 被折叠
grep.txt / find.txt / cargo_err_color.txt       0.0%（不折叠，符合形状门）
```

注意 `traceback` 那一行：`ls` 的折叠把 73 个目录的 `total {}` 合并成一条，
**清单因此失去了"哪个目录有什么"**；丢掉的 21 个文件名是各目录首条（被 `[×N]` 吃掉）。
结论：低基数**不是**安全丢弃条件，安全条件是"值的形状是否是可枚举的噪声"+"丢失后能否定位"。

**8. token 估算器只在"相对比较"上够用；一旦当绝对预算就会偏，而 squeez 默认路径的偏差最大的相反方向。**

复算（附录 A.6，真实 + 合成语料，cl100k）：

| 内容 | chars/token | `chars/4` 误差 | squeez `estimate` 误差 | squeez `estimate_classed` 误差 | 分类 |
|---|---:|---:|---:|---:|---|
| grep.txt | 4.10 | +2.5% | +21.4% | **+104.9%** | Dense |
| mcp.json | 4.57 | +14.4% | +23.6% | **+128.7%** | Dense |
| find.txt | 4.08 | +2.0% | +22.4% | +104.0% | Dense |
| ls.txt | 2.15 | -46.4% | -45.8% | +7.3% | Dense |
| json.txt | 2.35 | -41.3% | -23.8% | +17.4% | Dense |
| traceback.txt | 3.23 | -19.3% | -13.5% | +61.4% | Dense |
| gitlog.txt | 3.92 | -1.9% | -13.0% | -13.0% | Mixed |
| install.txt | 2.59 | -35.0% | -22.3% | +29.7% | Dense |
| 英文散文 | 4.49 | +12.2% | -3.0% | +21.4% | Prose |
| cargo 构建日志 | 2.31 | -42.3% | -45.8% | -45.8% | Mixed |
| 源码 | 3.06 | -23.6% | -13.2% | +52.9% | Dense |
| minified JSON | 3.34 | -16.6% | +16.2% | +66.8% | Dense |
| 中文日志 | 0.92 | **-77.0%**（按字符）/ -31.0%（按字节） | -8.0% | -75.1% | Prose |
| base64 | 1.71 | -57.1% | -53.7% | -53.6% | Prose |
| diff | 2.86 | -28.6% | -23.1% | +42.8% | Dense |
| 堆栈 | 2.66 | -33.5% | -25.8% | +32.9% | Dense |

`squeez` 的 `classify()` 把几乎所有内容判成 `Dense`，然后按 `/2.0` 除——在真实语料上比它自己
的 `estimate()` 更差（grep +104.9%、mcp +128.7%），而 `wrap.rs:581-585` 与 `:658-662` 用的正是
`estimate_classed`（`config.class_density` 默认 true）。squeez 的文档只承诺"non-ASCII ~1 token/char"，
没有给中文的百分比；实测 `estimate()` 在中文日志上是 -8.0%（低估），是同表里最好的非 ASCII 结果，
但同一份内容走 `estimate_classed` 变成 -75.1%（因为它把中文判成 `Prose` 再除 3.7）。

顺带修正一个易错的换算：squeez 的 `net_win_min_tokens = 24`（`config.rs:353`）是**估算** token。
推导：估算器 `est = chars / (估算器的 chars/token)`，真实 `真 = chars / (真实 chars/token)`，
两式消掉 `chars` 得 **`真 = est × (估算器的 chars/token) / (真实 chars/token)`**。
对 `chars/4`（估算器的 chars/token = 4）就退化成 `24 × 4 / (chars/token)`：
散文 21.4、cargo 日志 41.6、grep.txt 23.4、`ls.txt` 44.7、中文 104.3、base64 56.0。
而 `wrap.rs:581-585` 与 `:658-662` 实际用的是 `estimate_classed`（Dense 除 2.0、Prose 除 3.7），
所以估算器的分母不是 4，同一阀值对应的真 token 也不同：
按 classed 口径 24 单位只对应 grep.txt **11.7**、中文 **96.4**、`ls.txt` 22.4 个真 token。

好消息：`chars/4` 用来给 **RTK 自己的 head/tail 输出**做闸门时误差极小（附录 A.2，
8 份有输出的语料上 `bytes/4` 与真实 token 的削减率相差 ≤0.35 个百分点，最大是 `bench.py` 的 0.34pp），
因为输出已经是"短行、结构均匀"的形状。
真正需要真实 tokenizer 的是**原始语料**上的绝对预算。

**9. ANSI 剥离：在唯一可得的真实 agent 轨迹上收益为 0，在真实带色输出上是 32.8%–67.9% 的 token。**

500 条轨迹里 10037 条 observation、649.6 万 token：`\x1b` 出现 **0** 次（原始 `.traj` 文件里也 0 次），
所以 CSI/OSC 剥离省 0 token。`\r` 覆盖帧省 17680 token（0.272%），分布在 33 条 observation（0.3%）。

但真实带色输出上（附录 A.7）：

```
cargo build --color always 的 E0308 报错  66392 字符 / 34351 tok → 11033 tok（-67.9%）
git log --color=always                    55785 字符 / 16824 tok → 11310 tok（-32.8%）
两者转义序列占字符数 45.6% / 11.5%，剥离后无残留 ESC
```

→ ANSI 是"几乎零成本、不常触发"的保险，不是头条收益。它的真实价值取决于上游会不会带色
（Claude Code / Pi 这类 host 常常已经剥过），所以**必须做成零成本的快路径**（无 `\x1b` 直接返回）。

**10. 三个 strip_ansi 实现都不完整，包括 RTK 的 `ansi` 方法。**

`engine/src/methods/ansi.rs` 标 `MethodInfo::LOSSLESS_TRANSFORM`，实际调 `rtk_text::strip_ansi`，
正则 `\x1b\[[0-9;]*[a-zA-Z]`（`text/src/lib.rs:18-23`）。四路对照实测（附录 A.8）：

| 输入 | `rtk-text`（方法用） | `shape.rs::strip_ansi` | squeez | lean-ctx |
|---|---|---|---|---|
| `ESC[32mok` / `ESC[1;31m` / `ESC[2K` | `ok done` ✅ | ✅ | ✅ | ✅ |
| `ESC[?25lhidden`（私有模式） | **残留** | ✅ | ✅ | ✅ |
| `ESC]8;;url BEL link` (OSC-8, BEL) | **残留** | ✅ | **残留** | `ttps://x.y␇link`（去掉前缀 `ESC]8;;` 后又多吃一个 `h`；BEL 保留） |
| `ESC]8;;url ST link` (OSC-8, ST) | **残留** | ✅ | **残留** | `ttps://x.yink`（同上；第二个 `ESC\\`+`l` 也被吃） |
| `ESC]0;title BEL` | **残留** | ✅ | **残留** | `itle␇text`（去掉 `ESC]0;` 后多吃一个 `t`） |
| `ESC(Bplain`（字符集指认） | **残留** | 留下多余 `B` → `Bplain` | **残留** | ✅ |
| `ESC[38:5:196m`（冒号参数） | **残留**（`red` 前还带着转义头） | ✅ | ✅ | ✅ |
| `ESC[6n12;40R` | `12;40R` | 同 | 同 | 同 |

（`ESC[6n12;40R` 这一行是"正确行为"的对照：CSI 在 `n` 终结，后面的 `12;40R` 是模型输出，四者都保留。）

→ 只有 `engine/src/shape.rs:49-86` 的状态机同时覆盖 CSI 与 OSC（BEL 与 ST 两种终止符），
但它在 `ESC(B` 上留下一个多余的 `B`（把 `(` 当终结字节）。产品里应该用 `vte 0.15`（状态机）
或 `strip-ansi-escapes 0.2.1`，不要手抄任何一个现有实现。
常见误述是"OSC 这点 RTK 处理了、squeez 没处理"——它**只在 `shape.rs` 层面成立**，
在 `ansi` 方法层面正好相反（方法用的是正则，OSC 全残留）。

### 0.2 候选与适用条件（不做优先级排序）

按"什么条件下值得做"组织，不排序。每条给出已验证的证据位置与尚未验证的前置条件。

| 技术 | 适用条件（已验证） | 收益量级（cl100k） | 已验证的失效 | 前置实验 |
|---|---|---|---|---|
| ANSI/OSC 剥离 | 内容含 `ESC`；无 `ESC` 时零成本短路 | 真实带色输出 -32.8% / -67.9% token；轨迹语料 0% | 三个现有实现都不完整；OSC/私有模式会残留 | 待补：上游 host 是否带色 |
| CR 进度帧取末帧 | 行内 `\r` 覆盖（进度条） | 轨迹语料 0.27% token / 0.3% obs | 用 `\r` 多段覆盖时丢中间段；CRLF 会被规范化成 LF | 无（量级已定，做零成本短路即可） |
| 相邻重复折叠 | 日志/构建输出有连续相同行 | 40 条相同 warning：319→11 tok（-96.6%） | 8 份真实语料上 0 次触发；标记行会双向碰撞（不幂等）；跨距离折叠会改因果 | 待补：真实轨迹上的触发率 |
| 模板折叠 + 槽位门 | 变量形状是"可枚举噪声"，且折叠后仍能定位 | 形状命中时 -96% 量级 | squeez 的形状门是意外（§0.1-7）；`ls` 丢 4.4% 文件名 | 待补：高基数槽位是否改变任务答案 |
| Drain 模板树 | 需要"位置化参数 + 可检索参数值" | 0.9–1.8M 行/秒匹配；1600 行→4 模板 | 空行/超长行 `LineTooLong`；不保留原文；堆栈帧会被固化成模板 | 已做合成；待补真实 cargo/pytest 模板质量 |
| 失败行追回门 | 任何 L2/L3 之后 | 追回被删的失败行与解释行 | 词表覆盖不到的失败形式 | 无（源码 + 正则即可移植） |
| grep 按文件分组 | 命中行是 `path:line:content`，路径占每行 token 的比例高 | 路径 41 字符 -47.8%（同正文）；11 字符 -17.8%；真实 fixture -8.4%（正文短）；真实 `grep.txt` -26% | 短行/单条会变大；`trim()` 与空行跳过是有损；绝对路径整块放行 | 无（已足量） |
| 路径前缀折叠 | 多文件结果且**仅有** `-l`/`--files-with-matches`/grep `-L`/rg `--files` 这类形状旗标；有公共目录；无 `\0\r\x1b` | `find.txt`（1709 行绝对路径，前缀 `/tmp/hr/`）-17.9% ch / **-18.4% tok** | 公共前缀只到 `/`（如相对名列表）就不折；Windows `\` 路径不折；`compact_path` 会破坏可点击性 | 无（附录 A.18） |
| 剥行号前缀 | 见 §4.1：仅当行号可识别**且**下游不靠行号定位 | 未单独测量 | 按行号 Edit 时有害；默认应关闭 | 未核：需真实 `cat -n` 语料 |
| JSON minify | 输入是合法 JSON | pretty→minified 体量视缩进而定 | 大整数/重复键/`1e6`/转义会变；非字节可逆 | 无（已足量） |
| JSON 骨架 | 目标是"看形状"而非"读内容" | 40 项响应 3824→2253→**28** tok | 所有值都没了，回答不了"某条记录是什么" | 无 |
| JSON 投影 | 已知相关字段（调用参数/Schema/键名匹配） | 40 项取 3/5 字段 2253→1523 tok（-32%） | 选错字段的值全部丢失 | 待补：字段信号从哪里来 |
| 列式铲平（`_defaults`） | 数组内对象键/值高度重复 | 40 项 8 字段：6755→4681 字符（-30.7%），**不 bytes 可逆** | 键序改变；高熵列会被丢（`_dropped`） | 待补：Value 等价是否足够 |
| TOON | 统一标量数组 **且** 键名相对值很长 | uniform_500 -44.9% token | 真实服务数组 **+27%**；嵌套/长值无用 | 待补：其他 tokenizer 上是否仍成立 |
| 源码签名骨架 | "读文件做定位"，不随后 Edit | 600 函数 -49.3% 字符，签名 600/600 | 轨迹里 25.1% 的 open（69.9% 的 open token）随后被 Edit | E6（部分）：是否引发额外重读待补 |
| Kneedle 预算选择（`compute_optimal_k`） | **候选集已按重要性排序** | 重排后信号词元保留 100%；未重排时真实 cargo 错误只留 3% | 假设顺序即重要性；对"前缀重复"型内容给 99%（几乎不压） | E10（附录 A.14） |
| 头尾窗口 | 仅"已确认无信号"时的兜底 | 见 §0.1-3 | 见 §0.1-3 | 不做默认 |
| token 真实计数 | 只要绝对预算就必须 | 见 §0.1-8 | 词表 2–4 MB；Anthropic 无公开 tokenizer | 待补：估算器决策翻转率 |

### 0.3 分片边界

本文覆盖：内容形状分类、L1 规范化（ANSI/CR/行号——注意是条件无损，见 §1、§4.1）、相邻去重与 RLE、日志模板化（含 Drain）、
grep/search 分组与前缀折叠、JSON minify/骨架/投影/列式铲平/TOON、源码语法裁剪、
token 估算与阈值、Rust 复用面与许可。
**不覆盖**：语义摘要与神经压缩论文、Headroom 与 CCR 的评估方法、长上下文重写、跨会话记忆。

---

## 1. 术语：四层"压缩"与三种"可逆"

不先拆开，后面所有百分比都不可比。

| 层 | 含义 | 可逆性 | 典型手段 | 模型读到的信息 |
|---|---|---|---|---|
| L0 传输/存储压缩 | gzip/zstd/CLP 等字节编码 | 字节级可逆 | DEFLATE、列式编码 | **不变**——模型看不到 raw bytes；字节数下降 ≠ token 数下降 |
| L1 规范化 | 去掉“渲染层”的表示（颜色、进度帧、行号前缀） | **条件无损**：只在下列条件成立时语义不变（ANSI 需完整终端状态机；CR 需“后帧覆盖前帧”语义；行号需可识别且下游不靠它定位）；否则归 L2 | strip ANSI、CR 取末帧、行号剥离 | 不变（条件成立时）；否则不变的是“文本”，定位/覆盖关系变了 |
| L2 结构保留裁剪 | 删内容但保留可定位结构与计数 | 需原文取回 | 模板折叠、RLE、按文件分组、签名骨架 | 减少，但保留"哪里/多少/哪类" |
| L3 相关性筛选 | 按任务相关性选择 | 需原文取回 | 错误行优先、首末保留、自适应 K | 明显减少，可能丢关键行 |

**明确不等价**：L0 的压缩率不能当模型侧 token 减少。CLP 能把日志压到 1–2% 且支持压缩态搜索
（Rodrigues et al., OSDI'21），但那 98% 是磁盘与 IO。

三种"可逆"见 §5.1，本文所有"无损"都标注到具体比较器。

---

## 2. 权威仓库认定

以下为核实过的一手仓库，提交为本次实际检出（`git rev-parse HEAD`）。

| 项目 | 仓库 | 提交 | 版本 | 许可 |
|---|---|---|---|---|
| RTK | `github.com/rtk-ai/rtk` | [`e542dca0`](https://github.com/rtk-ai/rtk/blob/e542dca0bdcc262fa035dcf7fc33910cdcc8cdaf/Cargo.toml) | 0.49.0 | **Apache-2.0（`rtk` CLI + `rtk-text`）+ BUSL-1.1（`rtk-engine` + `rtk-proxy` + `plugin`）** |
| LM Resizer | `github.com/phuetz/lm-resizer` | [`c3007c25`](https://github.com/phuetz/lm-resizer/blob/c3007c25197a98a359e5304c2d28949968d39247/Cargo.toml) | 0.2.6 | Apache-2.0 |
| squeez | `github.com/claudioemmanuel/squeez` | [`b8d46531`](https://github.com/claudioemmanuel/squeez/blob/b8d46531efa550f41d6f4057af125fcf4c812bbc/Cargo.toml) | 1.48.18 | Apache-2.0 |
| lean-ctx | `github.com/yvgude/lean-ctx` | [`dc41bb19`](https://github.com/yvgude/lean-ctx/blob/dc41bb19982e5bd93a2c0862702bfd3a0bcea169/LICENSE_MATRIX.toml) | 3.11.4 | Apache-2.0（分域，见 §5.4） |
| TOON 规范 | `github.com/toon-format/spec` | [`d09c3d33`](https://github.com/toon-format/spec/blob/d09c3d33e37389345a74bd96981eb96dd9be6c5e/SPEC.md) | v4.4 WD | MIT |
| TOON Rust | `github.com/toon-format/toon-rust` | [`a1172f14`](https://github.com/toon-format/toon-rust/tree/a1172f1489efc3b5571f475c06847874fb1a37a9) | 0.7.0（RTK 钉 0.5.0） | MIT |
| Drain3 | `github.com/akshatagarwl/drain3` | [`b04f7e53`](https://github.com/akshatagarwl/drain3/tree/b04f7e53024f84f243d74ac4765c9396b40f1240) | 0.3.0 | Apache-2.0 |

**已排除的猜测**（crates.io API，2026-10-19 核）：
- crates.io `rtk` 0.1.0 是 "Rust Type Kit"（`reachingforthejack/rtk`），**无关**。
- crates.io 无 `lm-resizer`（该仓库仅在 GitHub，star 4）。
- `squeez` 1.48.18、`lean-ctx` 3.11.4、`toon-format` 0.7.0、`drain3` 0.3.0 的仓库字段均指向上表。

---

## 3. RTK：分层确定性管线的设计

### 3.1 架构

```
Layer 1+2  shape.rs     normalize(ANSI/CR/行号) → Features(定点千分比) → classify() → Shape
Layer 3    strategy.rs  lossless RLE 预处理 → 按 Shape 选一个 lossy 策略 → 渲染
Layer 4    methods/     可组合 Method，chain 串联 + never_worse 守卫
Layer 5    pipeline     TOML 规则：hook.before_tool / hook.after_tool / proxy.request
```

### 3.2 关键源码事实（均带行号，见 §9 的永久链接）

- **`shape.rs:113 normalize()`**：`strip_suffix('\r')`（所以 CRLF 被记进 `had_crlf` 标签）、本地 ANSI 状态机、
  然后识别行号前缀。行号识别要求：非空行 ≥5、带号行 ≥80%、分隔符一致、编号严格递增；
  `ALL_SEPS = ['\t','→','|',':']`，但只有 `TRUSTED_SEPS = ['\t','→','|']` 才算 `file_coords`——
  只有 `file_coords` 为真，省略标记才敢写真实行号。
- **`shape.rs:212 extract()`**：全部特征用定点整数（千分比），`indent_sd` 用整数牛顿法 `isqrt`。
- **`shape.rs:303 classify()`** 阈值：`NESTED_INDENT_SD_MILLI = 1500`、`REPETITIVE_PREFIX_MILLI = 500`、
  `FLAT_UNIQUE_MILLI = 950`、`MONOLITHIC_MAX_LINES = 3`、`MONOLITHIC_MIN_CHARS = 600`、
  JSON 解析上限 5 MiB。代码注释解释了顺序偏好："misrouting a log to `Nested` costs ratio,
  misrouting code away from it costs the paging loop, so `Nested` is checked first"。
- **`strategy.rs:415 render()`**：先 RLE；若 `collapsed > 0 && count(&deduped_body) < floor` 就**直接返回**，
  不再走 lossy 步骤——这是"损失最小化"的显式表达。
- **`strategy.rs:357 rle_collapse()`**：比较键是 `n.content.trim()`（`to_slines()` 的 `key`），
  所以 `"  x"` 与 `"    x"` 会被当成同一行；`rle.rs` 因此标 `lossy: true`。
- 四个策略（`strategy.rs:8-28` 常量）：`HeadTail`（20 头 + 8 尾）、
  `Structural`（保留最小缩进行 + "下一行更深"的行 + 注释行，上限 `struct_max_lines()` 默认 48）、
  `JsonSkeleton`（字符串 >24 字符变 `"short…" (N chars)`，`depth==0` 数组变 `[N× first]`，
  `depth==1` 数组变 `[N items]`，`depth>=2` 对象变 `{N keys}`，键循环到 700 字符、整体硬上限 900 字符——见 §4.5.2）、
  `CharWindow`（400 头 + 200 尾字符）。
- **`chain.rs:21 never_worse()`**：`estimate_tokens(output) > estimate_tokens(input)` 就返回 `None`；
  注释诚实说明它"只能拒绝变大和空操作，无法判断方法是否在缩小的同时破坏了内容"。
  `run_chain_caught` 用 `catch_unwind` 保证 fail-open。
- **`text/src/lib.rs:83 estimate_tokens_from_len()`** = `(len / 4.0).ceil()`，`text/` 全 139 行。
- **`proxy/src/tokenizer.rs`** 是另一套：`tiktoken_rs::o200k_base()` 真实 BPE，加载失败回退 `len/4`。
  注释明写 "Anthropic's tokenizer is not public"。

### 3.3 内置管线实况

六套 TOML 里 `shape`/`rle` 零引用（§0.1-2）。实际生效的规则：

`builtin/default.toml`
```
[[proxy.request.tool_result]] name = "normalize"  use = ["ansi","progress","dedup"]
[[hook.before_tool.shell]]    action = "rewrite"  name = "rtk-rewrite"
[guards] never_worse = true   min_size = 512
```
头部自述（原文）："Nothing here is lossy and nothing rewrites conversation history." 与
"Benchmark: not published."；同一段注释还说明了 `normalize` 的 cache 论证：
"Each method is a pure function of the block's own content, so the request prefix stays byte-identical
across turns and the upstream prompt cache keeps hitting -- `validate` enforces that property
(\"not cache-stable\" is an error under `proxy.request`). `rtk-proxy --normalize false` turns it off."

`builtin/posthook.toml`（extends default）新增：`after_tool.shell`（`when={is_rtk_command=false}`）用 `term`；
`lockfiles`（`basename_in` 四个 lockfile）；`after_tool.search` 用 `matches`（即 `grep-group`）；
`after_tool.web` 用 `web`。`json-reads` 与 `strip-line-numbers` 都是 `enabled = false`。

`builtin/proxy.toml`（extends default）只加 `cache-align`，并注明 "CCR … not enabled here"。
注意三者的作用点不同：`default` 的 `normalize` 作用在 **proxy.request**（发送前，不落盘）；
`posthook` 的 `after_tool.*` 作用在 **hook**（工具返回后、写入 conversation 前），
这才是“入口”语义；`proxy.toml` 只做 cache 对齐。

→ **这里有一处直接的自相矛盾**：`default.toml` 头部断言 "Nothing here is lossy"，
而它启用的 `normalize` 链里 `progress` 的 `MethodInfo` 是 `lossy: true, recovery: Recall`
（`progress.rs:18-27`，注释自陈 "overwritten segments are gone … Lossy-by-design, recoverable"）；
`dedup` 也是 `LOSSLESS_TRANSFORM` 但输出会插入 `[xN]` 行。所以那句话只能理解为
"默认链里没有 L2/L3 选择性删除"，不能当作"字节无损"的宣传语引用。

→ **设计可借，行为不可照抄**：它的默认入口压缩是 `term` 链；最有意思的 `shape` 只在可选 proxy 路径上，
且默认对 `Read` 关闭（`read_min_tokens = usize::MAX`）。

### 3.4 可借鉴 / 不可照抄

可借鉴：定点特征（分类不漂移）、行号可信度分级（`TRUSTED_SEPS`）、
"RLE 够了就不做 lossy"的降级顺序、每个 Method 显式声明
`lossy / recovery / idempotent / cache_stable / deterministic / cost` 六维、`grep_group` 的整块放行规则、
`floor_for_tool` 按"恢复成本"而非"体积"分档。

不可照抄：`headtail` 当默认兜底；`rle` 用 trim 后内容做键却只在小注释里提"缩进丢失"；
`dedup` 标 `LOSSLESS_TRANSFORM` 但输出会插入 `[xN]` 行（字节变了、且标记占 token）；
`ansi` 方法用正则且漏 OSC；`grep_group` 标 `lossy` 而文档表格写 "every match kept"（两处口径不一致）；
`shape`/`rle` 在 TOML 里不存在。

---

## 4. 逐技术剖析

每条：机制 → 改变什么信息 → 保留规则 → 失效例子（实测）→ 成本 → 复用与复杂度。

### 4.1 ANSI / CR 进度帧 / 行号前缀（L1）

**机制与实现差异**（详见 §0.1-10 的四路对照）
- RTK `methods/ansi.rs`：`if !input.contains('\x1b')` 快路径，否则调 `rtk_text::strip_ansi`
  ——**正则**，只覆盖 CSI 数字/分号参数，漏 OSC、私有模式 `?`、`ESC(B`。
- RTK `shape.rs:49 strip_ansi`：手写状态机，覆盖 CSI（`0x40..=0x7e` 终结）与 OSC（BEL / ESC `\`），
  但 `_ => { it.next(); }` 会把 `ESC(B` 的 `B` 吞掉。
- squeez `smart_filter::strip_ansi`：`ESC` + `[` 后丢到字母为止；非 `[` 的转义原样留下（OSC 全残留）。
- lean-ctx `compressor.rs:13`：`ESC` 后一直丢到"字母"为止——OSC 的情况下第一个字母就是 URL 的首字母，
  所以会吃掉正文（实测 `link` → `ttps://x.y` 之类）。

**CR 进度帧**：`methods/progress.rs` 对每行取**最后一个非空 `\r` 段**
（`line.rsplit('\r').find(|seg| !seg.is_empty())`），显式 `lossy: true, recovery: Recall`。
它同时把 CRLF 规范化成 LF（测试 `test_crlf_normalized_to_lf`），这条容易被忽略。

**行号**：`methods/strip_line_numbers.rs:12-20` 是 `lossy: true, recovery: Recovery::None, idempotent: false`，
门是 `if !normalized.had_numbering || !normalized.file_coords { return Outcome::Skip }`（`:25`）。
`had_numbering` / `file_coords` 由 `shape::normalize`（`shape.rs:113-167`）算：
非空行 ≥5、能解析成 `num<sep>…` 的行占 ≥80%、所有编号行同一分隔符、编号严格递增，四条全成立才算
`had_numbering = true`；再看分隔符是否属于 `TRUSTED_SEPS = ['\t','→','|']`（`shape.rs:8`）。
`ALL_SEPS` 包含 `':'`，但 `':'` 不在 `TRUSTED_SEPS` 里，所以 `grep -n` 的 `12:` 形态**必然**被拒。
默认 `enabled = false`（posthook.toml）。

**失效例子**
- OSC/私有模式残留（§0.1-10 表）。
- `\r` 多段覆盖的程序会丢中间段。
- 行号：`grep -n` 的 `12:` 若被当行号剥掉会改语义；RTK 用 `TRUSTED_SEPS` 排除了 `:`，
  且要求编号严格递增（`grep -C` 的重叠上下文不递增 → 整块不剥）。
  反过来，**按行号做 Edit 的 agent 需要行号**，所以这也解释了 §0.1-7 的 `ls` 现象不是行号问题。

**这三个子项不是普遍语义无损，各自有前提**（§1 把 L1 记为“条件无损”就是这个意思）：

| 手段 | 安全当且仅当 | 不成立时的实际后果 |
|---|---|---|
| strip ANSI | 用的是完整终端状态机（CSI 全参数类 + OSC 的 BEL/ST + 私有模式 `?` + `ESC(` 类字符集切换）；OSC 8 超链的可见文本与 URL 分离处理 | 漏 OSC 就残留字节；吞错字符就吃掉正文（lean-ctx）；`ESC(B` 处理错会多出一个 `B` |
| CR 取末帧 | 输出方确实用 `\r` 做“重绘同一行”（进度条），且末帧就是最终状态 | 多段覆盖（`\r` 分段打印不同内容）会丢中间段；`\r` 后无新帧的行会变成空行；CRLF 会被改成 LF |
| 剥行号 | 行号前缀可识别（`TRUSTED_SEPS` = `\t`/`→`/`\|`，**不含 `:`**）**且** `shape::normalize` 的四条判据全成立（非空行 ≥5、编号行占 ≥80%、所有行同一分隔符、编号严格递增）**且** 下游不靠行号定位 | 剥掉 `grep -n` 的 `12:` 会改语义；要按行号编辑时 agent 得重读原文（多一轮上下文）；编号不严格递增（如 `grep -C` 重叠上下文）时整块弃用，不会错剥 |

三条都是“**输出侧的渲染层**去掉后文本仍可读”，不保证“**信息**不变”：颜色可能编码了级别
（红=error），行号可能编码了顺序，CR 覆盖可能编码了“这条被后一条取代”的因果。
所以保守做法是：ANSI 走完整状态机；CR 只在行内有 `\r` 时取末帧且保留“发生过覆盖”的计数；
行号默认不剥（RTK 自己也默认 `enabled = false`）。

**成本**：O(n) 单遍，无额外分配；`contains('\x1b')` 快路径让无转义输入零开销。**复杂度：低。**

**复用**：`vte 0.15`（`Apache-2.0 OR MIT`，终端状态机）或 `strip-ansi-escapes 0.2.1`
（`Apache-2.0/MIT`）。别手抄现有四个实现中的任何一个。

### 4.2 相邻重复折叠 / RLE

**机制**
- RTK `methods/dedup.rs`：仅**相邻**、仅非空、`MIN_RUN = 3`，输出 `原始行` + 独立一行 `[xN]`；
  `MethodInfo::LOSSLESS_TRANSFORM`。文档理由："Cross-distance dedup would destroy structure and is
  deliberately not done. Blank-line runs are exempt (marker noise would cost more than it saves)."
- RTK `methods/rle.rs` + `strategy::rle_collapse`：键是 trim 后内容，输出 `[… ×N identical lines …]`
  （有 `coord` 时带 `(lines a–b)`），标 `lossy: true`。
- squeez `strategies/dedup.rs`：**跨距离**（全文 `HashMap` 计数），第一处输出 `{line}  [×N]`。
  这会把 `A B A` 变成 `A[×2] B`，改变因果顺序。

**改变的信息**：相邻折叠保留"重复了几次"，不保留"第几处"（除 RLE 的坐标版）；
跨距离折叠会丢位置信息。

**实测**
- 40 条相同 `warning: unused import in module_x`：319 → 11 tok（`[x40]`，-96.6%）；
  同句加长版（带 `std::collections::HashMap`）：599 → 18 tok。标记自身 3 tok（缩进版 5 tok）。
- `A B A B …` ×10：相邻折叠**不触发**（run 恒为 1），跨距离版 159 → 24 tok（-84.9%）。

**成本**：O(n) 时间 O(1) 空间；跨距离版 O(unique) 内存。**复杂度：极低。**

**真实语料上它不触发——这是本节最实际的结论**。把 `Dedup::apply` 直接跑在 8 份真实语料上
（附录 A.17）：`grep.txt` / `find.txt` / `ls.txt` / `gitlog.txt` / `traceback.txt` / `install.txt` /
`bench.py` / `failing.py` 全部 `p1 == input`（字节相同），**0 次触发**。
所以上面的 319→11 tok 只能说明"相邻字面重复确实能压 96%"，
不能说明"真实工具输出里这种重复常见"。要把它当主力，得先在同一批真实轨迹上测触发率。

**幂等与标记碰撞**：因为 `dedup` 在中间插入 `[xN]` 行，它**不是**字节幂等的，而且碰撞方向是双向的
（直接调 `Dedup::apply` 连跑两遍，附录 A.17）：
- 输入含 `[x3]` 形态的行时，第一遍把 `A×3` 折成 `A` + `[x3]`，新标记行又与原有 `[x3]` 行凑成新 run，
  第二遍再折一次：`A A A [x3] [x3] [x3]` → **15→11 tok** → 第二遍变 3 行（`p1 != p2`）。
- 反向：4 行相同的 `[x9]`（合法内容，日志里会带十六进制行号）被折成 `[x9]` + `[x4]`（**12→6 tok**）。
- 标记形态只出现一次（`ok` / `[x40]` / `done`）不触发（7→7 tok）。

所以"lossless transform"只对**不含标记形态的输入**成立。hook + proxy 双层调用必须靠外部标记，
不能指望它幂等。

### 4.3 日志模板化（log template）与 run-length 摘要

这是收益最大、也最容易出错的一项。

#### 4.3.1 三种不同做法

| 做法 | 代表 | 机制 | 变量值处理 |
|---|---|---|---|
| 白空格分词 + token 形状门 | squeez `log_template.rs:25/56/71/98` | 逐 token：`key=value` 保留 key（当 value 像 id）；否则 token 像 id 就换 `{}`；模板重复 ≥3 折成 `[×N] <template>  (e.g. <sample≤100 字符>)` | **丢弃**，只留一个样本 |
| 固定深度前缀树 | Drain（ICWS 2017）/ `drain3` | 第 1 层按 token 数分桶，第 2..(d−1) 层按前 d−2 个不含数字的 token 走，叶子里放日志组；与组模板逐位比 `simSeq`，≥`st` 并入，不同位置改通配 `*` | 位置化通配 `*`，参数可按位置取回 |
| 分级选择 + 全局预算 | lm-resizer `log_compressor.rs` | 识别格式（pytest/npm/cargo/jest/make/generic）→ 行级 `LogLevel` → 栈帧状态机 → 打分（error 1.0 / warn 0.5 / info 0.1 / debug 0.05 / trace 0.02，栈 +0.3，汇总行 +0.4，封顶 1.0）→ 按类别选首末 + 高分 → 补上下文 → 全局上限（默认 100 行） | 不折叠，而是**选择保留哪些行** |

Drain 论文的关键量化事实：复杂度 `O((d+cm)n)`（`c` 为叶子候选组数，视作常数 → 线性）；
`simSeq = Σ equ(seq1(i), seq2(i)) / n`，代价 `O(m1+m2)`（对比 SHISO/Spell 的 LCS 是 `O(m1·m2)`）；
流式处理，内存不随日志总量增长。论文用 `depth` 3（BGL/HDFS/Zookeeper）或 4（HPC/Proxifier），
`st` 0.3/0.4/0.5。论文还有一步**预处理删除**（如 `blk [0-9]+` 直接删而非替换），与"位置化通配"是两种语义。

#### 4.3.2 squeez 的真实行为：token 形状门，不是基数门

（详见 §0.1-7；探针 `sq2`，直接编译 `log_template.rs`）

```
A ISO-Z 时间戳带 T    60→60 行  0.0%      ← numeric_shaped 不含 T/Z
B ISO-空格时间戳      60→ 1 行 -96.6%
C 裸时钟              60→ 1 行 -96.1%
D 方括号时间戳        60→ 1 行 -96.2%
E kv id/status/ms     60→ 1 行 -95.9%
F ERROR code= handler_: 60→60 行  0.0%     ← handler_0: 的冒号破坏复合 id 判断
G 裸 hex              60→ 1 行 -96.0%
H 裸 epoch            60→ 1 行 -96.2%
I 小数 + 单位         60→ 1 行 -95.2%
F 的变体（同一 60 行，只改一处写法）：
J1 去掉那个冒号        60→ 1 行 -96.5%    ← 一行之差
J3 冒号前加空格        60→ 1 行 -96.5%    ← 空格把冒号切出 token
J4 每行再变一个英文词  60→ 4 行 -85.9%    ← 变化词也进模板
J5 "a.rs:{n}:3" ×60    60→60 行  0.0%     ← path:line:col 不被折叠
```

`looks_like_id()`（`:98`）的四条判定：`numeric_shaped`（只含数字与 `.:-/T+`）、`is_num_unit`
（数字 + ≤4 字母/`%` 后缀）、hexish（≥8 位、≥2 数字、全 hex-or-dash）、
compound（含数字 + 含 `-`/`_` + 全字母数字或 `-_.`）。`is_variable()` 先 trim `,;()[]{}"'<>`——
**不含 `:`**，这就是 J1/J3 与 F 的差异来源。同理 `a.rs:12:3` 里的 `:` 让 `looks_like_id` 判否，
所以源码 `path:line:col` 引用不会被折叠（J5）——这对"错误定位必须保留"是好消息，
但也意味着**这类内容在这套算法下完全没有收益**。

**因此"把错误码全部抹掉"不是它的实际行为**：在 F 形状上它什么都不做。
它是"只对形状像 id 的 token 动手"。这一点必须写清，否则会把一个不触发的技术误当成高危技术。

折叠确实发生时的信息损失，用真实语料看（探针 `sqtest`，附录 A.5）：

```
ls.txt        19490 → 15460 tok (-20.7%)
  475 个不同文件名 → 454（丢 21 个 = 4.4%，都是各目录首条，被 [×N] 吃掉）
  73 个目录列表 → "[×73] total {}" 折成 1 条 → 清单失去"哪个目录有什么"
traceback.txt  3240 → 1295 tok (-60.0%)
  20 条相同 traceback → 1 条 + [×20]，行号变 {}，样本行保留原值
gitlog.txt    252909 → 247412 tok (-2.2%)   install.txt 417 → 304 (-27.1%)
grep/find/cargo_err   0.0%（形状门不命中）
```

→ **`ls` 类清单一律不要走模板折叠**：它的"重复"是权限/属主/日期字段相同，
而文件名正是被折叠掉的部分。这与"变量是噪声 → 可折叠"的二分法不同：判据不是"变量是不是噪声"，
而是**"折叠后还能不能定位"**。

#### 4.3.3 若要做模板折叠，四条保留规则

1. **先判断这个内容类型该不该折叠**：`ls`/`git status`/`find` 这类"一条记录一个名字"的清单，
   折叠率再高也不要折。日志（时间戳+级别+消息）才适合。
2. **槽位记录 distinct 计数**：每槽输出 `k` 个不同值、首次出现位置；
   `k` 接近行数时不折叠。注意区分两种"低基数"：
   "低基数是枚举（status=ok/fail, 2 个值）"和"低基数是巧合（73 个目录的 total 恰好都写成 `total 776`）"——
   前者可折，后者不可折，**判据是字段的语义角色，不是基数数字**。
3. **失败行永不折叠，并做后置追回**：lm-resizer `diagnostic_gate.rs` 可直接借用，
   见 §4.3.5。
4. **折叠标记放首次出现处**（squeez/RTK 一致），保持时间线可读；标记写清 `[×N]`，不静默删除。

#### 4.3.4 Drain 作为算法基座（实测）

`drain3 0.3.0`（Apache-2.0，依赖 `bon/fastrand/regex/snafu/string-interner/smallvec`）：

```
train with blanks FAILED: line too long: 0 bytes (max: 1024)
train 1600 lines: 2.6 ms, templates=4
match_line(blank) -> (0, [], false)
matched 40k lines in 43.6 ms (916735 lines/s), ok=40000
templates (count): 400 INFO <*> heartbeat ok / 400 WARN <*> queue depth <*> /
                   400 ERROR <*> failed job <*> code <*> / 400 at handler (src/worker.rs:42:5)
```

**坑 1**：空行或超长行返回 `Error::LineTooLong`，`train()` 直接失败——必须预先过滤或包一层。
**坑 2**：`Template` 只暴露 `tokens()` / `is_param(i)` / `count()`，**不保留每行原文**；
要"取回具体值"得自己旁路收集或读 `match_line()` 的 `args`。
**坑 3**：上面第 4 个模板把堆栈帧 `at handler (src/worker.rs:42:5)` 当成了**模板字面量**而不是参数——
`parametrize_numeric_tokens` 默认 true 也没能把它参数化，因为它在括号里且带冒号。
所以"每条错误都不同"的日志里，Drain 可能把关键定位信息固化进模板。
**坑 4**：`param_string` 默认 `<*>`，`max_clusters` 可封顶内存。

吞吐在不同运行里 0.9–1.8M 行/秒（两次测量 916,735 与 1,766,745 lines/s，见附录 A.9），
数量级足够。**复杂度：中**（不要 `bon/snafu/string-interner` 的话，树 + simSeq 约 200 行可重写）。

#### 4.3.5 失败行追回门（可直接借用的保护层）

lm-resizer `crates/lm-resizer-core/src/transforms/diagnostic_gate.rs`：

- 模块文档自述实测问题："measured on two real GitHub Actions logs, the log compressor kept
  0/2 and 2/10 `##[error]` lines — through `compress`, the MCP tool, and the HTTP proxy's live zone alike"。
- `FAILURE_SIGNAL` 是一条很长的 `(?i)` 正则（不是几个小模式拼起来的），覆盖 `##[error]`、`\berror\b`、
  `\b\w*error\b`、`\bfail(ed|ures?|s)?\b`、`\bFAIL\b`、`✘|✗|×`、`exception|panic|traceback`、
  `\bassertion\b`、`\b(expected|actual|received)\b`、`:line\s+[0-9]+`、`\bvalues differ\b`、
  `\bdiff(erence)?\b`、`exit code [1-9]`、`timed? ?out`、`called .*unwrap\(\)`、
  `(?m)^\s*(E\s+|assert\b)`（pytest 的 E 行）、`(?m)^\s*[\w./\\-]+\.[[:alpha:]]\w*:\d+(:\d+)?`（`file.ext:line`）、
  `(?:\bat\s+|❯\s+).+:\d+(:\d+)?`（堆栈帧）、`\breturned a non-zero code\b`、`\bcancell?ed\b`、`npm ERR!`。
- `LABELLED = ^(\[[^\]]*\]\s*)?[A-Za-z][A-Za-z ]{0,28}:(\s|$)`——用来把失败的"解释行"也追上。
- `SGR` 先剥 `\x1b\[[0-9;]*m` 再匹配。
- `lost_failure_lines(before, after)` 返回 `after` 里缺失的失败行（含其解释行：labelled 或缩进更深、
  且在第一个空行前），保持原序。
- 显式排除的非失败行：`% Total    % Received`、`diff --git `、`test result: ok.`。

这个门是"任何 L2/L3 压缩之后的强制后置"，成本 O(n)，与具体压缩算法解耦。**复杂度：低。**

### 4.4 grep / search 分组与前缀折叠

**机制**

RTK `methods/grep_group.rs`：
```
{total} matches in {F}F:

[file] {path} ({n}):
  {line:>4}: {content.trim()}
```
- 解析：`splitn(3, ':')` 且 `parts[1].parse::<usize>()` 成功。**任一行不成立 → 整块 `Continue(input)`**（`:47`）。
- `total == 0` 同样整块放行（`:58`）。
- 跳过 `line.trim().is_empty()` 的行（`:40`）——空行命中会消失。
- `MethodInfo { consumes: [Matches], lossy: true, recovery: Recall }`；
  注释："Trimming match lines drops their indentation -- which IS the semantics in Python or YAML --
  and blank matches vanish."
- 不设每文件上限（测试 `test_all_matches_emitted_no_cap`）。

**区分另一条路径**：`rtk grep` 命令的打印器**是有损的**：
`src/core/config.rs:238 grep_max_per_file: 25`，`src/cmds/system/search.rs:845` `.take(per_file)`，
超出补 `  +{remaining} more in {file} {hint}`（`:899-902`，hint 是 tee 到文件的路径）。
`docs/guide/pipelines/methods.md:29` 表格写 "every match kept" 指的是**方法**，不是命令。

**实测**（详见 §0.1-4 的表）

结论四条：
1. **收益 ∝ 路径 token 占每行总 token 的比例**，不是路径字符数。同一份 43 命中正文，
   只改路径（附录 A.15）：`haystack.rs`（11 字符/3 tok）**-17.8%**、
   `src/hay.rs`（10 字符但 4 tok）**-22.7%**、`engine/src/hay.rs`（17/5）**-27.1%**、
   `crates/outrim-core/src/format/haystack.rs`（41/12）**-47.8%**、
   60 字符/17 tok 深路径 **-56.4%**。注意 10 字符那条比 11 字符那条收益更高——
   `src/hay.rs` 的 `/` 与 `.` 占 token，字符数并不决定预算。
   分组输出本身几乎不随路径变化（同一序列下 530 → 533 → 539 → 544 tok，
   涨的那点就是表头里多出的那一份路径），所以削减率完全由原文每行里重复的路径预算决定。
2. **门是硬的，但也把绝对路径整块挡在门外**：`C:/…` 与 `C:\…` 形态的第一个 `:` 落在盘符后，
   `parts[1].parse::<usize>()` 失败 → `Outcome::Continue(原文)`（`:47`），
   **收益为 0（非负收益，也非负损失）**。Windows 上 `grep`/`rg` 默认输出就是这种形态，
   所以这个技术在默认 Windows 环境里基本不触发。这是实测（附录 A.15），不是推测。
3. `never_worse` 必须存在：短行（+6.0%）和单条（+42.3%）会变大，`chars/4` 与真实 token 同向，能拦住。
4. **不能宣称"只改排版"**：trim 与空行跳过是有损的，YAML/缩进敏感内容的语义就在缩进里
   （附录 A.1 的 `m19.py`：4 条缩进 YAML 命中 41→44 tok，缩进被抹平；含一条空行命中的 3 条输入
   25→34 tok，空行消失。两者都被 `never_worse` 拒了，但那是运气）。

**前缀折叠**（正交技巧，`search.rs:633 fold_path_prefix`）：多文件结果的公共目录折成 `{prefix} ({N} files)`；
遇 `\0`/`\r`/`\x1b` 或 <2 个文件就放弃；只在**整组件**边界切；而且只在
`is_bare_file_list`（`-l`/`--files-with-matches`/grep `-L`/rg `--files`，且**没有其他形状旗标**）为真时才跑，
折叠后还要过 `never_worse`（`search.rs:588`）。
**实测**（`m37.py`，本地语料）：`find.txt`（1709 行绝对路径，公共前缀 `/tmp/hr/`）
ch 76378→62729（-17.9%）、tok 18721→15269（**-18.4%**）；`ls.txt`（872 行相对名）公共前缀只有 `/`
→ **不折**；`grep.txt`（1500 行 `path:line:text`，不满足 bare-file-list 条件）强拆路径后为 -11.7% ch / -10.8% tok
— 这个数字不能算作该技术的收益，只说明"改掉格式后还有多少剩余冗余"。
→ 结论：这个技术对小而深的配置仓库有用（20% 量级），对平坦目录几乎无效。
`compact_path()`（`:1178`）把 >50 字符且 >3 层的路径压成 `first/.../parent/leaf`——
**破坏可点击/可打开的路径**，属有损，必须独立开关。

**lm-resizer 的 `search_compressor.rs` 是另一种东西**：按"上下文词重叠 + 行重要性 + 关键词"打分，
`adaptive_sizer::compute_optimal_k`（Kneedle 拐点 + zlib 比率校验）算总预算，再按文件分配，
**会产生 `[... and N more matches in file]`——会丢命中**，属于 L3，不是分组。
区别实质：分组的目标是**一次都不丢**，而它的目标是在预算内**取舍**。

这个 `k` 选择器值得单独看（我直接编译了它的 `adaptive_sizer.rs` 跑真实语料，附录 A.14）。
算法三层：`n <= 8` 全留 → simhash 去重后 ≤3 组就直接留 3 → 否则在"唯一 bigram 覆盖率曲线"上
找拐点（要求偏离对角线严格 >0.05），有多样性下限 `n*(0.3+0.7*diversity)`；
最后用 zlib(level=1) 压缩比差 >15% 就把 k 调高 20%。依赖只有 `flate2 1` + `md-5 0.10`
（该 crate 自己 `license = "Apache-2.0"`，仓内 `LICENSE` 是 Apache-2.0 全文）。

关键发现：**`k` 的选择强烈依赖"输入的排列顺序"**。它默认 `items` 是按重要性排好序的，
只保前 k 条。直接把原始行序喂进去，信号损失很严重（附录 A.14，信号词元 =
含 error/fail/panic/warn/`file:line` 的词）：

| 语料 | n | k | 原序保留的信号词元 | 按行信号密度重排后 |
|---|---:|---:|---:|---:|
| `cargo_err_color.txt`（真实 cargo 错误） | 1323 | 32 | **3%** | k=263，**100%** |
| `gitlog.txt` | 17425 | 3255 | **50%** | k=2991，**100%** |
| `bench.py`（日志格式） | 248 | 12 | **11%** | k=24，**100%** |
| `traceback.txt` | 200 | 12 | 100%（本来只有 2 个信号） | k=86，100% |
| `grep.txt` | 2000 | 1984 | 99%（几乎不压） | 99% |
| `ls.txt` | 872 | 796 | **80%** | k=796，100% |

两个结论：
1. 它在一手错误日志上把信息量压到 3%（k=32/1323）**是因为它假定调用方已经排好序**。
   这个前提不适用的场景（原始工具输出）下它比头尾窗口还危险。
2. 反过来，只要把"信号行"提到前面，同一个选择器的保留率就是 100%——
   **算法本身没问题，问题在排序信号的来源**。这对 §4.3.5 的失败行追回门是个互补证据：
   不需要额外机制，只要在候选集上做一次确定性重排就能让预算算法变安全。
3. `grep.txt` 上它给 99.2%（几乎不压），说明拐点检测在"前缀重复"型内容上没有拐点——
   这类内容应当交给 grep 分组而不是预算裁剪。

**成本**：O(n) 解析 + O(n) 输出；`HashMap<&str, Vec<…>>` 内存 O(hits)。**复杂度：低。**

### 4.5 JSON：minify / 骨架 / 投影 / 列式铲平 / TOON

#### 4.5.1 minify：不是字节无损，`arbitrary_precision` 也不够

`serde_json` 解析再序列化。**必须知道的六类变化**（附录 A.4）：

| 输入 | 默认 | `+arbitrary_precision +preserve_order` |
|---|---|---|
| `123456789012345678901234567890` | `1.2345678901234568e+29` | 逐字保留 ✅ |
| `{"f":1.0}` / `{"f":2.50}` | `1.0` / `2.5` | `1.0` / `2.50` ✅ |
| `{"e":1e6}` | `1000000.0` | `1e+6`（写法变了） |
| `{"neg":-0}` | `-0.0` | `-0` ✅ |
| `{"dup":1,"dup":2}` | `{"dup":2}` | `{"dup":2}`（仍塌） |
| `{"u":"\u00e9"}` / `{"esc":"a\/b"}` | `é` / `a/b` | 同（仍变） |
| `{"nan":1.0e400}`（溢出 f64） | 拒绝/报错 | `{"nan":1.0e+400}`（`arbitrary_precision` 接受任意精度十进制） |

→ `arbitrary_precision` **只保数值文本**；重复键、转义写法、空白、键序（需 `preserve_order`）另算。
RTK 只开了 `preserve_order`（`engine/Cargo.toml:15`），所以它标 `lossy: true` 是准确的；
`minify_json.rs` 文件头那句 "lossless JSON compaction" 是错的。lean-ctx 的 lossless 模式
`reconstruct` 后 **Value 相等、字节不等**（键被提到 `_defaults`）。

#### 4.5.2 骨架

RTK `strategy.rs:213 JsonSkeleton`：字符串 >24 字符 → `"short…" (N chars)`；`depth==0` 数组 → `[N× first]`；
`depth==1` 数组 → `[N items]`；`depth>=2` 对象 → `{N keys}`。
`JSON_SKELETON_MAX_CHARS=700`（`:31`）用在两处：对象键循环在 `out.len() > 700` 时停下补 `… +N more keys`；
最后若整体 >900 字符（`MAX_CHARS + 200`）再硬截断补 `…`（`:283-293`）。所以骨架的**上界是 900 字符**，不是 700。
（注：四处 `len()` 都是**字节**，中文/emoji 会提前触发。）
`cmds/system/json_cmd.rs` 另有一套：`--keys-only`，默认截断 >80 字符字符串、数组 >5 项变 `[first, ... +N more]`。

实测（40 项 GitHub 风格响应，附录 A.13）：pretty 3824 → minified 2253 → **skeleton 28 tok**。
骨架保留 `items[40]`、键集合、`total_count: 40`、`page: 1`，丢掉全部标题/正文。
真实语料上 RTK 输出：`json.txt` 42630 字符 → 124 字符（`[json array, 400 items] [400× {id: 0, …}]`），
`mcp.json` 137697 字符 → 112 字符（`{version: 1, servers: {codegraph: {6 keys}, …}}`）。
→ **适合"这 JSON 长什么样/有多少条"，不适合"某条记录的内容"。**

#### 4.5.3 投影与列式铲平：不要用"不丢值"描述投影

按字段白名单裁剪数组元素：40 项取 `id/title/body` 三字段，2253 → 1523 tok（-32%）。
**注意表述**：投影保留的是**被选中字段**的值；未选中字段的值全部丢弃。
所以它是"字段级列选择"，丢的是整列，与骨架的区别只在于被选列保留了真实值——
不能写成"不丢值"或"无损"。

更值得借的是 lean-ctx `rust/src/core/json_crush.rs` 的**列式铲平**（两条路，语义不同）：

- `crush_lossless`：把"在**所有** item 里都存在"的键按主导值提升到 `_defaults`（`MIN_DOMINANCE = 0.5`，
  `min_dominant_count(n) = max(ceil(n×0.5), 2)`），每个 item 只留偏离值；缺失即"等于默认"。
  候选键只取交集（`:238-247`），因为"省略"必须无歧义。
- `crush_lossy`：**额外**丢掉"distinct 比例 ≥ `drop_entropy`"的高熵列，记在 `_dropped`，
  并要求 `res.text.len() * KEEP_DATA_DIVISOR(2) <= trimmed.len()`（`:137`）才返回。
  文档明确："the caller MUST persist the verbatim original out-of-band (CCR) before emitting it
  — the dropped columns are never reconstructible from the text"。

实测（附录 A.4 的 `jc` 探针，40 项 × 8 字段，其中 2 项 status=fail）：

```
minified 6755 字符
lossless 铲平 4681 字符（0.693）  lossless=true    reconstruct: Value 相等、字节不等（键序变）
lossy 铲平     310 字符（0.046）  _dropped=[id, latency_ms, name, trace, ts]
                                   _defaults={owner, region, status}
                                   → 只剩 2 个 status=fail 的偏离
```

→ 这是准确的"基数门"参考实现，但注意**它的极性与直觉相反**：丢的是**高** distinct 比的列
（时间戳/UUID 被当作噪声），保留的是**低**基数（可作默认）的列。
且它有两个硬门：只在 2× 收益时触发，且必须外置原文。
对 Outrim 的启示：如果要做列裁剪，`_dropped` 之外必须有原文句柄；只靠 `_dropped` 列名无法恢复值。

另外，`crush_lossless` 的"Value 相等≠字节相等"说明 **它的 lossless 也只是 L2 语义级**。
如果产品的契约是"字节可逆"，这条不能算无损。

#### 4.5.4 TOON

TOON（`SPEC.md` v4.4）是 JSON 数据模型的行式、缩进式编码，四种形态：
inline（原始类型数组，`[3]: 1,2,3`）、list、tabular（`users[2]{id,name}:` + 行）、keyed tabular。
规范强制：数组保序；对象键序除 tabular 按表头重排外保序；数字必须是**最简规范十进制**
（`1e6`→`1000000`、`-0`→`0`、`1.0`→`1`），NaN/±Inf→null，孤立代理项报错。
tabular 内一层嵌套对象可折进表头（`customer{name,country}`），行保持扁平。

实测（`toon-format 0.7.0` + `serde_json`，附录 A.3）

数值与结构 round-trip（`toonrt` 探针）：
```
[{"a":1.0,"b":2.50}] → "[1]{a,b}: 1,2.5"          → 回 [{"a":1,"b":2.5}]
[{"a":1e6}]          → "1000000"                   → 回 [{"a":1000000}]
[{"a":-0}]           → "0"                         → 回 [{"a":0}]
[{"a":123…890}]      → "123456789012345680000000000000" → 回 1.2345678901234568e+29
[{"a":"x,y"},{"a":"p\nq"}] → 引用 + 转义，逐字 round-trip ✅
{"a":[{"x":1}],"b":{"c":2}} → 嵌套形态，逐字 round-trip ✅
```
→ 结构可逆；**数值文本会被规范化**（`1.0`→`1`、`-0`→`0`、大整数→float）。

字符 vs token（见 §0.1-5 表）：`uniform_500` 字符 0.521 / **token 0.551**；
`github_issues_40` 字符 **1.025** / token **1.216**；真实 `json.txt` 1.075 / 1.273；`mcp.json` 1.139 / 1.070。

**RTK 的形状门**`is_uniform_flat_array`（`toon.rs:57`）：`Array`、`len() >= 2`、全 `Object`、
`obj.len() == keys.len()`、`obj.keys().zip(keys).all(eq)`（**键序也必须一致**）、所有值 `!is_array && !is_object`。
通过后比 `.len()`（**字节**，不是 token）与 minified，取小者，且 `terminal: true` + `ShortCircuit`
（理由：编码后的数据模型可能需要完整，下游 `truncate` 会把 minified 单行砍到 500 字符）。
`lossy: true`（`toon.rs:24-27`，理由同 §4.5.1）。

**判断**：
- TOON 是"键名重复度高的表格数组"专用编码，不是通用 JSON 压缩。
- 必须**与 minified 比 token**，不能比字符，也不能比字节。
- 必须"能解码回来"，且解码回来的比较器要明确是 Value 还是字节（默认：Value）。
- 收益 ∝ 行数 × 键名长度 / 值长度。真实服务数组上它是负收益，这条要在产品里当反例写清。

### 4.6 源码语法裁剪（syntax-aware code selection）

#### 4.6.1 三种路线

| 路线 | 代表 | 机制 |
|---|---|---|
| 树查询（tree-sitter） | lean-ctx `signatures_ts/`（~27 语言）；lm-resizer `source_compressor.rs` | query 捕获声明节点，保留签名行到 body 起始行 + 闭合括号 + import，空档变 `  // ...` |
| 正则 + 括号计数 | RTK `core/filter.rs::AggressiveFilter`；lean-ctx `signatures.rs`（自称 fallback） | 行级：保留签名行/import/`const` 行，跟踪 `{`/`}` 深度 |
| 外部符号索引 | lm-resizer `source_compressor.rs` 的 Code Explorer 模式 | 调外部 `code-explorer`（CLI Cypher 或 MCP stdio），不可用则回退 `conservative-fallback` |

lean-ctx `signatures_ts/ast_prune.rs` 的细节（值得学）：
`take(body_start.min(sig_start + 3).saturating_sub(sig_start) + 1)`（签名行到 body 起始最多 3 行）、
`keep[body_end] = true`（保留闭合行）、空档输出 `"  // ..."`（`:80`）、非 body 节点 `take(end.min(sig_start + 2)…)`。
注释里还写了 `.take()` 越界会 "panic (debug) or an enormous `.take()` → OOM (release)"——说明这是踩过的坑。

lm-resizer 额外保留 leading doc-comments，失败时降级为"只剥注释与空行"（`compress_conservative`，
`engine_used: "conservative-fallback"`）。

#### 4.6.2 实测

`rtk-engine` 上 600 个函数（86180 字符）：`structural` 策略 86180 → 43690 字符（-49.3%），
签名保留 600/600，`elided_ranges` 元数据 1799 字符（占输出 4%）。
真实语料 `bench.py`（8648 字符）：→ 2952 字符（-65.9%），`elided_ranges` 71 字符。

**适用性实测（E6，轨迹语料）**：
- 2128 次 `open` 里 534 次（25.1%）读的是"同一轨迹稍后会被 Edit 的文件"，
  这 534 次占全部 open observation token 的 **69.9%**（1,109,719 / 1,588,659）。
  （注：`m15.py` 按"非空 observation"统计得出 2125 次、`m16.py` 按"开放调用"统计 2128 次，
  差 3 次是空 observation 的 open；两个分母各自一致，不混用。）
- 3067 次非空 `old_string` 编辑里 572 次（18.7%）的串逐字出现在该文件最后一次读的内容里，
  这些串的 token 量占 72265 中的 5790（8.0%）。

→ 结论：**签名骨架在 70% 的读 token 上不可安全使用**（这些读的下一步就是改文件）。
可用的窄场景是"读文件做定位/回答结构问题"，需要能判定"这次读不是为编辑做准备"。

**这条边界不是可选的**：把它当默认行为，等于在最常被 Edit 的那 70% 读上删掉函数体，
而 `old_string` 里 8.0% 的逐字引用正是从上一次读里取的。入口层能看到"工具调用 + 参数"，
所以可以按 `open`/`read` 与 `edit`/`write` 的先后关系判定；但**跨会话或跨分支的历史读不可见**，
那时只能退化为"不裁剪"。

#### 4.6.3 失效例子

- Python 缩进语义：RTK `MinimalFilter` 用 `advance_triple_quote()` 状态机（`filter.rs:167`），
  `filter.rs:540` 起有 **10 个** 三引号/引号复用相关测试，`filter.rs:733` 起另有 **5 个**
  块注释/`#2385`（`src/*.rs` 里的 `/*` 被当块注释）相关测试——说明这类坑是连续踩出来的。
- Rust 的 `//` 出现在字符串里：同上，`#2385` 的修复是"只在行首判断块注释开始"。
- 宏/属性/条件编译：正则路线无法识别 `#[cfg(...)]` 与 `impl` 块归属，会把 `impl` 里的函数签名当顶层。
- 生成代码/压缩后的 JS：单行巨型文件，任何"按行"策略退化成"全保留或全删"。
- tree-sitter 的运行时成本：每次读要解析 O(bytes)；grammar 编进二进制每个约 0.5–2 MB；
  不支持的语言需要 feature 化。**没有启动成本**（无模型加载），但每语言要写 query 与测试。

#### 4.6.4 复杂度与依赖

`tree-sitter 0.27.1`（MIT）+ `tree-sitter-rust 0.24.2` / `tree-sitter-typescript 0.23.2` /
`tree-sitter-python 0.25.0`（均 MIT）。正则路线 `regex 1.13.1`（MIT OR Apache-2.0），约 150 行。
**复杂度：高（tree-sitter，每语言 query + 测试）/ 低（正则，正确性上限低）。**

### 4.7 token 估算与阈值

**现状**
- RTK：`chars/4`（`text/src/lib.rs:83`），CLI 与 engine 共用；proxy 侧另有 `o200k_base` 真实 BPE
  （`proxy/src/tokenizer.rs`，`tiktoken-rs`，失败回退 `len/4`）。
- squeez：两条路。`estimate()` = `ceil(alnum/3.8 + punct/2.0 + wide)`（blank 不计；`wide` 是非 ASCII **字符数**）；
  `estimate_classed()` = 按 64 KiB 头部样本分类后 Dense `/2.0`、Prose `/3.7`、Mixed 回退 `estimate`。
  `config.class_density` 默认 true，所以 `wrap.rs:581-585` 与 `:658-662` 走的是 classed 路径。
  （两条路的分叉代码：`if config.class_density { estimate_classed(..) } else { estimate_scaled(..) }`。）
  源码注释引 pxpipe："dense payloads (JSON/code/diffs, N=391) run ~1.91 chars/token,
  while English prose runs ~3.5-3.7"——**二手来源，未核**（§8）。
- lm-resizer：`tokenizer/` 三后端（tiktoken 字节等价、HF `tokenizer.json`、估算），
  文档自述 proxy 尚未接线，且 Anthropic tokenizer 未公开。

**偏差表见 §0.1-8。** 四条操作结论：

1. **相对闸门用 `chars/4` 是安全的**：`never_worse` 比较的是同一段内容的两次估算，
   误差同向抵消；在 RTK 自己的 head/tail/structural 输出上实测削减率误差 ≤0.35 个百分点（附录 A.2）。
2. **绝对预算必须用真实 tokenizer**：squeez 的 `net_win_min_tokens = 24`（`config.rs:353`）是**估算** token 单位，
   换真 token 的公式是 `真 = 估算 × 4 / (chars/token)`（因为 `chars/4` 口径下 `估算 = chars/4`，
   而 `真 = chars / (chars/token)`）：中文 104、散文 21，差 5 倍；
   而 squeez 实际用的是 classed 估算器（分母不是 4），同一阈值在 grep.txt 上只相当于 **11.7** 个真 token。
3. **`chars/4` 的"4"到底除什么，中文上差 46 个百分点**：RTK 的 `estimate_tokens` 是
   `text.len() / 4`（`text/src/lib.rs:83`，`len()` 是 **bytes**），而 Python 侧 `chars/4` 常按字符数算。
   同一份中文日志（920 字符 / 2760 字节 / 1000 token）：按字节算是 -31.0%，按字符算是 **-77.0%**。
   所以引用任何"`chars/4` 误差"时都必须写明分母是 `str.len()` 还是 `chars().count()`；
   本文 §0.1-8 表给的是**字符口径**，用字节口径的只有本行。
4. **不要用 `estimate_classed` 这类"分类 + 除数"的启发式**：在真实语料上它比 `estimate` 差，
   因为它把 8 份真实语料判成 7 份 `Dense` 然后除 2（只有 `gitlog.txt` 是 `Mixed`）。分类器本身也不稳：
   同一份 `gitlog.txt` 因为 `::` 出现在 ASCII art 里被判 `Mixed`，于是它反而是唯一走 `estimate` 的。
   方向也不一致——`grep`/`mcp` 高估 105%/129%，`chinese`/`base64` 低估 75%/54%，
   而 `estimate` 在 `chinese` 上只低估 8%。

真实 tokenizer 的成本：`tiktoken-rs 0.12.1`（MIT）把词表编进二进制（每个 2–4 MB），首次有一次性初始化；
`tokenizers 1.0`（Apache-2.0）可加载 HF `tokenizer.json` 但需文件分发。
Anthropic 无公开 tokenizer → **标注为未知**，只能用相对比较或按内容类别校准。
**复杂度：低（接 tiktoken-rs）/ 中（多后端 + 缓存 + 回退链）。**

### 4.8 兜底：头尾窗口

- **机制**：RTK `cap`（`HEAD_LINES=50`、`TAIL_LINES=200`、`MAX_LINES = 50+200+50`、`MAX_LINE_CHARS=2000`，
  标记 `[{n} lines elided]`、`{kept} [+{n} chars]`）；RTK `strategy::HeadTail`（20 头 + 8 尾）；
  RTK `methods/truncate`（300 行 / 500 字符，**不在任何链里**，文件头自述 "Reserved");
  squeez `truncation`（头/尾 + `[... N lines truncated]`）；lm-resizer `live_zone` 的 head/tail 预览。
- **理由（源码原文）**：`cap.rs` 文档 "tail-biased … Bash errors live at the END of output"。
- **反例**：§0.1-3 的两组实测。补两条更具体的（附录 A.13）：
  (i) 360 行合成构建日志（错误在 index 300 之后）：`head50/tail8` **同时丢掉 `error[E0432]` 与 `error[E0308]`**
  （只保住 `87 passed` 与失败用例名）；同一份输入上 `cap(50,200)` 因为 200 行尾巴盖住了错误、两个错误都保住了。
  (ii) 2845 行日志（错误在 index **1400**，前后各 1400 行噪音）：`cap(50,200)` 同样**丢掉 `E0432`**
  （37613→3255 tok）。→ 就连 200 行尾巴也只覆盖末 200 行，位置一旦离底部超过尾部预算就失效。
  结论：窗口大小（而不是“有窗口与否”）就决定成败，而且这个失败模式不依赖具体项目。
- **RTK 自己已经在别处否定了它**：`read_min_tokens() = usize::MAX` 的注释写着
  "A `Read` is the agent asking for exact file bytes, and head+tail elision removes the middle without
  giving it back. The agent then pages through the file to reconstruct the elided middle, each recovery
  read adds a turn, and session cost grows with turns faster than per-turn compression saves.
  Size does not predict this; recovery cost does."
  同一份代码里 `cap` 是默认 `term` 链的最后一环。这个自相矛盾就是最好的证据。
- **建议**：只作为"已确认无可保留信号"时的最后手段，且必须：
  1. 前后各留一段 + 明确计数与被删范围；
  2. 先跑确定性信号提取（错误行/失败行/`path:line`/锚点），把它们固定在输出里；
  3. 提供原文取回句柄。
  若做不到第 2 条，就不要开窗口。

---

## 5. 交叉议题

### 5.1 三种"可逆"，必须按名字使用

| 说法 | 含义 | 检查方式 | 本文实例 |
|---|---|---|---|
| 字节可逆 | `decode(encode(x)) == x` 逐字节 | 逐字节比较 | 没有任何一个现有方法真正到这一层：`ansi` 的正则漏 OSC（§0.1-10）；`progress` 会把 CRLF 改成 LF；`strip-line-numbers` 的 `recovery: None` 且 `idempotent: false` |
| Value 可逆 | JSON 解析后 `Value` 相等 | 用固定比较器 | lean-ctx `crush_lossless`（键序已变） |
| 语义等价 | 结构/数值等价（`1.0` vs `1`、键序、重复键） | 解析 + 声明比较器 | TOON（数值文本规范化） |
| 信息等价 | 完成任务所需的事实还在 | oracle / 锚点集 | 所有 L2/L3 方法的主张 |

RTK 的 `MethodInfo` 用 `lossy: bool` + `recovery: None|Recall|RetrieveTool|Pointer` 二维表达，
比笼统的"lossless"精确。但它自己也有两处不一致：
`methods/ansi.rs` 标 `LOSSLESS_TRANSFORM` 而正则会漏 OSC；`methods/minify_json.rs:1` 文档写
"lossless JSON compaction" 而 `:28` 是 `lossy: true`（注释自陈 "Same serde limits as toon"）。

**建议直接用这个二维标注，并额外加第三个字段——比较器**（`bytes | value | semantics | info`），
否则 `lossy: false` 会被误读成字节可逆；`recovery` 也要区分"真能把原文拿回来"（`Recall`/`RetrieveTool`）
和"只是告诉你被删了多少"（`Pointer` 之外的计数标记不算恢复通道）。

### 5.2 全局守卫（必做）

- **never-worse**：`estimate_tokens(out) > estimate_tokens(in)` → 原文。代价小。
  注意它只保证"不变大"，不保证"没破坏"（RTK 源码注释自己承认）。
- **fail-open**：panic/解析失败返回原文（RTK `run_chain_caught` + `catch_unwind`；
  squeez `stash_refused` 走"不压缩"分支）。
- **幂等**：无损方法必须幂等，否则 hook + proxy 双重处理会二次损伤。
  注意 `dedup` 会插入 `[xN]` 行，字节不幂等，靠"标记已处理"兜。
  实测 `rtk-engine` 的 `strategy::render` **不幂等**（附录 A.12 / E11）：把 `render()` 的输出再送一次，
  `gitlog.txt` 1447 ch → **199 ch（再 -86.2% ch / -70.0% tok）**，
  `bench.py` 897 tok → 703 tok（-21.6%），因为 head/tail 输出不再是 `repetitive`
  （`prefix_rep` 640→0、`indent_sd` 1274→1712，重新落到 `nested` → `structural`），
  而 `json` 输出变成 `flat-unique` 所以 `render=None`。`grep`/`traceback` 只再动 0.1%（几乎幂等）。
  → 入口压缩如果可能被 hook 与 proxy 双重调用，**必须自己做幂等标记**，不能指望算法幂等。
- **信号保留**：任何 L2/L3 之后跑一次 §4.3.5 的失败行追回。
- **可恢复性声明**：只在确实写入原文时给句柄。squeez 的做法值得抄：`wrap.rs:717` 的
  `stash_eligible` 与 `:740` 的 `net_win_gate` 把 marker 的成本算进净收益判定，
  注释解释了这个 bug 的历史（"It used to be appended after the gate, so a call that saved 25 tokens
  could emit a 40-token marker and still print a header claiming a win"）。
- **`is_net_loss()`**（`wrap.rs:1078`）= `min_tokens > 0 && input - output - marker_cost < min_tokens`。

### 5.3 输出成本（句柄/标记的实测价）

用户已定"原文指针极短、无免责声明"。给这个约束配实测数字（附录 A.10）：

| 形态 | tokens | 字符 |
|---|---:|---:|
| `74039156`（8 位十进制句柄） | **3** | 8 |
| `[1873L/74039156]` | 9 | 16 |
| `[+1873L 74039156]` | 10 | 17 |
| `[1873 lines elided] 74039156` | 11 | 28 |
| `[1873L elided \| 74039156]` | 12 | 25 |
| `7a3f9c2e1b4d6a80`（16 位 hex，单样本） | 15 | 16 |
| `[outrim:7a3f9c2e1b4d6a80]` | 20 | 25 |
| squeez 的回收 marker（完整句） | **39** | 140 |
| `[squeez:…key="0000000000000000"…]` 占位版 | 38 | 138 |

其他标记：`[x40]` 3 tok、`  [x40]` 5 tok、`[850 lines elided]` 6 tok、`    [+193 chars]` 5 tok、
`43 matches in 1F:` 7 tok、`2000 matches in 40F:` 8 tok、`[file] crates/outrim-core/src/filters/mod_10.rs (120):` 17 tok。

**关键发现**：随机 hex 在 cl100k 里约 **0.56–0.65 token/字符**，十进制约 **0.33–0.38 token/字符**。
30 个随机样本的均值（附录 A.10，seed 20261019）：

| 长度 | hex 均值 | dec 均值 |
|---:|---:|---:|
| 8 字符 | 5.20 tok | 3.00 tok |
| 12 字符 | 7.10 tok | 4.00 tok |
| 16 字符 | 9.73 tok | 6.00 tok |
| 24 字符 | 13.47 tok | 8.00 tok |

单条方差很大（16 位 hex 一条实测 15 tok、另一条 7 tok），所以上表里的 `7a3f9c2e1b4d6a80` = 15 是**该样本**的值，
不代表均值（同长度 hex 样本均值 9.73）。结论：**同样字符长度下十进制比 hex 便宜 38%–44%**，8 位十进制只需 3 token
（数字串按 ~2–3 位一 token 合并）。`[+1873L 74039156]` = 10 tok 是很好的折中：
带被删行数（有助于模型判断"要不要取回"）+ 3 token 的句柄。
`[xN]`、`[N lines elided]` 这类纯计数标记都很便宜（≤6 tok）。
`squeez` 的 39-token 自述句是纯浪费——这正是"极短指针"决策的量化依据。

**另外**：RTK 的 `elided_ranges` 元数据在 600 函数源码上占输出 4%（1799 字符），
**不要塞进 prompt**，留在结构化侧信道里。

### 5.4 Rust 复用面与许可

| 用途 | crate | 版本 | 许可 | 备注 |
|---|---|---|---|---|
| JSON 解析/序列化 | `serde_json` | 1.0.154 | MIT OR Apache-2.0 | 需要 `preserve_order`；`arbitrary_precision` 按需 |
| 正则 | `regex` | 1.13.1 | MIT OR Apache-2.0 | |
| 多模式匹配 | `aho-corasick` | 1.1.5 | Unlicense OR MIT | 信号行检测 |
| 字节查找 | `memchr` / `bstr` | 2.8.3 / 1.13.1 | Unlicense OR MIT / MIT OR Apache-2.0 | |
| 终端状态机 | `vte` | 0.15.0 | Apache-2.0 OR MIT | 推荐用于 ANSI |
| ANSI 剥离 | `strip-ansi-escapes` | 0.2.1 | Apache-2.0/MIT | 覆盖 OSC |
| TOON | `toon-format` | 0.7.0 | MIT | RTK 钉 0.5.0 |
| 日志模板 | `drain3` | 0.3.0 | Apache-2.0 | 依赖偏重，可重写 |
| 真实 tokenizer | `tiktoken-rs` | 0.12.1 | MIT | 词表 2–4 MB |
| 语法树 | `tree-sitter` | 0.27.1 | MIT | + 每语言 grammar |
| Rust grammar | `tree-sitter-rust` | 0.24.2 | MIT | |
| TS grammar | `tree-sitter-typescript` | 0.23.2 | MIT | |
| Python grammar | `tree-sitter-python` | 0.25.0 | MIT | |
| JSON 路径 | `jsonpath-rust` | 1.0.12 | MIT | 投影用 |
| diff | `similar` | 3.2.0 | Apache-2.0 | |
| 有序 map | `indexmap` | 2.14.2 | Apache-2.0 OR MIT | |
| 懒初始化 | `once_cell` | 1.21.4 | MIT OR Apache-2.0 | |
| 内容哈希 | `blake3` | 1.8.7 | CC0-1.0 OR Apache-2.0 OR Apache-2.0 WITH LLVM-exception | 原文句柄 |
| unified diff 解析 | `unidiff` | 0.4.1 | MIT | |

（全部经 crates.io `/api/v1/crates/<n>/<v>` 核于 2026-10-19。）

**lean-ctx 的许可要按分域读**（`LICENSE_MATRIX.toml`，**19 条** `[[rules]]`，order 1–19）：
文件头 `distribution = "apache-host"`、`authority = "LICENSE.md"`、`pre_v4_apache_cut = "b4429b80…"`。
带 `forbidden_public = true` 的是 **6 条**：order 1 `private-service`、order 2 `commercial-source`、
order 3 `commercial-license-text`、order 14 `free-runtime-license-draft`、order 16 `enterprise-license`、
order 17 `private-runtime-build`。规则 2 覆盖 `rust/src/commercial/**`、`rust/src/team/**`、
`rust/src/enterprise/**`、`commercial/**`、`enterprise/**`；
规则 1 覆盖 `rust/src/core/license/**`、`rust/src/core/licensing/keygen.rs`、`rust/src/cli/license_cmd.rs`、
`rust/src/core/billing/success_fee.rs`、`rust/src/core/billing/stripe_invoice.rs`。
规则 11 `public-trust-core-runtime`（`license = "apache"`）覆盖 `rust/**`，但 `exclude` 里除了
规则 1/2 的那些路径，还排掉了 `rust/tests/**`、`rust/examples/**`、`rust/crates/lean-ctx-sdk/**`
——后三者由规则 8 `tests-and-examples` / 规则 9 `sdk-and-protocol` 各自以 apache 覆盖。

→ 引用 lean-ctx 的 `json_crush.rs`、`ast_prune.rs`、`compressor.rs`（都在 `rust/src/core/` 且不在任何
排除列表里）属 apache 域；但**引用前要按文件逐个对 `LICENSE_MATRIX.toml` 的 include/exclude 核**，
不能按"整个仓库 Apache-2.0"概括，也不能只记住规则 11 的那几条排除项。

---

## 6. 候选对比表

**复杂度**：低 ≈ 100–300 行；中 ≈ 300–1000 行或 1 个中等依赖；高 ≈ 多语言语法/多后端。
**CPU 预算**：入口压缩要"毫秒级、不影响首 token"。

| 技术 | 层 | 内容类型 | 信息变化 | 保留规则 | 实测/已知失效 | CPU / 内存 | Rust 复用 | 复杂度 | 许可 |
|---|---|---|---|---|---|---|---|---|---|
| ANSI/OSC 剥离 | L1 | 全部终端输出 | 去渲染指令 | 保留文本 | 现有四个实现都不完整；OSC/私有模式/`ESC(B` 会残留或吞字 | O(n)/O(1)，无 `ESC` 零开销 | `vte`、`strip-ansi-escapes` | 低 | Apache-2.0 OR MIT |
| CR 进度帧取末帧 | L1 | 交互式 CLI | 丢中间帧 + CRLF→LF | 末帧全文 | `\r` 多段覆盖丢信息 | O(n)/O(1) | 手写 | 低 | — |
| 行号剥离 | L1 | 文件读 | 丢定位（可再生） | 仅 `TRUSTED_SEPS` + `file_coords` | `12:` 误判；按行号 Edit 时有害 | O(n)/O(1) | 手写 | 低 | — |
| 相邻重复折叠 | L2 | 日志/构建 | 丢"位置重复"，插入 `[xN]` 行 | 计数、仅相邻、仅非空、run≥3 | 跨距离折叠改因果 | O(n)/O(1) | 手写 | 极低 | — |
| 模板折叠（token 形状门） | L2 | 日志 | 丢变量值 | 只折形状像 id 的 token；`[×N]`+样本 | 形状门是意外（冒号）；`ls` 丢 4.4% 文件名、73 个目录折成 1 条 | O(n)/O(unique) | 手写 ~240 行 | 低 | Apache-2.0 |
| Drain 模板树 | L2 | 日志 | 位置化通配 | `simSeq ≥ st`、`max_clusters` | 空行/超长行报错；堆栈帧可能被固化成模板；不保留原文 | 0.9–1.8M 行/s / O(模板) | `drain3 0.3` | 中 | Apache-2.0 |
| 分级选择 + 全局预算 | L3 | 构建/测试 | 丢未选中行 | 行级 `LogLevel` 打分（error 1.0 / warn 0.5 / info·unknown 0.1 / debug 0.05 / trace 0.02，栈 +0.3、汇总行 +0.4，封顶 1.0）+ `max_total_lines: 100` | 分数表只看日志级别，不看是否含 `path:line`；源日志无级别标记时全部落到 0.1 | O(n log n)/O(n) | lm-resizer `log_compressor.rs` | 中 | Apache-2.0 |
| Kneedle 预算选择 | L3 | search/多文件结果 | 预算内取舍 | 拐点检测 + 多样性下限 + zlib 验证 | 未重排时真实 cargo 错误只留 3% 信号词元；重排后 100%；对前缀重复型内容给 99%（几乎不压） | O(n log n)/O(n) | lm-resizer `adaptive_sizer.rs`（`flate2`+`md-5`） | 中 | Apache-2.0 |
| 失败行追回门 | 保护层 | 全部 | 追回若干行 | `FAILURE_SIGNAL` 正则 + 原序重插 + 计数 | 词表覆盖不到的失败形式；只追回被删的，不改输出内部 | O(n)/O(n) | lm-resizer `diagnostic_gate.rs` | 低 | Apache-2.0 |
| grep 按文件分组 | L2 | search | 改排版 **+ trim/空行有损** | 任一行不可解析即整块放行 | 短行/单条变大；YAML 缩进被抹；单条 +42% | O(n)/O(hits) | 手写 ~120 行 | 低 | — |
| `rtk grep` 每文件上限 25 | L3 | search | **丢命中** | 超出 tee 到文件 + `+N more` | 与"分组无损"混淆 | O(n)/O(1) | 手写 | 极低 | — |
| 路径前缀折叠 | L2 | search/glob | 丢重复前缀 | 仅 `-l`/`--files` 类形状 + ≥2 文件 + 公共目录 + 无 `\0\r\x1b` | 公共前缀为 `/` 时不折；Windows `\` 路径不折；折叠后过 `never_worse` | O(n)/O(n) | 手写 ~50 行 | 极低 | — |
| JSON minify | L1/L2 | JSON | 空白；数值文本/重复键/转义可能变 | 需要字节/Value 契约时分档声明 | `1e29` 塌成 f64；`1e6`→`1e+6`；`{"a":1,"a":2}`→`{"a":2}` | O(n)/O(n) | `serde_json` | 极低 | MIT OR Apache-2.0 |
| JSON 骨架 | L2 | JSON | 丢所有值 | 键/长度/类型/小字符串 | 回答不了"这条记录是什么" | O(n)/O(n) | 手写 ~120 行 | 低 | — |
| JSON 投影 | L2 | JSON | **丢未选中字段**（保留被选字段的真实值） | 需要字段相关性信号 | 选错字段 = 信息归零 | O(n)/O(n) | `serde_json` + `jsonpath-rust` | 低—中 | MIT |
| 列式铲平（`_defaults`） | L2 | JSON 数组 | 键序变；lossy 版丢高熵列 | 2× 收益门 + 外置原文 | lossless 版也不是字节可逆 | O(n)/O(n) | lean-ctx `json_crush.rs`（apache 域） | 中 | Apache-2.0 |
| TOON | L2 | 统一标量数组 | 数值文本规范化 | 形状门 + 与 minified 比 **token** | 真实服务数组 +27%；嵌套/长值无效 | 统一数组 encode 1.35–1.47 ms/35 KB（23–25 MB/s） | `toon-format 0.7` | 低 | MIT |
| 源码签名骨架 | L2 | 源码 | 丢函数体 | 签名/导入/doc/闭合括号 | 需要改代码时有害；正则对宏/impl 失效 | 解析 O(bytes)，grammar 0.5–2 MB/语言 | `tree-sitter` + grammars；或 `regex` | 高（TS）/ 低（正则） | MIT |
| token 真实计数 | 度量 | 全部 | 无 | 只用相对比较 / 绝对预算用真实值 | Anthropic 无公开 tokenizer | 首次初始化毫秒级；词表 2–4 MB | `tiktoken-rs` | 低—中 | MIT |
| 头尾窗口 | L3 | 全部 | 丢中间 | 仅"已确认无信号" | 语料级只削 6.4%；触发的那 29 条平均削 83%，错误词元只剩 32.5%、`path:line` 只剩 5.5% | O(n)/O(n) | 手写 | 极低 | — |

**吞吐参考（`rtk-engine` release，Windows x86_64，附录 A.2/A.12）**：
每次均跑 3 遍取区间，同一台机器上单次波动可达 ±30%，所以给区间而不是单值。

```
strategy::render 全量（含校验）  2.9216 MB → 47–72 ms   (41–63 MB/s)
单独 classify（自写原型）        600 函数源码  0.56–0.60 ms/op (143–155 MB/s)
                                 3000 行日志  0.42–0.45 ms/op (322–351 MB/s)
                                 2000 grep 命中 0.29–0.48 ms/op (337–546 MB/s)
模板折叠 + 槽位统计              3000 行日志  1.92–2.03 ms/op (72–76 MB/s)
                                 600 函数源码 1.49–1.73 ms/op (50–58 MB/s)
grep 分组                       2000 命中    0.74–0.91 ms/op (176–216 MB/s)
TOON encode                      500 行统一数组 1.35–1.47 ms/op (23–25 MB/s)
Drain train+match                1600 行→4 模板 1.0–2.6 ms；匹配 0.9–1.8M 行/s
```

→ 这些操作都在**单毫秒到几十毫秒**量级，对首 token 延迟不构成风险；唯一例外是 `render()` 的全量通关
（41–63 MB/s，因为它对每份语料都先 normalize + extract + JSON 试解析）。
入口压缩应该按内容类型直接分派，不要对每段输出都跑完整分类。

---

## 7. 实验：已完成与待补

已完成（本轮）与待补（需要模型/上游）分开列。已完成的都给了探针编号（附录 A）。

### 已完成

**E1 形状分类在真实语料上的分布**（`engprobe/bin/shapeprobe`）
10 份真实语料：`repetitive` 5 份（grep/find/ls/gitlog/install）、`nested` 3 份
（`bench.py`、`failing.py`、`traceback.txt`——后者因 4 空格堆栈帧 `indent_sd=6053` 被判 nested）、
`structured` 2 份（`json.txt`、`mcp.json`，均为单行 JSON）。`install.txt`（18 行）`prefix_rep=500` 刚好压线。
→ 阈值 1500/500/950 在这批语料上"能跑"，但 `nested` 与代码的分界靠 `indent_sd` 单特征，日志会混进来。

**E2 grep 分组忠实性**（`sc2/m1.py`、`sc2/m2.py`、`sc2/m5.py`、`m10.py`、`m19.py`）
见 §0.1-4。已确认：整块放行生效（`-C` 上下文）、`never_worse` 拦住两种变大情形、
trim/空行确实有损、`rtk grep` 的 25/file 是另一条路径。

**E3 无损层净收益**（`m12.py`、`m14.py`、`m25.py`、`ansicmp`）
见 §0.1-9。轨迹语料 ANSI = 0；真实带色输出 -32.8%/-67.9%；四个 strip 实现均不完整。

**E4 模板折叠的槽位门**（`sq2`、`sqtest`、`m27.py`、`m28.py`）
见 §0.1-7 与 §4.3.2。已确认：形状门是 token 形状而非基数；`ls` 类清单不能折；
`traceback` 折叠把行号变 `{}`（不可恢复，除非留原文）。

**E5 JSON 三路对比**（`m21.py`、`m22.py`、`toonrt`、`aptest`、`jc`、`sc2/toon/*`）
见 §0.1-5、§0.1-6、§4.5。已确认：字符比与 token 比差异大；真实服务数组 TOON 负收益；
`arbitrary_precision` 只保数值文本；lean-ctx lossless 铲平不是字节可逆。

**E6 源码裁剪的适用边界（部分）**（`m16.py`、`m17.py`）
25.1% 的 open（69.9% 的 open token）随后被 Edit；18.7% 的 `old_string` 逐字来自最后一次读（8.0% token）。
→ 仍缺"骨架是否引发额外重读"的直接测量（需要跑 agent）。

**E7 估算器决策分歧率（部分）**（`m31.py`）
§0.1-8 表。已确认 `chars/4` 在相对闸门上够用（RTK 输出上削减率误差 ≤0.35pp，最大 `bench.py` 0.34pp），
`estimate_classed` 在真实语料上更差。仍缺"同一语料上 `never_worse` 决策是否翻转"的直接统计。

**E8 Drain 可行性**（`drain_probe`）
见 §4.3.4。已确认空行炸弹、吞吐、堆栈帧被固化成模板字面量。

**E9 真实轨迹语料的工具分布与规模**（`m13.py`、`m15.py`）
10037 条非空 observation / 649.6 万 token（58 个工具名）；`edit`/`open`/`goto`/`insert` 四个工具占 **80.9%**
语料 token，且**全部不触发窗口**（`edit` 36.2%、`open` 24.5%、`goto` 11.7%、`insert` 8.6%）。
换句话说：cap 的成本（83% 削减 / 32.5% 错误词元）只落在占语料 8.4% token 的 29 条超大输出上。

**E10 Kneedle 预算选择器在真实语料上的可移植性与排序依赖**（`/tmp/ask`，附录 A.14）
直接编译 lm-resizer 的 `adaptive_sizer.rs`：真实 cargo 错误日志上未重排时只保留 **3%** 信号词元
（k=32/1323），按信号密度重排后同算法保留 **100%**（k=263）。`gitlog` 50%→100%、`bench.py` 11%→100%。
→ 预算算法本身可用，但**必须自带确定性重排**，不能假定输入已按重要性排序。

**E11 `render()` 的幂等性**（`renderprobe` 二次运行 + `tiktoken` 对比，附录 A.12）
把 `strategy::render` 的输出再送一次：

| 语料 | 一遍 | 二遍 | 变化 | 二遍分类 |
|---|---|---:|---:|---|
| `gitlog.txt` | repetitive → headtail，1447 ch | 199 ch | **-86.2% ch / -70.0% tok** | `nested` → `structural`（`indent_sd>=1.5`） |
| `bench.py` | nested → structural，2952 ch | 2202 ch | -25.4% ch / -21.6% tok | `nested` → `structural` |
| `grep.txt` | repetitive → headtail，3293 ch | 3290 ch | -0.1% | repetitive → headtail（只把标记计数改了） |
| `traceback.txt` | nested → headtail，1464 ch | 1462 ch | -0.1% | nested → headtail |
| `json.txt` | structured → json-skeleton，124 ch | 124 ch | 0 | `render = None`（变 `flat-unique`） |

→ **lossless 层（RLE）幂等，lossy 层不幂等**：`render` 的输出重新分类后会落到不同策略上，
`gitlog` 被二次削掉 86%。所以「hook 先压 + proxy 再压」这种双层部署必须靠外部标记
（`docs/guide/pipelines/methods.md:36-38` 承诺"lossy 方法会标记输出"）或干脆只在一个层做。
注意 `grep.rendered` 的第二次 `[… 1 lines elided …]` 指的是**第一遍留下的井号标记行**被当普通行处理，
不是内容丢失——但这也说明标记本身会参与下一轮分类。

### 待补（需要模型或上游，本轮不做）

- **端到端净收益**：压缩后模型是否多读一次原文、多问一轮。任何局部压缩率都证明不了这个，
  需要真实 agent 跑分（本轮无模型侧测量）。
- **其他 tokenizer**：`o200k_base` 与 Claude 侧的字符/token 关系（Claude 无公开 tokenizer）。
- **头尾窗口 vs 信号保护的对照**：同一轨迹上"`cap`"与"`cap` + 信号固定"的成功率差异；
  本轮只测了信号保留率，没测任务成功率。
- **tree-sitter 在 Rust/TS/Python 上的 query 正确率**：需要人工标注集。
- **行号剥离的收益**：只有机制核实与条件判定，没有独立收益测量（需带 `cat -n`/`nl` 的真实语料）。
  路径前缀折叠已补测（附录 A.18，`find.txt` -18.4% tok）。

### 局部压缩率不能证明什么

- 不能证明**任务净 token** 下降：压缩后模型可能多读原文、多问一轮，或多输出解释
  （squeez 的 39-token marker、RTK 的 CCR marker 都是净成本）。
- 不能证明**质量不变**：`test result: FAILED. 87 passed; 1 failed` 保住了，但失败用例名丢了还得再跑。
- 不能证明**提示缓存友好**：任何改变历史块字节的压缩都会让上游 prefix cache 失效。
  RTK 有明确设计（`builtin/default.toml` 要求 proxy 方法 `cache_stable`，`validate` 把
  "not cache-stable" 当错误）。**入口压缩的好处是发生在内容首次进入上下文之前**，不重写历史；
  但同一工具输出在同一次请求里被多次引用时要小心。
- 不能证明**成本下降**：本地 CPU、原文存储（SQLite/文件）、GC 压力都要计。

---

## 8. 未解事实（明确标注为未知）

1. **Anthropic 的真实 tokenizer 未公开**：Claude 侧 token 数只能估算。本文全部实测用 cl100k_base，
   对 Claude 只能作"内容类别间的相对关系"参考。
2. **TOON 在其他 tokenizer 上的收益**：只测了 cl100k（`o200k_base` 未测）。
   `toon-format` 规范附录的 benchmark 属 informative 且由参考实现测量，**未核**。
3. **Drain 在"构建/测试日志"上的聚类质量**：论文评测是系统日志（HDFS/BGL 等），非编译器输出。
   本轮只测了合成日志的吞吐与三类坑，**真实 cargo/pytest 上的模板数与错误类型区分度未测**。
4. **RTK `shape` 在生产中的实际触发率**：只在 proxy CCR 路径上（`COMPRESS_MIN_TOKENS = 1500` 默认、
   `Bulk` 300、`Read` 为 `usize::MAX`，三者均可用环境变量/`proxy.request` 的 `tokens_gt` 覆盖），
   六套内置 TOML 零引用。实际收益取决于是否开 proxy，以及上游 host 是否经 proxy。
5. **lean-ctx 的默认压缩路径**：`auto_mode_resolver.rs` 的 `auto` 是一条多分支链
   （`instruction_file`/`binary`/`context_kit`/`cache_hit`/`small_file`(≤200 tok)/`small_code_file`(≤400)/
   `config_data`/`active_diagnostic`/`task_suspect_file`/`intent`/学习信号(默认关)/`cognitive`/`progressive`），
   还有 `edit_quality` 的失败升级回退（`RISKY_ENTER_RATE=0.25`、`RISKY_EXIT_RATE=0.15`、`RISKY_MIN_FAILS=2`）。
   我没有全面评估"哪条是默认入口"，也未验证 README 宣传的 "up to 99%" 在什么负载上成立
   （**营销数字，不作为依据**）。可确认的是：它有**按文件类型/编辑失败率回退到 full** 的机制，
   这比"固定压缩率"更接近正确的产品形态。
6. **lm-resizer 基准的可信度**：`bench/RAPPORT.md` 自报 22 fixture（3 真实 captures + 19 合成），
   `o200k_base`，全局中位 74.7% / oracle 100%；RTK 中位 0.0% / oracle 79.5%；Headroom 0.0% / 100%；
   延迟中位 LM 15 ms / RTK 9 ms / Headroom 361 ms（checkout `df30334…`，二进制 SHA-256 在文件里）。
   它有诚实的免责声明（"La généralisation hors de ce corpus n'est pas démontrée"、
   "Ce que je n'ai pas pu vérifier"）并逐条列出缺失事实。
   但 fixture 自建、仓库只有 4 star、crates.io 无发布 → **只能作"存在这种失效"的旁证，不能当基准**。
   它逐案数据很有价值：`pytest_fail` RTK 99% 粗暴压缩但 oracle 20%；`git_log` 98%/4%；`json_large` 99%/33%。
7. **squeez 的 "pxpipe 1.91 chars/token"**：仅源码注释引用（`tokens.rs:55`，N=391），未核。
   作为"密集内容约 2 chars/token"的数量级参考可用，但不是可引用的一手测量。
8. **端到端延迟**：本文只有算法级 microbenchmark（Windows x86_64 release），
   没有端到端（hook 启动 + 进程 spawn + 写原文 + 上游往返）延迟。
9. **`compact_path` 的实际收益**：未单独测量（`fold_path_prefix` 已测，附录 A.18）。
10. **`\r` 覆盖帧在真实语料上的分布**：轨迹语料只有 33 条命中（0.3%），不足以给"常见/罕见"下结论。

---

## 9. 主要一手来源

**RTK** @ [`e542dca0bdcc262fa035dcf7fc33910cdcc8cdaf`](https://github.com/rtk-ai/rtk/tree/e542dca0bdcc262fa035dcf7fc33910cdcc8cdaf)
- [`Cargo.toml`](https://github.com/rtk-ai/rtk/blob/e542dca0bdcc262fa035dcf7fc33910cdcc8cdaf/Cargo.toml)（两 crate 的分工与 BUSL 声明）
- [`engine/Cargo.toml:8`](https://github.com/rtk-ai/rtk/blob/e542dca0bdcc262fa035dcf7fc33910cdcc8cdaf/engine/Cargo.toml#L8)（`license = "BUSL-1.1"`）、
  [`engine/Cargo.toml:15,17`](https://github.com/rtk-ai/rtk/blob/e542dca0bdcc262fa035dcf7fc33910cdcc8cdaf/engine/Cargo.toml#L15-L17)（`serde_json` 只开 `preserve_order`；`toon-format = 0.5.0`）
- [`engine/LICENSE`](https://github.com/rtk-ai/rtk/blob/e542dca0bdcc262fa035dcf7fc33910cdcc8cdaf/engine/LICENSE)（BUSL 全文与 Additional Use Grant）
- [`text/src/lib.rs:18-23,83`](https://github.com/rtk-ai/rtk/blob/e542dca0bdcc262fa035dcf7fc33910cdcc8cdaf/text/src/lib.rs#L18-L23)（`strip_ansi` 正则、`estimate_tokens_from_len`）
- [`engine/src/shape.rs:49-86,113,212,296-301,303`](https://github.com/rtk-ai/rtk/blob/e542dca0bdcc262fa035dcf7fc33910cdcc8cdaf/engine/src/shape.rs#L49-L86)（本地 ANSI 状态机、normalize、extract、阈值、classify）
- [`engine/src/strategy.rs:8-28,213,338-352,357,415`](https://github.com/rtk-ai/rtk/blob/e542dca0bdcc262fa035dcf7fc33910cdcc8cdaf/engine/src/strategy.rs#L357)（常量、JsonSkeleton、`to_slines`、`rle_collapse`、`render`）
- [`engine/src/chain.rs:21`](https://github.com/rtk-ai/rtk/blob/e542dca0bdcc262fa035dcf7fc33910cdcc8cdaf/engine/src/chain.rs#L21)（`never_worse`）
- [`engine/src/methods/mod.rs:40-62`](https://github.com/rtk-ai/rtk/blob/e542dca0bdcc262fa035dcf7fc33910cdcc8cdaf/engine/src/methods/mod.rs#L40-L62)（五条链）
- [`engine/src/methods/grep_group.rs:25-27,38-50,57-70`](https://github.com/rtk-ai/rtk/blob/e542dca0bdcc262fa035dcf7fc33910cdcc8cdaf/engine/src/methods/grep_group.rs#L25-L70)
- [`engine/src/methods/toon.rs:20-27,32-56,57-75`](https://github.com/rtk-ai/rtk/blob/e542dca0bdcc262fa035dcf7fc33910cdcc8cdaf/engine/src/methods/toon.rs#L20-L75)
- [`engine/src/methods/minify_json.rs:1,28`](https://github.com/rtk-ai/rtk/blob/e542dca0bdcc262fa035dcf7fc33910cdcc8cdaf/engine/src/methods/minify_json.rs#L1)（文档 vs `lossy: true` 的不自洽）
- [`engine/src/methods/cap.rs:12-15`](https://github.com/rtk-ai/rtk/blob/e542dca0bdcc262fa035dcf7fc33910cdcc8cdaf/engine/src/methods/cap.rs#L12-L15)、
  [`progress.rs`](https://github.com/rtk-ai/rtk/blob/e542dca0bdcc262fa035dcf7fc33910cdcc8cdaf/engine/src/methods/progress.rs)（含 `test_crlf_normalized_to_lf`）、
  [`dedup.rs`](https://github.com/rtk-ai/rtk/blob/e542dca0bdcc262fa035dcf7fc33910cdcc8cdaf/engine/src/methods/dedup.rs)、
  [`rle.rs:12-21`](https://github.com/rtk-ai/rtk/blob/e542dca0bdcc262fa035dcf7fc33910cdcc8cdaf/engine/src/methods/rle.rs#L12-L21)、
  [`strip_line_numbers.rs`](https://github.com/rtk-ai/rtk/blob/e542dca0bdcc262fa035dcf7fc33910cdcc8cdaf/engine/src/methods/strip_line_numbers.rs)、
  [`truncate.rs:1-5`](https://github.com/rtk-ai/rtk/blob/e542dca0bdcc262fa035dcf7fc33910cdcc8cdaf/engine/src/methods/truncate.rs#L1-L5)
- [`engine/builtin/default.toml`](https://github.com/rtk-ai/rtk/tree/e542dca0bdcc262fa035dcf7fc33910cdcc8cdaf/engine/builtin)（含 posthook/proxy/terse/verbatim-diffs/delegate-reads）
- [`src/core/config.rs:223-247`](https://github.com/rtk-ai/rtk/blob/e542dca0bdcc262fa035dcf7fc33910cdcc8cdaf/src/core/config.rs#L223-L247)（`grep_max_per_file: 25`）
- [`src/cmds/system/search.rs:633,845,899-902,1178`](https://github.com/rtk-ai/rtk/blob/e542dca0bdcc262fa035dcf7fc33910cdcc8cdaf/src/cmds/system/search.rs#L633)（`fold_path_prefix`、`grep_max_per_file` 使用、`compact_path`）
- [`src/core/filter.rs:167,540,733`](https://github.com/rtk-ai/rtk/blob/e542dca0bdcc262fa035dcf7fc33910cdcc8cdaf/src/core/filter.rs#L167)（三引号状态机 + 10 个相关测试；`#2385` 块注释 + 5 个测试）
- [`src/core/guard.rs:21`](https://github.com/rtk-ai/rtk/blob/e542dca0bdcc262fa035dcf7fc33910cdcc8cdaf/src/core/guard.rs#L21)、
  [`proxy/src/tokenizer.rs`](https://github.com/rtk-ai/rtk/blob/e542dca0bdcc262fa035dcf7fc33910cdcc8cdaf/proxy/src/tokenizer.rs)（o200k_base + 回退）
- [`proxy/src/pipeline/ccr/mod.rs:69,94,116,130,342`](https://github.com/rtk-ai/rtk/blob/e542dca0bdcc262fa035dcf7fc33910cdcc8cdaf/proxy/src/pipeline/ccr/mod.rs#L69)（阈值分档、`read_min_tokens = usize::MAX`、`strategy_mode`）
- [`docs/guide/pipelines/methods.md`](https://github.com/rtk-ai/rtk/blob/e542dca0bdcc262fa035dcf7fc33910cdcc8cdaf/docs/guide/pipelines/methods.md)（方法表与链定义）
- [`docs/guide/resources/editions.md:66-77`](https://github.com/rtk-ai/rtk/blob/e542dca0bdcc262fa035dcf7fc33910cdcc8cdaf/docs/guide/resources/editions.md#L66-L77)（`--no-default-features` 与 grant 边界）

**squeez** @ [`b8d46531efa550f41d6f4057af125fcf4c812bbc`](https://github.com/claudioemmanuel/squeez/tree/b8d46531efa550f41d6f4057af125fcf4c812bbc)
- [`src/strategies/log_template.rs:25,56,71,98,116,151`](https://github.com/claudioemmanuel/squeez/blob/b8d46531efa550f41d6f4057af125fcf4c812bbc/src/strategies/log_template.rs#L98)（`apply`/`normalize`/`templatize`/`looks_like_id`/`is_num_unit`）
- [`src/tokens.rs:28-52,55-95`](https://github.com/claudioemmanuel/squeez/blob/b8d46531efa550f41d6f4057af125fcf4c812bbc/src/tokens.rs)（`estimate`、`classify`、pxpipe 注释）
- [`src/commands/wrap.rs:717,740,1078,1093`](https://github.com/claudioemmanuel/squeez/blob/b8d46531efa550f41d6f4057af125fcf4c812bbc/src/commands/wrap.rs#L717)（marker 成本进净收益、`is_net_loss`、marker 文本）
- [`src/config.rs:209,353`](https://github.com/claudioemmanuel/squeez/blob/b8d46531efa550f41d6f4057af125fcf4c812bbc/src/config.rs#L353)（`net_win_min_tokens: 24`）
- [`src/strategies/dedup.rs`](https://github.com/claudioemmanuel/squeez/blob/b8d46531efa550f41d6f4057af125fcf4c812bbc/src/strategies/dedup.rs)（跨距离折叠）、
  [`src/strategies/smart_filter.rs:19-36`](https://github.com/claudioemmanuel/squeez/blob/b8d46531efa550f41d6f4057af125fcf4c812bbc/src/strategies/smart_filter.rs#L19-L36)（CSI-only strip）
- [`src/economy/preservation.rs:25,27,56`](https://github.com/claudioemmanuel/squeez/blob/b8d46531efa550f41d6f4057af125fcf4c812bbc/src/economy/preservation.rs#L56)、
  [`src/context/factsheet.rs:14,19,144`](https://github.com/claudioemmanuel/squeez/blob/b8d46531efa550f41d6f4057af125fcf4c812bbc/src/context/factsheet.rs#L14)

**lean-ctx** @ [`dc41bb19982e5bd93a2c0862702bfd3a0bcea169`](https://github.com/yvgude/lean-ctx/tree/dc41bb19982e5bd93a2c0862702bfd3a0bcea169)
- [`rust/src/core/json_crush.rs:1-16,99-127,150,236-268,313-337`](https://github.com/yvgude/lean-ctx/blob/dc41bb19982e5bd93a2c0862702bfd3a0bcea169/rust/src/core/json_crush.rs#L236-L268)（`_defaults` 提升、`_dropped`、2× 门）
- [`rust/src/core/signatures_ts/ast_prune.rs:39-80`](https://github.com/yvgude/lean-ctx/blob/dc41bb19982e5bd93a2c0862702bfd3a0bcea169/rust/src/core/signatures_ts/ast_prune.rs#L39-L80)
- [`rust/src/core/compressor.rs:13-33,45-95`](https://github.com/yvgude/lean-ctx/blob/dc41bb19982e5bd93a2c0862702bfd3a0bcea169/rust/src/core/compressor.rs#L13-L33)（strip_ansi、`aggressive_compress` 分派）
- [`rust/src/core/auto_mode_resolver.rs:133-290`](https://github.com/yvgude/lean-ctx/blob/dc41bb19982e5bd93a2c0862702bfd3a0bcea169/rust/src/core/auto_mode_resolver.rs#L133)（`auto` 决策链）
- [`rust/src/core/edit_quality.rs:31-45,415`](https://github.com/yvgude/lean-ctx/blob/dc41bb19982e5bd93a2c0862702bfd3a0bcea169/rust/src/core/edit_quality.rs#L31-L45)（编辑失败率回退阈值）
- [`LICENSE_MATRIX.toml`](https://github.com/yvgude/lean-ctx/blob/dc41bb19982e5bd93a2c0862702bfd3a0bcea169/LICENSE_MATRIX.toml)（分域）

**LM Resizer** @ [`c3007c25197a98a359e5304c2d28949968d39247`](https://github.com/phuetz/lm-resizer/tree/c3007c25197a98a359e5304c2d28949968d39247)
- [`crates/lm-resizer-core/src/transforms/diagnostic_gate.rs`](https://github.com/phuetz/lm-resizer/blob/c3007c25197a98a359e5304c2d28949968d39247/crates/lm-resizer-core/src/transforms/diagnostic_gate.rs)（失败行追回门）
- [`crates/lm-resizer-core/src/transforms/source_compressor.rs:151-165,304,413,461,705,952`](https://github.com/phuetz/lm-resizer/blob/c3007c25197a98a359e5304c2d28949968d39247/crates/lm-resizer-core/src/transforms/source_compressor.rs#L304)
- [`bench/RAPPORT.md`](https://github.com/phuetz/lm-resizer/blob/c3007c25197a98a359e5304c2d28949968d39247/bench/RAPPORT.md)（22 fixture 自报基准，含免责声明与逐案缺失事实）

**TOON** @ [`d09c3d33e37389345a74bd96981eb96dd9be6c5e`](https://github.com/toon-format/spec/blob/d09c3d33e37389345a74bd96981eb96dd9be6c5e/SPEC.md)
- [`SPEC.md:92-108`](https://github.com/toon-format/spec/blob/d09c3d33e37389345a74bd96981eb96dd9be6c5e/SPEC.md#L92-L108)（适用性与非目标）、`§9.3` tabular、`§9.5` keyed tabular、`§17` 媒体类型
- 实现 [`toon-format/toon-rust` @ `a1172f14`](https://github.com/toon-format/toon-rust/tree/a1172f1489efc3b5571f475c06847874fb1a37a9)（`toon-format 0.7.0`，MIT）

**Drain**：He, Zhu, He, Lyu, *Drain: An Online Log Parsing Approach with Fixed Depth Tree*, ICWS 2017
（固定深度树、`simSeq`、`O((d+cm)n)`、`depth`/`st` 取值）；
实现 [`akshatagarwl/drain3` @ `b04f7e53`](https://github.com/akshatagarwl/drain3/tree/b04f7e53024f84f243d74ac4765c9396b40f1240)（0.3.0，Apache-2.0）

**CLP**（仅用于区分 L0/L1）：Rodrigues et al., *CLP: Efficient and Scalable Search on Compressed Text Logs*, OSDI 2021

**包许可与版本**：crates.io API，2026-10-19 查询，清单见 §5.4。

---

## 附录 A：探针与可复现

所有探针建在 `%TEMP%`（= `C:/Users/wudon/AppData/Local/Temp`，本文简称 `/tmp`），**不入库**。
`rtk-engine` 是 BUSL-1.1，只作研究性本地链接，**产品不得链接或复制其代码**。

**环境**：Windows x86_64、rustc/cargo 1.98.1、Python 3.14.3、`tiktoken 0.14.0`（`cl100k_base`）、
`toon-format 0.7.0`、`drain3 0.3.0`、`serde_json 1.0.154`。日期 2026-10-18/19。

A.1 **Python / grep 分组**：`/tmp/sc2/m1.py`（合成 2000/120/43/39/1/含 `-C` 六种）、
`/tmp/sc2/m2.py` + `m5.py`（RTK 真实 fixture 路径长 11 / 14 字符 + 41 字符路径对照）、
`/tmp/m10.py`（忠实复刻 RTK gate 跑真实 `corpus/grep.txt`）、`/tmp/m19.py`（trim/空行对照）。
语料：`/tmp/corpus/{grep,find,ls,json,traceback,gitlog,install}.txt`、`mcp.json`、`bench.py`、`failing.py`。

A.2 **rtk-engine 分类与渲染**：`/tmp/engprobe`（`src/main.rs`、`src/bin/shapeprobe.rs`、`src/bin/renderprobe.rs`），
链接 `/tmp/ccresearch/rtk-ai_rtk/engine`。`render()` 用 `floor = 512`、`count = bytes/4`。
输出写 `/tmp/rendered/<stem>.rendered.txt`，保留率统计 `/tmp/m18.py`。

A.3 **TOON**：`C:/tmp/sc2/toon/<case>.{pretty.json,min.json,toon}` 由 `/tmp/probe/src/bin/toon_dump.rs` 生成
（注意：探针里写死的是 `/tmp/sc2/toon`，在 `C:/tmp` 下；Python 计数脚本 `m21.py`/`m22.py` 也指向 `C:/tmp/sc2/toon`）；
计数 `/tmp/m21.py`（8 种形状，字符/token 双栏）。真实语料 `/tmp/toonrt/src/bin/real.rs` → 计数 `/tmp/m22.py`。
版本对照 `/tmp/toon05`（`toon-format =0.5.0`，RTK 所用版本）。round-trip `/tmp/toonrt/src/main.rs`。

A.4 **JSON**：`/tmp/aptest`（`arbitrary_precision + preserve_order`）、
`/tmp/aptest2`（默认 feature，用来出"默认"那四行的对照值）；
`/tmp/jc`（编译 lean-ctx `json_crush.rs`，测 `crush_lossless` 的 reconstruct 与 `crush_lossy` 的 `_dropped`）。

A.5 **squeez 模板折叠**：`/tmp/sqtest`（真实语料批量）、`/tmp/sq2`（形状对照 A–J6，
`cargo run --release` 输出字符口径）、`/tmp/m27.py`（真实语料 token 计数）、
`/tmp/m28.py`（`ls` 文件名丢失统计）。前两个以
`#[path = "…/squeez/src/strategies/log_template.rs"]` 直接编译源码（不改源码、不 vendor）。

A.6 **token 估算偏差**：`/tmp/m31.py`（`chars/4`、`estimate`、`estimate_classed` 三栏 + 分类 + 24-unit 换算）。

A.7 **真实 ANSI 语料**：`/tmp/ansi/cargo_err_color.txt`（`cargo build --color always`，一个 60 函数的
类型错误 crate，66392 字符）、`/tmp/ansi/gitlog_color.txt`（`git log --color=always`，55785 字符）；
计数 `/tmp/m25.py`（完整 stripper vs `rtk-text` 正则）。

A.8 **strip_ansi 四路对照**：`/tmp/ansicmp`（`rtk-text` 正则、`shape.rs` 状态机、squeez、lean-ctx 四者并排，
10 种输入；后两者按源码逐字转写，因为直接 `#[path]` 会拖入无关依赖）。

A.9 **Drain**：`/tmp/probe/src/bin/drain_probe.rs`（训练/匹配吞吐、空行炸弹、模板打印）。

A.10 **标记与句柄成本**：`/tmp/m20.py`（squeez marker、`[xN]`、`elided`）；
`/tmp/m32.py` 逐条打印 §5.3 所有字面量，并对 8/12/16/24 字符的 hex 与十进制各采样 30 个随机串（`random.seed(20261019)`）。

A.11 **真实 agent 轨迹**：`/tmp/jb/trajectories/lindenbauer/main_experiments/local-Qwen3-32B*/*/*.traj`
（500 条 SWE-agent 轨迹）。`/tmp/m12.py`（规模）、`m13.py`（工具直方图）、`m14.py`（ANSI/CR 净收益）、
`m15.py`（`cap(50,200)` 触发率与信号保留）、`m16.py`（open→edit 序列）、`m17.py`（`old_string` 命中率）。

A.12 **幂等性与吞吐**：把 `/tmp/rendered/<stem>.rendered.txt` 再送一次
`cargo run --release --bin renderprobe -- … C:/tmp/rendered_tmp`（默认输出目录是 `%TEMP%/rendered`），
然后用 `tiktoken` 对比两遍 token。
分类/模板/分组/TOON 的 per-op 耗时取自 `/tmp/probe/src/bin/proto.rs`（`cargo run --release --bin proto`）。

A.13 **JSON 骨架/投影与 cap 对照**：`/tmp/meas6.py`（40 项响应：pretty 3824 → minified 2253 → skeleton 28 tok；
投影 3/5 字段 2253 → 1523 tok；另含一份 2845 行合成构建日志：错误在 index 1400，
`cap(50,200)` **丢掉 `E0432`**、保住 `87 passed`，37613→3255 tok）、`/tmp/meas5.py`（360 行构建日志：
`head50/tail8` 丢掉 `error[E0432]` 与 `error[E0308]`、保住 `87 passed` 与失败用例名；
`cap(50,200)` 因 200 行尾巴盖住错误位置而两者都保住）。

A.14 **Kneedle 预算选择器**：`/tmp/ask`（`#[path]` 直接编译 lm-resizer 的
`crates/lm-resizer-core/src/transforms/adaptive_sizer.rs`，依赖 `flate2 1` + `md-5 0.10`）。
`cargo run --release -- <corpus> [bias]`；bias 默认 1.0，`min_k = 10`，`max_k = n`。
第二个“按信号密度重排”的对照在同一 `main.rs` 里（信号词元定义为含
error/fail/panic/assert/exception/warn 或 `file:line` 形态的空白分隔词）。

A.15 **grep 分组收益 vs 路径长度/形态**：`/tmp/m34.py`（43 命中 × 同一正文，只换路径，
实测 4/6/10/11/17/41/60 字符七档 → 2/2/4/3/5/12/17 tok；注意 10 字符的 `src/hay.rs` 比 11 字符的
`haystack.rs` 贵一 token，因为 `/` 自己占 token）
+ `/tmp/m35.py`（真实 fixture 正文 × 11 字符 vs 41 字符路径，用来把“正文短”与“路径短”两个因素分开）。
复刻 `grep_group.rs` 的 gate（`splitn(3,':')` + 第二段 `parse::<usize>()`）与输出格式。
`C:/…` 与 `C:\…` 两种形态触发 gate 失败 → 原文放行。

A.16 **`cap(50,200)` 只看触发样本**：`/tmp/m33.py`（与 `m15.py` 同一语料与正则，
但跳过 `cap(o) is o` 的未触发样本，汇总触发样本的 token 与 error/`path:line` 计数）。
→ 29 条触发样本：545,009 → 90,988 tok（-83.3%），error 保留 32.5%，`path:line` 保留 5.5%。

A.17 **`dedup` 的直接触发/幂等/标记碰撞**：`/tmp/engprobe`（`src/bin/idem.rs`、`idem2.rs`、`idem3.rs`），
直接调 `rtk_engine::methods::dedup::Dedup::apply`（绕过 `chain::run_chain` 的 `never_worse`），
`Block{subject: ToolResult, format: Term}`。
- 8 份真实语料：`p1 == input` 全部为真 → 0 次触发。
- `[x9]`×4 → `[x9]`+`[x4]`（12→6 tok）；`A A A [x3] [x3] [x3]` → 15→11 tok 且 `p1 != p2`；
  `ok`/`[x40]`/`done` → 7→7 tok。
- token 由 `idem3` 写盘 + `tiktoken cl100k_base` 计数（与前文同环境）。

A.18 **`fold_path_prefix` 收益**：`/tmp/m37.py`，逐行复刻 `search.rs:633-678`（含 `\0`/`\r`/`\x1b` 拒绝、
<2 行、空行、整组件切、`prefix.len() <= 1` 拒），对本地语料跑：
`find.txt` 1709 行 → `-17.9% ch / -18.4% tok`；`ls.txt` 公共前缀只有 `/` → `None`；
`grep.txt` 是 `path:line:text`，真实调用路径上根本不会进 `is_bare_file_list`，此处只作为“强拆后残余冗余”参考。
注意：脚本不含 `never_worse`（`search.rs:588`），因为折叠只在真正有共享前缀时发生且必然变小。

**复现注意**
- `chars` 一律按 Unicode 字符数（Rust `chars().count()`），`bytes` 明写。
- `tiktoken` 的 `cl100k_base` 与 Claude 实际 tokenizer 无关（§8-1）。
- 所有 RTK 数值取自提交 `e542dca0`；`toon-format` 数值取自 `0.7.0`，与 RTK 钉的 `0.5.0` 交叉验证了两个关键用例。
- 探针都不入库：仓库里只有本文。探针源码留在 `%TEMP%`（少数输出落在 `C:/tmp`，见 A.3/A.12），
  节点（文件 + 函数 + 参数）在 A.1–A.18 里可定位；
  依赖版本见 §5.4 与本节环境行。
- 本轮对每个引用过的脚本都重跑了一次以确认还能复现（`sc2/m1.py`、`m10.py`、`m14.py`、`m21.py`、`m31.py`、
  `m34.py`、`m35.py`、`meas5.py`、`meas6.py`），数字与正文一致。
- 时间是本机单次测量（无重复试验、无 CI），只用于"数量级"判断；`drain_probe` 与 `renderprobe`
  的两次运行出现过 2 倍差异，凡此类都写成区间。
