# 实现：storage 读写器与会话树命令（save/load_session_tree）

Type: task
Status: resolved

## Question

按《技术设计文档》§6（存储）与 §7（会话树）落地 M1 持久化第一切片（S4 已钉死 C-4 结构）：

1. storage 模块：append-only 分段 JSONL 读写器（分段边界、损坏尾段容错）、原子写（temp+rename 或文档指定机制）。
2. 会话树存取：`save_session_tree` / `load_session_tree` Tauri 命令，承载 §6.1 定稿的 headId/messages（含 `status?`）结构。
3. roundtrip 工程化：Rust 侧单测 + 前端契约测试（§12.3 manifest 校验方式），对齐 S4 spike（07 号票）已验证的结构与测试。

边界：不含分支/编辑/重生成的 UI 落地、崩溃恢复完整策略（后续票）；不含多会话 Tab。

完成标准：命令可通、roundtrip 测试绿、检查链全过；如实现与 §6/§7 有出入，票 Answer 记录并在最小范围内修订文档对应行。

## Answer

（2026-10-27 完成。分支 `feat/storage-session-tree`：`7199d63`（refactor，类型上移）+ 本票 feat/docs 提交。）

### 1. 结构落地（对齐 §6.1/§6.3/§6.4/§7.1–§7.4，与文档无出入）

- **条目类型（`storage/entries.rs` ↔ `ui/src/ipc/types.ts`）**：§6.1 三条目（session/message/meta，serde tag=kind）+ §3.2 WirePart M1 六变体（image 属 M2 不含）；`status?` 按 assistant-ui 0.15.31 实际类型建模为判别联合对象（`{"type":"complete","reason":"unknown"}` 等，kebab-case 变体），非字符串——S4 的 TS 镜像即此形态，Rust 侧逐一镜像；可选字段序列化一律省略（`parentId` null→省略，§6.3）。
- **读写器（`storage/segment.rs`）**：`0001.jsonl` 起分段；封口条件条目数≥200 或字节数≥512KB（§7.2），仅整批写完边界判定（本切片批=一次 save 的全量追加，即安全点，天然不拆 run 组）；封口原子写 `.segmeta.json`（段清单 file/count/bytes/headHash + headFingerprint，§7.1/§7.2）。指纹=追加链 h₀=SHA256("")、hₙ=SHA256(hex(hₙ₋₁)‖行字节)；恢复时校验 segmeta 覆盖前缀的链值，失配只新增新段、绝不截断（§7.2）。损坏尾段容错（§7.3「无半行」）：仅最后一段——无换行收尾的半行与首个非法行起的内容全部跳过且不入指纹链；非最后一段损坏直接报错（真实损伤 fail fast）。每落盘批次末一次 `sync_all`（§7.3）。
- **引擎（`storage/session_tree.rs`）**：`save_session_tree` = 会话锁（`storage/locks.rs`，§7.4 MutexMap）→ baseRevision==当前 revision（不等 E_SESSION_REV，§6.4）→ 既有 id 规范化 JSON 串比对（语义=内容哈希，变则 E_SESSION_REV，§6.3）→ 仅追加缺失节点（父存在性再校验，导出序父先子后）→ 空会话自动写 session 头 → 追加 meta 行推进 revision。`load_session_tree` = 全量条目重建 + 最新 meta 投影（activeLeafId/revision，追加式最新赢）。
- **原子写（`util/atomic.rs`，§7.4 文档指定位置）**：tempfile 同目录临时文件 + `sync_all` + rename（Windows 即 MoveFileExW REPLACE_EXISTING）+ Unix 目录 fsync；segmeta 与测试锁均走此路径。
- **命令层（`host/commands/session_tree.rs`）**：D14 负载 camelCase、错误 `{code,message,detail?}`（`host/error.rs`，From<StorageError> 逐码映射）；注册进 lib.rs 的 `ipc_commands!` 宏——invoke_handler 与 ipc_manifest 同源展开（§12.3 防漂移），manifest 测试落盘 `target/ipc-manifest.json`（commands/frames/events/tools，后三者为后续层占位空集）。

### 2. 测试清单（Rust 42 + 前端 32 全绿）

- Rust（独立 `*_test.rs`，遵守「源码零测试」）：原子写 3 例（覆盖/不留临时/自动建目录/无效路径）；locks 2 例；segment 10 例（序列化形状含 kebab status/planSnapshot、往返、200 条与 512KB 封口滚动、segmeta 记录、崩溃半行跳过+不续写既有段、尾段非法行、非尾段损坏报错、篡改指纹失配+滚新段、无 segmeta 可续写）；session_tree 12 例（roundtrip 逐条目、session 头唯一、幂等全量同步、追加分支字节前缀不变、空树、跨段多轮 save、baseRevision 冲突、同 id 内容漂移、孤儿/未知叶、非法 sessionId、NotFound、camelCase 契约形状）；error 2 例；ipc manifest+fixture 1 例。
- 前端（`ui/src/ipc/contract.test.ts`，§12.3）：manifest 双向扫描 2 例（client.ts 每个 invoke 名 ⊆ manifest.commands；存储两命令 args_type/ret_type 契约名）+ 跨语言 fixture roundtrip 3 例——**Rust serde 落盘字节**（cargo test 产出 `target/session-tree-fixture.jsonl`）解析后与 TS 手写镜像逐条目全等、还原 assistant-ui 树断言（结构/headId/status 对象/runId/createdAt/工具合并）、保存载荷=条目剥离 kind。CI 顺序=cargo test→vitest，缺产物即红（不静默跳过）。

### 3. 与文档的出入及边界说明

**§6/§7 无出入，文档未修订。** 内容比对以「规范化 JSON 串比对」实现（键序确定，语义等价于内容哈希）；封口阈值检查为写前判定，滚段发生在超限行的前一条边界。以下属切片边界而非出入，留给后续票：①`load_session_tree` 为票面命名，§3.3 注册表暂无此行（只有 #15 `load_session` 的 SessionData 形态），命名收敛（合并或补行）随会话接线票处理；②`E_STORAGE_IO` 为 §3.6 注册表暂缺的磁盘 IO 码（本次未越权改 §3.6，建议后续票补行）；③index.json/revision 双写、toolres 边车、fsync 表格中的 run 生命周期落盘点、segmeta headHash 逐段核验（当前校验全会话链前缀）随各自票接入；④manifest frames/events/tools 为空集占位。
