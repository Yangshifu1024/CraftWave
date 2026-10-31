/**
 * Composer 图片附件体系：附件条 UI、附件
 * 状态管理、Tauri 拖拽事件接线与 vision 拦截提示。
 *
 * 组件结构：ComposerBar（Thread.tsx）经 useComposerAttachments 挂附件状态，
 * AttachmentBar 渲染附件条（缩略图 + 尺寸 + 移除），DirectoryConfirmDialog
 * 承接「拖拽目录 → 添加为项目」确认。
 *
 * 拖拽通道说明（定案）：Tauri v2 webview 原生接管文件拖拽（wry file
 * drop handler），HTML5 drop 事件拿不到拖入物（File 对象/绝对路径均不可
 * 用）——拖拽统一走 `getCurrentWebview().onDragDropEvent`（绝对路径），
 * 图片经 `attach_image` 的 filePath 形态由后端读字节；粘贴（clipboardData
 * 的 File 对象）与附件按钮（file input）不受影响，走字节直传。
 */

import { getCurrentWebview } from "@tauri-apps/api/webview";
import { X } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { Dialog } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { t, useT } from "@/i18n/core";
import { attachImage, saveProject } from "@/ipc/client";
import { useSessionsStore } from "@/stores/sessions";
import {
  type ComposerImageAttachment,
  createAttachment,
  formatBytes,
  rejectMessage,
} from "./attachments";
import { isSupportedImageMime } from "./image-ref";

/** 附件条的一条短暂提示（超限/失败/vision 拦截；自动消退）。 */
export interface AttachmentHint {
  text: string;
  /** 提示形态：error 红 / info 灰（vision 拦截提示用 info）。 */
  tone: "error" | "info";
}

/** 提示自动消退时长（毫秒）。 */
const HINT_TTL_MS = 4000;

/**
 * 附件状态管理 hook：附件条数据 + 提示 + 三个入口的统一加件通道。
 * 附件状态为组件态（SessionTab keep-mounted，每会话一份，切换 Tab 天然
 * 隔离）；上传（attach_image）在加件时即完成，发送只带引用。
 */
export function useComposerAttachments(sessionId: string): {
  attachments: ComposerImageAttachment[];
  hint: AttachmentHint | null;
  showHint: (text: string, tone?: "error" | "info") => void;
  addFiles: (files: File[]) => Promise<void>;
  addFromPath: (path: string, mime: string) => Promise<void>;
  remove: (id: string) => void;
  clear: () => void;
} {
  const [attachments, setAttachments] = useState<ComposerImageAttachment[]>([]);
  const [hint, setHint] = useState<AttachmentHint | null>(null);
  const hintTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const showHint = useCallback((text: string, tone: "error" | "info" = "error") => {
    setHint({ text, tone });
    if (hintTimer.current !== null) clearTimeout(hintTimer.current);
    hintTimer.current = setTimeout(() => setHint(null), HINT_TTL_MS);
  }, []);

  // 会话切换：清空上一会话的附件草稿（附件跟 Tab 走，Tab 复用只发生在
  // 「新建但未发送」的空会话复用面，清空无丢失风险）
  useEffect(() => {
    // sessionId 引用进依赖（会话切换即清空附件草稿；体内判空兼防呆）
    if (sessionId === "") return;
    setAttachments([]);
    setHint(null);
  }, [sessionId]);

  useEffect(
    () => () => {
      if (hintTimer.current !== null) clearTimeout(hintTimer.current);
    },
    [],
  );

  const addFiles = useCallback(
    async (files: File[]) => {
      for (const file of files) {
        const count = attachments.length;
        try {
          const added = await createAttachment(file, sessionId, count);
          setAttachments((prev) => [...prev, added]);
        } catch (err) {
          // 校验失败（≤4 张/5MiB/mime 白名单）或上传失败：就地提示
          showHint(err instanceof Error ? err.message : String(err));
        }
      }
    },
    [attachments.length, sessionId, showHint],
  );

  const addFromPath = useCallback(
    async (path: string, mime: string) => {
      if (!isSupportedImageMime(mime)) {
        showHint(t("workspace.attachUnsupportedPath", { name: path.split("/").at(-1) ?? path }));
        return;
      }
      try {
        const { blobHash } = await attachImage({ sessionId, filePath: path, mimeType: mime });
        setAttachments((prev) => [
          ...prev,
          {
            id: crypto.randomUUID(),
            name: path.split("/").at(-1) ?? path,
            mimeType: mime,
            size: 0,
            previewUrl: "",
            blobHash,
          },
        ]);
      } catch (err) {
        showHint(err instanceof Error ? err.message : String(err));
      }
    },
    [sessionId, showHint],
  );

  const remove = useCallback((id: string) => {
    setAttachments((prev) => prev.filter((a) => a.id !== id));
  }, []);

  const clear = useCallback(() => setAttachments([]), []);

  return { attachments, hint, showHint, addFiles, addFromPath, remove, clear };
}

