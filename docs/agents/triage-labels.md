# Triage Labels

The skills speak in terms of five canonical triage roles. This repo's issue tracker uses a smaller lifecycle vocabulary, recorded as a `Status:` line inside each ticket file under `docs/specs/<name>/tasks/`.

| Label in mattpocock/skills | Status in our tracker | Meaning                                             |
| -------------------------- | --------------------- | --------------------------------------------------- |
| `needs-triage`             | _(no equivalent)_     | 新票先在 map 的雾区/Not yet specified 里记录，不单独立票 |
| `needs-info`               | _(no equivalent)_     | 待澄清问题直接走 grilling 票（HITL）                  |
| `ready-for-agent`          | `open`                | 已立票待领（对应 map 上未 claimed 的票）              |
| `ready-for-human`          | `open`（Type: grilling） | 用户亲自执行的票（如真实使用验收）                  |
| `wontfix`                  | _(no equivalent)_     | 砍单决策记在 map 的 Out of scope，不立票              |

## Our lifecycle vocabulary

- `claimed`：代理已领票开工（动工前置条件）。
- `resolved`：票完成，Answer 节写结论，map 的 Decisions so far 追加一行索引。
- `open`：已立票未完成（含等待人工执行的票）。

When a skill mentions a triage role, map it to the vocabulary above rather than the five canonical strings.
