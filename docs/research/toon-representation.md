# TOON 作为入口表示：可省多少 token、在哪些边界失真

> 研究票：WodenJay/Outrim#5（辅助材料，不代替决策）。
> 起点：`docs/research/structural-compression.md`（下称"结构化报告"）。本文**不重做**竞品调查，只补齐 TOON 这一条线的可核实证据与边界。
> 前提已定（来自 #5 的阶段确认）：只做**入口**、不改写历史、默认只用当前输出 + host 提供的工具名/参数/状态；不预设压缩强度与指标权重。

**证据约定**

- 计数一律用 **`tiktoken 0.14.0` + `cl100k_base` 真实 BPE**；同时给 `o200k_base` 复核。字符数会明写。
- TOON 一律由 **`toon-format 0.7.0`（`default-features = false`，默认 `encode_default`：逗号分隔、2 空格缩进）** 产生。
- 环境：Windows x86_64、rustc/cargo 1.98.1、Python 3.14.3。
- 标注：**[事实]** 有一手来源或可复现探针；**[建议]** 我的判断；**[未验证]** 没有做完追溯，不能当结论用。
- **本文没有做任何模型理解测试**。凡涉及"模型是否还看得懂"的地方，一律是 [未验证]。

---

## 0. 结论摘要

1. **在我测过的 20 个形状（含重复记录案例）+ 3 个真实/半真实语料文件上，TOON 是否省 token 主要由“键名总长 / 值总长”决定，而不是“JSON 是否写得紧凑”。** 键名远长于值且行数多时明显省（0.202–0.62）；键短、或数组不同构、或出现数组列时变贵（1.01–2.00）。**这是这组样本上的观察，不是“只有某类数据才收益”的普遍结论。** **[事实]**（§4 表）
2. **官方自己公布的 token 效率里，TOON 对紧凑 JSON 是负收益。** 官方 benchmark 的 Mixed-Structure Track 汇总：TOON 264,734 tok，紧凑 JSON 260,451 tok，即 **TOON 多用 1.6%**；逐数据集里 3/5 是正数（+5.6%、+19.9%、+6.7%）。**[事实]**（§2）
3. **最小值基线不是 JSON，是 CSV/TSV 或"字段清单 + 行数组"。** 500 行同构表：紧凑 JSON 10,502 tok、扁平 CSV 5,505 tok、TOON 6,509 tok。**TOON 比 CSV 多 18%**，而这个差值几乎就是"每行 2 空格缩进"的成本。**[事实]**（§5）
4. **可逆性必须分三层说，混合叙述会出错。** ①字节/原文可逆：基本从不成立；②JSON 数据模型等值（规范定义的那种）：通常成立；③语义/信息等价：未验证，且不等价于②。**[事实]** + **[未验证]**（§3）
5. **超范围数字的精度损失发生在"上游 JSON 解析器"这一层，不是 TOON 格式本身。** `serde_json` 默认就把 30 位整数读成 f64；`1e400` 默认直接**解析报错**，不会到 TOON；只有开启 `arbitrary_precision` 时才会被 toon-format 静默转成 `null`。把这条当"TOON 失真"是错的归因。**[事实]**（§3.2）
6. **TOON 不做重复消除。** 500 条完全相同的记录：TOON 8,012 tok，而“一条记录 + 次数”约 29 tok。重复记录该走 dedup/RLE，不是 TOON。**[事实]**（§4.4）
7. **要额外 prompt 吗：一行标签约 17 tok，真语法教程约 70 tok。** 在小样本上教程开销会吃掉全部收益（8 tok/行的省法 ≈ 8.8 行才回本）。**[事实]** 测法见 §6；"加了教程模型是否不降"是 **[未验证]**。
8. **Rust 侧可用，但要主动关掉默认 feature。** `toon-format` 的 `default = ["cli"]` 会拉 clap / ratatui / crossterm / arboard / syntect / tiktoken-rs 等 TUI 依赖；`default-features = false` 后运行时依赖只有 serde / serde_json / indexmap / thiserror。**[事实]**（§8）

---

## 1. 版本与一手来源

