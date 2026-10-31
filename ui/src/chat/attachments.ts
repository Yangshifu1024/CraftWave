/**
 * Composer 图片附件体系（含拖拽半边）。
 *
 * 职责（纯逻辑与事件提取，React 状态归 ComposerBar 组件态）：
 * - 附件校验：单条消息 ≤4 张、单张 ≤5MiB、png/jpeg/gif/webp 白名单
 *   （与后端 put_blob 同口径，双侧防线）；
 * - File → base64（FileReader 承诺包装，dataURL 与裸 base64 两种形态）；
 * - 拖拽/粘贴事件的文件提取与路由分类（图片 → 附件、目录 → 提议添加
 *   项目、普通文件 → 路径引用 chip）；
 * - 附件对象（ComposerImageAttachment）：id/文件名/mime/本地预览 dataURL/
 *   后端 blobHash——上传（attach_image）在加入附件条时即完成，发送时
 *   只带引用。
 */

import { t } from "@/i18n/core";
import { attachImage as clientAttachImage } from "@/ipc/client";
import { isSupportedImageMime, MAX_ATTACHMENT_BYTES, MAX_ATTACHMENTS } from "./image-ref";

/** 一张待发图片附件（Composer 附件条的数据面）。 */
export interface ComposerImageAttachment {
  /** 附件条内唯一 id（crypto.randomUUID；移除按钮的键）。 */
  id: string;
  /** 原始文件名（附件条 title 提示用）。 */
  name: string;
  /** MIME 类型（png/jpeg/gif/webp 白名单内）。 */
  mimeType: string;
  /** 字节数（超限校验与附件条尺寸提示）。 */
  size: number;
  /** 本地预览 data URL（附件条缩略图直接显示，不回源后端）。 */
  previewUrl: string;
  /** 后端 blob 哈希（attach_image 返回；内容寻址）。 */
  blobHash: string;
}

/** 附件校验失败的原因（UI 提示文案直接可读）。 */
export type AttachmentReject =
  | { kind: "too-many"; limit: number }
  | { kind: "too-large"; name: string; limitBytes: number }
  | { kind: "unsupported-type"; name: string };

/** 附件校验的当前结果：可加 / 拒绝原因。 */
export type AttachmentCheck = { ok: true } | { ok: false; reject: AttachmentReject };

/**
 * 校验一张待加图片：数量上限（现有 count + 1 ≤ 4）、单张字节 ≤5MiB、
 * mime 白名单。纯函数——ComposerBar 在加入前调用并就地提示。
 */
export function checkAttachment(
  file: { name: string; size: number; type: string },
  currentCount: number,
): AttachmentCheck {
  if (currentCount >= MAX_ATTACHMENTS) {
    return { ok: false, reject: { kind: "too-many", limit: MAX_ATTACHMENTS } };
  }
  if (file.size > MAX_ATTACHMENT_BYTES) {
    return {
      ok: false,
      reject: { kind: "too-large", name: file.name, limitBytes: MAX_ATTACHMENT_BYTES },
    };
  }
  if (!isSupportedImageMime(file.type)) {
    return { ok: false, reject: { kind: "unsupported-type", name: file.name } };
  }
  return { ok: true };
}

/** 附件校验拒绝原因 → 用户可读提示文案（附件条与拦截提示共用）。 */
export function rejectMessage(reject: AttachmentReject): string {
  switch (reject.kind) {
    case "too-many":
      return t("workspace.attachLimit", { limit: reject.limit });
    case "too-large":
      return t("workspace.attachTooLarge", { name: reject.name });
    case "unsupported-type":
      return t("workspace.attachUnsupported", { name: reject.name });
  }
}

/** File → data URL（本地预览；FileReader 包装为承诺）。 */
export function readFileDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(reader.error ?? new Error(t("workspace.attachReadFailed")));
    reader.readAsDataURL(file);
  });
}

