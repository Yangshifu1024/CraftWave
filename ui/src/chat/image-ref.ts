/**
 * 消息内图片引用（cwimg://）的编解码（17 号票，§3.2 image part 的前端载体）。
 *
 * 设计对齐：WirePart::Image 只存 `{ blobHash, mimeType }`（不含图片本体，
 * JSONL 不膨胀）；assistant-ui 的 ImageMessagePart 只有一个 `image` 字符串
 * 字段。两者之间的桥就是本模块的 cwimg 引用 URL：
 *
 *   `cwimg://<blobHash>?mime=<mimeType>`
 *
 * - 发送/保存方向：树里的 image part（image 字段 = cwimg 引用）解析回
 *   `{ blobHash, mimeType }`（runtime.toWire 与 tree-persistence.partToWire
 *   共用）；
 * - 恢复/渲染方向：WirePart::Image 构造回 cwimg 引用进树，渲染组件按
 *   blobHash 经 `read_image_blob` 拉 base64 data URL 显示（blob 缺失灰显）。
 *
 * 这个文件是 cwimg 引用格式的单一事实源——格式改动只落这里。
 */

/** cwimg 引用的协议前缀。 */
const CWIMG_SCHEME = "cwimg://";

/** blob 哈希形状（与后端 is_valid_blob_hash 同口径：32 位小写 hex）。 */
export function isBlobHash(hash: string): boolean {
  return hash.length === 32 && /^[0-9a-f]+$/.test(hash);
}

/** MIME 白名单（与后端 sniff_image_mime 同一份口径）。 */
export const IMAGE_MIME_WHITELIST = ["image/png", "image/jpeg", "image/gif", "image/webp"];

/** mime 是否在白名单内。 */
export function isSupportedImageMime(mime: string): boolean {
  return IMAGE_MIME_WHITELIST.includes(mime);
}

/** 构造 cwimg 引用 URL（树内 image part 的 image 字段值）。 */
export function cwImageUrl(blobHash: string, mimeType: string): string {
  return `${CWIMG_SCHEME}${blobHash}?mime=${mimeType}`;
}

/**
 * 解析 cwimg 引用 URL。非 cwimg 形态（外部 data:/https 链接等）、哈希形状
 * 不合法或 mime 缺失时返回 null——调用方按「不是本应用的图片引用」处理
 * （不解析、不误伤）。
 */
export function parseCwImageUrl(image: string): { blobHash: string; mimeType: string } | null {
  if (!image.startsWith(CWIMG_SCHEME)) return null;
  const rest = image.slice(CWIMG_SCHEME.length);
  const marker = "?mime=";
  const at = rest.indexOf(marker);
  if (at < 0) return null;
  const blobHash = rest.slice(0, at);
  const mimeType = rest.slice(at + marker.length);
  if (!isBlobHash(blobHash) || !isSupportedImageMime(mimeType)) return null;
  return { blobHash, mimeType };
}

/** 附件上限（FR-12-02）：单条消息 ≤4 张。 */
export const MAX_ATTACHMENTS = 4;

/** 附件单张字节上限（FR-12-02）：≤5MiB（与后端 put_blob 同口径）。 */
export const MAX_ATTACHMENT_BYTES = 5 * 1024 * 1024;
