# 实现：图片输入与模型读图（附件、blob 存储、vision 消费）

Type: task
Status: resolved

## 要做什么

用户可附图发问（按钮/拖拽/粘贴三个入口），vision 模型真正看懂图片，重启后历史图片仍可回看；非 vision 模型附图有前置拦截；模型也能用 read 工具主动读工作区图片。覆盖 FR-12-02/03 + FR-13-16 拖拽半边 + §3.2 image part + §6.2 wire 映射 + §7.7 blob 存储。

## 具体做什么

### 后端（src-tauri/）

- **wire/存储层打通**（现状：`backend/wire.rs` 对 image part 按参数错误拒绝、`storage/entries.rs` WirePart 为 M1 子集无 image——本票 lifts）：
  - `WirePart` 增 `{ type:"image", blobHash, mimeType }`（§3.2 字段级已有；`storage/entries.rs` 条目形态同步，serde tag=kind 机制不变，条目追加纪律不变）。
  - blob 存储（§7.7 行逐字）：`sessions/<sid>.imgblob/<hash>.bin`，hash=SHA-256 前 32 hex 内容寻址（sha2 已有依赖）；写入去重（同 hash 跳过）；GC——会话删除时随目录整体删除；引用计数精确 GC 归 M3 保留期票（**票内取舍记录，回写 §7.7 注明 v1 口径**）。
  - wire 重建（§6.2 行逐字）：vision 模型 → 多模态 content 块（blob 经 Rust 侧读 base64 嵌入，rig 防腐层 image 载体接线）；非 vision 模型 → `[image: <hash>]` 占位文本；mime 白名单 png/jpeg/webp/gif。
  - steer 例外（§5.5 行已有）：带图消息不入 steer——`inject_run_message` 检测 image part 返回 queued=false（M1 前端驱动形态下转普通发送）。
- **`read` 工具读图**（FR-12-03）：`kind:"image"` 主体已在（`tools/read.rs` MAX_IMAGE_BYTES=3MiB）——本票补齐：mime 魔数嗅探（png/jpeg/webp/gif 四种，其余 E_INVALID_ARG）+ 结果 base64 进 vision 请求链 + uiSummary「读取图片 xxx.png（1.2MiB）」。
- **vision 校验与历史降级（2026-10-29 用户确认 Q30）**：非 vision 模型允许继续含历史图片的会话，历史图片仅在发给模型时转明确文字占位，界面保留原图并提示“当前模型无法查看历史图片”。start_chat 只拒绝本次新附图，read(kind=image) 主动读图同样拒绝并提示切换视觉模型；后端校验区分新输入与可信持久历史，不能因历史有图拒绝纯文字请求，也不能由前端把新图标成历史绕过检查。
- **承载命令缺口**：blob 上传/读取无既有命令——新增 `attach_image { sessionId, dataBase64, mimeType } → { blobHash }` 与 `read_image_blob { sessionId, blobHash } → { dataBase64, mimeType }`（**§3.3 M2 命令表无此二行，先回写 §3.3 再实现**；root 归属：仅本会话 imgblob 可读）。

### 前端（ui/）

