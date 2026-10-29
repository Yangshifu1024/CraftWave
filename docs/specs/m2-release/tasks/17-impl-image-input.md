# 实现：图片输入与模型读图（附件、blob 存储、vision 消费）

Type: task
Status: open

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

- [ ] 含历史图片的会话切非 vision 模型后可纯文字继续；模型收到明确占位，界面保留原图并提示模型看不到；新附图及主动读图拒绝。

- [ ] 附件/拖拽/粘贴三入口附图（≤4 张、单张 5MiB）发送成功
- [ ] vision 模型真实消费图片（wire 多模态块），非 vision 模型前置拦截且有后端兜底
- [ ] blob 落 `sessions/<sid>.imgblob/`，JSONL 正文不含图片数据（体积不膨胀）
- [ ] 重启后历史图片消息缩略图恢复可看，blob 缺失有灰显占位
- [ ] read 工具读图（≤3MiB、png/jpeg/webp/gif）卡片缩略展示；拖拽目录提议添加项目
- [ ] §3.3 新命令回写完成，检查链全绿
