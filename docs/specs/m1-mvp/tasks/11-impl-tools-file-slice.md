# 实现：工具系统只读/写切片（read/write/edit/list_files/grep + 围栏判定）

Type: task
Status: resolved

## Question

按《技术设计文档》§8 落地 M1 工具系统第一切片——五个文件类工具与围栏判定纯逻辑：

1. 共用文本解码器（encoding_rs + chardetng）：BOM（UTF-8/UTF-16LE/BE）→ 严格 UTF-8 校验 → chardetng 探测 → 均不中按最佳猜测渲染；放 util 或 tools（按 §2.2 判定），命名便于后续 read_workspace_file 命令复用（C-10 定案要求两者共用，见 §3.2 #28 与 §8.3 read 行）。
2. `read`：文本按行窗口（默认 2000 行，offset/limit）；kind:"image" 走 base64（≤3MiB，vision 模型前置校验）；§8.3 中 PDF 相关语义若涉及文档未定的依赖选型，纸面记录到票 Answer 不实现。
3. `write`：仅新建，已存在拒绝（E_INVALID_ARG）；落盘一律 UTF-8。
4. `edit`：§8.4 算法（oldText/newText、version? 乐观锁），结果含 lineChanges{add,del}；落盘一律 UTF-8。
5. `list_files`：树形清单，上限 500 条目，respectGitignore 默认 true。
6. `grep`：内嵌 ripgrep 生态（grep-regex + grep-searcher + ignore），maxResults 截断。
7. safe_join 路径边界（roots 内校验，越界报错）。
8. 围栏 P0 判定纯逻辑：高危黑名单 + 只读白名单 + 权限四档（ReadOnly/FileWrite/Interactive/Network）能力分级判定。

边界：不含 command/ask/suggest/plan 工具（command 依赖 S5 收割结论，其余依赖 run loop 集成）；不含审批门与工具卡片 UI 接线；不含写后检查（M2）。

完成标准：上述能力 headless 可测——独立 _test.rs / tests/（含 GBK fixture 解码用例、safe_join 越界用例、围栏黑白名单用例、edit 行变更统计用例）；检查链全过；如实现与 §8 有出入，Answer 记录并最小修订 §8 对应行，无出入则声明。

## Answer

（2026-10-27 实现，七项检查链全绿：cargo fmt --check / build / clippy --all-targets -- -D warnings / test（90 例）+ pnpm typecheck / test（27 例）/ lint。**与 §8 无文字性出入，技术设计文档未修订**；以下「实现细化记录」均为 §8 未定细节的落地选择，无一处与既有条文矛盾。）

### 解码器归属与实现要点

- **归属 `util/encoding.rs`**：消费方是 tools 的 `read` 工具与 host（未来）的 `read_workspace_file` 命令，跨两层共用；util 是被所有层依赖的最底层（§2.2），放这里无依赖方向问题，满足 C-10「共用模块」要求。纯函数接口 `decode_text(bytes) -> DecodedText{text, encoding, had_bom}`，无 IO，两边零成本复用。
- 判定链按 C-10：①BOM（UTF-8/UTF-16LE/BE，encoding_rs `Encoding::for_bom`）→ ②严格 UTF-8（`str::from_utf8`）→ ③chardetng 探测 → ④探测恒有候选，配合 encoding_rs 替换语义渲染即「最佳猜测」，任何路径不报错（宁乱码）。二进制判定独立函数 `is_probably_binary`（前 8KB 含 NUL）。
- chardetng 取 **1.0.0**（2024 年后的稳定主版本，非 0.1.x）：其 API 与 0.1 不同——`EncodingDetector::new(Iso2022JpDetection)`、`guess(tld, Utf8Detection)` 直接返回 `&Encoding`（不再 Option）。两处探测开关均取 `Deny`：UTF-8 已在②严格校验失败，不允许探测回退 UTF-8；ISO-2022-JP 误判率高，本地文件场景拒绝。
- `Encoding::decode` 的第三返回值是 **had_errors 而非 had_bom**（实现中踩过：BOM 标志须由 `for_bom` 自行记录），后续复用方注意。

### 依赖版本（rsproxy 拉取，cargo add 最新稳定）

encoding_rs 0.8.42 / chardetng 1.0.0 / grep-regex 0.1.14 / grep-searcher 0.1.17 / ignore 0.4.33 / regex 1.13.1（fence 黑名单）/ sha2 0.11.1（edit version）/ base64 0.23.1（read image）/ tempfile 3.27.0（dev）。

### 模块落位（§2.2）

- `util/encoding.rs`（解码器）、`util/errors.rs`（ToolError 错误码载体——safety 与 tools 双消费方，放 util 避免环）、`util/test_support.rs`（测试用临时工作区，cfg(test)）。
- `safety/safe_join.rs`（PathGuard）、`safety/fence.rs`（围栏 v0.1 纯判定）、`safety/tier.rs`（权限四档 + Capability + §8.5 矩阵 `decide_gate`）。
- `tools/mod.rs`（ToolSpec/ToolAnnotations/ToolOutcome/注册表/filter_for_tier/compact_text）+ 五工具各一文件。`filter_for_tier` 是三处同源过滤的单一实现（§8.1），放 tools（引用 safety 判定，方向合规）。

### 测试清单（90 例，全部独立 _test.rs / tests/，fixture 在 tests/fixtures/）