| 项 | 值 | 来源 |
|---|---|---|
| 规范 | **4.4**，日期 2026-10-08，状态 **Working Draft**，作者 Johann Schopplich，MIT | <https://github.com/toon-format/spec/blob/main/SPEC.md> |
| 官方 Rust 实现 | `toon-format` **0.7.0**（2026-10-12），仓库 `toon-format/toon-rust`，HEAD `a1172f1489efc3b5571f475c06847874fb1a37a9` | <https://github.com/toon-format/toon-rust>、<https://crates.io/crates/toon-format> |
| 官方生态页给 Rust 标注的规范版本 | **4.1**（0.7.0 的 CHANGELOG 写着 "support spec v4.4"） | <https://toonformat.dev/ecosystem/implementations.html> |
| 官方 benchmark | 244 题 × 6 格式 × 4 模型，计数用 `gpt-tokenizer` **o200k_base** | <https://toonformat.dev/guide/benchmarks.html> |

**[事实]** 生态页与 crate 自述的规范版本不一致（4.1 vs 4.4）。crate 的 CHANGELOG `0.7.0` 条目为 "`[breaking]` support spec v4.4"，`src/lib.rs` 文档头写 `toon-spec: 4.4`。**我不知道生态页是没更新还是有意为保守口径** —— 这条是 [未验证]，不要用它否定 crate 的合规性。

**[事实]** 规范正文本身**没有**任何 token 节省承诺；节省声明只出现在官网与 benchmark 页。规范只承诺"deterministic representation of structured data"。

---

## 2. 官方自己的减法（不当产品收益引用）

**[事实]** `/guide/benchmarks.html` 的 Token Efficiency 汇总（o200k_base，计数范围是整份数据）：

| 格式 | Mixed-Structure Track 总量 | TOON 相对它 |
|---|---:|---|
| JSON（pretty, 2 空格） | 393,637 | −32.7% |
| **JSON（compact）** | **260,451** | **+1.6%（TOON 更贵）** |
| YAML | 314,140 | −15.7% |
| XML | 447,936 | −40.9% |
| TOON | 264,734 | — |

**[事实]** 逐数据集对紧凑 JSON：e-commerce 嵌套 +5.6%、semi-uniform 事件日志 **+19.9%**、深嵌套配置 +6.7%、feature flags −32.8%、contacts −42.9%。

**[事实]** Flat-Only Track 总量：CSV 64,247、TOON 68,030，**TOON 比 CSV 多 5.9%**。

**[事实]** 首页的 "72.2% vs 71.4% 准确率、少 42.6% token"，基线是 **2 空格缩进的 JSON**，不是紧凑 JSON。同一页的 Flat-Only 分表里，紧凑 JSON 是 58.0%（253/436，2,718 tok），TOON 63.1%（275/436，1,994 tok） —— 也就是说在这套数据上 TOON 的准确率高于紧凑 JSON，但**该页自己也写明各格式的 Wilson 置信区间重叠，差异不构成统计显著**。**[未验证]** 我没有复核其统计代码。

> **口径提醒**：−42.6% 是"对 pretty JSON"的比较，作为 Outrim 的产品收益主张不成立；Outrim 的默认输入是工具原始文本，压缩基线应是**紧凑 JSON / CSV / 字段+行数组**。

---

## 3. 保真边界：管线、分层与实测

### 3.1 管线（必须分开讲的四段）

| 段 | 组件 | 对数字做了什么 |
|---|---|---|
| ① 上游 JSON 解析 | `serde_json`（默认 feature） | 整数落在 i64/u64 → 精确；否则读成 `f64`；超出 f64 范围 → **报错**（`number out of range`）。**这一步就可能已经改值。** |
| ② toon-format 的宿主模型 | `src/types/value.rs` 的 `Number { PosInt(u64), NegInt(i64), Float(f64) }`；`From<serde_json::Value>` 依次 `as_i64 → as_u64 → as_f64`，全失败则 `Null` | 没有任意精度数；超域值只能落到 f64 或 null |
| ③ 编码 | `src/utils/number.rs` `format_f64_canonical`：整值 f64 → 整数；否则用 Rust `f64` 的 `Display`（**永不使用指数**）再去尾零 | `1e6`→`1000000`、`1.0`→`1`、`-0.0`→`0`、`1e308`→309 位十进制 |
| ④ 解码 | 规范 §4：token 匹配 JSON number 文法才当数字；整数 token 在 i64/u64 内 → 精确整数；超出则"是该 f64 的规范拼写时→f64，否则→字符串" | 见 §3.3 |