- **Composer 附件**（FR-12-02/FR-13-16）：附件按钮（`<input type=file>` accept=image/*）+ 拖拽入 Composer 区（dragover 高亮）+ 粘贴（paste 事件 clipboardData 提取 image）；**≤4 张、单张 ≤5MiB**（超限即拒并提示）；附件条（缩略图+移除）；发送时 FileReader 读 base64 → `attach_image` 换 blobHash → save_session_tree 消息 part 携 `{type:"image", blobHash, mimeType}`（§6.3 双写协议不变）。
- **拖拽目录=提议添加项目**（FR-13-16）：拖拽 entry 为目录时弹「添加为项目」确认 → `save_project { name, homeDir }` 既有命令。
- **普通文件拖入（FR-13-16，接 28 号票）**：复用真实路径 chip 与发送引用段，文档内容由既有 read 管线读取；roots 外路径仍走 24 号票授权，不因拖入自动放开。普通路径引用不受 vision 模型限制，不另建通用二进制附件上传系统。
- **渲染与恢复**：消息内 image part → `<img>` 经 `read_image_blob` 取 base64（data URL；CSP `img-src 'self' data: blob:` 已放行 §9.5）；重启后 load_session_tree 携 image part 按 blobHash 拉取渲染（blob 缺失灰显占位）；vision 拦截 UI：非 vision 模型激活时附件按钮置灰+拖拽/粘贴拦截提示（vision 标记来自 get_config 的 ProviderModel.vision）。
- **read 工具卡图片态**：ReadCard 增缩略图展示（kind:"image" 结果，detail 边车/帧面 base64 取舍随 toolres 既有恢复链）。

- **请求预算接缝（Q18 用户确认）**：附图、切模型及下一次模型请求均走 04 号票的实际请求预算检查并预留回复空间；没有历史 usage 也不能跳过，压缩后仍超限则暂停保留输入。

### 测试

- 拖拽路由回归：图片走图片附件，目录提议添加项目，普通文件转路径引用；非 vision 模型可引用普通文件，外部路径不跳过授权。
- 后端：blob 去重与哈希稳定、image part save/load_session_tree roundtrip、wire 重建双形态（vision 多模态块/非 vision 占位）、非 vision 兜底拒、>5MiB/非法 mime 拒、attach_image 越权读他会话 blob 拒、带图 steer 拒绝路径。
- 前端 vitest：≤4 张/5MiB 校验、粘贴/拖拽事件提取、附件条增删、vision 置灰态；含历史图片时切非 vision 后纯文字继续、模型收到占位、界面保留原图与提示；新附图和 read 图拒绝。
- 全链手测：附图发送 → vision 模型真实消费（mock provider 断言多模态块）→ 重启恢复缩略图。

## 边界（不在本票）

- vision 标记的设置编辑 UI（ProviderModel.vision 字段已存在）；blob 引用计数精确 GC 与保留期清理联动（M3）；截图/浏览器截图（M3 §8.10）；图片生成（多模态输出）；会话导出图片内容（27 号票已定仅引用、不内联）。

## 验收

- [x] 含历史图片的会话切非 vision 模型后可纯文字继续；模型收到明确占位，界面保留原图并提示模型看不到；新附图及主动读图拒绝。
  - e2e `non_vision_rejects_new_image_and_degrades_history_to_placeholder`：历史带图 + 纯文字 prompt 正常 run 到 Done，请求 history 中历史图为 `[image: <hash>; 当前模型无法查看历史图片]` 占位文本、无 image_url 多模态块；新附图（prompt 带图）命令面 E_INVALID_ARG 且零模型请求；read(kind=image) 由执行器分发面按 ProviderModel.vision 拒绝。前端气泡内提示行（image-vision-hint）按 visionOf 判定渲染。

- [x] 附件/拖拽/粘贴三入口附图（≤4 张、单张 5MiB）发送成功
  - 附件按钮（file input accept 白名单四格式）与粘贴（clipboardData.files）走字节直传（FileReader base64 → attach_image）；拖拽走 Tauri onDragDropEvent（webview 原生接管，HTML5 drop 不可用——见决议记录 ④）经 filePath 形态由后端读字节。≤4 张/5MiB 双侧校验（前端 checkAttachment + 后端 put_blob/attach_image）。发送 = thread.append 携 cwimg 引用 image part（空闲）或 inject attachments 转 followUp（运行中/Alt+Enter）。

- [x] vision 模型真实消费图片（wire 多模态块），非 vision 模型前置拦截且有后端兜底
  - e2e `vision_model_consumes_image_as_multimodal_block`：mock provider 请求体断言 `image_url` 块、data URL 前缀与 base64 字节逐字节一致；wire 层单测钉住 vision/非 vision/缺 blob 三形态。前端 visionOf 置灰 + 后端 split_prompt_visioned（Q30 新输入与持久历史后端区分）双防线。

- [x] blob 落 `sessions/<sid>.imgblob/`，JSONL 正文不含图片数据（体积不膨胀）
  - `storage/imgblob.rs`（内容寻址 + 同哈希去重 + 5MiB/白名单校验）；fixture 行（u3 条目）逐字节钉住 image part 序列化形态 `{type:"image",blobHash,mimeType}`（跨语言往返测试双侧通过）；会话删除连带删 `<sid>.imgblob/`（imgblob_test::delete_session_removes_imgblob_dir）。

- [x] 重启后历史图片消息缩略图恢复可看，blob 缺失有灰显占位
  - 恢复链 = load_session_tree → entryToMessageLike（WirePart::Image → cwimg 引用 image part）→ MessageImagePart 组件按 blobHash 经 read_image_blob 拉 data URL 渲染；blob 缺失（E_NOT_FOUND）灰显占位（image-missing）；contract.test 往返等价断言（树还原 → messageToEntry 逐字段一致）保证恢复后再保存不漂移。

- [x] read 工具读图（≤3MiB、png/jpeg/webp/gif）卡片缩略展示；拖拽目录提议添加项目
  - read_image 改魔数嗅探（不信任扩展名，文本伪装拒）；ReadCard 增图片态（边车 base64 → 缩略图，恢复链照常）；拖拽目录（webkitGetAsEntry/后端 path_kind 判定）弹「添加为项目」确认 → save_project 既有命令（DropDirectoryDialog）。

- [x] §3.3 新命令回写完成，检查链全绿
  - §3.3 增 #59h `attach_image`（dataBase64/filePath 二选一形态注记）/ #59i `read_image_blob` / #59j `path_kind`，#21 行补 attachments 参数与带图不入 steer 注记；§5.5 补 17 号票实装注记；§7.7 imgblob 行补 v1 GC 口径。检查链七项全过（cargo fmt --check / build / clippy --all-targets 零告警 / test 587 通过（lib 540 + 集成 47）；前端 typecheck / vitest 289 全过 / biome lint 零诊断）。

## 决议记录（2026-10-30 实施）

1. **带图 steer 的实现形态（票面「返回 queued=false」与 §5.5「进 followUp」的取舍）**：按 §5.5 原文落地——`inject_run_message` 增 `attachments` 参数，非空时无论 mode 与活跃状态一律转 followUp 队列（返回 mode="followUp" + entryId），不采用 queued=false 防御态。理由：M2 后端队列已是既有形态，followUp 支持附件持久化（`FollowUpEntry.attachments` 从 `Vec<serde_json::Value>` 预留位改为类型化 `Vec<QueuedImageAttachment>`，预留位语义「恒空」故无兼容负担），前端无需处理罕见防御态；queued=false 语义保持不变（正常路径恒 true 的契约注记不动）。§3.3 #21 与 §5.5 已按此回写。
2. **新增第三命令 `path_kind`（#59j）**：票面「承载命令缺口」只列了二命令，但拖拽路由（目录提议添加项目）需要前端判定拖入路径是目录还是文件——HTML 拖放被 webview 原生接管后前端只有绝对路径字符串，无法 stat。后端补纯探测命令（kind 三态 + 扩展名图片识别），§3.3 已回写。
3. **attach_image 增 filePath 形态**：拖拽入口只有绝对路径（Tauri webview 原生接管文件拖拽，wry file-drop handler 生效时 HTML5 drop 事件拿不到 File 对象与字节），附件按钮/粘贴入口仍走 dataBase64 直传。命令签名改为 `dataBase64?` / `filePath?` 二选一（都有/都空 E_INVALID_ARG）。filePath 形态**不做 roots 校验**：用户显式拖入与文件选择器选图等价（同为把图交给会话的主动动作），魔数白名单与 5MiB 照检；模型侧文件可见性仍由 read 管线授权链把关（FR-13-16「不因拖入自动放开」）。§3.3 #59h 已注记。
4. **前端拖拽通道统一 Tauri onDragDropEvent**：dragover 高亮经 enter/over 事件的坐标与 Composer 区 DOM 矩形命中判定（PhysicalPosition 按 devicePixelRatio 换算逻辑坐标）；drop 按 paths 逐个调 path_kind 分流（目录→项目确认、图片→附件、普通文件→@ 路径引用文本）。纯浏览器环境（vitest）无 `__TAURI_INTERNALS__` 注入，listenDragDrop 静默 no-op。
5. **图片 token 估算常数**：estimator 对 image part 计 2048 token/张（宁高勿低，对应主流视觉模型中分辨率图消耗上沿）+ 哈希占位文本；视觉 token 随供应商/分辨率分化无法从字节估算，Q18 预算检查照常覆盖（附图计入估算面）。
6. **日志纪律落点**：32 号票会话日志未实现，本票图片元数据（哈希/格式/字节数）仅在 debug 构建经 eprintln 落进程终端（blob 装配跳过提示），无既有会话日志通道可写；全链无图片字节入日志/事件面（spec「图片与日志」条），32 号票实现时按 spec 承接。
7. **vision 前端判定的刷新节奏**：`visionOf(sessionId)` 读 zustand 缓存（get_config 拉取），ComposerBar 挂载/会话切换刷新一次 + ModelPicker 打开时拉取同节奏。设置页改 vision 标记后需重开会话生效（M1 简化，与既有 ModelPicker 拉取节奏一致）；后端判定永远以 start_chat 当次 resolve_model 为准，前后端口径不会矛盾（前端置灰多拦不影响正确性）。
8. **fixture 行序**：带图条目（u3）挂在分支链（a2/a3/marker）之后——`ExportedMessageRepository.fromBranchableArray` 的树重排会改变跨分支消息的枚举序，行序保持「父先子后 + 分支链优先」与两侧往返断言一致（contract.test 钉死）。
