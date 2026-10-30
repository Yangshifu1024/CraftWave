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

describe("image-ref：cwimg 引用编解码", () => {
  const HASH = "a".repeat(32);

  it("构造与解析往返一致（blobHash/mimeType 保真）", () => {
    const url = cwImageUrl(HASH, "image/png");
    expect(url).toBe(`cwimg://${HASH}?mime=image/png`);
    expect(parseCwImageUrl(url)).toStrictEqual({ blobHash: HASH, mimeType: "image/png" });
  });

  it("非 cwimg 形态拒识（外来 data:/https 引用不误伤）", () => {
    expect(parseCwImageUrl("data:image/png;base64,AAAA")).toBeNull();
    expect(parseCwImageUrl("https://example.com/a.png")).toBeNull();
    expect(parseCwImageUrl("cwimg://no-mime-marker")).toBeNull();
    expect(parseCwImageUrl("")).toBeNull();
  });

  it("坏哈希与白名单外 mime 拒识", () => {
    expect(parseCwImageUrl(`cwimg://short?mime=image/png`)).toBeNull();
    expect(parseCwImageUrl(`cwimg://${"A".repeat(32)}?mime=image/png`)).toBeNull(); // 大写
    expect(parseCwImageUrl(`cwimg://${HASH}?mime=image/svg+xml`)).toBeNull();
    expect(parseCwImageUrl(`cwimg://${HASH}?mime=`)).toBeNull();
  });

  it("isBlobHash 与白名单判定（与后端同口径：32 位小写 hex / 四格式）", () => {
    expect(isBlobHash(HASH)).toBe(true);
    expect(isBlobHash("zzz")).toBe(false);
    expect(isSupportedImageMime("image/webp")).toBe(true);
    expect(isSupportedImageMime("image/heic")).toBe(false);
  });
});

describe("attachments：附件校验（≤4 张 / 5MiB / 白名单）", () => {
  const OK = { name: "a.png", size: 1024, type: "image/png" };

  it("合法图片通过；数量到 4 张后拒绝第 5 张", () => {
    expect(checkAttachment(OK, 0)).toStrictEqual({ ok: true });
    expect(checkAttachment(OK, MAX_ATTACHMENTS - 1)).toStrictEqual({ ok: true });
    const result = checkAttachment(OK, MAX_ATTACHMENTS);
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.reject.kind).toBe("too-many");
  });

  it("超过 5MiB 拒绝（边界：恰好 5MiB 通过）", () => {
    const edge = { name: "edge.png", size: MAX_ATTACHMENT_BYTES, type: "image/png" };
    expect(checkAttachment(edge, 0)).toStrictEqual({ ok: true });
    const big = { name: "big.png", size: MAX_ATTACHMENT_BYTES + 1, type: "image/png" };
    const result = checkAttachment(big, 0);
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.reject.kind).toBe("too-large");
  });

  it("白名单外 mime 拒绝（svg/heic 不进附件）", () => {
    for (const type of ["image/svg+xml", "image/heic", "text/plain", ""]) {
      const result = checkAttachment({ name: "x", size: 1, type }, 0);
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.reject.kind).toBe("unsupported-type");
    }
  });

  it("拒绝文案可直接展示（三类各成句）", () => {
    expect(rejectMessage({ kind: "too-many", limit: 4 })).toContain("4");
    expect(rejectMessage({ kind: "too-large", name: "big.png", limitBytes: 1 })).toContain(
      "big.png",
    );
    expect(rejectMessage({ kind: "unsupported-type", name: "x.svg" })).toContain(
      "png/jpeg/gif/webp",
    );
  });
});

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

describe("attachments：拖拽/粘贴分类路由（FR-13-16）", () => {
  it("目录条目 → directory 路由（提议添加项目，目录名带出）", () => {
    const route = classifyDrop(itemsOf([{ isDirectory: true, name: "my-proj" }]), null);
    expect(route).toStrictEqual({ kind: "directory", path: "my-proj" });
  });

  it("图片文件 → image 路由（可多张）", () => {
    const raw = [
      { name: "a.png", type: "image/png" },
      { name: "b.jpg", type: "image/jpeg" },
    ];
    const route = classifyDrop(null, filesOf(raw));
    expect(route).toEqual({ kind: "image", files: raw });
  });

  it("普通文件 → file 路由（路径引用；不受 vision 限制）", () => {
    const raw = [{ name: "main.rs", type: "" }];
    const route = classifyDrop(null, filesOf(raw));
    expect(route).toEqual({ kind: "file", files: raw });
  });

  it("混合投递：图片归图片路由（其余文件按普通文件逐个处理）", () => {
    const raw = [
      { name: "a.png", type: "image/png" },
      { name: "x.txt", type: "text/plain" },
    ];
    const route = classifyDrop(null, filesOf(raw));
    expect(route).toEqual({ kind: "image", files: [raw[0]] });
  });

  it("空投递返回 null（忽略）", () => {
    expect(classifyDrop(null, null)).toBeNull();
    expect(classifyDrop(null, filesOf([]))).toBeNull();
  });

  it("粘贴分类（clipboardData.files，无目录概念）", () => {
    const raw = [{ name: "shot.png", type: "image/png" }];
    expect(classifyPaste(filesOf(raw))).toEqual({ kind: "image", files: raw });
    expect(classifyPaste(null)).toBeNull();
  });
});

describe("attachments：人读字节（附件条尺寸提示）", () => {
  it("三档格式化", () => {
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(2048)).toBe("2 KB");
    expect(formatBytes(5 * 1024 * 1024)).toBe("5.0 MiB");
  });
});