**[事实]** 规范 §2 允许两种超域策略（输出无损十进制字符串，或输出会丢精度的近似数），并要求实现文档化；§2 还写 "Encoders SHOULD expose an option for lossless stringification of out-of-domain numbers"。
**[事实]** `toon-format 0.7.0` **没有**这样的选项（`grep -ri lossless src/` 无结果；`EncodeOptions` 只有 `delimiter` / `indent`）。选的是"近似数"分支，符合规范但没提供无损出口。

### 3.2 数字保真实测（`serde_json` 默认 feature）

| 输入字面量 | ①解析后宿主值 | ③TOON 输出 | ④回读 | 原文文本一致 |
|---|---|---|---|:--:|
| `9223372036854775807`（i64::MAX） | 同 | `9223372036854775807` | 同 | ✅ |
| `18446744073709551615`（u64::MAX） | 同 | 同 | 同 | ✅ |
| `123456789012345678901234567890` | **`1.2345678901234568e+29`** | `123456789012345680000000000000` | `1.2345678901234568e+29` | ❌ |
| `-9223372036854775809` | **`-9.223372036854776e+18`** | `-9223372036854775808` | `-9223372036854775808` | ❌ |
| `1.0` / `2.50` / `1e6` / `-0.0` / `1E+03` | 规范化 | `1` / `2.5` / `1000000` / `0` / `1000` | 同 | ❌ |
| `1e-7` | 同 | `0.0000001` | `1e-7` | ❌ |
| `1e308` | 同 | **309 位十进制** | `1e+308` | ❌ |
| **`1e400`** | **①直接报错，进不了 TOON** | — | — | — |

**[事实]** 关键分界：**超范围整数的精度是在 ①（上游解析器）丢的，不是在 TOON 丢的**。回读值与 ①解析后的宿主值一致（按数学值比较），所以"TOON 把 30 位整数弄坏了"是错误归因；正确的说法是"`serde_json` 默认不保留超 u64 整数，TOON 继承了这个既有损失，并把它变成 30 位十进制字面量"。
**[事实]** 我另做了一次 `serde_json` 开 `arbitrary_precision` 的对照：此时 30 位整数能被解析器保住，但 toon-format 的 `From<serde_json::Value>` 仍只走 `as_i64/as_u64/as_f64`，依旧降到 f64。并且在这个配置下 `1e400` 不再报错，而是被 toon-format 静默转成 **`null`**（`From` 的 fallback 分支）。**这是一条"实现 × 上游配置"的交互事实，不是格式本身的规定。** **[事实]**

> **修正说明**：我在过程中曾写过"`1e400`→null"作为格式结论，那是只在 `arbitrary_precision` 配置下成立的现象。默认配置下 `1e400` 在解析阶段就被拒绝。本文以此表为准。

### 3.3 三层可逆（这是本文最需要 Outrim 采纳的一条）

| 层 | 定义 | TOON 是否成立 | 证据 |
|---|---|---|:--|
| L1 字节/原文可逆 | `toon` 文本能还原出**逐字节相同**的输入 JSON 文本 | **基本否** | 所有数字字面量、tabular 键序、转义形式都会变（§3.2、§3.4） |
| L2 JSON 数据模型等值 | 规范 §2 定义：数字按**数学值**比较（`-0 == 0`）；对象按有序键序列比较，**但 tabular / keyed tabular 形态按表头字段序重排** | **通常是**；前提是 ① 没丢 | `value_equal` 探针；注意该探针用的是 `serde_json::Value` 的 `==`，比规范定义更严（见下） |
| L3 语义/信息等价 | 人类/模型读到的信息不变 | **[未验证]** | 本文未做模型测试；见 §2 的第三方结论 |

**[事实]** L1 与 L2 的差距不是边角：`[{"a":1,"b":2},{"b":3,"a":4}]` 编码为 `[2]{a,b}:` 后回读成 `[{"a":1,"b":2},{"a":4,"b":3}]`。按规范 §2 的等值定义这**算相等**（tabular 重排是明写的例外），但按 `serde_json::Value` 的 `==`（有序 map）/字节比较则**不相等**。所以"lossless round-trip"这句话的含义完全取决于用哪个比较器。
**[建议]** Outrim 在文档与断言里必须写清用的是哪个比较器；结构化报告 §5.1 提的四个可逆级别（byte / value / semantic / info）在这里正好用得上，TOON 只能主张到 value 级。

### 3.4 字符串与转义（实测无失真）