/** File → 裸 base64（attach_image 的 dataBase64 参数；去 data URL 前缀）。 */
export function readFileBase64(file: File): Promise<string> {
  return readFileDataUrl(file).then((dataUrl) => {
    const comma = dataUrl.indexOf(",");
    if (comma < 0) throw new Error(t("workspace.dataUrlMalformed"));
    return dataUrl.slice(comma + 1);
  });
}

/**
 * File → 附件对象：校验 + 读预览 + 上传换 blobHash（attach_image）。
 * 上传失败抛错（调用方提示；附件不加条）。IPC 依赖可注入（测试换假实现）。
 */
export async function createAttachment(
  file: File,
  sessionId: string,
  currentCount: number,
  deps: {
    attachImage: (args: {
      sessionId: string;
      dataBase64: string;
      mimeType: string;
    }) => Promise<{ blobHash: string }>;
  } = { attachImage: clientAttachImage },
): Promise<ComposerImageAttachment> {
  const check = checkAttachment(file, currentCount);
  if (!check.ok) throw new Error(rejectMessage(check.reject));
  const [previewUrl, dataBase64] = await Promise.all([readFileDataUrl(file), readFileBase64(file)]);
  const { blobHash } = await deps.attachImage({
    sessionId,
    dataBase64,
    mimeType: file.type,
  });
  return {
    id: crypto.randomUUID(),
    name: file.name,
    mimeType: file.type,
    size: file.size,
    previewUrl,
    blobHash,
  };
}

/** 拖拽/粘贴投递的分类（拖拽路由）：图片走附件、目录提议添加
 *  项目、普通文件转路径引用。 */
export type DropRoute =
  | { kind: "image"; files: File[] }
  | { kind: "directory"; path: string }
  | { kind: "file"; files: File[] };

/** 判断 File 是否白名单图片（mime 声明面）。 */
function isImageFile(file: File): boolean {
  return isSupportedImageMime(file.type);
}

/**
 * 从拖拽事件提取投递物（路由判定的核心）：
 * - 事件里有目录条目（webkitGetAsEntry 判 isDirectory）→ directory 路由
 *   （目录名作提议添加项目的默认名；多目录取第一个）；
 * - 其余按 mime 分流：图片文件（可多张）→ image；普通文件 → file。
 *
 * 用 items 优先（能识别目录），items 不可用时退回 files（粘贴事件没有
 * entry 概念，直接按 mime 分流）。读取 entries 是同步 API（webkitGetAsEntry
 * 不返回承诺），但目录**内部**遍历不在处理范围——目录只做「添加为项目」。
 */
export function classifyDrop(
  items: DataTransferItemList | null,
  files: FileList | null,
): DropRoute | null {
  // 目录优先：webkitGetAsEntry 的 isDirectory（Chromium/WebKit 均实现）
  if (items !== null) {
    for (let i = 0; i < items.length; i++) {
      const entry = (
        items[i] as DataTransferItem & {
          webkitGetAsEntry?: () => { isDirectory: boolean; name?: string } | null;
        }
      ).webkitGetAsEntry?.();
      if (entry?.isDirectory === true) {
        return { kind: "directory", path: entry.name ?? t("workspace.unnamedDirectory") };
      }
    }
  }
  if (files === null || files.length === 0) return null;
  const all: File[] = Array.from(files);
  const images = all.filter(isImageFile);
  if (images.length === all.length && images.length > 0) {
    return { kind: "image", files: images };
  }
  // 混合投递：图片归图片，其余归普通文件路径引用（两边都处理）
  if (images.length > 0) {
    return { kind: "image", files: images };
  }
  return { kind: "file", files: all };
}

/** 粘贴事件 → 分类（clipboardData.files 提取；粘贴无目录概念）。 */
export function classifyPaste(clipboardFiles: FileList | null): DropRoute | null {
  return classifyDrop(null, clipboardFiles);
}

/** 字节数的人读形态（附件条尺寸提示）。 */
export function formatBytes(size: number): string {
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(0)} KB`;
  return `${(size / (1024 * 1024)).toFixed(1)} MiB`;
}
