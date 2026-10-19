# 日志尾部裁剪：适用边界与验证候选

辅助研究，服务于决策票 [决定内容分类、压缩策略与算法验证 #5](https://github.com/WodenJay/Outrim/issues/5)。本文不实现产品，不选择默认行数、启用门槛或验收阈值。

## 结论与证据边界

- **用户已明确的方向**：日志可以更大胆地压缩，信息不足时由 Agent 用已有工具查原文；“10 行”只是举例，不是默认值。
- **静态观察**：纯尾部可以大幅减少长日志，但可能只留下失败结论，丢掉具体失败单元与原因。多包执行中“前面失败、后面成功”是一个已观察到的形态，不是风险的穷尽分类。
- **候选，不是结论**：将纯尾部，与“尾部 + 少量诊断线索”做配对比较。后者也可能漏诊、误匹配、失去上下文，不能称为无损。
- **尚未测量**：Agent 是否主动恢复、是否修对、额外轮次、真实任务 Token 与耗时。因此现有观察不能证明任一候选具有正的任务净收益。
- **不以规则白名单限制覆盖**：通用日志尾裁不必等待每个构建工具都有专用解析器；专用规则可以改善效果，但不应成为唯一入口。

## 1. 修订说明：撤回首版中的错误数字

首版提交 `f7485b65d4356742bffee499e32b7c735abb1691` 经复核有以下问题；本修订取代其相关结论，不应继续引用旧表：

1. **首版 §2.2 的身份召回表撤回**：Go 的匹配表达式把 `ok package` 也计入身份，pytest 又重复计算同一失败单元，不能解释为失败身份召回。
2. **首版 §4.4 的 `16.7% → 83.3%` 撤回**：`an4.py` 的 `kept_err = len(keep)` 统计的是被省略区间的全部候选错误行，但实际只插入 `picked = keep[:cap]`。候选数不等于实际保留数，该提升幅度不能作为锚点策略的证据。本修订不发布未经独立核定的替代总量召回率。
3. **固定 `35/48 token` 不是通用常数**：它取决于特定路径、标记与 tokenizer，且分别计数再相加不保证等于完整拼接文本的计数。应比较实际完整输出，不能用固定值宣布通用硬门槛。
4. **首版 §4.3 为字符口径**，不能与 Token 口径混用；其中“净省”也不包含恢复或额外轮次，不能当成任务净收益。
5. **“Outrim 总是第二层闸门”不成立**：宿主有截断能力，不代表每条输出都已截断；只有实际被截断的输出需要区分宿主先前省略与 Outrim 本次省略。
6. **“风险只分两类”“任何单一 N 都会系统性漏”等推广撤回**：有限探针与一个用户的会话分布不能证明风险穷尽、通用最优长度或实际任务失败概率。

原探索包含本机 Pi 会话的聚合分析，但原始会话与分析脚本没有纳入仓库；本修订不以这些统计选择产品默认值，不发布会话原文。下文保留可明确限定的探针观察与一个自包含反例。

## 2. 一个可直接检查的反例

以下是本机 `go test ./...` 探针输出快照（12 个包，一个包内的一个测试失败）。这是人工生成的小项目，不代表真实负载分布。`go_test_quiet_earlyfail.txt` 原有 16 行：

```text
ok  	example.com/verbose/pkg0	0.389s
--- FAIL: TestCase02 (0.00s)
    p_test.go:15: compute(2) = 2, want 4
FAIL
FAIL	example.com/verbose/pkg1	0.527s
ok  	example.com/verbose/pkg10	0.488s
ok  	example.com/verbose/pkg11	0.349s
ok  	example.com/verbose/pkg2	0.427s
ok  	example.com/verbose/pkg3	0.471s
ok  	example.com/verbose/pkg4	0.445s
ok  	example.com/verbose/pkg5	0.502s
ok  	example.com/verbose/pkg6	0.372s
ok  	example.com/verbose/pkg7	0.543s
ok  	example.com/verbose/pkg8	0.575s
ok  	example.com/verbose/pkg9	0.413s
FAIL
```

仅保留末尾 10 行时，仍有最终 `FAIL`，但 `TestCase02`、`p_test.go:15` 与失败包 `pkg1` 全部消失。增加窗口能解决这个特定短例，但不能保证任意并发、多包日志的失败都靠近结尾。

### 最小复核

将上面的文本保存为 `sample.txt`。以下脚本只复核该快照，不读取会话，也不是产品实现。身份分别按失败测试名和失败包名计算，不把成功包或单独的 `FAIL` 当成失败身份；该指标不是完整诊断质量。

```python
import re
from pathlib import Path
import tiktoken

lines = Path("sample.txt").read_text(encoding="utf-8").splitlines()
pattern = r"^--- FAIL: (\S+)|^FAIL\t(\S+)"

def identities(text):
    return {
        next(group for group in match.groups() if group)
        for match in re.finditer(pattern, text, re.MULTILINE)
    }

original = "\n".join(lines)
body = "\n".join([f"[{len(lines) - 10} lines elided]"] + lines[-10:])
pointer = (
    "[outrim]: The full output is available at "
    "C:/Users/wudon/AppData/Local/Temp/outrim/7a3f9c2e.log"
)
view = body + "\n" + pointer
enc = tiktoken.get_encoding("cl100k_base")
print("failure identities:", len(identities(original)), len(identities(body)))
print("tokens:", len(enc.encode(original)), len(enc.encode(view)))
```

本次以 Python、`tiktoken 0.14.0 / cl100k_base` 复核，结果为：

```text
failure identities: 2 0
tokens: 212 178
```

计数先统一 `splitlines()` 再用换行拼接，不计原始末尾换行。178 已包含示例标记与特定路径提示，**不含恢复或额外模型往返**。这里只省 34 个该词表的 Token，却丢掉两个失败身份：短日志的尾裁未必划算。路径、标记或词表改变时应重新计数。

## 3. 其他小探针观察

本机探索还真跑了 `cargo build`、`cargo build --workspace --keep-going`、`go test -v ./...`、`pytest -q`、`make -k -j` 等小项目。下表是使用 `final2.py` 后复核的**单个快照**，不是总体成功率：

| 探针 | 原文 Token | 尾 10 行 + 标记 + 路径 Token | 保留的失败身份 / 原有失败身份 |
|---|---:|---:|---:|
| Go 多包 verbose，失败后还有大量成功输出 | 5,323 | 122 | 0 / 2 |
| Go 多包非 verbose（上文快照） | 212 | 178 | 0 / 2 |
| pytest，末尾含 short test summary | 1,573 | 207 | 9 / 16 |
| Cargo workspace，两 crate 失败 | 371 | 168 | 1 / 2 |
| make 并发继续构建，前部目标失败 | 99 | 99 | 0 / 1 |

均使用同一 `cl100k_base`、换行规范化与示例路径；Go 身份为失败测试/包，pytest 为 `FAILED nodeid`，Cargo 为错误位置，make 为失败目标，**不同工具的分母不能混成统一的语义召回指标**。除第 2 节快照外，其余 fixture 和探针脚本仅在临时目录，不是仓库内可持续复现的基准，未来正式实验需重新固化非敏感输入与计算脚本。

这些观察只支持：

- 尾部摘要能否保留失败单元依赖输出形态，不能仅看最后是否有 `FAIL`。
- 保留少量前部诊断行值得作为对照候选；尚无可用的总体召回提升结论。
- 激活门槛与保留长度是不同变量；不应因为一条输出超过 N 行就必定裁到 N 行。
- 若提示等完整新增文本抵消了缩短量，应考虑放行；字符减少与真实 Token 减少不是同一个承诺。

## 4. 宿主与既有实现：只能在能力边界内引用

### Pi 本地安装观察

本机安装为 Pi 1.3.0；源码位于 `C:/Users/wudon/.pi/agent/install/releases/1.3.0/node_modules/@earendil-works/pi-coding-agent/dist/`：

- `core/tools/bash.js:295-301`：非零退出结果包含 `isError: true`、`structuredContent.exit_code`，并在正文追加 `Command exited with code N`。
- `core/tools/truncate.js:10-11`：默认上限为 2,000 行、50 × 1,024 字节。
- `core/tools/bash.js:243-249`：发生宿主截断时，正文追加行范围与原输出文件提示。

这说明应保留宿主状态与已有提示；**不能推导每次调用都已截断、元数据必然以同样方式传给模型、或 OpenCode 具有相同能力**。本轮未独立核实 OpenCode 的对应路径。

### RTK 一手源码侧证

来源固定在 [`rtk-ai/rtk @ 9a501c8d3b783b23a96073261708715381eb5ceb`](https://github.com/rtk-ai/rtk/tree/9a501c8d3b783b23a96073261708715381eb5ceb)：

- [`engine/src/methods/cap.rs`](https://github.com/rtk-ai/rtk/blob/9a501c8d3b783b23a96073261708715381eb5ceb/engine/src/methods/cap.rs)：`HEAD_LINES = 50`、`TAIL_LINES = 200`、`MAX_LINES = 300`、`MAX_LINE_CHARS = 2000`；有按其估算器判断不应变大的守卫。
- [`src/core/runner.rs`](https://github.com/rtk-ai/rtk/blob/9a501c8d3b783b23a96073261708715381eb5ceb/src/core/runner.rs)：存在解析不到摘要时的 `OUTPUT (last 5 lines):` 兜底。

这是既有实现选择，不是经过本项目验证的最优参数，也不能证明其静态估算能保证所有模型词表下不变大。

## 5. 建议的最小验证计划（待用户决定与开发阶段执行）

在相同任务、起始状态、模型与宿主条件下，至少对比：

1. 未压缩基线。
2. 纯尾部。
3. 尾部 + 预算受限的诊断线索。

将**何时启动**与**启动后保留多少**作为独立变量；同时考虑行数和单行特别长的情况，不能把源码、配置或结构化数据无差别交给日志裁剪。具体长度、诊断匹配方式及是否按成功/失败区分，尚未决定。

验证分层：

- **局部检查**：完整候选文本（含标记、路径）的字符量与选定 tokenizer 计数；未改写的调用状态；保存的入口原文是否可读；覆盖与放行原因。
- **信息检查**：失败位置、单元与相关上下文；抽取行要标明省略，避免伪装成连续日志。关键词匹配只是代理指标。
- **真实 Agent 任务**：是否成功、最终质量、恢复次数与恢复内容量、额外轮次、总 Token 与耗时；不能只从局部减少量反推净收益。

恢复继续使用 Agent 已有工具。`grep -n` 等方式有时很便宜，有时会命中大量内容、甚至比直接读原文更长；本研究不引入自动恢复流程、额外 Tool 或强制恢复策略。

`outrim gain` 可以帮助观察入口局部收益，但其统计不能替代最后一层实验。具体统计口径留给 [#7](https://github.com/WodenJay/Outrim/issues/7)。

## 6. 明确未验证

- 没有压缩后 Agent 修复质量、返工轮次或实际账单数据。
- 没有证明 Agent 信息不足时一定会主动恢复，更没有证明一次恢复就够。
- 没有验证抽取诊断行后，模型是否正确理解不连续文本。
- 没有从有限样本得到通用默认 N、最低节省阈值或风险类别频率。
- 没有证明 `cl100k_base` 的绝对值或相对排序适用于全部目标模型；字符估算与账单 Token 分开看。

本报告的用途是约束下一步候选与实验，而不是以静态压缩率提前宣布产品有效。