**[事实]** 下列都能逐字节往返：像结构的普通字符串（`"007"`、`"true"`、`"null"`、`""`、`" x "`）、含分隔符（`"a,b"`）、含冒号/井号/前导横线（`"k: v"`、`"#c"`、`"-"`）、含换行（`"line\nbreak"`）、字面量 `"[]"`。引号规则按 §7.2 生效，decoder 也接受非规范键 token（§7.4）。
**[事实]** `\u00e9` → 字面 UTF-8 `é`（文本不同，值相同）。**[事实]** 孤立代理项（`\ud800`）在 toon-format 里**报错**而不是替换成 U+FFFD —— 与规范 §3 一致，且比很多实现严格。
**[事实]** 重复键 `{"dup":1,"dup":2}` 在 ① `serde_json` 就已经塌成 `{"dup":2}`；手写 TOON 里的重复键由严格解码报 `Duplicate sibling key`。

---

## 4. 哪些 JSON 形状适合 / 不适合（实测）

全部为 `cl100k_base`；`ratio = TOON tok / 紧凑 JSON tok`；`<1` 才省。

**这张表的边界**：样本是脚本生成的（字段名与值的形态见表左列），不是 Outrim 的真实输出分布；约 20 个合成形状 + `corpus/json.txt` / `corpus/mcp.json` / `corpus/bench.py` 三个真实/半真实文件。它足以说明“机制上为什么会省/会贵”，**不足以推出“某类内容一定收益”的分布结论**。所有案例都是合法 TOON 输出、规范合规；差别只在 token 数。

**关于等值**：下表除“原始数字写法”一节（§3.2）外的所有合成案例，回读后 `serde_json::Value ==` 成立（包括降级到 list form 的异构/嵌套案例）。也就是说：**变贵不变错**。

### 4.1 省（ratio < 1）

| 形状 | 紧凑 JSON tok | TOON tok | ratio |
|---|---:|---:|---:|
| 100 行 × 15 个超长键 | 16,502 | 3,338 | **0.202** |
| 10 行 × 20 个长键 | 2,202 | 603 | **0.274** |
| 30 行 × 8 个短键 | 1,202 | 560 | 0.466 |
| 500 行 × 4 字段（id/name/email/active） | 10,502 | 6,509 | 0.620 |
| 120 行 + 外层 meta（含嵌套 user.login） | 3,017 | 1,828 | 0.606 |
| 100 条订单号/金额**以字符串给出** | 1,703 | 1,210 | 0.711 |
| 源码按行拆成记录（file/line/text） | 2,293 | 1,781 | 0.777 |

### 4.2 变贵（ratio > 1）

| 形状 | 紧凑 JSON tok | TOON tok | ratio |
|---|---:|---:|---:|
| 40 行 GitHub issue 风格（嵌套 labels/user） | 1,282 | 1,682 | **1.312** |
| 200 行半同构（40% 多一个嵌套字段） | 3,922 | 5,002 | **1.275** |
| 深嵌套配置单对象（无数组） | 55 | 70 | 1.273 |
| 4 行稀疏异构（每行键不同） | 18 | 30 | 1.667 |
| 10 行带数组列（`vals: [..]`） | 132 | 192 | 1.455 |
| 纯数组的数组 `[[1,2,3],...]` | 19 | 38 | **2.000** |
| 单个 30 键扁平对象 | 181 | 209 | 1.155 |
| 100 个原始数（根数组） | 201 | 203 | 1.010 |

### 4.3 基本持平（0.99–1.00，转换无意义）

| 形状 | 紧凑 JSON tok | TOON tok | ratio |
|---|---:|---:|---:|
| 10 行 × 1200 字符长文本 | 2,592 | 2,567 | 0.990 |
| 一整份源码塞进一个字符串字段 | 3,031 | 3,030 | 1.000 |

**[事实]** “长文本字段”与“源码塞进字符串”两行说明一件事：**TOON 只能压缩结构（重复键名），压不动内容**。它不碰引号内的字节，也不做任何内容变换。

