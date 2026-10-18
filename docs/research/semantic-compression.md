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

**1. 在本报告复刻的 LLMLingua-2 流水线上，按 token 保留/删除会破坏 tool output 的行内结构；该行为可追溯到库的最终拼接步骤（按源码阅读），但结论限定于所测 mBERT 量化图 + 复刻流水线。**

用按其源码实现的近似复刻（logits→`p_preserve`→WordPiece 合并成词→全局百分位阈值
→`tokenizer.convert_tokens_to_string`；**尚未与官方 API 对拍**，见 §2 末）
压 `llmlingua/utils.py`（8095 字符 / 2746 token）：32 个 snake_case 标识符 **0 个按原样保留**，
`ast.parse` **失败**，整份文件被压成 **1 行**（原 245 行）。加模型卡推荐的
`force_tokens=['\n','.','!','?',',']` 也**未**救回行结构（仍 1 行、仍 0/32；`force_tokens` 官方语义未对拍，见 §11-11）。
这些数字只说明所测配置（mBERT-base int8 图 + 该复刻流程）的表现，不能当成库所有配置的结论。见 §4.2。

**2. 把"删 token"换成"整行保留/丢弃 + 原文切片"，可以避免「被选中片段」的行内改写；这不等于"删除内容无损"或"语义正确"。**

只在行级聚合后用**原文切片拼接**（不重新 detokenize），能保证选中的行不被改写。
**代价是未选中的整行被丢弃**，且仍有边界偏差：

| 实现 | 与原文某行字节相同的保留行 / 非空行总数 | 行内损坏 | 标识符是否全在 |
|---|---|---|---|
| Python 复刻 | **74/74**（keep 0.3）、**122/122**（keep 0.5） | 0 | — |
| Rust 端到端原型（8 KB） | 80/217（keep 0.3）、122/217（keep 0.5） | 1 | 16/32（keep 0.3）、28/32（keep 0.5） |
| Rust 端到端原型（24 KB / 50 KB） | 157/431、260/431 / 424/1071、673/1071 | 0–1 | — |

（上表仅取有证据的项：Python 取自 `bench_linescore.py`；Rust 侧的列与 §4.3 表同源（`final_rust_p_*` + `score_rust.py`），
**行内损坏与丢失标识符如实保留**，不隐去。）
cl100k 压缩比 8 KB→3.84x、50 KB→3.48x（keep 0.3）。**这是抽被选取的原文片段，不是能重建原文。** 见 §4.3、§4.4。

**3. 在两个自定义小探针上，没有观察到模型打分相对手写词法规则的明确优势（n 极小、标注与基线同源，不构成普遍结论）。**

在 cargo 错误（n=23 行）与 pytest 失败日志（n=25 行）上，模型行为见 §5.2。
（曾记“行级 AUC 0.667”，但该值**未留存可复算的脚本/输出**——`sem2/*.json` 里没有 AUC 字段——
故本文不把它当作可核验数字。）
显式词法规则（含错误码 / `file:line` / 数字 / 关键字密度）在 recall 与 purity 上不低于模型。
模型打分与一个**长度/信息量代理**（`bench_linescore.py:75`，`len + 4×标点 + 6×数字`）的 top-30% 行集合
Jaccard 为 **0.147**（源码）/ **0.103**（grep 命中）——两边选得不一样，
**但 Jaccard 低并不证明模型“不挑长行”，也不证明它在“挑连贯句子”**；那只是我当时的推测，未经检验。
**注意**：我的“证据行”标注和词法基线用了同一个正则，所以词法胜出部分是构造性的；
该结果只能支撑“**这两个自定义探针**未观察到明确优势”，不能支撑“模型未提供额外信号”或
“模型比最好的词法规则差”。更强的反例见 §5.4。

**4. Rust 侧能跑，但“能跑”取决于量化方案，不是取决于模型。**

- `rten 0.27.0`（MIT OR Apache-2.0，rust 1.94）：**fp32 ONNX 与 ONNX Runtime 逐位一致**
  （logits corr = 1.000000、MAE = 0.0）；**ORT 默认 dynamic int8 量化图在 rten 上数值全错**
  （corr = −0.022，keep 决策一致率 0.47–0.57）。
- 只有“per-channel + `reduce_range=True` 自己量化”的 178 MB 图能对齐（corr = 0.9896，keep 一致率 0.95–0.97），
  且这张图在 ORT 上比社区 int8 图快 **1.6–2.2x**。
- `ort 2.0.0-rc.14`（MIT OR Apache-2.0）能正确跑所有量化图，产物是 **22 MB 静态 exe + 不需要外部 onnxruntime.dll**
  （拷到空目录仍可运行），代价是首次 `load_ms` 317–794 ms。
见 §7。

**5. 本机实测是秒级（8–50 KB 需 1.4–7.9 s）；是否可接受取决于未定的入口延迟预算。**

本机 BERT-base int8（178 MB）单 508-token 窗口：ORT 123–208 ms（0.24–0.41 ms/token）、
rten fp32 278 ms。端到端 Rust 原型（分词→逐窗前向→行级阈值→切片）：
8 KB/2 746 tok **1.37–1.73 s**、24 KB/5 999 tok **2.97–3.85 s**、50 KB/12 892 tok **6.63–7.91 s**；
加载 267–421 ms；峰值 RSS fp32 **783 MB** / 量化图 **243 MB**。见 §6。
**这些只说明“在这台机器上的这个实现有多快”，不说明它超了某个已定预算——可接受的入口延迟尚未定。**

**6. 与外部证据的正面冲突（§5.4）：我的探针不能证明“模型无用”。**

Squeez 在其 618 例自建测试集上：微调 2B 行选择器 **recall 0.86 / prec 0.80 / 移除 92%**；
同一张表里该论文的 **BM25(10%) 基线只有 recall 0.22**。我在两个 ≤23 个非空行的探针上看到“手写正则不低于未条件化模型”，
但这两种“词法”不是一回事（前者是**按本文探测集手写的领域正则**，后者是该论文自己的检索/位置基线）。
**结论只能是：未条件化的 LLMLingua-2 在「本文两个探测集」上没观察到超出手写领域正则的优势；
不能说模型无用。** 见 §5.4。

**7. 本次检索找到一条把 LLMLingua-2 接到 coding agent 上的评测，结果是负面的——但该论文自身数字自相矛盾。**