/** 附件条（Composer 输入框上方）：缩略图 + 文件名/尺寸 + 移除钮。 */
export function AttachmentBar({
  attachments,
  onRemove,
}: {
  attachments: ComposerImageAttachment[];
  onRemove: (id: string) => void;
}) {
  if (attachments.length === 0) return null;
  return (
    <div data-testid="attachment-bar" className="flex flex-wrap gap-2 px-1 pt-1">
      {attachments.map((a) => (
        <div
          key={a.id}
          data-testid="attachment-item"
          className="group relative flex items-center gap-2 rounded-lg border border-[var(--border)] bg-[var(--muted)]/60 py-1 pr-1 pl-1.5 text-ui-sm"
        >
          {a.previewUrl ? (
            <img
              src={a.previewUrl}
              alt={a.name}
              className="size-9 shrink-0 rounded-md object-cover"
            />
          ) : (
            <span className="flex size-9 shrink-0 items-center justify-center rounded-md bg-[var(--accent)] text-ui-sm text-[var(--muted-foreground)]">
              {t("workspace.attachThumbAlt")}
            </span>
          )}
          <span className="flex min-w-0 max-w-40 flex-col">
            <span className="truncate text-[var(--foreground)]">{a.name}</span>
            <span className="text-ui-sm text-[var(--muted-foreground)]">
              {a.size > 0 ? formatBytes(a.size) : t("workspace.attachUploaded")}
            </span>
          </span>
          <button
            type="button"
            aria-label={t("workspace.attachRemoveAria", { name: a.name })}
            onClick={() => onRemove(a.id)}
            className="flex size-5 shrink-0 items-center justify-center rounded text-[var(--muted-foreground)] hover:bg-[var(--accent)] hover:text-[var(--foreground)]"
          >
            <X size={12} aria-hidden />
          </button>
        </div>
      ))}
    </div>
  );
}

/** 附件提示行（超限/失败红字、vision 拦截灰字；HINT_TTL_MS 自动消退）。 */
export function AttachmentHintLine({ hint }: { hint: AttachmentHint | null }) {
  if (hint === null) return null;
  return (
    <p
      data-testid="attachment-hint"
      className={`px-1 pt-1 text-ui-sm ${hint.tone === "error" ? "text-[var(--destructive)]" : "text-[var(--muted-foreground)]"}`}
    >
      {hint.text}
    </p>
  );
}

/** Tauri 拖拽事件的接线条目（position + 类型）。 */
export interface DragEventPayload {
  type: "enter" | "over" | "drop" | "leave";
  /** 逻辑像素坐标（物理坐标已按 devicePixelRatio 换算，可直接比对 DOM 矩形）。 */
  x: number;
  y: number;
  /** drop 事件携带的绝对路径（enter/over/leave 缺省）。 */
  paths: string[];
}

/**
 * 订阅 Tauri 拖拽事件并把物理坐标换算成逻辑像素（onDragDropEvent 的
 * position 是 PhysicalPosition，DOM getBoundingClientRect 是逻辑像素）。
 * 返回取消订阅函数。纯浏览器环境（vitest happy-dom / 非 Tauri webview，
 * 无 `__TAURI_INTERNALS__` 注入）静默 no-op——测试渲染线程不加载 Tauri IPC。
 */
export function listenDragDrop(handler: (event: DragEventPayload) => void): Promise<() => void> {
  if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) {
    return Promise.resolve(() => {});
  }
  return getCurrentWebview()
    .onDragDropEvent((event) => {
      const scale = window.devicePixelRatio || 1;
      const payload = event.payload;
      if (payload.type === "drop") {
        handler({
          type: "drop",
          x: payload.position.x / scale,
          y: payload.position.y / scale,
          paths: payload.paths,
        });
      } else if (payload.type === "leave") {
        handler({ type: "leave", x: 0, y: 0, paths: [] });
      } else {
        handler({
          type: payload.type,
          x: payload.position.x / scale,
          y: payload.position.y / scale,
          paths: [],
        });
      }
    })
    .then((unlisten) => () => unlisten())
    .catch(() => () => {});
}

/** 坐标是否落在元素矩形内（拖拽路由的 Composer 命中判定）。 */
export function isPointInElement(el: HTMLElement | null, x: number, y: number): boolean {
  if (el === null) return false;
  const rect = el.getBoundingClientRect();
  return x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;
}

/** 拖入目录的「添加为项目」确认态。 */
export interface DropDirectory {
  path: string;
  name: string;
}

/** 「添加为项目」确认弹窗（拖拽目录 → save_project 既有命令）；
 *  复用既有 Dialog 骨架（遮罩/Esc 关闭与 a11y 语义由其承担）。 */
export function DropDirectoryDialog({
  drop,
  onClose,
}: {
  drop: DropDirectory | null;
  onClose: () => void;
}) {
  const t = useT();
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (drop !== null) {
      setName(drop.name);
      setError(null);
    }
  }, [drop]);
  const submit = () => {
    if (drop === null || busy) return;
    setBusy(true);
    setError(null);
    void (async () => {
      try {
        await saveProject({ project: { name: name.trim() || drop.name, homeDir: drop.path } });
        await useSessionsStore.getState().refresh();
        onClose();
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      } finally {
        setBusy(false);
      }
    })();
  };
  return (
    <Dialog
      open={drop !== null}
      title={t("workspace.dropDirTitle")}
      onClose={onClose}
      width={480}
      actions={[
        { label: t("workspace.cancel"), variant: "ghost", onClick: onClose },
        {
          label: t("workspace.dropDirConfirm"),
          variant: "default",
          disabled: busy,
          onClick: submit,
        },
      ]}
    >
      <p className="mb-4 text-ui-sm text-[var(--muted-foreground)]">
        {t("workspace.dropDirBody", { path: drop?.path ?? "" })}
      </p>
      <Input
        value={name}
        onChange={(e) => setName(e.target.value)}
        placeholder={t("workspace.dropDirNamePlaceholder")}
        data-testid="drop-dir-name"
      />
      {error !== null && <p className="mt-3 text-ui-sm text-[var(--destructive)]">{error}</p>}
    </Dialog>
  );
}

/** 附件校验失败原因 → 提示（re-export 转发，Composer 内单一引用面）。 */
export { rejectMessage };