**[事实]** 真实语料复核（与结构化报告同源文件）：`corpus/json.txt`（400 行 service 数组，含 `tags` 数组列）紧凑 12,961 → TOON 16,502（**1.273**），pretty 24,160；`corpus/mcp.json` 紧凑 30,101 → TOON 32,219（**1.070**），pretty 38,365。`o200k_base` 复核：1.235 / 1.056 —— 方向一致，幅度小一点。
**[事实]** 例外：`corpus/bench.py` 按行拆成记录后 0.777（省），而把整份源码当**一个字符串字段**则 1.000（不省）。同一个文件两种形状，结论相反 —— 这也说明“源码”不是一个能整体定性的类别。
**[建议]** 这两个真实 JSON 文件恰好落在变贵侧，说明 Outrim 真实接到的 JSON（MCP 响应、带数组字段的列表）**默认不该假设 TOON 有收益**，必须先判形状。

### 4.4 重复记录

**[事实]** 500 条**完全相同**的记录：紧凑 13,003 tok → TOON 8,012 tok（0.616）。而"一条记录 + 出现次数"约 **29 tok** —— 差两个数量级。
**[建议]** 重复是 dedup / RLE 的活，TOON 不参与；反过来，把"重复记录多"当作选 TOON 的理由是错的。

### 4.5 稀疏 / 异构数组与数组列

**[事实]** 规范 §9.3 的 tabular 检测是强条件（每元素都是对象、键集完全相同、每列都是 uniform-primitive 或 nested-uniform；**任何一列混了 null+对象、或含数组、或含空对象，整个数组降级**到 §9.4 list form）。实测降级后语法正确、L2 等值成立、但 ratio 1.275–2.000。
**[事实]** 官方 `.out-of-scope/encoder-data-rewrites.md` 明确拒绝"为凑 tabular 而补 null / 删字段 / 拆成 base+extras / 缩写键名"，理由是那属于 application 层的数据转换，不是编码器职责。
**[建议]** 想靠“公共字段提取 / 列式铲平”省钱，必须 Outrim 自己在下游做，TOON 不会替你做过。而这一步已经改变数据形状，属于结构化报告里另一条技术线。

### 4.6 `[N]` 与字段清单：是“校验能力”，不是“省空间手段”

**[事实]** 官方 `.out-of-scope/optional-array-length.md` 把数组长度定为不可选项：§6 强制、§13.1 要求编码器写实际长度、§14.1 要求严格解码器在行数不匹配时报错。原文把 `[N]` 称为“a fundamental invariant”，说它支撑“truncation detection”、“structure awareness / structural validation”以及“a big part of TOON's differentiation from 'CSV + indent'”。
**[事实]** 官方 `.out-of-scope/key-folding.md` 记录了键折叠（`a.b.c: 1`）在 v4 被移除的原因，其中一条是 **0.00% token savings on all six of the project's own benchmark datasets**。
**[建议]** 这两条合起来看：TOON 相对 CSV 多出来的东西里，真正“值钱”的那部分（`[N]` 长度声明、字段清单、确定性引号）是**可靠性**，不是体积；而真正能省体积的键短化路线被官方自己测成 0.00%。所以对 Outrim 来说，用 TOON 的理由应当写成“我需要可校验的表格表示”，而不是“我需要更少的 token”。

---

## 5. 最小可比基线（同数据、同计数器）

**[事实]** 500 行同构表（`id/name/email/active`，字符串型 email）：

| 表示 | cl100k tok | 相对紧凑 JSON |
|---|---:|---:|
| 紧凑 JSON | 10,502 | 1.000 |
| **扁平 CSV（RFC4180 引号）** | **5,505** | 0.524 |
| TSV 扁平 | 5,504 | 0.524 |
| JSON 数组的数组（无字段清单） | 6,501 | 0.619 |
| CSV + 2 空格缩进 | 6,505 | 0.619 |
| **TOON（逗号，2 空格）** | **6,509** | **0.620** |

**[事实]** 缩进敏感度：CSV 每行前导 0 / 1 / 2 / 4 空格 → 5,505 / 6,005 / 6,505 / 6,505 tok（≈ 1 tok/行/层，2 与 4 空格在 BPE 下合并）。TOON `comma1` = 6,009、`comma2` = 6,509、`comma4` = 6,509、`tab2` = 6,509、`pipe2` = 7,512。
→ **TOON ≈ CSV + 缩进**：6,509 与 CSV+2 空格 6,505 只差 4 tok。TOON 相对扁平 CSV 的 18% 溢价，几乎全部是"每行 2 空格"与表头 `[N]{...}` 的代价。

