/**
 * cwimg 图片引用编解码与附件校验的单元测试（17 号票）。
 *
 * image-ref：引用 URL 的构造/解析往返（树内 image part ↔ 存储 image part
 * 的桥）；非法形态（非 cwimg、坏哈希、白名单外 mime）拒识。
 * attachments：附件校验（≤4 张/5MiB/白名单）、拖拽/粘贴分类路由（目录提议
 * 添加项目、图片转附件、普通文件转路径引用）、拒绝文案与人读字节。
 */

import { describe, expect, it } from "vitest";
import {
  checkAttachment,
  classifyDrop,
  classifyPaste,
  formatBytes,
  rejectMessage,
} from "./attachments";
import {
  cwImageUrl,
  isBlobHash,
  isSupportedImageMime,
  MAX_ATTACHMENT_BYTES,
  MAX_ATTACHMENTS,
  parseCwImageUrl,
} from "./image-ref";

/** DataTransferItemList 的最小 mock（classifyDrop 只用 length 与 [i] 索引）。 */
function itemsOf(entries: { isDirectory: boolean; name?: string }[]): DataTransferItemList {
  const list = entries.map((e) => ({
    webkitGetAsEntry: () => e,
  }));
  return Object.assign(list, { length: list.length }) as unknown as DataTransferItemList;
}

/** FileList 的最小 mock（classifyDrop 只用 length 与 [i] 索引、Array.from）。 */
function filesOf(files: { name: string; type: string }[]): FileList {
  return Object.assign([...files], { length: files.length }) as unknown as FileList;
}

describe("attachments（17 号票）", () => {
  const HASH = "a".repeat(32);

  it("image-ref：cwimg 引用编解码往返、非法形态拒识、哈希与白名单判定", () => {
    // 构造与解析往返一致（blobHash/mimeType 保真）
    const url = cwImageUrl(HASH, "image/png");
    expect(url).toBe(`cwimg://${HASH}?mime=image/png`);
    expect(parseCwImageUrl(url)).toStrictEqual({ blobHash: HASH, mimeType: "image/png" });
    // 非 cwimg 形态拒识（外来 data:/https 引用不误伤）
    expect(parseCwImageUrl("data:image/png;base64,AAAA")).toBeNull();
    expect(parseCwImageUrl("https://example.com/a.png")).toBeNull();
    expect(parseCwImageUrl("cwimg://no-mime-marker")).toBeNull();
    expect(parseCwImageUrl("")).toBeNull();
    // 坏哈希与白名单外 mime 拒识
    expect(parseCwImageUrl(`cwimg://short?mime=image/png`)).toBeNull();
    expect(parseCwImageUrl(`cwimg://${"A".repeat(32)}?mime=image/png`)).toBeNull(); // 大写
    expect(parseCwImageUrl(`cwimg://${HASH}?mime=image/svg+xml`)).toBeNull();
    expect(parseCwImageUrl(`cwimg://${HASH}?mime=`)).toBeNull();
    // 与后端同口径：32 位小写 hex / 四格式白名单
    expect(isBlobHash(HASH)).toBe(true);
    expect(isBlobHash("zzz")).toBe(false);
    expect(isSupportedImageMime("image/webp")).toBe(true);
    expect(isSupportedImageMime("image/heic")).toBe(false);
  });

  it("附件校验红线（≤4 张/5MiB/白名单）+ 拒绝文案 + 人读字节", () => {
    const OK = { name: "a.png", size: 1024, type: "image/png" };
    // 合法图片通过；数量到 4 张后拒绝第 5 张
    expect(checkAttachment(OK, 0)).toStrictEqual({ ok: true });
    expect(checkAttachment(OK, MAX_ATTACHMENTS - 1)).toStrictEqual({ ok: true });
    const tooMany = checkAttachment(OK, MAX_ATTACHMENTS);
    expect(tooMany.ok).toBe(false);
    if (!tooMany.ok) expect(tooMany.reject.kind).toBe("too-many");

    // 大小边界：恰好 5MiB 通过，超过拒绝
    const edge = { name: "edge.png", size: MAX_ATTACHMENT_BYTES, type: "image/png" };
    expect(checkAttachment(edge, 0)).toStrictEqual({ ok: true });
    const tooLarge = checkAttachment(
      { name: "big.png", size: MAX_ATTACHMENT_BYTES + 1, type: "image/png" },
      0,
    );
    expect(tooLarge.ok).toBe(false);
    if (!tooLarge.ok) expect(tooLarge.reject.kind).toBe("too-large");

    // 白名单外 mime 拒绝（svg/heic/普通文本/无类型不进附件）
    for (const type of ["image/svg+xml", "image/heic", "text/plain", ""]) {
      const result = checkAttachment({ name: "x", size: 1, type }, 0);
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.reject.kind).toBe("unsupported-type");
    }

    // 拒绝文案可直接展示（三类各成句）
    expect(rejectMessage({ kind: "too-many", limit: 4 })).toContain("4");
    expect(rejectMessage({ kind: "too-large", name: "big.png", limitBytes: 1 })).toContain(
      "big.png",
    );
    expect(rejectMessage({ kind: "unsupported-type", name: "x.svg" })).toContain(
      "png/jpeg/gif/webp",
    );

    // 人读字节（附件条尺寸提示）：三档格式化
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(2048)).toBe("2 KB");
    expect(formatBytes(5 * 1024 * 1024)).toBe("5.0 MiB");
  });

  it("拖拽/粘贴分类路由（FR-13-16）：目录/图片/文件/混合/空投递", () => {
    // 目录条目 → directory 路由（提议添加项目，目录名带出）
    expect(classifyDrop(itemsOf([{ isDirectory: true, name: "my-proj" }]), null)).toStrictEqual({
      kind: "directory",
      path: "my-proj",
    });
    // 图片文件 → image 路由（可多张）
    const images = [
      { name: "a.png", type: "image/png" },
      { name: "b.jpg", type: "image/jpeg" },
    ];
    expect(classifyDrop(null, filesOf(images))).toEqual({ kind: "image", files: images });
    // 普通文件 → file 路由（路径引用；不受 vision 限制）
    expect(classifyDrop(null, filesOf([{ name: "main.rs", type: "" }]))).toEqual({
      kind: "file",
      files: [{ name: "main.rs", type: "" }],
    });
    // 混合投递：图片归图片路由（其余文件按普通文件逐个处理）
    const mixed = [
      { name: "a.png", type: "image/png" },
      { name: "x.txt", type: "text/plain" },
    ];
    expect(classifyDrop(null, filesOf(mixed))).toEqual({ kind: "image", files: [mixed[0]] });
    // 空投递返回 null（忽略）
    expect(classifyDrop(null, null)).toBeNull();
    expect(classifyDrop(null, filesOf([]))).toBeNull();
    // 粘贴分类（clipboardData.files，无目录概念）
    const paste = [{ name: "shot.png", type: "image/png" }];
    expect(classifyPaste(filesOf(paste))).toEqual({ kind: "image", files: paste });
    expect(classifyPaste(null)).toBeNull();
  });
});