- 解码器 9：BOM 三态、严格 UTF-8 快路径、GBK 探测、GBK fixture 端到端（tests/fixtures/sample_gbk.txt，iconv 生成的真实 GBK 文件）、latin-1 兜底、NUL 二进制（含 8KB 窗口边界）、空文件。
- safe_join 9：相对/绝对通过、`..` 越界、绝对越界、symlink 逃逸（link 本身与 link/../ 穿透）、不存在路径 write 场景、空/控制字符、Windows 保留名（含 con.txt 形态、concourse 误伤排除）、双 root（项目+数据目录）。
- fence 6：黑名单 17 条命令 × 4 档位全 Block、plan 白名单放行与 fail-closed Confirm、写信号 13 条 × 3 档 Confirm、围栏 Allow、黑名单优先于一切、规则集编译失败恒 Confirm（fail-closed 直接置 None 验证）。
- tier 3：§8.5 矩阵全格断言 + serde roundtrip。
- read 9、write 3、edit 10、list_files 5、grep 7、compact/ToolOutcome 5、注册表集成测试 5（名称顺序钉死 ["read","write","edit","list_files","grep"]、spec 形态、能力归属、filter_for_tier）。

### 实现细化记录（§8 未定细节的落地选择，非出入）

1. **read 的 PDF**：M1 不实现。PDF 文档管线（含 pdf-extract 选型与「>10 页须显式页码」）整体属 M3（§8.9），M1 无文档未定的依赖选型；多数 PDF 因内部二进制流被 NUL 判定自然拒绝，报「二进制文件不支持文本读取」。
2. **read 行窗口**：offset 1-based（缺省 1，0 容错为 1）；除默认 2000 行外，响应体叠加 C-10 的 ≤512KB 上限（超限整行边界截断 + truncated=true），防止超长单行撑爆模型通道。
3. **read kind:"image"**：大小 ≤3MiB 与格式（png/jpeg/gif/webp 扩展名嗅探）在本工具内校验；「vision 模型前置校验」依赖模型能力上下文，由调用方（run loop）在选择 image 前完成，本切片纯逻辑不感知模型。
4. **edit §8.4 步骤 4/5（同批预演、倒序提交、组回滚）**：属批次事务，依赖 run loop 的工具批次调度，随后续切片落地；本切片为单文件 edit，落盘走同目录 tmp + fsync + rename 原子替换。
5. **edit version 语义顺序**：按 §8.4 括号语义实现为——version 不匹配时仍先跑匹配：唯一命中 → 降级警告执行（data.warning + ui_summary 标注）；0 或多命中 → E_EDIT_VERSION_STALE（而非 NO_MATCH/AMBIGUOUS，阻止盲改）。
6. **edit L2 落位**：缩进平移只用于「匹配」；替换区间含原首行缩进，newText 原样落位（缩进由模型自带），与 L3 行区间替换语义统一。
7. **edit lineChanges 口径**：del/add = oldText/newText 的行数（lines().count()，末尾有无换行不影响），与替换区间边界解耦，可确定性测试。
8. **fence 黑名单正则集的具体化**（§8.7 规则 1 为摘要）：rm 递归变体（-r/-f 组合与 --recursive）全覆盖；`git push` 含 `-f` 短旗标；shutdown 族加 halt/poweroff；管道执行覆盖 sh/bash/zsh/dash/ksh；Windows format 按 `format X:` 盘符形态匹配（裸 `\bformat\b` 会误拦 git format-patch，语义化收窄但方向仍 fail-closed）。
9. **fence 词法局限**（v0.1，保守方向）：按 `&&/||/;/|/换行` 词法分段、不解析引号——引号内元字符参与判定，只可能误拦（Confirm）不可能误放；白名单/写信号逐段判定。AST 管线（tree-sitter）为 P1（§8.7 M2 段）。
10. **fence fail-closed 实现形态**：规则集编译失败时 `Fence` 持 `None`，`judge` 恒返回 Confirm（有测试钉死）。
11. **grep 默认值**：maxResults 默认 100、硬上限 1000（§8.3 未定默认）；二进制文件（BinaryDetection::quit）跳过；单文件 IO 失败跳过不中断整体。
12. **list_files 输出形态**：缩进树（两空格/层，目录带 `/` 后缀），不用 ├└ 连线——500 条截断时连线无法保证准确；ignore crate `require_git(false)`：非 git 项目同样应用 .gitignore（工具语义，ignore 默认 require_git 会跳过）。
13. **params_schema 生成方式**：本切片以 serde_json::json! 手写常量（结构符合 §8.1：object + additionalProperties:false + required）；schemars 生成待接 backend/镜像工具时统一评估切换，避免本切片引入额外依赖。
14. **io 错误码映射**：NotFound → E_NOT_FOUND；其余 io → E_INTERNAL_STATE 兜底。§3.6 注册表无通用 IO 码且本票只允许动 §8，故借用 E_INTERNAL_STATE（message 附原始错误）；若后续票愿意补一行 E_IO 更贴切。
15. **compact_text**（§8.2 模型通道）顺带落地：成功序列化 data（头 4KB + 尾 8KB，UTF-8 字符边界安全截断），失败 error code+message；warnings/detail 不进模型文本。

### 偏离票面的决定

- 无。任务说明中「检查链六项」与收尾节「七项」口径不一致（fmt 单列），按七项执行全过。