**[事实]** envelope_120（meta + 120 行，嵌套 `user.login`）：紧凑 3,017 / TOON 1,828 / meta JSON + 扁平 CSV 1,580 / meta + CSV+2 空格 1,820。TOON 的收益仍然被"把嵌套字段拍平后写 CSV"吃掉。

**[建议]** 产品讨论时要同时摆这四个基线：紧凑 JSON、CSV/TSV、`{"fields":[...],"rows":[[...]]}`、TOON。只跟 pretty JSON 比会系统性高估 TOON。官方 `/reference/efficiency-formalization.html` 自己也写明"arrays of arrays 是唯一 TOON 不如 JSON 的结构"——但那是**字符级模型 + 受限形状空间**的结论；我的 token 实测显示嵌套/异构/深配置同样变贵（§4.2），两者不矛盾，但前者的适用范围比标题看起来窄。

---

## 6. 需不需要额外格式教程 prompt，以及成本怎么测

**[事实]** 官方 `/guide/llm-prompts.html` 的建议是"show, don't describe"：把数据放进 ` ```toon ` 代码块，附一个 2–5 行小例子，比写段落有效；并给出一句可复用的标签。

**[事实]** 这些字符串的 `cl100k_base` 计数（本文实测）：

| 文本 | 字符 | tok |
|---|---:|---:|
| 一行标签 `Data is in TOON format (2-space indent, arrays show length and fields).` | 71 | **17** |
| 对照：`The following is JSON.` | 22 | 5 |
| 3 行 fenced 例子 | 76 | 37 |
| 语法教程（说明 `key[N]{f1,f2}`、逗号行、2 空格嵌套、引号规则） | 309 | **70** |
| 标签 + 教程 + 例子 | 458 | 124 |

**[事实]** 摊销：`uniform_500` 上 TOON 省 3,993 tok = **8.0 tok/行**。因此 70 tok 的教程 ≈ 8.8 行的收益；17 tok 的一行标签 ≈ 2.1 行。
**[建议]** 摊销门槛必须和**单次输出行数**一起算：几十行以下，教程会把收益吃光甚至净亏；几百行以上才可忽略。这也是"信息基本没变但省 token"在小输出上经常不成立的原因。

**[未验证]** 三个关键问题本文无法回答：
1. 加了一行标签 vs 加了完整教程，模型的**任务质量**是否不同？
2. 提示注入式的教程放在 system 里（一次性）还是随每次 tool_result 重复？（前者单价低但可能被长会话稀释，后者线性付费）
3. 不用任何说明时，模型会不会把 `[100]{a,b}:` 误读成别的结构？

**[事实]** 有一条第三方证据指向"不能假设理解不降"：arXiv 2605.29676《Notation Matters》在 BFCL / MCPToolBenchPP / MCP-Universe / StableToolBench 四个 agentic benchmark、五个 open-weight 模型上测 TOON 与 TRON，摘要原文称 TOON "achieves up to 18% reduction at a similar 9pp accuracy cost, but additionally cascades on multi-turn parsing failures and collapses parallel tool-call output for most models"，代码在 <https://github.com/lkutschka/notation-matters>。**[未验证]** 我没复现该实验，也没核对其 prompt 与基线配置；这里只作为"存在相反方向的第三方信号"记录，不作为结论。

---

## 7. 工具消费者要求 JSON 时的边界

**[事实]** OpenAI 的 function tool：模型返回的 `arguments` 是**JSON 编码的字符串**（`"arguments": "{\"location\":\"Paris, France\"}"`），参数 schema 是 JSON Schema；`strict: true` 才保证严格贴合 schema。自定义工具（`type: "custom"`）才是任意字符串。来源：<https://platform.openai.com/docs/guides/function-calling>

**[建议]** 因此：
- **入站方向**（tool_result 进上下文）：TOON 可用，只要最终送给模型的是文本。
- **出站方向**（模型生成 tool call 参数）：**不能**用 TOON 换取 token —— 消费者是 JSON 解析器。第三方论文里 TOON 在"并行工具调用输出"上的崩坏也集中在这一侧。
- 若下游是"要求 JSON 的 MCP/函数调用消费者"，把 TOON 再解回 JSON 意味着**解码开销 + 二次序列化 + 解析失败面**，净收益要重算。

**[未验证]** Anthropic 侧 `tool_result` 的 text 块是否允许任意文本（我以为允许，但没拿到一手页面引用，故不作为事实）。

---

## 8. Rust 库现状：官方程度、许可证、依赖

| 项 | 值 | 来源 |
|---|---|---|
| 包名 | **`toon-format`**（不是 `toon` / `toon-rust` / `serde_toon_format`） | crates.io |
| 版本 / 日期 | 0.7.0 / 2026-10-12；历史 0.6.1、0.6.0(2026-10-05)、0.5.0(2026-05-22) | <https://crates.io/crates/toon-format> |
| 许可证 | **MIT**（`LICENSE`：Copyright 2025-PRESENT Shreyas S Bhat / Johann Schopplich） | crate |
| 官方程度 | 生态页 Official 表里的 Rust 行；README 自称 "official, spec-compliant Rust implementation" | 生态页 / README |
| 库依赖（关 default） | `serde 1.0.228`(derive)、`serde_json 1.0.145`(+`float_roundtrip`,`preserve_order`)、`indexmap 2.0`、`thiserror 2.0.17` | `Cargo.toml` |
| 传递依赖 | 关掉 `default` 后整棵依赖树（含 `toon-format` 自身、不含探针 crate）＝ 18 个包；除 `toon-format` 外的 17 个传递包为：serde、serde_core、serde_derive、serde_json、indexmap、equivalent、hashbrown、itoa、memchr、foldhash、zmij、thiserror、thiserror-impl、proc-macro2、quote、syn、unicode-ident。许可：多为 MIT OR Apache-2.0，`foldhash` 为 Zlib，`zmij` 为 MIT，`unicode-ident` 为 (MIT OR Apache-2.0) AND Unicode-3.0 | `cargo tree --offline -e normal` |
| **默认 feature 陷阱** | `default = ["cli"]`，`cli` 拉 `clap 4.5` / `anyhow` / `tiktoken-rs 0.12` / `comfy-table 8` / `ratatui 0.29` / `crossterm 0.28` / `tui-textarea 0.7` / `arboard 3.4` / `syntect 5.2` / `unicode-width` / `chrono` | `Cargo.toml:44-58` |
| 其他 feature | `json_stream`（I/O 便利，**不是**有界内存流式；v4.4 的形态选择需要整棵子树）、`layout`（实验性，README 明说 "not part of the TOON specification"） | `Cargo.toml`、README |
| 深度上限 | 编码/解码均 256 层，超限报错 | `src/constants.rs:14`、`src/lib.rs` 文档 |
| 同名混淆 | crates.io 上 `toon`、`toon-rust`、`serde_toon_format`、`tru`、`serde_toon`、`toon-format-rs` 都不是它 | 检索结果 |

**[事实]** `toon-format` 只做传输转换，不做“值不值得转”的判断：`encode_default` 对任何 JSON 都给 TOON 文本（§4.2 的变贵案例都是合法输出）。形状门必须调用方自己做。**结构化报告记录的 RTK 做法（`is_uniform_flat_array` 白名单 + 与 minified 比 `.len()` 字节数）比的是字节不是 token**，因此会放行“字节变小但 token 变大”的案例：本文的 `many_keys_one_obj_30`（30 键扁平对象）就是字符 401→369（0.920）、token 181→209（**1.155**）。**[建议]** 若 Outrim 用门槛，应用真实 tokenizer 或至少用“键长/值长”这类结构特征，而不是字节数。

---

## 9. 与结构化报告的关系

- **不重复的部分**：RTK / lean-ctx 等竞品、许可矩阵、通用算法选型，见 `docs/research/structural-compression.md`。
- **需要更新的一处**：结构化报告 §4.5.4 引用 SPEC v4.4 的数值规范化规则，本文补上了"规则在管线哪一段生效、以及 ①上游解析器先改值"这一层（§3.1–3.2）。
- **可沿用的部分**：§5.1 的四个可逆级别；本文实测确认 TOON 的"lossless"只能主张到 value 级（§3.3）。
- **新增的可比基线**：CSV/TSV 与"字段+行数组"在本文 §5 有与 TOON 同计数器的数字，建议并入结构化报告的基线表。

---

## 10. 供产品讨论的候选（仅建议，不实现）

以下四条是我从上面的证据里能支撑的候选做法，不是推荐结论；每条都写了“凭什么”和“还没验什么”。

| 候选 | 凭什么（事实） | 还没验什么 |
|---|---|---|
| **A. 先不引入 TOON，把 CSV / “字段清单 + 行数组”当第一版结构化输出的基线** | 扁平 CSV 5,505 vs TOON 6,509（§5）；官方 Flat-Only Track 里 TOON 也比 CSV 多 5.9%（§2） | 模型对这两种表示的读取质量对比；CSV 的引号/换行陷阱在实际输出上的频率 |
| **B. 若用 TOON，必须先过“形状门”，且用真实 tokenizer 或键/值长度特征判定，而不是字节数** | RTK 的字节比门会放行 token 变大的案例（§8；`many_keys_one_obj_30` 字符 0.920 / token 1.155，§4.2） | 门的误判率在 Outrim 真实输出上的分布；tokenizer 会不会成为热点（可参考结构化报告的 latency 结论） |
| **C. 重复记录、长文本、源码优先走 dedup/RLE/去格式化，不走格式转换** | 500 条重复记录 TOON 8,012 vs “1 条 + 次数”约 29 tok（§4.4）；长文本 0.990、整块源码 1.000（§4.3） | dedup 后模型是否能正确重展开；指针/引用丢失时的容错 |
| **D. 若必须用 TOON，依赖写 `default-features = false`，并在文档/断言里写清用哪个等值比较器** | 默认 feature 拉 11 个 TUI/CLI 依赖（§8）；tabular 形态会重排键序，按字节/有序 map 比就会不等值（§3.3） | 没有——这两条是工程约束，不是研究开放题 |

**关于“要不要额外教程 prompt”**：实测成本明确（一行标签 17 tok，教程 70 tok，§6），但**“不加提示模型会不会读错”完全没有测**。所以产品讨论里“零 prompt”这个选项是**未经验证**，不能当默认。

---

## 11. 明确未验证项

1. **模型理解是否下降**：本文零模型测试。官方 benchmark 与 arXiv 2605.29676 结论方向不一致（前者称 TOON 不低于紧凑 JSON，后者称有 9pp 级损失且多轮会级联失败）。两者都没被我复现。
2. **教程/标签对质量的真实影响**：只测了 token 成本，没测效果。
3. **生态页标的 4.1 vs crate 的 4.4**：差异原因未知。
4. **`1e400` 在不同上游配置下的行为矩阵**：只测了 `serde_json` 默认与 `arbitrary_precision` 两种；其他 JSON 解析器（如 `simd-json`、`json5`）未测。
5. **Anthropic `tool_result` 是否允许任意文本**：未取得一手引用。
6. **TOON 在 Outrim 真实输出分布上的收益**：本文只用 2 个真实文件 + 合成样例，没有 Outrim 自己的样本集。
7. **长会话下的 prompt 摊销**：一行标签是一次性还是每次重复，取决于 Outrim 的注入位置，未测算。

---

## 12. 复现细节

**探针二进制**（临时目录，未提交仓库；`/tmp` = `C:\Users\wudon\AppData\Local\Temp`）

- `toonrt/src/bin/toonprobe.rs` — 读 stdin 的 `{case: json}`，输出 `minified` / `toon` / `roundtrip` / `value_equal`；依赖 `toon-format = { version = "0.7.0", default-features = false }` + `serde_json 1`。
- `toonrt/src/bin/toonopts.rs` — 同一份数据跑 `comma2/tab2/pipe2/comma1/comma4` 五种选项。
- `toonrt/src/bin/numdefault.rs` — 四段式管线探针（解析 → 编码 → 解码 → 文本比对），默认 `serde_json` feature。
- `toonraw/src/bin/numprobe.rs` — 同上，但 `serde_json` 开 `arbitrary_precision, preserve_order, float_roundtrip`，用于分离"上游解析器"与"TOON 实现"的贡献。

**计数**：`tiktoken 0.14.0`，`cl100k_base` 与 `o200k_base`。

**样本来源**

- 合成样例：脚本内生成，种子固定（`range(n)`），字段名与值形态见 §4 表左列。
- 真实语料：沿用结构化报告的 `corpus/json.txt`（400 行 service 数组）与 `corpus/mcp.json`（MCP 配置响应），未重新采集。
- 源码语料：`corpus/bench.py`（8.4 KB），分别作为"整份一个字符串"与"按行拆记录"两种形状测。

**计数口径**：所有 ratio 是"整份文本送进 tokenizer 的 token 数之比"，不含任何 prompt 包装；prompt 成本单列在 §6。