SWE-Pruner（[arXiv:2601.16746v4](https://arxiv.org/abs/2601.16746)）Table 3（SWE-Bench Verified，50 例子集）：
Mini SWE Agent 52.3% / 0.972M token → **+LLMLingua-2 42.1% / 0.856M**，+SWE-Pruner 41.1% / 0.670M。
但**同一篇正文写的是**"our method achieves the best success rate (**64%**), outperforming the
vanilla agent baseline (**62%**)"，且说 LLMLingua2 掉到 **54%**、RAG 掉到 50%。
**表格与正文两套数字互不相容**（52.3/41.1/42.1 vs 62/64/54），我无法判断哪个对应最终实验。
两套数字的**方向一致**：加 LLMLingua-2 后成功率下降 5–10 个点。这是"通用 QA 结论不能当 agent 证据"的
直接反例，但**不要引用其中任何一个绝对数**，见 §8.3。

**8. 一篇大规模 agent 级研究（≈35 000 次运行）在「其测试矩阵内」显示：primitive 的选择比“删多少”更重要。**

[arXiv:2609.32961](https://arxiv.org/abs/2609.32961) 在其测试矩阵（SWE-bench Verified + Terminal-Bench，
3 个开源模型，65 种策略）内报告：
Tool-result clearing（**删掉历史中的旧工具输出**、插 `[output cleared]`）在 **SWE-bench** 上基本持平基线
（token/latency/计费 0.57×/0.79×/0.71×）；而 summarization 用几乎同样多 token（0.59×）却掉 **11.0 pp**。
**Terminal-Bench 上** 12 个策略里 11 个降 5–13 pp，且**全部变慢**（token 少 ≠ 快，可慢 20–80%）。

**边界（必须同时读）**：TRC 删的是**历史里的旧输出**，且那些输出**模型先前已经看过**；
它不是“首次入口选择”，也**本身不具可恢复性**（只留占位符）。
因此这组结果**不能**直接推出“行级入口选择更安全”或“原文可恢复有实证价值”——
后者在本报告中仍只是一个设计意图，**没有外部实验支持**。见 §8.4。

**9. 本次检索与本地探针范围内的形态候选（不是优先级）。**

在本文考察的候选中，最值得继续验证的是**行/块级 extractive 选择 + 原文切片 + 字面量保护层**；
“token 删除”在本复刻流水线上已观察到行内破坏，“latent / 软提示”需改推理栈（§1.4）。
后两者是否“都不应进流水线”，超出本文证据范围——本文只能说明它们在**本产品这一形态**上没显示出优势。
如果做行级选择，建议**先建一个小的真实轨迹标注集**（见 §10-1），再判断要不要模型——
否则无法区分“模型有用”与“我的正则恰好写出了同样的东西”。
另外，本文**未找到**覆盖“本产品这种首次入口压缩配置”的公开评测：上述 35 000 次运行的研究只测了
TR/TRC/SU 三类，Squeez / SWE-Pruner 做的是任务条件化的每轮检索/剪枝，都不是同一配置。

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

- **A 类：纯文本进、纯文本出。** 任何 hosted LLM 都能吃，Outrim 只需一个本地进程。
  候选 A、B、D、E 与 F 的部分形态都在这一类。
- **B 类：需要改模型或推理栈**（soft prompt / KV 压缩）。出范围，理由见 1.4。
- **C 类：需要额外服务或外部权重下发**（2B 生成式 Squeez 形态、需要 GPU 才有合理延迟的前向）。
  候选 C 的 Squeez 分支属这一类；它的 SWE-Pruner 分支（0.6 B reranker）在本机 CPU 上可本地跑，属 A 类但延迟见 §6.3。
  **stdio/NDJSON 单进程是本文的建议形态，不是用户已定的硬边界**（见 §8.6）。

### 1.3 与"任务无关 vs 任务相关"正交

LLMLingua-2 是**任务无关**（task-agnostic）：只有上下文，没有 query。
LongLLMLingua / SWE-Pruner / Squeez 是**任务相关**：需要一个 question / goal / query。
这个维度决定了"要不要在压缩器里塞任务上下文"，见 §8.2。

### 1.4 为什么本文不把 latent 方案当作候选（带证据，不靠直觉）

`arXiv:2605.11051`《On Problems of Implicit Context Compression for Software Engineering Agents》
报告的结论是：In-Context Autoencoder 一类隐式压缩在**单轮代码理解**上可用，
在**多步 agentic coding** 上失效。叠加“只处理首次入口、不改历史”的设定后，本文看不到它的优势：
入口压缩的价值来自“少喂无用 token”，而 latent 方案额外要求宿主换推理栈。
**这是本文在“不改宿主推理栈”这个约束下的取舍，不是“latent 方案无价值”的结论。**
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
| 小模型（**许可未核实，暂不列为候选**） | [`JueZhang/lingua2_tinybert_meetingbank_only`](https://huggingface.co/JueZhang/lingua2_tinybert_meetingbank_only) | 元数据 `license: null`（不等于无授权） | 14 253 218 参数；uint8 ONNX 14 442 511 B；下载 7；mobilebert 版同样未声明 |
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
**能说的**：官方 checkpoint 是在 MeetingBank 会议转写构造的数据上微调得到的，所以**没有证据表明**它针对代码/日志分布调过。
**不能说的**：这并不能证明它的预训练阶段没见过代码与日志——微调数据不等于预训练数据，本文未核查其预训练语料。

**[本次检索范围]** 我查了 HF 搜索（`llmlingua` / `lingua2` / `lingua`），除这两个官方 checkpoint 外只看到第三方 fork；
`slingua` 0 结果、`securitylingua` 2 条。**这是“本次未找到经核验可直接采用的工件”**，
不是“不存在可用权重”——HF API 的 `license` 字段为空只说明**元数据未填**，不等于实际无授权，
需逐个读仓库 LICENSE / README 才能定。因此：
“用 tinybert 尺寸换 10x 速度”这条路**目前没有已核验许可可用的工件**，本文**暂不作为候选**；
[`JueZhang/lingua2_tinybert_meetingbank_only`](https://huggingface.co/JueZhang/lingua2_tinybert_meetingbank_only)
HF API 显示 `license: null`、14 253 218 参数、下载 7（**许可未核实**）。
14 MB 的 tinybert 我只用来自证“速度上限在哪”，**不作为候选**。

**[未复验]** 官方 Python 包本体（`pip install llmlingua`）未在本机跑起来（无 torch）。我改为
**按源码实现的近似复刻**（`__compress` / `__merge_token_to_word` / `__token_prob_to_word_prob`）
并用同一 checkpoint，**尚未与官方 API 对拍**。因此 §4.2 的破坏性结论只能归到
“本文复刻流水线 + 所测 mBERT 量化图”这一配置，**不能当成库在所有配置（含各类参数组合）下的已验证行为**。
具体地：`force_tokens` 的官方语义我未实跑验证（§11-11），它的实际行为可能与我的复刻不同。

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

**这一条是按源码推断，未经官方 API 实跑验证**（本文复刻复现了同样的现象，见 §4.2）。
`rate` 在库里是"保留比例"，`reduce_rate = 1 - rate`（`:930`）；`target_token>0` 时
`rate = min(target_token/n_original_token, 1.0)`（`:923`）。

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
subsequence recovery），但它的参考实现不适合直接搬到 Outrim 的入口路径**：7B 模型逐 token 算 PPL
在本机（16 GB RAM、CPU）上跑不动；而且它压的是历史/多文档，不是单个 tool output。
**这是“参考实现 + 本机硬件”的组合限制，不是“模型普遍不能上 CPU”的结论。**

---

## 4. 实测：破坏面

### 4.1 探针与口径

| payload | 字符 | 行 | BERT tok | cl100k tok |
|---|---:|---:|---:|---:|
| `payload_utils.py` = 真实 `llmlingua/utils.py` 全文 | 8 095 | 245 | 2 746 | 2 015 |
| `rust_p_8k.txt`（同上，切片为正好 8 KB） | 8 095 | 245 | 2 746 | 2 015 |
| `rust_p_24k.txt`（真实 `prompt_compressor.py` 切片） | 24 576 | 452 | 5 999 | 4 821 |
| `rust_p_50k.txt`（同上前 50 KB） | 51 200 | 1 127 | 12 892 | 10 285 |
| `pay_cargo.txt` = 真实 `cargo build` 类型错误（我故意改坏一个 crate 得到） | 918 | 24（非空 21） | 331 | 264 |
| `pay_pytest.txt` = 真实 pytest 失败格式 | 1 270 | 26（非空 21） | 767 | 279 |

"标识符"= 文本里出现的 snake_case 词；"数字"= `0x…` 或多位十进制；"行保留"= 输出行与原文某行**字节相同**。

### 4.2 按源码实现的近似复刻（token 删除 + detokenize）

探针 `bench_damage2.py`，**尚未与官方 API 对拍**。

**口径修正（已查脚本）**：我的探针参数名写的是 `rate`，但 `bench_damage2.py:51` 实际执行
`np.percentile(wq, int(100*rate)+1)`，即它是**删掉比例**。库里的对应参数是
`reduce_rate`，而对外 API 的 `rate` = 保留比例，`reduce_rate = 1 - rate`
（`prompt_compressor.py:872,930,2319,2386`）。所以：

| 本文探针的 `rate` | 等价于库 API 的 `rate` |
|---|---|
| 0.5 | 0.5 |
| **0.7** | **0.3**（保留 30%） |

→ 表中“0.7 行压缩更大”与“`rate` 是保留比例”**不矛盾**，因为本文的 0.7 是删掉比例；
但**本文表格的列名会误导**，应以库 API 名重新理解。“0.5”一行两边一致，结论不受影响。

| 本文 `rate`（=删掉比例） | 模型 | 字符比 | 标识符 原样 | 标识符 去空白后 | 数字 | 输出行数 | `ast.parse` |
|---:|---|---:|---:|---:|---:|---:|---|
| 0.5 | mBERT int8 | 1.83x | **0/32** | 30/32 | 2/2 | **1**（原 245） | **SyntaxError** |
| 0.5 | tinybert int8 | 1.67x | **0/32** | 21/32 | 2/2 | **1** | **SyntaxError** |
| 0.7 | mBERT int8 | 2.68x | **0/32** | 26/32 | 2/2 | **1** | **SyntaxError** |
| 0.7 | tinybert int8 | 2.68x | **0/32** | 15/32 | 2/2 | **1** | **SyntaxError** |

加 `force_tokens=['\n','.','!','?',',']`（模型卡推荐值，探针 `bench_forcetokens.py`）：
rate 0.5 仍 **1.81x / 1 行 / 0-32 标识符 / AST 失败**；再加 `_` 仍 1.72x / 1 行 / 0/32。
**注**：`force_tokens` 在官方 API 里的确切语义我未实跑验证（§11-11），因此
“force_tokens 也不起作用”只能归到**我的复刻实现**，不能归于官方库行为。
我复刻里观察到的原因是 §3 第 6 步（`convert_tokens_to_string` 把整段并成一行，
`force_tokens` 只在词级生效），但这个解释本身也未经官方对拍。

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
（`rtenprobe/src/main.rs`，tokenize → 逐窗 rten 前向 → 行级均值 → 百分位 → 原文切片）。

**先说清它保证什么、不保证什么**：原文切片保证的是 **被选中片段不被行内改写**；
它**不**保证“删除的内容无损”，也**不**保证语义正确。本节所有数字只能支持前者。
Rust 版：

| 输入 | keep | 字符比 | cl100k 比 | 输出行数 | 保留行字节一致 | 行内损坏 |
|---|---:|---:|---:|---:|---:|---:|
| 8 KB（8 095 字符 / 217 非空行） | 0.3 | 3.56x | **3.84x** | 243 | 80/217 | **1** |
| 8 KB | 0.5 | 2.07x | **2.14x** | 243 | 122/217 | **1** |
| 24 KB（24 576 字符 / 431 非空行） | 0.3 | 4.16x | **4.09x** | 447 | 157/431 | 0 |
| 24 KB | 0.5 | 2.24x | **2.21x** | 450 | 260/431 | **1** |
| 50 KB（51 200 字符 / 1 071 非空行） | 0.3 | 3.61x | **3.48x** | 1 122 | 424/1071 | 0 |
| 50 KB | 0.5 | 2.17x | **2.07x** | 1 125 | 673/1071 | 0 |

标识符：8 KB/rate 0.5 = 28/32 仍在输出里（rate 0.3 = 16/32，因为整行被丢）。
“保留行数”几乎不掉是因为**空白行也被计入**——这是一个真实的误导陷阱，我把数字算准了：

| payload | keep | 输出行 | 其中空白/仅空白 | 空白占行 | 空白占字符 |
|---|---:|---:|---:|---:|---:|
| 8 KB | 0.3 | 243 | 178 | **73.3%** | 7.8% |
| 8 KB | 0.5 | 243 | 135 | 55.6% | 3.4% |
| 24 KB | 0.3 | 447 | 318 | **71.1%** | 5.4% |
| 24 KB | 0.5 | 450 | 235 | 52.2% | 2.1% |
| 50 KB | 0.3 | 1 122 | 801 | **71.4%** | 5.6% |
| 50 KB | 0.5 | 1 125 | 590 | 52.4% | 2.5% |

→ **两种口径差异很大**：按“行数”看，压缩几乎是假的（七成保留行是空白）；
按“字符/token”看，空白只占 2.1–7.8%，所以上表的 3.48x/3.84x **总体压缩比是真的**。
结论：**空白行不构成 token 成本，但会污染任何以“行数”为口径的指标**。
“保留行字节一致”一列的分母已经是**非空行**数（217/431/1071），未被空白污染；
被污染的是“输出行数”一列和任何按行算的压缩率。
注：这一点 `structural-compression.md` 未正面处理，属本报告新增发现。

本次记录到 1 条“行内损坏”，来自 **Rust 端到端原型**（已核对输出文件；Python 版在本组参数中未观察到）：
`rust_p_8k.txt:49` 的 `        )  # add special tokens` 在 `final_rust_p_8k_0.3.txt:49`
与 `final_rust_p_8k_0.5.txt:49` 都变成 `        ) # add special tokens`（两个空格塌成一个）。

**根因已定位，不是窗口边界**：`rtenprobe/src/main.rs:134-163` 的拼接逻辑（关键分支在 `:154`），在两个相邻 kept token
**之间**若 gap 里没有换行符，就固定 `out.push(b' ')`——即**任何行内多空格 gap 都被规整为一个空格**。
本例的 gap 是 `) ` 与 `#` 之间的 `"  "`（token 394 结束于 1312，token 395 起于 1314），
token 394–399 全在第 1 个 508 窗内，与窗口切分无关。
只要切片器的实现是“按 token 重建行内空白”，这个偏差就必然存在；正文按 token 切片、再靠 gap 重建空格，
本质上是**行内重建**，不是纯原文切片。
**修法**：同一行内相邻 kept span 之间应直接取 `text[l..*s]` 原文，不要替换成单个空格。
Python 版（`bench_linescore.py`）按整行取原文切片，因此天然没有这个问题。

Python 版复现（`bench_linescore.py`，keep=0.3）：245 行 → 74 行，**74/74 行字节一致**、3.30x；
keep=0.5 → 122/122 字节一致、1.86x；grep 型 payload keep=0.3 → 3.37x、32/32 字节一致。
**两面都要看**：Python 版在上述参数下未观察到行内偏差；
Rust 版则出现了 1 条行内损坏（上例）与 16/32–28/32 的标识符保留。
→ **“原文切片不改写选中片段”在 Python 版这组参数上成立；Rust 原型的行内重建偏差仍在（根因已定位，见上）。**

### 4.4 真实错误输出上的表现（最关键的一张表）

`pay_cargo.txt`（真实 `cargo build` 错误）与 `pay_pytest.txt`（真实 pytest 失败格式），
Rust 行级模式，含 `-->` / `file:line` / 错误码：

| payload | keep | cl100k 比 | 保留原行 | 丢失的证据项 |
|---|---:|---:|---:|---|
| cargo 错误（918 字符 / 21 非空行） | 0.3 | 2.78x | 3/21 | **丢 `-->`、丢 `src\main.rs:27:19`、丢 `try_into`** |
| cargo 错误 | 0.5 | 2.08x | 8/21 | 无 |
| pytest 失败（1 270 字符 / 21 非空行） | 0.3 | 2.63x | 6/21 | **丢 `DID NOT RAISE`、丢 `pytest.raises`** |
| pytest 失败 | 0.5 | 1.71x | 7/21 | 无 |

keep=0.3 的 cargo 输出实样（95 cl100k token）：

```
Compiling rtenprobe v0.1.0 (C:\Users\wudon\AppData\Local\Temp\rtenprobe)
error[E0308]: mismatched types
27 | let _x: u64 = text.len();
error[E0425]: cannot find function `compress_tool_output` in this scope
28 | let _y = compress_tool_output(&text, rate);
Some errors have detailed explanations: E0308, E0425.
```

错误码、行内容、`error[N]` 都在；**`-->` 与 `file:line:col` 没了，缩进与相邻性也没了**
（上例为排版整理；原始输出里保留行之间夹着大段空行，`error[E0308]` 与它的 `27 |` 行被空行隔开）。
对"agent 要去改文件"的场景这是净损失：`-->` 那一行是主要的定位锚。
**结论**：行级选择需要在行内做"字面量/定位符保护"；在 `file:line` 密集的 payload 上应降低压缩强度。

### 4.5 哪些 payload 形态值得优先验证、哪些暂不建议

下表是**基于本文探针的候选判断**，不是普遍定理，也不是用户已定的产品策略。

| payload 形态 | 行级可压性（本文观察） | 依据 |
|---|---|---|
| `read` 大文件 / 长文档 | 较高（8 KB 3.8x、50 KB 3.5x） | §4.3 |
| 真实构建错误（n 小） | 中（keep 0.5 未观察到丢证据，0.3 丢定位） | §4.4 |
| grep 命中（43 行 9 608 字符） | 低—中（3.37x 但每行都是命中） | §4.3 |
| `ls` / `find` 列表 | 低（行内重复前缀，属结构性折叠） | `structural-compression.md` §4.4、§4.5 |
| diff / 已截断的 bash（50 KB / 2 000 行） | **保守策略：暂不压**（`--`/`++`/`@@` 与行号本身就是证据） | 本文判断，待真实轨迹验证后可能推翻 |

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

| payload | keep | 行/证据（非空行/证据行） | 模型 `win` | 模型 `ind` | 词法 | firstN | lastN | random |
|---|---:|---|---|---|---|---|---|---|
| cargo 错误 | 0.2 | 23/8 | 0.12/0.20 | 0.12/0.20 | **0.62/1.00** | 0.25/0.40 | 0.25/0.40 | 0.25/0.40 |
| cargo 错误 | 0.3 | 23/8 | 0.25/0.29 | 0.38/0.43 | **0.88/1.00** | 0.38/0.43 | 0.25/0.29 | 0.12/0.14 |
| cargo 错误 | 0.5 | 23/8 | 0.62/0.42 | 0.75/0.50 | **1.00/0.67** | 0.50/0.33 | 0.50/0.33 | 0.75/0.50 |
| pytest 失败 | 0.2 | 25/5 | 0.20/0.20 | 0.20/0.20 | **1.00/1.00** | 0.00/0.00 | 0.60/0.60 | 0.20/0.20 |
| pytest 失败 | 0.3 | 25/5 | 0.80/0.50 | 0.60/0.38 | **1.00/0.62** | 0.00/0.00 | 1.00/0.62 | 0.60/0.38 |
| pytest 失败 | 0.5 | 25/5 | 1.00/0.42 | 1.00/0.42 | 1.00/0.42 | 0.00/0.00 | 1.00/0.42 | 0.40/0.17 |

**数据来源需注明**：本表取自 `bench_linefair.py`（`linefair.json` 风格输出），其“证据行”正则
（`bench_linefair.py:78`，含 `DID NOT RAISE`、`^\s*[<>] ` 等）在 pytest payload 上给出 **5** 条 gold。
另一支探针 `bench_final2.py` 的 `EVIDENCE`（`:55`）少了 `DID NOT RAISE`，同一 payload 给出 **4** 条 gold，
于是同一测试上的绝对数变成 model 0.25/0.20、lexical 0.75/0.60（keep 0.2）等。
**两套 gold 都不等于人工标注**，本报告不据此比较模型与词法的高下；只用它说明
“**在本探针上模型都没超过同源正则**”这一方向。

### 5.3 这张表怎么读（必须读）

1. **n 极小**：cargo 是 23 行、pytest 是 25 行（各自的非空行见 §4.4）。
   这不是 benchmark，是探针。任何"X 比 Y 好"都不成立。
2. **循环论证**：我的"证据行"正则与"词法基线"的正则几乎同一组模式，所以词法赢有构造性成分。
   在该标注下，它能支撑的结论只有一句：**这两个自定义探针未观察到模型打分的明确优势**。
   它**不能**支撑“模型未提供额外信号”——标注与基线同源，这个对比本身就是自证的。
3. **非循环的那一项**：行级 AUC（模型分数 → 证据行）当时算得 0.667（cargo）。
   **但这一数字在本机临时目录里没有对应的脚本或输出留档**（`sem2/*.json` 无 AUC 字段），
   所以只能当作“当时看起来弱有信息”的备注，不足以支撑任何结论；若要引用需重算。
4. **两边选出的行集合确实不同**：top-30% 行集合 Jaccard = **0.147**（源码）/ **0.103**（grep）。
   **这个对比的另一方不是 §5.2 的领域正则**：`bench_linescore.py:75-77` 的 `lexical_scores` 是
   `len(line) + 4×标点数 + 6×数字数`，本质是个**长度/信息量代理**。
   所以它只能说明“模型选的行与一个长度启发式选的行差别很大”；
   它**不**证明模型“不挑长行”（未与纯 `len` 单独比过），也**不**证明模型“挑连贯句子”——
   后者只是当时的推测，未经检验。
5. `win` vs `ind` 在 cargo 上 `ind` 略好、pytest 上 `win` 略好，**方向不一致**。
   这只能说明“**本探针上**未观察到跨行上下文的稳定收益”，
   **不能**推出“逐行独立打分不吃亏”，更不能推出“一次长上下文前向必定等效”。
   这两种实现形态的等价性仍需测（见 §10）。

### 5.4 与外部证据的正面冲突（必须处理）

我的 §5.2 观察（“手写正则不低于未条件化模型”）与 Squeez Table 4 **方向相反**：

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

1. **两者不是同一个“词法”。** 该论文的基线是它自己的检索与位置启发式（First-N / Last-N / Random）；
   我 §5.2 的“词法”是**按本次探测集手写的领域正则**（错误码、`file:line`、数字、关键字密度）。
   两种基线名同实异，可比性有限。
   （**措词修正**：本文先前把该论文的 BM25 基线称为“查询无关的检索”，
   这是不恰当的——BM25 能否用上 query 取决于具体实现，本文未核对其实现。
   下文一律称“**该论文的 BM25 基线**”。）
2. **它同时是一个警告**：我的手写正则就是**人工注入的领域知识**，而该论文的结论是
   微调过的模型能学到比它那几个启发式好得多的 tool-specific extraction policy（0.86 vs 0.22 recall）。
   → **“手写词法能不能覆盖未知输出形态”是开放问题**，而那正是微调模型可能胜出的地方。
3. **任务条件化是真实差异**：Squeez 有 focused query，我无任务上下文。§5.2 的对比**并未测试
   “模型+查询” vs “词法”**，所以我的表不能用来否定 Squeez。
4. **数据集性质不同**：Squeez 的 gold 是人工标的**逐字证据行区间**（11 477 训练 / 618 测试，
   由 SWE-bench 仓库交互 + 合成多生态 tool output 构成），且强调保留“verbatim evidence block”；
   我的“证据行”是我自己用正则定的。→ **我的标注不是 ground truth**。

**修正后的结论**：
- 可以说：**在本机两个 ≤23 个非空行的小探针上，未观察到未条件化 LLMLingua-2 超出手写领域正则的明确优势。**
- **不能**说：“词法优于模型”、“模型无用”。该论文自建 618 例上，微调 2B 的 recall 是它 BM25 基线的 **3.9 倍**。
- §10-1 的标注集建议是一个**可起步的探索手段**，不是所有后续工作的硬准入条件（见 §10-1）。

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
| tinybert uint8（**许可未核实，暂不候选**） | 14 442 511 B | ORT | 89 | 25.0 | 0.049 |

### 6.2 端到端（Rust 原型，含分词/切片；rten fp32）

| payload | tok | 分词 | 前向 | 切片 | 合计 |
|---|---:|---:|---:|---:|---:|
| 8 KB | 2 746 | 2.3–4.0 ms | 1 371–1 732 ms | 0.1 ms | **1.37–1.73 s** |
| 24 KB | 5 999 | 7.8–11.1 ms | 2 969–3 854 ms | 0.1–0.3 ms | **2.97–3.85 s** |
| 50 KB | 12 892 | 17.9–28.2 ms | 6 630–8 754 ms | 0.3–0.7 ms | **6.63–7.91 s** |

量化图（`ll2rr`）同 50 KB：前向 5 737–6 118 ms，`load_ms` 166–189，峰值 RSS **243 MB**
（fp32 峰值 RSS **783 MB**）。

### 6.3 其他形态的成本

| 形态 | 工件 | 大小 | 本机 CPU 实测 |
|---|---|---:|---|
| SWE-Pruner 形态（Qwen3-Reranker-0.6B，[apache-2.0](https://huggingface.co/Qwen/Qwen3-Reranker-0.6B)；HF API 报 **595 776 512** 参数） | safetensors（bf16） | 1 191 588 280 B | (goal,chunk) 对：140 tok → 105 ms、268 tok → 195 ms、**524 tok → 447 ms**；50 KB ≈ 26 对 ≈ **11.6 s（推算）** |
| Squeez 形态（2B 生成式） | bf16 safetensors | 4 548 221 488 B | **未测**（本机无 torch；`ort` 未导出该图） |
| LLMLingua 原路径（PPL，LLaMA-2-7B-Chat） | 7B 因果 LM | bf16 约 13 GB | **未测**（本机 16 GB RAM，跑不动；但这是本机限制，不是产品不可行） |

**结论（限定条件）**：本机 CPU 上，上述实现是 **10²–10³ ms（单窗）到秒级（端到端）** 量级。
**可接受的入口延迟预算尚未确定**，因此本文**不判断**“够不够快”。
可选的工程边界（不是结论，是待选方案）：① 只对大 payload 用（`read` 大文件、长日志）；
② 只在异步/预热路径用；③ 不用模型。哪条合适取决于真实目标任务的延迟/质量要求（见 §10-4）。
另需明确：以上全是 **CPU + 单机** 结果；GPU 或其他硬件上的数量级未经本文测量，
不能从这些数字推出“模型普遍不能上 CPU”。

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

- **输入形态（候选，待真实轨迹验证）**：`read` 的大文件、长文档、长日志
  （行结构清晰、行内冗余高、单行不承担定位语义）。
- **保守候选策略（不是用户已定决策，也不是普遍定理）**：grep/find/ls 的紧凑命中列表、
  unified diff、**已截断的** bash 输出、`file:line` 密集的错误块目前不建议作为首批目标。
  理由是这些形态里行本身可能就是全部价值；但如果在真实任务上验证出收益，这个策略可以推翻。
- **前提（需与 Pi 报告对齐）**：默认钩子看到的是**工具自己截断之后**的结果（Pi 侧 `bash` 50 KB/2 000 行、
  `read` 50 KB/2 000 行、`grep` 50 KB、MCP 20 KB；见 `pi-integration.md` §3）；
  **但并非超过截断就“拿不到原文”**——如该报告的 `fullOutputPath` 类出口所示，
  部分工具会把完整输出落到临时文件，可以另读；代价是额外的 I/O、路径处理与生命周期管理。
  本文的压缩比承诺只针对**钩子实际拿到的（可能已截断的）内容**，并在能读至完整文件时另行评估。

### 8.2 任务上下文：需要什么、代价多少

| 方案 | 需要的上下文 | 获取成本 | 上下文 token 成本 |
|---|---|---|---|
| LLMLingua-2 | **无**（task-agnostic） | 0 | 0 |
| LongLLMLingua | `question` 字符串（**强制**，`prompt_compressor.py:565`） | agent 当前目标或工具入参 | 短（10–30 tok） |
| SWE-Pruner | goal hint（"focus on error handling"） | agent 显式给 | 短 |
| Squeez | 一段 focused query（≤3 000 字符，超出截断；`squeez/inference/extractor.py:_build_messages`） | 工具入参或 agent 目标 | 短 |

**[建议]** 最便宜可得的任务上下文是**工具入参本身**（`path` / `pattern` / `command` / `offset`），
不需要额外模型调用。这里只确认“短指针”的意图：指针应当短。
**但不要把它当成完整的恢复机制**：3 token 的是一个**句柄**，不是可 `read` 的路径，
也不意味着“只要 3 token 就能恢复全部原文”——真正取回还需一次额外的工具调用与它的输出成本。
同时**不要把“被压缩了 N 行”这类自述句塞进 prompt**（squeez 的 39-token 标记就是反例）。

### 8.3 外部证据的确切形状（含自相矛盾）

SWE-Pruner 实验设置（论文 §4.2 / Table 3 脚注）：SWE-Bench Verified，**随机 50 例子集**，Mini SWE Agent 作宿主。

| 来源 | Mini SWE Agent | +LLMLingua-2 | +SWE-Pruner |
|---|---|---|---|
| **Table 3**（表格） | 52.3% / 0.972M tok | **42.1% / 0.856M** | 41.1% / 0.670M |
| **正文 §4.2**（文字） | 62% | **54%** | **64%** |

两处对不上：表格里 SWE-Pruner **低于**基线（41.1 < 52.3），正文说**高于**基线（64 > 62）。
本文无法判断哪一套对应最终实验、“表格未同步”只是猜测。**所以：**

- **可用的方向性结论**：两套数字里，加 LLMLingua-2 都比各自的基线低 5–10 个点（52.3→42.1、62→54）。
  这与论文的定性解释（"token-level pruning disrupts code syntax"）一致，
  也与 §4.2 复刻观察到的行内结构破坏不矛盾；但**这是同向而非同一现象**的印证，不能当因果证明。
- **不可用的**：任何绝对数、任何"SWE-Pruner 比基线好 X 点"的说法。
- **不可外推的**：它压的是**整段检索上下文（每轮）**，我们是**入口单次**且可放行原文；它的基线是
  固定 4x/8x 约束，我们允许"不压"。

其他可用于校准的外部数字：

- **read 类操作占 76.1% token**（Figure 2，Claude Sonnet 4.5 + Mini-SWE-Agent）——这是"入口压缩"
  最大受益面的量化支撑，也与"只压工具输出、不压历史"的设计吻合。
- **SWE-Pruner 声称首 token 延迟 < 100 ms**（Figure 4），0.6 B 骨干。我本机 CPU 上对 524-token 对
  实测 447 ms（§6.3），**差 4x 以上**——它的 <100 ms 极可能是 GPU 上的数字，且它按 chunk 并行。
  这说明"0.6 B 也能很快"这类论断必须绑定硬件。

### 8.4 压缩策略的 agent 级系统证据（本文找到的最强外部证据，但仅限其测试矩阵）

[arXiv:2609.32961](https://arxiv.org/abs/2609.32961)（UT Austin，CC BY 4.0，v1 2026-09-26）把压缩策略分解为
**primitive（怎么删）× trigger（何时删）× depth（删多少）**，在 SWE-bench Verified（100 issue）与
Terminal-Bench 1.0（40 任务）上跑 **65 种策略 × 3 个开源模型 × 3 次重复 ≈ 35 000 次运行**。
它明确列出被调研的生产系统，其中包括 **Pi Coding Agent**。
**以下结论均限于该论文的测试矩阵（该两套 benchmark、3 个开源模型、65 种策略）。**

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

**对我们的含义（在其测试矩阵内，且不能外推到入口选择）**：

1. **在该论文的 SWE-bench 设置下，TRC 是它所测策略中“基本不掉点”的一个**。不要把它读成
   “整个领域唯一不掉点的做法”——它只是该矩阵、该套策略里的一个观测。
   它的代价是 **信息全丢**（占位符 `[output cleared]`），而不是“压得准”。
   **它不能证明“行级入口选择更安全”**：TRC 删的是历史旧输出（模型已看过）、不可恢复、无选择步骤；
   与“首次入口 + 行级选择 + 原文可恢复”是两种不同机制。
2. **“省 token”不能当唯一卖点**。在该论文的 Terminal-Bench 设置下，token 下降可与延迟上升同时发生。
   所以产品指标建议看“**下游是否少跑一轮 / 是否不用重读**”，而不是单纯压缩比（§10-1）。
3. **不能拿单一模型的结果外推**。该论文自述“同一策略在不同模型上表现可差很多”，
   这支持“Pi + OpenCode 双宿主各自验证”的工程约束。

**[边界]** 该文的 TRC 作用于**历史**中的旧输出，而 Outrim 谈的是**入口**。
所以它的数字不能直接当我们的预期；它能提供的仅是“**primitive 选择在它那个矩阵里比 depth 更重要**”
这一**待迁移假设**，不是已证结论。
**删正**：本文早前版本曾把 Terminal-Bench 上 12 个策略的 5–13 pp 写成“任何写操作”的普遍风险；
那是错误的推广，已删。它只是**该 benchmark 上那批策略**的结果。

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
→ **待测假设（不是结论）**：如果要做模型打分，“一次前向覆盖整段”值得优先尝试。
本文**不能**说这与 §5.3 第 5 点“互相支持”——§5.3 只是说本探针上未观察到跨行收益的方向一致性，
两者都没有直接测过“逐行 vs 整段”的等价性。**这只是一个需测的假设（见 §10）。**
另外，“更省资源”不等于“更准”：它的 EM（36.98）还略低于 AttnComp（37.16）。

**[不可迁移]** 它是 RAG + 问答，有真 query；我们的 tool output 只有工具入参。
且它的 0.045 s 是 **RTX 4090**，本机 CPU 上同规模骨干（mBERT 数量级）是 **§6.1 的 123–280 ms/508-token**。
**不要把 GPU 秒数当 CPU 预算。**

### 8.6 与宿主契约的待定边界

- **策略需稳定（不是要求代数幂等）**：同一输入在**同一策略/配置**下应得到结果稳定。
  注意这**不等于** `f(f(x)) = f(x)`——代数幂等是更强的性质，入口单次压一次在逻辑上并不要求它。
  真正要防的是**重复处理**：若 hook 与 proxy 双重调用，同一份 tool output 可能被压两次。
  可行方向：恢复读**旁路**（不经过压缩路径）；重试时以 `toolCallId` 或缓存条目避免重复处理。
  **具体机制待设计**，本文不定。
- **失败必须无损放行**：模型加载失败、超时、“压缩后反而更长”一律返回原文。
  `structural-compression.md` §5.2 的 `is_net_loss()` 判据（来自 squeez `wrap.rs:1078`）可参考。
- **stdio/NDJSON 单进程是建议，不是既定硬边界**：它的好处是跨宿主（Pi + OpenCode）共用一份实现、
  不依赖宿主语言；但最终形态取决于两个宿主的接入方式（见 `pi-integration.md` / OpenCode 报告）。
  不使用 argv 传 payload 是因为 Windows argv 有长度上限，这一条是硬约束。
- **恢复用短指针**，不写长免责声明（短指针只是意图，具体格式未定）。

---

## 9. 候选（不排序，只写适用条件）

| # | 形态 | 用模型 | 实测压缩 | 实测代价 | 适用条件 / 已知失效 |
|---|---|---|---|---|---|
| A | **行级词法打分 + 保护行 + 预算**（无模型） | 否 | 见 `structural-compression.md` §0.2 与 §4.3（Kneedle 预算 + 分级选择）；本文 §5.2 的手写正则基线在 21–23 个非空行的两个探针上 recall 不低于未条件化模型（n 极小） | O(n)；`rtk-engine` release 吞吐见 `structural-compression.md` §6 | 需要维护“证据模式”词表；**对未知输出形态可能漏**（这只靠词表本身的性质判断，没有专门实验；参见 §5.4 关于“手写词法能否覆盖未知形态是开放问题”）；预算器在未重排时会把信号砍到 3%；空白行需单独处理（见 §4.3） |
| B | **LLMLingua-2 行级聚合 + 原文切片 + 字面量保护** | 是（178 MB） | 8 KB 3.84x / 50 KB 3.48x（keep 0.3）。**与原文某行字节相同的行数 / 非空行总数**：Python 74/74、122/122；Rust 80/217、122/217（8 KB）、157/431、260/431（24 KB）、424/1071、673/1071（50 KB）——**Rust 侧多数行并非字节一致**。**行内损坏 0–1 条（Rust）**；标识符保留 16/32（keep 0.3）、28/32（keep 0.5） | 1.4–7.9 s / 8–50 KB；RSS 243–783 MB | **必须**行级聚合 + 切片；**不要**用 `convert_tokens_to_string`；**已知失效**：Rust 原型会把行内多空格 gap 压成单空格（`main.rs:154`，§4.3），且**整行丢弃本身不可逆**（这是压缩，不是无损变换）；`file:line` 密集时降级放行 |
| C | 任务条件化行选择器（SWE-Pruner / Squeez 形态） | 是（0.6 B / 2 B） | **Squeez：0.86 recall / 0.80 prec / 92% 移除**（618 例自建测试集，LoRA 2B）；SWE-Pruner：23–54% token 减少（绝对成功率数字自相矛盾，见 §8.3） | 0.6 B 本机 447 ms/524-tok 对；2 B 未测 | 需要 focused query；本文找到的行级选择证据中较强的一份（recall 远超该论文自己的 BM25 基线）；Squeez 硬依赖 `openai` 包；论文报 <100 ms 首 token 但那是 GPU 量级 |
| D | 检索式（BM25 / embedding 选块） | 可选 | 外部：该论文的 BM25(10%) 基线 recall **0.22**（Squeez Table 4，其自建 618 例） | 低 | **不能外推**：那是 Squeez 的任务形态（有 query、逐轮），与本产品入口单次不同构；只能说明“该论文这一配置下检索式基线不强” |
| E | **字面量 / 定位符保护层**（任何形态都要） | 否 | — | O(n) | 必须：错误码、`file:line:col`、路径、端口、版本号、16 进制、UUID |
| F | **一次全上下文编码句子/行打分**（EnComp 形态） | 是（ModernBERT-large 级） | 外部：保留率 16.0%、EM 36.98/F1 47.53（≈AttnComp 37.16/47.65） | 外部 0.045 s / 1.86 GB（**RTX 4090**）；本机 CPU 同规模骨干 123–280 ms/508-tok | 需要 query；代码域未验；关键优化是“一次前向拿全部分数”而非更小模型 |

**[建议]** A + E 是本文范围内值得优先验证的组合；B 只在"payload 大且行冗余高"时作为可选增强；
F 给出了“一次全上下文前向”的工程形态，本机 CPU 上是 10² ms 级，是否可接受取决于未定的延迟预算。
C 的外部证据只能说明"token 删除会掉点"，不能说明"行级选择会涨点"；且该论文自己的表格与正文不一致（§8.3），
连“行级选择是否能匹配全上下文”都未在当前证据下定论。**压缩本身不保证成功。**

关于 TRC（重要区分）：在上述 ≈35 000 次运行的研究里，TRC 是**删掉历史里的旧工具输出**、
且那些输出**模型先前已经看过**；它**不是入口选择**，也**本身不可恢复**（只留占位符）。
因此“丢信息但可恢复”并不是那个实验的结论——**本文不能从 TRC 推出“行级入口更安全”，
也不能把原文可恢复当成已有实证价值**；后者仍是一个待验证的设计意图。

---

## 10. 待做的小实验（不排序，按依赖关系列出）

1. **建一个小标注集（可起步的探索建议，不是唯一前置，也不是硬阻断）**：30–50 条真实 Pi/OpenCode 轨迹里的 tool output，
   人工标"哪几行是 agent 后续真正用到/引用的"；可以先用"**同一工具是否被重跑**"作弱标签跑起来。
   没有它，§5 的模型/词法对比仍受循环论证污染；但其他工作（如延迟、失败放行、指针恢复）**可以并行推进**，
   不必等它完成。
2. **A vs B 的 head-to-head**：在 1 的集合（或其弱标签子集）上，比较 `词法+保护`、`LLMLingua-2 行级+切片`、`firstN`。
   指标建议看"证据行 recall"与"下游是否需要重跑"。**不要**用通用 QA 指标。
3. **行内 gap 保真**：按 §4.3 的根因修掉 `main.rs:154` 的 `out.push(b' ')`（同一行内直接搬原文 gap），
   再复测“保留行字节一致”的比例（Python 版目前 74/74、122/122；Rust 侧 80/217–673/1071，需先修才谈得上字节一致）。
4. **延迟与质量都要在真实目标任务上验证**：确定入口可接受延迟（**本文不给值**），
   然后在不同 payload 大小上实测延迟 + 下游质量。
   **不要**从 8 KB 的秒数线性外推到小 payload 就断言“没收益”——
   固定开销（加载、分词、调用）占总时间比重随 payload 变小而上升，小 payload 的真实性价比未测。
5. **`ort` 静态二进制在 macOS arm64 / Linux 上的产物大小与 `download-binaries` 行为**（未测）。
6. **4-bit 工件**（社区仓库有 `_q4.onnx` / `_bnb4.onnx`）在 `ort` 上的速度/精度权衡（未测）。
7. **“逐行独立打分 vs 一次长上下文前向”的等价性**（未测）：本文 §5.3 只是方向不一致的小探针，
   §8.5 只是别人的 GPU 结果；两者都**不能**推出两种形态等效。
8. **本文未找到覆盖“本产品这种入口配置”的公开评测**：`arXiv:2609.32961` 只考察 TR / TRC / SU 及组合；
   Squeez / SWE-Pruner 做的是任务条件化的每轮检索/剪枝。
   但**不能**因此说“任何人没测过行级选择”——Squeez 做的就是行级选择（只是不同任务形态）。

---

## 11. 未核实 / 未知

1. **本文未找到覆盖“本产品这种首次入口压缩配置”的公开评测**。≈35 000 次运行的研究
   （[arXiv:2609.32961](https://arxiv.org/abs/2609.32961)）只测了 TR/TRC/SU 三类；
   Squeez / SWE-Pruner 做的是任务条件化的行级选择但**每轮检索/剪枝**，不是入口单次。
   **不能**说“行级选择性保留没有任何公开测量”——Squeez 正是行级选择，只是任务形态不同。
2. **rten 量化失败的算子级根因未定位**。§7.1 给的是"可复现判据 + 官方注释里的已知风险"，
   不是证明。要定位需要 layer-by-layer 对比（我只在早期对个别层做过 cos 比较，未系统化）。
3. **`ort` 的 `download-binaries` 在 CI/离线环境的行为未测**；本机是首次运行自动下载（缓存于
   `C:/Users/wudon/AppData/Local/ort.pyke.io/`）。
4. **SWE-Pruner 的 n = 50，且 Table 3 与正文数字矛盾**（§8.3），两套都对不上。它的基线是固定 4x/8x
   约束下的**每轮**压缩，与我们"入口单次、可放行原文"不同构。**不要引用任何绝对成功率。**
5. **Squeez 的所有数字来自其自建 618 例集**，无第三方复现；2 B 生成模型在 CPU 上的延迟我**没有测**。
6. **本文未找到**覆盖“真实 coding-agent tool output 上的首次入口行级选择”的公开 benchmark。
   本文 §4/§5 的自测 n 极小（23–1 127 行），只能当探针。
7. **XLM-R-large（MIT，559 M）未测**：更准（未验证）但更慢，且 2.2 GB 工件对分发是负担（可行性未评）。
8. **ModernBERT reranker（`gte-reranker-modernbert-base`，apache-2.0，598 436 708 B）未完成测量**：
   探针在 8 000-token 单对上 OOM（`BFCArena` 请求 24.6 GB），需要改成短对才能测。
9. **LongCodeZip 无公开 checkpoint**，只有论文数字（SWE-Pruner Table 4 里 8x 约束下实际 7.85x 压缩，
   论文自述为“leverages program structure”，不属本文语义路径）。
10. **`llmlingua` 官方包未实跑**（无 torch），§4.2 是**按源码实现的近似复刻，尚未与官方 API 对拍**。
    因此 §4.2 的破坏性结论限定于“本文复刻流水线 + 所测 mBERT 量化图”。
11. **`force_tokens` 在官方 API 里的确切语义**（是否也影响最终拼接）我按源码读了但未实跑验证；
    我的复现是"词级给 p=1.0"，若官方还有别的处理，“force_tokens 也不起作用”这个观察可能不成立。
12. **EnComp 的 0.045 s 是 RTX 4090 上的数字**；本机无 GPU，未做 CPU 复现。
    它的“一次全上下文编码”在 CPU 上的真实代价**未知**。
---

## 附录 A：探针脚本与数据（均在 `C:/Users/wudon/AppData/Local/Temp/`，非仓库内容）

| 脚本 | 作用 |
|---|---|
| `sem2/bench_sem.py` | 首个延迟/单窗存活/AST 探针（`keep()` 统计口径有 bug，仅作历史） |
| `sem2/bench_damage2.py` | **按源码实现的近似复刻**（merge→mean→percentile→detokenize，未与官方 API 对拍），exact/loose 标识符、数字、AST、行数 |
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
