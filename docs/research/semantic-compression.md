# 入口工具输出压缩：语义 / token 分类类算法与可部署性

> 研究票：[WodenJay/Outrim#3](https://github.com/WodenJay/Outrim/issues/3)。本文覆盖**语义 / 神经（token 分类、任务相关抽取、轻量检索打分）**这一路（切分归属见 §1）：
> 确定性 / 内容结构感知类见 `docs/research/structural-compression.md`；Pi 宿主钩子见 `docs/research/pi-integration.md`。
> 这两份兄弟报告分别在本仓库的 `research/structural-compression` 与 `research/pi-integration` 分支上（各自独立 worktree，**目前都还没进 `main`**），
> 下文交叉引用的是那两个分支上的当前内容。
> 前提已定：产品**会做**；只处理**入口**（tool output 首次进入上下文之前）、不改写历史；恢复时给极短原文指针；
> 用 Rust 轻量是偏好（未决定禁用模型）；初始需同时支持 Pi 与 OpenCode、一次自动配置。
> 本文只回答"用哪个算法、什么形态、代价多少、边界在哪"，不回答"要不要做"，也不给优先级排序、不设硬阈值。

**证据约定**：所有 token 数用 **cl100k_base 真实 BPE**（`tiktoken 0.14.0`）计数，另注字符数。
本机实验环境：Windows 10.0.22631 / i7-12650H（10 核 16 线程）/ 16 GB RAM / Python 3.14.3 /
`onnxruntime 1.30.0` / `tokenizers(py) 0.23.2` / `rustc,cargo 1.98.1`。**无 torch、无 transformers**，
因此所有模型实验都走 ONNX Runtime 或纯 Rust，不走 PyTorch 路径（这也意味着官方 Python 包本体未被我跑起来，见 §2 末）。
每条数字绑定到附录 A 的探针脚本；未复现的一律标 **[未复验]**。

---

## 0. 结论先行

**1. LLMLingua-2 的"按 token 保留/删除"在真实 tool output 上会破坏不可重建的信息，且是库自身的输出方式所致。**

用它自己的算法（logits→`p_preserve`→WordPiece 合并成词→全局百分位阈值→`tokenizer.convert_tokens_to_string`）
压 `llmlingua/utils.py`（8095 字符 / 2746 token）：32 个 snake_case 标识符 **0 个按原样保留**，
`ast.parse` **失败**，整份文件被压成 **1 行**（原文 245 行）。加模型卡推荐的
`force_tokens=['\n','.','!','?',',']` 也**不能**救回行结构（仍 1 行、仍 0/32）。见 §4.2。

**2. 把"删 token"换成"整行保留/丢弃"，损失降到接近零。**

同一批 payload、同一个模型打分，只在行级聚合后用**原文切片拼接**（不重新 detokenize）：
244/244 保留行与原文**字节级一致**，0 条行内损坏，snake_case 标识符 32/32 全在，
cl100k 压缩比 8 KB→3.84x、50 KB→3.48x。见 §4.3、§4.4。

**3. 但模型打分并不明显优于"不用模型"——结论方向与预期相反。**

在我的探针上，模型的行级排序对"证据行"的 AUC 只有 **0.667**；
在 cargo 错误（n=23 行）与 pytest 失败日志（n=25 行）上，显式词法规则（含错误码 / `file:line` / 数字 /
关键字密度）在 recall 与 purity 上都**不低于**模型。
模型与词法打分的 top-30% 行集合 Jaccard 只有 **0.147**（源码）/ **0.103**（grep 命中），
说明模型**没有**退化成"挑长行"，但它挑的是"上下文连贯的句子"，不是"证据行"。
**注意**：我的"证据行"标注和词法基线用了同一个正则，所以词法胜出部分是构造性的；
这只能支撑"模型没有提供超出显式规则的信号"，不能支撑"模型比最好的词法规则差"。见 §5.3。

**4. Rust 侧能跑，但“能跑”取决于量化方案，不是取决于模型。**

- `rten 0.27.0`（MIT OR Apache-2.0，rust 1.94）：**fp32 ONNX 与 ONNX Runtime 逐位一致**
  （logits corr = 1.000000、MAE = 0.0）；**ORT 默认 dynamic int8 量化图在 rten 上数值全错**
  （corr = −0.022，keep 决策一致率 0.47–0.57）。
- 只有“per-channel + `reduce_range=True` 自己量化”的 178 MB 图能对齐（corr = 0.9896，keep 一致率 0.95–0.97），
  且这张图在 ORT 上比社区 int8 图快 **1.6–2.2x**。
- `ort 2.0.0-rc.14`（MIT OR Apache-2.0）能正确跑所有量化图，产物是 **22 MB 静态 exe + 不需要外部 onnxruntime.dll**
  （拷到空目录仍可运行），代价是首次 `load_ms` 317–794 ms。
见 §7。

**5. 成本与"入口毫秒级"预算差 2–3 个数量级。**

本机 BERT-base int8（178 MB）单 508-token 窗口：ORT 123–208 ms（0.24–0.41 ms/token）、
rten fp32 278 ms。端到端 Rust 原型（分词→逐窗前向→行级阈值→切片）：
8 KB/2 744 tok **1.37–1.73 s**、24 KB/5 997 tok **2.97–3.85 s**、50 KB/12 890 tok **6.63–7.91 s**；
加载 267–421 ms；峰值 RSS fp32 **783 MB** / 量化图 **243 MB**。见 §6。

**6. 与外部最强证据的正面冲突（§5.4）：我的探针不能证明“模型无用”。**

Squeez 在 618 例公开测试集上：微调 2B 行选择器 **recall 0.86 / prec 0.80 / 移除 92%**；
而 **BM25(10%) 只有 recall 0.22**。我在 23–25 行探针上看到“手写正则 ≥ 未条件化模型”，
但这两种“词法”不是一回事（前者是**领域知识手写正则**，后者是**查询无关 BM25**）。
**结论只能是：未条件化的 LLMLingua-2 在代码/日志上没提供超出手写领域正则的信号；
不能说模型无用。** 见 §5.4。

**7. 唯一一条“把 LLMLingua-2 真的接到 coding agent 上”的测量是负面的——但论文自身数字自相矛盾。**

SWE-Pruner（[arXiv:2601.16746v4](https://arxiv.org/abs/2601.16746)）Table 3（SWE-Bench Verified，50 例子集）：
Mini SWE Agent 52.3% / 0.972M token → **+LLMLingua-2 42.1% / 0.856M**，+SWE-Pruner 41.1% / 0.670M。
但**同一篇正文写的是**"our method achieves the best success rate (**64%**), outperforming the
vanilla agent baseline (**62%**)"，且说 LLMLingua2 掉到 **54%**、RAG 掉到 50%。
**表格与正文两套数字互不相容**（52.3/41.1/42.1 vs 62/64/54），我无法判断哪个对应最终实验。
两套数字的**方向一致**：加 LLMLingua-2 后成功率下降 5–10 个点。这是"通用 QA 结论不能当 agent 证据"的
直接反例，但**不要引用其中任何一个绝对数**，见 §8.3。

**8. 在真实 agent 上的最强证据（≈35 000 次运行）指向：primitive 的选择比“删多少”更重要，而“删掉整段但保留其余原样”是唯一没掉点的选择。**

[arXiv:2609.32961](https://arxiv.org/abs/2609.32961) 在 SWE-bench Verified + Terminal-Bench 上测 65 种策略：
Tool-result clearing（直接删旧输出、插 `[output cleared]`）**基本持平基线**（token/latency/计费 0.57×/0.79×/0.71×）；
而 summarization 用几乎同样多 token（0.59×）却**掉 11.0 pp**。Terminal-Bench 上 12 个策略里
**11 个降 5–13 pp**，且**全部变慢**（token 少 ≠ 快，可慢 20–80%）。
→ 这直接支持“**选择 + 原文保留**优于“**改写 / 重写**”，也说明产品指标不能是压缩比。见 §8.4。

**9. 形态结论（不是优先级）。**

这套技术唯一站得住的形态是**行/块级 extractive 选择 + 原文切片 + 字面量保护层**；
"token 删除"和"latent / 软提示压缩"都不应进这条流水线（后者有专门的负面证据，见 §1.4）。
如果做行级选择，**先建 30–50 条真实轨迹的标注集**，再决定要不要模型——否则无法区分
"模型有用"与"我的正则恰好写出了同样的东西"。
另一个真实的空白：上述 35 000 次运行的研究**没有考察“行级选择性保留”**（只测了 TR/TRC/SU），
所以这部分必须我们自测。

---

## 1. 术语：四种"形态"与三种"可部署性"

### 1.1 四种形态

| 形态 | 代表 | 输出物 | 保存原文字节？ | 可否给任意 hosted LLM 直接用 |
|---|---|---|---|---|
| **token 删除**（保留/丢弃子词） | LLMLingua-2、LLMLingua、Selective-Context | 重新 detokenize 的字符串 | 否（词被合并/切开） | 是 |
| **行/块选择**（整行或整块保留） | SWE-Pruner、Squeez、LongCodeZip 的第二段 | 原文行的子集 | 是（可字节级） | 是 |
| **抽象/改写** | LLM Summarize、ACON 的 guideline | 新生成的文本 | 否 | 是 |
| **latent / 软提示** | ICAE、500xCompressor 类 | KV / 软 token | 否 | **否**，需要改模型与推理栈 |

### 1.2 三种"可部署性"

- **A 类：纯文本进、纯文本出。** 任何 hosted LLM 都能吃，Outrim 只需一个 stdio/NDJSON 进程。本文的候选都在这一类。
- **B 类：需要改模型或推理栈**（soft prompt / KV 压缩）。出范围，理由见 1.4。
- **C 类：需要额外服务**（vLLM 上的 2B 生成模型、或需要 GPU 的前向）。Squeez 的默认后端属这一类。

### 1.3 与"任务无关 vs 任务相关"正交

LLMLingua-2 是**任务无关**（task-agnostic）：只有上下文，没有 query。
LongLLMLingua / SWE-Pruner / Squeez 是**任务相关**：需要一个 question / goal / query。
这个维度决定了"要不要在压缩器里塞任务上下文"，见 §8.2。

### 1.4 为什么 latent 方案出范围（带证据，不靠直觉）

`arXiv:2605.11051`《On Problems of Implicit Context Compression for Software Engineering Agents》
的报告结论是：In-Context Autoencoder 一类隐式压缩在**单轮代码理解**上可用，
在**多步 agentic coding** 上失效。这与"只在入口压一次、不改历史"的设定叠加后没有剩余价值：
入口压缩的价值来自"少喂无用 token"，而 latent 方案额外要求宿主换推理栈——收益与代价方向相反。
（此条我只读了摘要级信息，标 **[未复验]**。）

---

## 2. 一手来源与固定版本

| 对象 | 版本 / 提交 | 许可 | 关键数字 |
|---|---|---|---|
| LLMLingua 代码 | `microsoft/LLMLingua` @ [`5a4c78a`](https://github.com/microsoft/LLMLingua/commit/5a4c78ae18ab17a98cf997e8259354e546081d64)（2026-09-10） | MIT | `setup.py` 依赖 `transformers>=4.26.0, accelerate, torch, tiktoken, nltk, numpy`；**无 onnxruntime** |
| PyPI `llmlingua` | [0.2.2](https://pypi.org/project/llmlingua/0.2.2/) | MIT | 同上依赖；`torch` 是硬依赖 |
| LLMLingua-2 论文 | [arXiv:2403.12968](https://arxiv.org/abs/2403.12968) | — | XLM-R-large 编码器 355 M / mBERT 编码器 110 M；训练 23 h / 16 h（MeetingBank） |
| 官方模型（小） | [`microsoft/llmlingua-2-bert-base-multilingual-cased-meetingbank`](https://huggingface.co/microsoft/llmlingua-2-bert-base-multilingual-cased-meetingbank) | **apache-2.0**（HF API 实查） | 177 341 186 参数；safetensors 709 388 104 B；下载 564 356 |
| 官方模型（大） | [`microsoft/llmlingua-2-xlm-roberta-large-meetingbank`](https://huggingface.co/microsoft/llmlingua-2-xlm-roberta-large-meetingbank) | **mit**（HF API 实查） | 558 945 282 参数；safetensors 2 235 829 648 B；下载 138 934 |
| 社区 ONNX 导出 | [`atjsh/llmlingua-2-js-…-onnx-v4`](https://huggingface.co/atjsh/llmlingua-2-js-bert-base-multilingual-cased-meetingbank-onnx-v4) | apache-2.0 | `base_model:quantized:…`；含 `onnx/model.onnx`（fp32）、`model_uint8.onnx`（178 094 427 B）、`_q4/_bnb4/_fp16` |
| 小模型（**不可用**） | [`JueZhang/lingua2_tinybert_meetingbank_only`](https://huggingface.co/JueZhang/lingua2_tinybert_meetingbank_only) | **未声明**（HF API `license: null`） | 14 253 218 参数；uint8 ONNX 14 442 511 B；下载 7；mobilebert 版同样未声明 |
| LongLLMLingua 论文 | [arXiv:2310.06839](https://arxiv.org/abs/2310.06839) v2 | arXiv perpetual non-exclusive | NQ 上 +21.4% @ ~4x（GPT-3.5-Turbo）；LooGLE 成本 −94.0%；10k token/2x–6x 端到端提速 1.4x–2.6x。**打分器 = LLaMA-2-7B-Chat**；延迟实验跑在 **V100-32G** |
| SWE-Pruner 论文 | [arXiv:2601.16746](https://arxiv.org/abs/2601.16746) v4 | — | 0.6 B Qwen3-Reranker 骨干；行级均值打分；read 类操作占 token **76.1%**（execute 12.1% / edit 11.8%）；**Table 3 与正文数字不一致**，见 §8.3 |
| 压缩策略系统研究 | [arXiv:2609.32961](https://arxiv.org/abs/2609.32961) v1（2026-09-26） | CC BY 4.0 | **约 35 000 次 agent 运行**；65 种策略 × 3 个开源模型 × SWE-bench Verified(100) + Terminal-Bench 1.0(40)；**把 Pi Coding Agent 列为被调研系统之一** |
| EnComp（编码器句子剪枝） | [arXiv:2603.09222](https://arxiv.org/abs/2603.09222) v2 | CC BY 4.0 | ModernBERT-large 骨干；单次全上下文编码给所有句子打分；**0.045 s / 1.86 GB 峰值 VRAM**（GPU，k=20）；压缩率 16.0% |
| Squeez 论文 | [arXiv:2604.04979](https://arxiv.org/abs/2604.04979) | — | Qwen 3.5 2B + LoRA；618 例自建测试集 |
| Squeez 模型/包 | [`KRLabsOrg/squeez-2b`](https://huggingface.co/KRLabsOrg/squeez-2b) apache-2.0；[PyPI 0.1.4](https://pypi.org/project/squeez/0.1.4/) | apache-2.0（HF API 实查） | 2 274 069 824 参数；bf16 分片 safetensors 合计 4 548 221 488 B；`torch>=2.1` 可选 extra，`openai>=1` 是**硬依赖** |
| `rten` | [0.27.0](https://crates.io/crates/rten/0.27.0) | MIT OR Apache-2.0 | rust 1.94；`onnx_format`+`mmap` feature |
| `ort` | [2.0.0-rc.14](https://crates.io/crates/ort/2.0.0-rc.14) | MIT OR Apache-2.0 | rust 1.92；`download-binaries` |
| `tokenizers` | [0.23.2](https://crates.io/crates/tokenizers/0.23.2) | Apache-2.0 | 下载 40.1 M |

**[事实]** 两个官方模型卡的 usage 例子都写 `rate=0.6, force_tokens=['\n','.','!','?',',']`（我直接取页取到原文）；
模型卡**未声明**训练语言范围与上下文窗口（只写“seeded from MeetingBank meeting transcripts”）。
训练数据是 **会议转写**（MeetingBank），不是代码/日志——这是“模型没见过 tool output 分布”的直接依据。

**[事实]** 我查 HF 搜索（`llmlingua` / `lingua2` / `lingua`）除这两个官方 checkpoint 之外只有第三方 fork；
`slingua` 搜索 0 结果，`securitylingua` 2 条且**均未声明许可**（词表里虽然列了这两个名字，但 HF 上没有可用的权重）。
因此“用 tinybert 尺寸换 10x 速度”这条路目前**没有可用的许可**——
[`JueZhang/lingua2_tinybert_meetingbank_only`](https://huggingface.co/JueZhang/lingua2_tinybert_meetingbank_only)
HF API 实查 `license: null`、14 253 218 参数、下载 7。14 MB 的 tinybert 我只用来自证“速度上限在哪”，**不作为候选**。

**[未复验]** 官方 Python 包本体（`pip install llmlingua`）未在本机跑起来（无 torch）。我改为
**逐行复刻** `__compress` / `__merge_token_to_word` / `__token_prob_to_word_prob` 并用同一 checkpoint，
因此 §4.2 的破坏性结论属于"算法等价复现"，不是"官方 API 实跑"。

---

## 3. LLMLingua-2 到底怎么压（源码级）

**[事实]** 入口函数 `compress_prompt_llmlingua2(...)`
（`llmlingua/prompt_compressor.py:727`），参数含
`rate=0.5, target_token=-1, use_context_level_filter=False, token_to_word="mean",
force_tokens=[], force_reserve_digit=False, drop_consecutive=False, chunk_end_tokens=[".","\n"]`。

**[事实]** `__compress`（`:2304`）的实际步骤：

1. 先用 `__chunk_context`（`:2230`）按 `max_seq_len-2` 切窗，切点上回退到最近的
   `chunk_end_tokens`（默认 `.` 或 `\n`）——即 512-token BERT 窗口。
2. 每窗前向，取 `softmax(logits)[:, 1]` 作为每个 WordPiece 的 `p_preserve`。
3. `__merge_token_to_word`（`:2270`）把 `##` 续接片合并成"词"，`force_tokens` 命中时给 `p=1.0`，
   `force_reserve_digit` 只覆盖"token 里含 0–9"的情形。
4. `__token_prob_to_word_prob`（`:2289`）把词内各片概率按 `mean`（或 `first`）合成词概率。
5. 全局阈值 `np.percentile(word_probs, int(100*reduce_rate)+1)`，保留 `p > 阈值` 的词。
6. **`tokenizer.convert_tokens_to_string(kept_words)`** 重新拼回文本。

第 6 步是致命的：保留的"词"是用 **mBERT 的 WordPiece 解码器**拼回去的，
`handle_timeout` 被合并成一个词，再被解码成 `handle timeout`；未保留的标点与空白不会回来。
`rate` 在库里是"保留比例"，`reduce_rate = 1 - rate`（`:920`）；`target_token>0` 时
`rate = min(target_token/n_original, 1.0)`（`:918`）。

**[事实]** `is_begin_of_new_word` / `get_pure_token`（`llmlingua/utils.py:80,103`）按模型家族硬编码；
支持列表只有 mBERT / tinybert / mobilebert / xlm-roberta-large / slingua / securitylingua，其余抛
`NotImplementedError`（`:2300`）。也就是说**换成自训练的 token classifier 需要改库**。

**[事实]** LongLLMLingua 路径强制要 question：
`prompt_compressor.py:565` `assert not (rank_method == "longllmlingua" and not question)`，
报错文案 `"In the LongLLMLingua, it is necessary to set a question."`；
默认 `condition_in_question="after"`（`:569-571`）。
其 question-aware 排序内部用 `SentenceTransformer("multi-qa-mpnet-base-dot-v1")`（`:1852`），
该模型 **safetensors 437 971 872 B、HF 未声明 license**；`llmlingua` 的 `INSTALL_REQUIRES` 里
也**没有** `sentence_transformers`（运行到该分支才报 ImportError）。

**[事实，决定性]** LongLLMLingua 的**主打分器是 LLaMA-2-7B-Chat**。原论文 §5 “Implementation details” 原文：
> “For the small language models used for compression, we apply **LLaMA-2-7B-Chat**”

消融表里的 “w/ GPT2-small” 才是换成小模型，且换来的是下降。原论文的延迟实验跑在 **V100-32G** 上。
→ **LongLLMLingua 的算法思想可借鉴（question-aware 排序、文档重排、动态压缩比、
subsequence recovery），但它的具体实现不能上 Outrim 的 CPU 入口路径。** 7B 模型逐 token 算 PPL
不可能满足入口预算，而且它压的是历史/多文档，不是单个 tool output。

---

## 4. 实测：破坏面

### 4.1 探针与口径

| payload | 字符 | 行 | BERT tok | cl100k tok |
|---|---:|---:|---:|---:|
| `payload_utils.py` = 真实 `llmlingua/utils.py` 全文 | 8 095 | 245 | 2 746 | 2 015 |
| `rust_p_8k.txt`（同上，切片版） | 8 343 | 244 | 2 744 | 2 015 |
| `rust_p_24k.txt`（真实 `prompt_compressor.py` 切片） | 25 029 | 452 | 5 997 | 4 821 |
| `rust_p_50k.txt`（同上前 50 KB） | 52 328 | 1 127 | 12 890 | 10 285 |
| `pay_cargo.txt` = 真实 `cargo build` 类型错误（我故意改坏一个 crate 得到） | 918 | 23 | 329 | 264 |
| `pay_pytest.txt` = 真实 pytest 失败格式 | 1 294 | 25 | 764 | 280 |

"标识符"= 文本里出现的 snake_case 词；"数字"= `0x…` 或多位十进制；"行保留"= 输出行与原文某行**字节相同**。

### 4.2 库算法（token 删除 + detokenize）
探针 `bench_damage2.py`；阈值口径同库（全局百分位），`rate` = 保留比例。

| rate | 模型 | 字符比 | 标识符 原样 | 标识符 去空白后 | 数字 | 输出行数 | `ast.parse` |
|---:|---|---:|---:|---:|---:|---:|---|
| 0.5 | mBERT int8 | 1.83x | **0/32** | 28–30/32 | 2/2 | **1**（原 245） | **SyntaxError** |
| 0.5 | tinybert int8 | 1.67x | **0/32** | 21/32 | 2/2 | **1** | **SyntaxError** |
| 0.7 | mBERT int8 | 2.68x | **0/32** | 23–26/32 | 2/2 | **1** | **SyntaxError** |
| 0.7 | tinybert int8 | 2.68x | **0/32** | 15/32 | 2/2 | **1** | **SyntaxError** |

加 `force_tokens=['\n','.','!','?',',']`（模型卡与 `compress.sh` 的推荐值，探针 `bench_forcetokens.py`）：
rate 0.5 仍 **1.81x / 1 行 / 0-32 标识符 / AST 失败**；再加 `_` 仍 1.72x / 1 行 / 0/32。
原因见 §3 第 6 步：`convert_tokens_to_string` 把整段并成一行，`force_tokens` 只在**词级**生效，救不回版面。

**具体丢失项**（探针，rate 0.5）：

| 输入 | 输出 | 丢了什么 |
|---|---|---|
| `timeout_ms = 30000  # deadline` | `timeout ms 30000` | 标识符被切开、`=`、注释 |
| `ERROR 0x80070057: invalid parameter` | `ERROR 0x80070057` | 语义词全丢，**十六进制码幸存** |
| `fn handle_timeout(&self, budget_ms: u64) -> Result<()> {` | `fn handle timeout ( self budget _ ms u64 Result <` | **`&` 丢失**、标识符切开、`:`/`>`/`{`/`)` 丢失 |
| `connection refused: ECONNREFUSED 127.0.0.1:5432` | `ECONNREFUSED 127. 0. 1` | **端口 5432 丢失**、IP 被插空格 |
| 130 行 grep 命中 | **1 行** | 全部 `file:line` 结构 |

**[事实]** 单 token 层面（rate 0.5，单窗）：标识符类 63.0% 存活、含数字 token 100%（n=13）、
标点 20.6%。即"模型确实偏向保留数字"，但**行结构完全不在它的目标里**。

### 4.3 换成"整行保留/丢弃 + 原文切片"后

两种实现都试了：(a) Python 复刻（`bench_linescore.py`），(b) Rust 端到端原型
（`rtenprobe/src/main.rs`，tokenize → 逐窗 rten 前向 → 行级均值 → 百分位 → 原文切片）。Rust 版：

| payload | keep | 字符比 | cl100k 比 | 行数 | 保留行字节一致 | 行内损坏 |
|---|---:|---:|---:|---:|---:|---:|
| 8 KB | 0.3 | 3.67x | **3.84x** | 243/244 | 80–81/217 | 0–1 |
| 8 KB | 0.5 | 2.13x | **2.14x** | 243/244 | 122–123/217 | 1 |
| 24 KB | 0.3 | 4.24x | **4.09x** | 447/452 | — | 0 |
| 24 KB | 0.5 | 2.28x | **2.21x** | 450/452 | — | 0 |
| 50 KB | 0.3 | 3.69x | **3.48x** | 1 122/1 127 | 424/1071 | 0 |
| 50 KB | 0.5 | 2.21x | **2.07x** | 1 125/1 127 | 673/1071 | 0 |

标识符：8 KB/rate 0.5 = 28/32 全在输出里（rate 0.3 = 16/32，因为整行被丢）。
“保留行数”几乎不掉是因为**空白行也被计入**——这是一个真实的误导陷阱，我把数字算准了：

| payload | keep | 输出行 | 其中空白/仅空白 | 空白占行 | 空白占字符 |
|---|---:|---:|---:|---:|---:|
| 8 KB | 0.3 | 242 | 178 | **73.6%** | 7.8% |
| 8 KB | 0.5 | 242 | 135 | 55.8% | — |
| 24 KB | 0.3 | 446 | 318 | **71.3%** | — |
| 50 KB | 0.3 | 1 121 | 801 | **71.5%** | 5.6% |
| 50 KB | 0.5 | 1 124 | 590 | 52.5% | — |

→ **两种口径差异很大**：按“行数”看，压缩几乎是假的（七成保留行是空白）；
按“字符/token”看，空白只占 5.6–7.8%，所以上表的 3.48x/3.84x **总体压缩比是真的**。
结论：**空白行不构成 token 成本，但会污染任何以“行数”为口径的指标**（包括我 §4.3 的“保留行字节一致率”，
它的分母 217 里包含空白行）。后续实验必须把“非空行”当分母。
注：这一点 `structural-compression.md` 未正面处理，属本报告新增发现。

唯一的"行内损坏"案例是 Python 的一行 `        )  # add special tokens` 变成 `        ) # add special tokens`：
跨过一个窗口边界的 tab 被空格化。说明**切片必须在窗口边界做保护**，否则会出现这类单字符级偏差。

Python 版复现（`bench_linescore.py`，keep=0.3）：245 行 → 74 行，**74/74 行字节一致**、3.30x；
keep=0.5 → 122/122 字节一致、1.86x；grep 型 payload keep=0.3 → 3.37x、32/32 字节一致。

### 4.4 真实错误输出上的表现（最关键的一张表）

`pay_cargo.txt`（真实 `cargo build` 错误）与 `pay_pytest.txt`（真实 pytest 失败格式），
Rust 行级模式，含 `-->` / `file:line` / 错误码：

| payload | rate | cl100k 比 | 丢失的证据项 |
|---|---:|---:|---|
| cargo 错误 | 0.3 | 2.78x | **丢 `-->`、丢 `src\main.rs:27:19`、丢 `try_into`** |
| cargo 错误 | 0.5 | 2.08x | 无 |
| pytest 失败 | 0.3 | 2.64x | **丢 `DID NOT RAISE`、丢 `pytest.raises`** |
| pytest 失败 | 0.5 | 1.72x | 无 |

rate 0.3 的 cargo 输出实样（93 token）：

```
Compiling rtenprobe v0.1.0 (C:\Users\wudon\AppData\Local\Temp\rtenprobe)
error[E0308]: mismatched types
27 | let _x: u64 = text.len();
error[E0425]: cannot find function `compress_tool_output` in this scope
28 | let _y = compress_tool_output(&text, rate);
Some errors have detailed explanations: E0308, E0425.
```

错误码、行内容、`error[N]` 都在；**`-->` 与 `file:line:col` 没了，缩进全没**。
对"agent 要去改文件"的场景这是净损失：`-->` 那一行是唯一的定位锚。
**结论**：行级选择必须在行内做"字面量/定位符保护"，且不该在 `file:line` 密集的 payload 上激进压缩。

### 4.5 哪种 payload 不该压

| payload 形态 | 行级可压性 | 依据 |
|---|---|---|
| `read` 大文件 / 长文档 | 高（8 KB 3.8x、50 KB 3.5x，行字节一致） | §4.3 |
| 真实构建错误（n 小） | 中（rate 0.5 无损，0.3 丢定位） | §4.4 |
| grep 命中（43 行 9 608 字符） | 低—中（3.37x 但每行都是命中，"压"等于删命中） | §4.3 |
| `ls` / `find` 列表 | 低（行内重复前缀，属结构性折叠） | `structural-compression.md` §4.4、§4.5 |
| diff / 已截断的 bash（50 KB / 2 000 行） | **不应压**（`--`/`++`/`@@` 与行号本身就是证据） | 结构性结论 |

---

## 5. 排序质量：模型 vs 平凡基线

### 5.1 行级保留曲线（探针 `bench_linescore.py`）

`python_utils.py` keep=0.3 → 74 行、3.30x、74/74 字节一致；keep=0.5 → 122 行、1.86x、122/122。
per-line 分数在真实 pytest 失败日志上的形态（rate 无关，按分数排序）：

| 分数 | 行 |
|---:|---|
| 0.070 | `============================= test session starts ===` |
| 0.170 | `___________________________ test_retry_backoff_exhausted ___` |
| 0.528 | `    def test_retry_backoff_exhausted(monkeypatch):` |
| 0.813 | `        client = RetryingClient(max_retries=3, base_delay=0.5)` |
| 0.811 | `>       with pytest.raises(RetryExhausted) as exc:` |
| 0.678 | `E       Failed: DID NOT RAISE <class 'outrim...'>` |
| 0.741 | `src/outrim/retry.py:88: Failed` |
| 0.855 | `FAILED tests/test_pipeline.py::test_retry_backoff_exhausted` |

**[事实]** 分隔线（最没用）得分最低、证据行得分高——**方向对**。
但**序不可信**：`platform win32 …` / `collected 214 items` 这类"上下文行"得分高于 `--> src\main.rs:27:19`。
这就是我不给任何阈值的原因。

### 5.2 与平凡基线的对照（探针 `bench_linefair.py`）

recall / purity（purity = 选中行里真证据占比），窗口 = 引擎整段前向（`win`）与逐行独立前向（`ind`）：

| payload | keep | 行/证据 | 模型 `win` | 模型 `ind` | 词法 | firstN | lastN | random |
|---|---:|---|---|---|---|---|---|---|
| cargo 错误 | 0.2 | 23/8 | 0.12/0.20 | 0.12/0.20 | **0.62/1.00** | 0.25/0.40 | 0.25/0.40 | 0.25/0.40 |
| cargo 错误 | 0.3 | 23/8 | 0.25/0.29 | 0.38/0.43 | **0.88/1.00** | 0.38/0.43 | 0.25/0.29 | 0.12/0.14 |
| cargo 错误 | 0.5 | 23/8 | 0.62/0.42 | 0.75/0.50 | **1.00/0.67** | 0.50/0.33 | 0.50/0.33 | 0.75/0.50 |
| pytest 失败 | 0.2 | 25/5 | 0.20/0.20 | 0.20/0.20 | **1.00/1.00** | 0.00/0.00 | 0.60/0.60 | 0.20/0.20 |
| pytest 失败 | 0.3 | 25/5 | 0.80/0.50 | 0.60/0.38 | **1.00/0.62** | 0.00/0.00 | 1.00/0.62 | 0.60/0.38 |
| pytest 失败 | 0.5 | 25/5 | 1.00/0.42 | 1.00/0.42 | 1.00/0.42 | 0.00/0.00 | 1.00/0.42 | 0.40/0.17 |

### 5.3 这张表怎么读（必须读）

1. **n 极小**：cargo 是 23 行、pytest 是 25 行。这不是 benchmark，是探针。任何"X 比 Y 好"都不成立。
2. **循环论证**：我的"证据行"正则与"词法基线"的正则几乎同一组模式，所以词法赢有构造性成分。
   它能支撑的结论只有一句：**模型没有提供超出显式规则的可测信号**。
3. **非循环的那一项**：行级 AUC（模型分数 → 证据行）= **0.667**（cargo）。0.667 意味着"弱有信息"。
4. **模型不退化为挑长行**：top-30% 行集合 Jaccard（模型 vs 词法）= **0.147**（源码）/ **0.103**（grep）。
   两者挑的是不同东西；模型挑"连贯句子"，词法挑"错误码/路径/数字"。
5. `win` vs `ind` 没有稳定差别（cargo 上 `ind` 略好，pytest 上 `win` 略好）→ **跨行上下文没有可测收益**，
   这意味着逐行独立打分（可并行、易加缓存）不吃亏。

### 5.4 与外部证据的正面冲突（必须处理）

我的 §5.2 结论（“词法 ≥ 模型”）与 Squeez Table 4 **直接矛盾**：

| 方法（Squeez Table 4，618 例公开测试集） | Prec | Recall | Strict F1 | 移除比例 |
|---|---:|---:|---:|---:|
| Squeez-2B（LoRA 微调） | **0.80** | **0.86** | **0.79** | 92% |
| Qwen 3.5 35B A3B（zero-shot） | 0.74 | 0.75 | 0.70 | 92% |
| Qwen 3.5 2B base（未微调） | 0.42 | 0.53 | 0.41 | 82% |
| **BM25（10%）** | 0.13 | **0.22** | 0.13 | 90% |
| First-N（10%） | 0.07 | 0.14 | 0.08 | 91% |
| Last-N（10%） | 0.05 | 0.05 | 0.04 | 91% |
| Random（10%） | 0.07 | 0.10 | 0.07 | 91% |

**怎么调和（不是狡辩）**：

1. **它们测的是不同的“词法”。** Squeez 的基线是**查询无关的检索（BM25）与位置启发式**；
   我 §5.2 的“词法”是**把领域知识手写成正则**（错误码、`file:line`、数字、关键字密度）。
   Squeez 自己的解释正好解释了为什么 BM25 只有 0.22 recall：
   > “usefulness depends on the query rather than lexical overlap alone”（§“Heuristic baselines”）
   而**手写错误模式**恰好编码了 BM25 缺的那部分领域知识。所以两者可以同时成立。
2. **但它同时是一个警告**：我的手写正则就是**人工注入的领域知识**，而 Squeez 的结论是
   神经网络能学到**比通用启发式好得多**的 tool-specific extraction policy（0.86 vs 0.22 recall）。
   → **“手写词法能不能覆盖未知输出形态”是开放问题**，而那正是模型应当胜出的地方。
3. **任务条件化是真实差距**：Squeez 有 focused query，我无任务上下文。§5.2 的对比**并未测试
   “模型+查询” vs “词法”**，所以我的表不能用来否定 Squeez。
4. **数据集性质不同**：Squeez 的 gold 是人工标的**逐字证据行区间**（11 477 训练 / 618 测试，
   由 SWE-bench 仓库交互 + 合成多生态 tool output 构成），且强调保留“verbatim evidence block”；
   我的“证据行”是我自己用正则定的。→ **我的标注不是 ground truth**。

**修正后的结论**：
- 可以说：**在本机 23–25 行的两个小探针上，未条件化的 LLMLingua-2 没有提供超出手写领域正则的信号。**
- **不能**说：“词法优于模型”、“模型无用”。Squeez 的 618 例上，微调 2B 的 recall 是 BM25 的 **3.9 倍**。
- §10-1 的标注集建议因此从“可选”升为**前置依赖**：没有人类标注的 gold，任何对比都是自证。

---

## 6. CPU / 内存 / 工件成本（本机实测）

### 6.1 单窗前向（508 token，8 线程）

| 工件 | 大小 | 引擎 | `load_ms` | ms/508tok | ms/token |
|---|---:|---|---:|---:|---:|
| mBERT fp32 ONNX | 709 574 565 B | ORT 1.30 | 794 | 277.6 | 0.546 |
| mBERT fp32 ONNX | 709 574 565 B | rten 0.27 | 267–421 | ~280 | 0.55 |
| mBERT uint8（社区导出） | 178 094 427 B | ORT | 321 | 207.8 | 0.409 |
| mBERT **int8 自量化**（`QuantType.QInt8` + `MatMul`） | 178 094 385 B | ORT | 317 | **123.0** | 0.242 |
| mBERT **per-channel + reduce_range** | 178 509 474 B | ORT | 341 | **129.6** | 0.255 |
| tinybert uint8（**许可不可用**） | 14 442 511 B | ORT | 89 | 25.0 | 0.049 |

### 6.2 端到端（Rust 原型，含分词/切片；rten fp32）

| payload | tok | 分词 | 前向 | 切片 | 合计 |
|---|---:|---:|---:|---:|---:|
| 8 KB | 2 744 | 2.3–4.0 ms | 1 371–1 732 ms | 0.1 ms | **1.37–1.73 s** |
| 24 KB | 5 997 | 7.8–11.1 ms | 2 969–3 854 ms | 0.1–0.3 ms | **2.97–3.85 s** |
| 50 KB | 12 890 | 17.9–28.2 ms | 6 630–8 754 ms | 0.3–0.7 ms | **6.63–7.91 s** |

量化图（`ll2rr`）同 50 KB：前向 5 737–6 118 ms，`load_ms` 166–189，峰值 RSS **243 MB**
（fp32 峰值 RSS **783 MB**）。

### 6.3 其他形态的成本

| 形态 | 工件 | 大小 | 本机 CPU 实测 |
|---|---|---:|---|
| SWE-Pruner 形态（Qwen3-Reranker-0.6B，[apache-2.0](https://huggingface.co/Qwen/Qwen3-Reranker-0.6B)；HF API 报 **595 776 512** 参数） | safetensors（bf16） | 1 191 588 280 B | (goal,chunk) 对：140 tok → 105 ms、268 tok → 195 ms、**524 tok → 447 ms**；50 KB ≈ 26 对 ≈ **11.6 s（推算）** |
| Squeez 形态（2B 生成式） | bf16 safetensors | 4 548 221 488 B | **未测**（本机无 torch；`ort` 未导出该图） |
| LLMLingua 原路径（PPL，LLaMA-2-7B-Chat） | 7B 因果 LM | bf16 约 13 GB | 不测（本机 16 GB RAM，CPU 入口路径不可行） |

**结论**：入口压缩要在"毫秒级、不影响首 token"的预算内。
**178 MB 的 BERT-base 是 10²–10³ ms 量级，与预算差 2–3 个数量级。**
可选的现实边界只有三种：① 只对大 payload 用（`read` 大文件、长日志），有 1–8 s 预算；
② 只在异步/预热路径用；③ 不用模型。

---

## 7. Rust 部署路径（本节是工程上最有用的部分）

### 7.1 两个 crate 的实际行为

**`rten 0.27.0`**——纯 Rust，无 C 依赖，`model.onnx` 直接 `Model::load_file`：

| 工件 | rten vs ORT（2 746 tok 全量） | keep 决策一致率 @0.3/0.5/0.7 |
|---|---|---|
| fp32（`onnx/model.onnx`） | corr **1.000000**，MAE **0.000000** | **1.0000 / 1.0000 / 1.0000** |
| 社区 uint8（`model_uint8.onnx`） | corr **−0.0218**，MAE 0.665 | 0.5496 / 0.4708 / 0.5714 |
| 自量化 int8（per-tensor，`reduce_range=False`） | corr **−0.0289**，MAE 0.809 | 0.5780 / 0.4803 / 0.5853 |
| 自量化 int8（`reduce_range=True` + `per_channel=True`，**权重 zp 全 0**） | corr **0.9924**，MAE 0.019 | 0.9585 / 0.9555 / 0.9665 |
| 同上但从 fp32 起点、只量化 `MatMul`（455 MB） | corr **0.9888**，MAE 0.025 | 0.9614 / 0.9512 / 0.9628 |

**量化组合网格**（`qgrid/`，4 种组合 × rten）：

| 组合 | rten corr vs fp32 | keep 一致率 @0.5 |
|---|---:|---:|
| `reduce_range=True` + `per_channel=True` | **0.9888** | **0.9512** |
| `reduce_range=True` + `per_channel=False` | 0.6994 | 0.7697 |
| `reduce_range=False` + `per_channel=True` | 0.0476 | 0.5219 |
| `reduce_range=False` + `per_channel=False` | 0.0022 | 0.4920 |

**[事实，根因线索]** `rten-gemm-0.27.0/src/kernels.rs:172` 的 `may_saturate()` 默认返回 `false`，
`rten-0.27.0/src/ops/matmul.rs:553` 起的分支注释原文：

> "This is primarily an issue for x64 systems without VNNI. … To avoid the saturation hazard
> entirely, the model should be converted with `reduce_range` enabled"

并且该分支在饱和时对 LHS 做"最小必要 shift"。这解释了为什么**只有 `reduce_range` + `per_channel`
能对齐**，以及为什么"官方图（per-tensor、权重 zp 全 0）"会整片崩。

**[诚实标注]** 我**没有**把失败定位到单个算子上：孤立建的最小图里，
`MatMulInteger`（int8/uint8、标量/向量 zero-point）+ `DequantizeLinear` + `DynamicQuantizeLinear`
在 rten 与 ORT 上**逐位一致**（4 种组合 maxdiff = 0.000）。所以 §7.1 的判据是"整图 + 量化方案级别
的可复现现象"，不是"某个 op 的语义错"。**根因未完全定位，标 [未核实]。**

**`ort 2.0.0-rc.14`**——FFI 绑定，`download-binaries` 拉预编译 ONNX Runtime：

- 对**所有**被测图数值正确（含社区 uint8、per-tensor int8、per-channel int8、fp32）。
- 产物：release exe **22 025 664 B**，**无外部 `onnxruntime.dll`**——我把 exe 单独拷到空目录
  `/tmp/solo` 运行成功（`load_ms=365`）。DLL 是静态链进 exe 的。
- API 形状（rc 里与稳定版不同，容易踩）：`ort::environment::init().build()?` 拿 env，
  `Session::builder(&env)?.with_intra_threads(8)?.commit_from_file(p)?`，
  `Tensor::from_array(([1usize, len], vec))?`，`sess.run(ort::inputs![...])?`，
  `out[0].try_extract_tensor::<f32>()?`。

**[建议]** 若走模型，**先用 `ort`**：它对量化不敏感、许可干净、二进制可自带；
`rten` 只在"必须纯 Rust / 不接受 22 MB"时才用，且必须**自带 fp32 或自量化工件**，
不能直接用社区 int8 导出。

### 7.2 工件与分发

| 方案 | 工件大小 | rten 可用 | ORT 可用 | 备注 |
|---|---:|---|---|---|
| fp32 | 677 MiB（709 574 565 B） | ✅ 逐位一致 | ✅ | RSS 783 MB |
| 社区 uint8 | 169.8 MiB | ❌ 数值全错 | ✅ | 快 1.33x vs fp32 |
| 自量化 int8（per-tensor） | 169.8 MiB | ❌ | ✅ | 最快（123 ms/窗） |
| 自量化 int8（per-channel + reduce_range） | 170.2 MiB | ✅ 0.989 | ✅ | fp32 的 2.14x 速度 |
| 4-bit / bnb4（社区仓库里有） | — | 未测 | 未测 | API 需要单独 test |

**[建议]** 不要把 178 MB / 677 MB 打进 Outrim 二进制，也不要指望"自动下载"——这两个都会撞到
"一次自动配置、不折腾用户"的约束。更现实的是：**入口默认不用模型**；若保留模型路径，做成
"可选组件 + 显式下载 + 版本锁定"，并在下载失败时无损放行。

### 7.3 构建与依赖面

`rten` 原型（`rten` + `rten-tensor` + `tokenizers[onig]`）：`cargo tree` **163 个 crate**，
`cargo clean` 后 release 冷构建 **70 s**，产物 **8 481 280 B**。
`ort` 原型（`ort[download-binaries,ndarray]`）：**72 个 crate**，release 72 s，产物 22 MB。

---

## 8. 适配边界

### 8.1 什么时候这套值得用

- **输入形态**：`read` 的大文件、长文档、长日志（行结构清晰、行内冗余高、单行不承担定位语义）。
- **不适用**：grep/find/ls 的紧凑命中列表、unified diff、已截断的 bash 输出、`file:line` 密集的错误块。
- **前提**：宿主钩子拿到的必须是**工具自身截断之后**的内容（Pi 侧 `bash` 50 KB/2 000 行、
  `read` 50 KB/2 000 行、`grep` 50 KB、MCP 20 KB；见 `pi-integration.md` §3）。
  超过上限的原文拿不到，任何"压缩比"承诺都只对 ≤50 KB 成立。

### 8.2 任务上下文：需要什么、代价多少

| 方案 | 需要的上下文 | 获取成本 | 上下文 token 成本 |
|---|---|---|---|
| LLMLingua-2 | **无**（task-agnostic） | 0 | 0 |
| LongLLMLingua | `question` 字符串（**强制**，`prompt_compressor.py:565`） | agent 当前目标或工具入参 | 短（10–30 tok） |
| SWE-Pruner | goal hint（"focus on error handling"） | agent 显式给 | 短 |
| Squeez | 一段 focused query（≤3 000 字符，超出截断；`squeez/inference/extractor.py:_build_messages`） | 工具入参或 agent 目标 | 短 |

**[建议]** 最便宜可得的任务上下文是**工具入参本身**（`path` / `pattern` / `command` / `offset`），
不需要额外模型调用。这也满足"极短原文指针、不写免责声明"的既定决策——
指针本身只要 3 token（8 位十进制句柄，见 `structural-compression.md` §5.3），
**不要把"被压缩了 N 行"这类自述句塞进 prompt**（squeez 的 39-token 标记就是反例）。

### 8.3 外部证据的确切形状（含自相矛盾）

SWE-Pruner 实验设置（论文 §4.2 / Table 3 脚注）：SWE-Bench Verified，**随机 50 例子集**，Mini SWE Agent 作宿主。

| 来源 | Mini SWE Agent | +LLMLingua-2 | +SWE-Pruner |
|---|---|---|---|
| **Table 3**（表格） | 52.3% / 0.972M tok | **42.1% / 0.856M** | 41.1% / 0.670M |
| **正文 §4.2**（文字） | 62% | **54%** | **64%** |

两处对不上：表格里 SWE-Pruner **低于**基线（41.1 < 52.3），正文说**高于**基线（64 > 62）。
这很可能是表格未同步更新（v4 已改过图）。**所以：**

- **可用的方向性结论**：两套数字都说 token 级剪枝（LLMLingua-2）使成功率降 5–10 个点。这个方向与
  论文的定性解释（"token-level pruning disrupts code syntax"）以及 §4.2 我实测的
  `convert_tokens_to_string` 破坏行为**互相印证**。
- **不可用的**：任何绝对数、任何"SWE-Pruner 比基线好 X 点"的说法。
- **不可外推的**：它压的是**整段检索上下文（每轮）**，我们是**入口单次**且可放行原文；它的基线是
  固定 4x/8x 约束，我们允许"不压"。

其他可用于校准的外部数字：

- **read 类操作占 76.1% token**（Figure 2，Claude Sonnet 4.5 + Mini-SWE-Agent）——这是"入口压缩"
  最大受益面的量化支撑，也与"只压工具输出、不压历史"的设计吻合。
- **SWE-Pruner 声称首 token 延迟 < 100 ms**（Figure 4），0.6 B 骨干。我本机 CPU 上对 524-token 对
  实测 447 ms（§6.3），**差 4x 以上**——它的 <100 ms 极可能是 GPU 上的数字，且它按 chunk 并行。
  这说明"0.6 B 也能很快"这类论断必须绑定硬件。

### 8.4 压缩策略的 agent 级系统证据（强度最高的外部证据）

[arXiv:2609.32961](https://arxiv.org/abs/2609.32961)（UT Austin，CC BY 4.0，v1 2026-09-26）把压缩策略分解为
**primitive（怎么删）× trigger（何时删）× depth（删多少）**，在 SWE-bench Verified（100 issue）与
Terminal-Bench 1.0（40 任务）上跑 **65 种策略 × 3 个开源模型 × 3 次重复 ≈ 35 000 次运行**。
它明确列出被调研的生产系统，其中包括 **Pi Coding Agent**。

它的原始话（可直接引）：

- **SWE-bench**：策略用掉约 **0.4–0.7×** token，成功率从 **−11.0 pp** 到 **+0.7 pp**。
  **Tool-result clearing（TRC）**基本持平全上下文，同时 token/latency/计费降到 **0.57× / 0.79× / 0.71×**。
  而 **summarization（SU）**用差不多 token（0.59×）却掉了 **11.0 pp**。
- **Terminal-Bench**：大多数策略压到 **0.25–0.4×**，但 **12 个策略里 11 个成功率降 5–13 pp**，
  且**所有策略都比全上下文更慢**。
- **token 少 ≠ 快**：Terminal-Bench + Qwen 上，用约 1/3 token 的策略可以比不压缩**慢 20–80%**。
- **聚合成功率会骗人**：同样 ✓ 成功率的两条策略可能解出**不同任务**；“相同成功率可以掩盖不同执行成本”
  （TRC 53.7% / OTRC 51.3%，但计费 0.71× vs 0.95×）。
- **跨模型不可移**：同一策略在不同模型上表现可差很多。

**对我们的直接含义（这是本文最重要的一条外部约束）**：

1. **TRC（删整个旧工具输出）是唯一在真实 agent 上“基本不掉点”的 primitive**。它的代价是
   **信息全丢**（占位符 `[output cleared]`），而不是“压得准”。这暗示：在 agent 场景下，
   **“选择性地删掉一部分但保留其余原样”的风险低于“改写/重写整段”**——与我们“行/块选择 + 原文切片”
   的方向一致，与传统 token 删除/摘要方向相反。
2. **“省 token”不能当唯一卖点**。这条在 35 000 次运行上成立：token 下降可能与延迟上升同时发生。
   所以产品指标必须是“**下游是否少跑一轮 / 是否不用重读**”，而不是“压缩比”。详见 §10-1 的标注集建议。
3. **不能拿单一模型的结果外推**。这直接支撑“Pi + OpenCode 双宿主必须各自验证”的工程约束。

**[边界]** 该文的 TRC 作用于**历史**中的旧输出，而 Outrim 只压**入口**、原文仍可恢复。
所以它的数字不能直接当我们的预期；但它的**定性结论（primitive 选择比 depth 更重要）**可直接迁移。

### 8.5 编码器式句子级选择的最新外部证据

[arXiv:2603.09222](https://arxiv.org/abs/2603.09222)（EnComp，CC BY 4.0）做的是
**query 驱动的句子剪枝**：用 **ModernBERT-large** 对**整段上下文一次编码**，就拿到**所有句子**的
边缘贡献分（counterfactual 训练信号 + 对比排序目标），不需要逐句前向。

它的实测（Table 8；RTX 4090，LoRA 微调；5 个 QA 数据集均值，读者 Llama-3.3-70B，k=20）：

| 方法 | 峰值 VRAM (GB) ↓ | EM ↑ | F1 ↑ | 压缩延迟 (s) ↓ | 保留率 (%) ↓ |
|---|---:|---:|---:|---:|---:|
| CompAct | 14.34 | 34.23 | 43.85 | 4.777 | 3.7 |
| Refiner | 18.10 | 32.86 | 42.61 | 4.710 | 5.7 |
| **LongLLMLingua** | **14.48** | 35.06 | 45.55 | **1.113** | 30.1 |
| RECOMP-abs | 6.78 | 32.38 | 42.12 | 1.260 | 1.8 |
| **AttnComp** | **6.80** | **37.16** | **47.65** | 0.125 | 9.3 |
| **EnComp（本文）** | **1.86** | 36.98 | 47.53 | **0.045** | 16.0 |

**[关键可迁移点]** “**一次全上下文编码、同时得到所有句子/行的分**”是它主要的效率来源，
而不是模型更小——它的骨干（ModernBERT-large）**比 mBERT 还大**。
具体地：**0.045 s / 1.86 GB vs LongLLMLingua 的 1.113 s / 14.48 GB**（延迟 ≈25x、显存 ≈7.8x）。
但注意 **它的 EM（36.98）并没有超过 AttnComp（37.16）**，它换来的是资源占用。
→ 对我们的启示：如果要做模型打分，**关键优化是“一次前向覆盖整段”**，
这与 §5.3 第 5 点（`win` vs `ind` 无可测差别）**互相支持**：我不必逐行前向，一次长上下文前向即可，
而且“更省资源”不等于“更准”。

**[不可迁移]** 它是 RAG + 问答，有真 query；我们的 tool output 只有工具入参。
且它的 0.045 s 是 **RTX 4090**，本机 CPU 上同规模骨干（mBERT 数量级）是 **§6.1 的 123–280 ms/508-token**。
**不要把 GPU 秒数当 CPU 预算。**

### 8.6 与宿主契约的硬边界

- 只处理**首次入口**，不改历史 → 压缩必须是**幂等**的（同一 payload 压两次结果一致），否则重试会漂移。
- 失败必须**无损放行**：模型加载失败、超时、"压缩后反而更长"一律返回原文。
  `structural-compression.md` §5.2 的 `is_net_loss()` 判据（来自 squeez `wrap.rs:1078`）可直接复用。
- 一次自动配置、Pi + OpenCode 双宿主 → 压缩器本体应为 **stdio/NDJSON 单进程**，
  不依赖宿主语言、不使用 argv 传 payload（Windows argv 上限）。
- 恢复用**极短指针**，不写长免责声明。

---

## 9. 候选（不排序，只写适用条件）

| # | 形态 | 用模型 | 实测压缩 | 实测代价 | 适用条件 / 已知失效 |
|---|---|---|---|---|---|
| A | **行级词法打分 + 保护行 + 预算**（无模型） | 否 | 见 `structural-compression.md` §0.2 与 §4.3（Kneedle 预算 + 分级选择）；本文 §5.2 的手写正则基线在 23–25 行探针上 recall 高于未条件化模型 | O(n)；`rtk-engine` release 吞吐见 `structural-compression.md` §6 | 需要维护“证据模式”词表；**对未知输出形态可能漏**（Squeez 上通用检索只有 0.22 recall，见 §5.4）；预算器在未重排时会把信号砍到 3%；空白行需单独处理（见 §4.3） |
| B | **LLMLingua-2 行级聚合 + 原文切片 + 字面量保护** | 是（178 MB） | 8 KB 3.84x / 50 KB 3.48x（keep 0.3），保留行字节一致 | 1.4–7.9 s / 8–50 KB；RSS 243–783 MB | **必须**行级聚合 + 切片；**不要**用 `convert_tokens_to_string`；`file:line` 密集时降级放行 |
| C | 任务条件化行选择器（SWE-Pruner / Squeez 形态） | 是（0.6 B / 2 B） | **Squeez：0.86 recall / 0.80 prec / 92% 移除**（618 例自建测试集，LoRA 2B）；SWE-Pruner：23–54% token 减少（绝对成功率数字自相矛盾，见 §8.3） | 0.6 B 本机 447 ms/524-tok 对；2 B 未测 | 需要 focused query；**外部最强行级证据**（远超 BM25 0.22 recall）；Squeez 硬依赖 `openai` 包；论报 <100 ms 首 token 但那是 GPU 量级 |
| D | 检索式（BM25 / embedding 选块） | 可选 | 外部：BM25(10%) recall **0.22**（Squeez Table 4） | 低 | 外部证据明确不支持；除非预算极紧 |
| E | **字面量 / 定位符保护层**（任何形态都要） | 否 | — | O(n) | 必须：错误码、`file:line:col`、路径、端口、版本号、16 进制、UUID |
| F | **一次全上下文编码句子/行打分**（EnComp 形态） | 是（ModernBERT-large 级） | 外部：保留率 16.0%、EM 36.98/F1 47.53（≈AttnComp 37.16/47.65） | 外部 0.045 s / 1.86 GB（**RTX 4090**）；本机 CPU 同规模骨干 123–280 ms/508-tok | 需要 query；代码域未验；关键优化是“一次前向拿全部分数”而非更小模型 |

**[建议]** A + E 是"入口默认路径"的自然候选；B 只在"payload 大且行冗余高"时作为可选增强；
F 给出了“一次全上下文前向”的工程形态，但需接受 CPU 上的 10² ms 级开销。
C 的外部证据只能说明"token 删除会掉点"，不能说明"行级选择会涨点"——SWE-Pruner 自己的表格里
它的成功率就低于全上下文基线。**压缩本身不保证成功；这恰好是"失败即无损放行"必须做硬的理由。**
另需注意：35 000 次运行的系统研究显示，**任何写操作（摘要类）都有 5–13 pp 的掉点风险**，
而**丢信息但可恢复**的做法（TRC）风险最低——这正是“原文可恢复”的价值所在。

---

## 10. 待做的小实验（每项半天内可完成，按依赖顺序）

1. **建标注集**（最高优先，缺它后面都无法判断）：30–50 条真实 Pi/OpenCode 轨迹里的 tool output，
   人工标"哪几行是 agent 后续真正用到/引用的"。用"**同一工具是否被重跑**"作为弱标签先跑起来。
   现在的 §5 结论受循环论证污染，必须先解掉这个。
2. **A vs B 的 head-to-head**：在 1 的集合上，比较 `词法+保护`、`LLMLingua-2 行级+切片`、`firstN`。
   指标只用"证据行 recall"与"下游是否需要重跑"。**不要**用通用 QA 指标。
3. **窗口边界保护**：修掉 §4.3 那 1 条 tab→空格 的行内偏差（在 508 边界回退到行/空白），
   验证"保留行字节一致率 = 100%"。
4. **预算可行性**：确定入口延迟预算（例如 50 ms / 200 ms），反推"多大的 payload 才配得上模型"。
   当前数据点：8 KB 需 1.4 s → 200 ms 预算只能处理 ≈1.2 KB，等于没有收益。
5. **`ort` 静态二进制在 macOS arm64 / Linux 上的产物大小与 `download-binaries` 行为**（未测）。
6. **4-bit 工件**（社区仓库有 `_q4.onnx` / `_bnb4.onnx`）在 `ort` 上的速度/精度权衡（未测）。
7. **有没有第三种 primitive 被系统性漏掉**：`arXiv:2609.32961` 只考察了 TR / TRC / SU 及组合，
   **没有考察“行级选择性保留”（本文 §4.3 的形态）**。这是一个真实的文献空白，也是我们要自己测的原因。

---

## 11. 未核实 / 未知

1. **“行级选择性保留”在真实 agent 上没有任何公开测量**。最强的外部研究（≈35 000 次运行，
   [arXiv:2609.32961](https://arxiv.org/abs/2609.32961)）只测了 TR/TRC/SU 三类，**未测**本文 §4.3 的形态。
   这是真实的文献空白，也是为什么本文只有小探针而不能下结论。
2. **rten 量化失败的算子级根因未定位**。§7.1 给的是"可复现判据 + 官方注释里的已知风险"，
   不是证明。要定位需要 layer-by-layer 对比（我只在早期对个别层做过 cos 比较，未系统化）。
3. **`ort` 的 `download-binaries` 在 CI/离线环境的行为未测**；本机是首次运行自动下载（缓存于
   `C:/Users/wudon/AppData/Local/ort.pyke.io/`）。
4. **SWE-Pruner 的 n = 50，且 Table 3 与正文数字矛盾**（§8.3），两套都对不上。它的基线是固定 4x/8x
   约束下的**每轮**压缩，与我们"入口单次、可放行原文"不同构。**不要引用任何绝对成功率。**
5. **Squeez 的所有数字来自其自建 618 例集**，无第三方复现；2 B 生成模型在 CPU 上的延迟我**没有测**。
6. **没有**任何公开 benchmark 覆盖"真实 coding-agent tool output 上的行级选择性保留"。
   本文 §4/§5 的自测 n 极小（23–1 127 行），只能当探针。
7. **XLM-R-large（MIT，559 M）未测**：更准但更慢，且 2.2 GB 工件对分发不可行。
8. **ModernBERT reranker（`gte-reranker-modernbert-base`，apache-2.0，598 436 708 B）未完成测量**：
   探针在 8 000-token 单对上 OOM（`BFCArena` 请求 24.6 GB），需要改成短对才能测。
9. **LongCodeZip 无公开 checkpoint**，只有论文数字（SWE-Pruner Table 4 里 8x 约束下实际 7.85x 压缩，
   论文自述为“leverages program structure”，不属本文语义路径）。
10. **`llmlingua` 官方包未实跑**（无 torch），§4.2 是算法等价复现。
11. **`force_tokens` 在官方 API 里的确切语义**（是否也影响最终拼接）我按源码读了但未实跑验证；
    我的复现是"词级给 p=1.0"，若官方还有别的处理，结论可能变。
12. **EnComp 的 0.045 s 是 RTX 4090 上的数字**；本机无 GPU，未做 CPU 复现。
    它的“一次全上下文编码”能否在 CPU 上于 200 ms 内处理 12 890 token，**未知**。
---

## 附录 A：探针脚本与数据（均在 `C:/Users/wudon/AppData/Local/Temp/`，非仓库内容）

| 脚本 | 作用 |
|---|---|
| `sem2/bench_sem.py` | 首个延迟/单窗存活/AST 探针（`keep()` 统计口径有 bug，仅作历史） |
| `sem2/bench_damage2.py` | **库算法等价复现**（merge→mean→percentile→detokenize），exact/loose 标识符、数字、AST、行数 |
| `sem2/bench_forcetokens.py` | `force_tokens` 各变体对行结构的效果 |
| `sem2/bench_spansplice.py` | A/E1/E2/E3 四种"原文字符切片"变体（E3 = 行级 + 保护 span） |
| `sem2/bench_variants.py` | A/A+protect/B 行/B+protect（行内阈值但保留库 detok） |
| `sem2/bench_linescore.py` | 行级打分、保留行字节一致性、与词法启发式的 Jaccard、pytest 日志逐行分数 |
| `sem2/bench_linefair.py` | §5.2 的 win/ind/lexical/firstN/lastN/random 对照 |
| `sem2/bench_final2.py` | 早期版本的行级对照（口径不如 `bench_linefair.py`，保留以追溯） |
| `sem2/score_rust.py` | 给 Rust 原型输出打"保留行字节一致 / 行内损坏 / 标识符 / 数字 / AST"分 |
| `sem2/isolate_quant.py` | 隔离量化算子（`MatMulInteger` / `DequantizeLinear` / QDQ）在 rten vs ORT 的一致性 |
| `sem2/fetch.py` | 抓 arXiv HTML 到 `p_<id>.html` 并按上下文 grep |
| `rtenprobe/src/main.rs` | Rust 端到端原型：`tokenizers` → rten 逐窗 → 行级/全局阈值 → 原文切片（3 种 mode） |
| `rtenprobe/src/bin/dump.rs` | 导出逐 token `p_preserve`（用于 rten vs ORT 对齐比较） |
| `rtenprobe/src/bin/dq.rs` | 单输入最小图跑 rten（量化算子隔离） |
| `ortprobe/src/main.rs` | `ort` crate 的单窗延迟基准（多模型） |
| `C:/Users/wudon/AppData/Local/Temp/mb/rb3.py` | Qwen3-Reranker 形态的 (query,doc) 对延迟（大输入 OOM，见 §11-7） |

**数据文件**：`sem2/damage.json`、`sem2/spansplice.json`、`sem2/variants.json`、
`sem2/line_ranking.json`、`sem2/final_eval.json`；模型工件 `ll2/`（uint8）、`ll2fp32/`、`ll2i/`、
`ll2rr/`、`qgrid/`（4 种量化组合）、`tb/`、`mb/`；逐 token 概率 `rt_probs_{fp32,atjsh,int8self,rr}.txt`、
`rt_q_rr*.txt`。

## 附录 B：复现注意

- token 计数一律 `cl100k_base`；**与 Claude 实际 tokenizer 无关**，只用于相对比较。
- `onnxruntime` 的 Python 侧 `intra_op_num_threads=8`；`ort` crate 侧 `with_intra_threads(8)`；
  `rten` 无显式线程数配置（内部 `rten-parallel`）。
- 量化图由 `onnxruntime.quantization.quantize_dynamic` 生成，参数见 §7.1 表格；
  复现命令：`quantize_dynamic(fp32, out, weight_type=QuantType.QInt8, op_types_to_quantize=["MatMul"],
  reduce_range=True, per_channel=True)`。
- `quantize_dynamic` 对 677 MiB 输入约需数分钟、峰值内存 >4 GB；`qgrid/` 四张图合计 1.7 GB。
- 所有 RTK / 结构性数字一律引用 `docs/research/structural-compression.md`（`research/structural-compression` 分支），本文不重复测量。
- 本文与 `structural-compression.md` 的 token 计数口径一致（同一 `cl100k_base`），因此两边的压缩比可以横向比较。
