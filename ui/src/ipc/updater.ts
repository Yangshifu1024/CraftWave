/**
 * 应用内更新编排层（33 号票，FR-16-03/§12.4 更新器骨架）。
 *
 * 职责边界：更新本体（检查/下载/安装/重启）直用官方插件 JS API——
 * `@tauri-apps/plugin-updater` 的 check/downloadAndInstall（实际下载与
 * minisign 签名校验都在 Rust 侧执行，不经 WebView，CSP connect-src 不动）
 * 与 `@tauri-apps/plugin-process` 的 relaunch；本文件只做三件事：
 * 1. 三态收敛：check() 的 `Update | null` + 异常收敛为
 *    available / upToDate / failed 三态（设置页「检查更新」区直接消费）；
 * 2. 「有新版」的 Update 是 Rust 侧资源（Resource），检查到新版后暂存于
 *    模块级槽位，安装（downloadAndInstall）或放弃（close）时释放——
 *    悬挂不关会泄漏 Rust 侧资源；
 * 3. 启动静默检查的间隔判定纯函数（shouldSilentCheck）与安装门前置
 *    （updatePrecheck IPC 命令在 client.ts，本文件编排调用）。
 *
 * 禁用兜底（票内 Answer）：tauri.conf.json 的 updater pubkey 为占位空串
 * （HITL 密钥未配）期间 check 必败——入口以 update_status.pubkeyConfigured
 * + config.update.enabled 双门禁用，不依赖 check 失败兜底。
 */

import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import * as client from "@/ipc/client";
import type { UpdatePrecheckResult } from "@/ipc/types";

/** 更新检查的三态（设置页「检查更新」区与启动静默检查共用）。 */
export type UpdateCheckState =
  | {
      kind: "available";
      /** 新版本号（latest.json 的 version）。 */
      version: string;
      /** 当前运行版本。 */
      currentVersion: string;
      /** release notes（latest.json 的 notes；可能缺省）。 */
      notes?: string;
    }
  | { kind: "upToDate" }
  | { kind: "failed"; message: string };

/** 下载安装进度态（Dialog 按钮区消费）。 */
export type InstallProgress =
  | { kind: "idle" }
  | { kind: "downloading"; received: number; total?: number }
  | { kind: "installing" }
  | { kind: "done" };

/** 「有新版」时暂存的 Rust 侧 Update 资源（同一时刻至多一个；安装或放弃
 *  后置回 null 并 close 释放）。模块级槽位：Update 实例不可序列化过 IPC，
 *  必须留在发起检查的同一 WebView 会话里。 */
let pendingUpdate: Update | null = null;

/** 读取当前暂存的更新资源（Dialog 安装按钮消费；无则 null）。 */
export function takePendingUpdate(): Update | null {
  return pendingUpdate;
}

/** 释放暂存的更新资源（Dialog 关闭不安装时调用；幂等）。 */
export function discardPendingUpdate(): void {
  if (pendingUpdate !== null) {
    // close 释放 Rust 侧资源；失败不抛（资源随进程退出兜底回收）。
    // Promise.resolve 包一层：close 契约是 Promise，防御非 Promise 返回值。
    void Promise.resolve(pendingUpdate.close()).catch(() => {});
    pendingUpdate = null;
  }
}

/** 发起一次更新检查（三态收敛；调用方负责 enabled/pubkey 门控）。 */
export async function checkForUpdate(): Promise<UpdateCheckState> {
  try {
    const update = await check();
    if (update === null) {
      return { kind: "upToDate" };
    }
    pendingUpdate = update;
    return {
      kind: "available",
      version: update.version,
      currentVersion: update.currentVersion,
      ...(update.body !== undefined ? { notes: update.body } : {}),
    };
  } catch (err) {
    // 插件错误（网络失败 / pubkey 占位配置错误）统一收敛为 failed 态，
    // message 原样带出（错误面按码本地化不在本层——updater 错误无稳定码）
    const message = err instanceof Error ? err.message : String(err);
    return { kind: "failed", message };
  }
}

/** 下载并安装已检查到的更新（进度经 onProgress 回调；完成后调用方提示
 *  「重启以完成更新」）。无暂存更新时抛错（调用序错误，不该发生）。 */
export async function downloadAndInstallUpdate(
  onProgress?: (progress: InstallProgress) => void,
): Promise<void> {
  const update = pendingUpdate;
  if (update === null) {
    // 内部调用序错误（开发者错误，非用户文案——不进 i18n 字典）
    throw new Error("no update checked; call checkForUpdate() first");
  }
  onProgress?.({ kind: "downloading", received: 0 });
  await update.downloadAndInstall((event) => {
    if (event.event === "Started") {
      onProgress?.({
        kind: "downloading",
        received: 0,
        ...(event.data.contentLength !== undefined ? { total: event.data.contentLength } : {}),
      });
    } else if (event.event === "Progress") {
      onProgress?.({ kind: "downloading", received: event.data.chunkLength });
    } else {
      onProgress?.({ kind: "installing" });
    }
  });
  onProgress?.({ kind: "done" });
}

/** 重启应用以完成更新（process 插件 relaunch；安装完成后由用户点击触发）。 */
export async function relaunchForUpdate(): Promise<void> {
  await relaunch();
}

/**
 * 启动静默检查的间隔判定（纯函数，vitest 表驱动）：`enabled` 关闭恒 false
 *（NFR-11：不发起任何更新外联）；无记账（lastUpdateCheckAt 缺省 0）视为
 * 从未检查过 → 检查；距上次检查不足 intervalHours 小时不重复检查。
 * 时钟全部由入参注入（now/lastCheckAt 均为 Unix 毫秒），测试无真实时钟依赖。
 */
export function shouldSilentCheck(
  now: number,
  lastCheckAt: number,
  intervalHours: number,
  enabled: boolean,
): boolean {
  if (!enabled) return false;
  if (!(intervalHours > 0)) return false;
  if (lastCheckAt <= 0) return true;
  return now - lastCheckAt >= intervalHours * 3_600_000;
}

/** 安装/重启前置（client.updatePrecheck 的同义转发；单独导出便于测试注入）。 */
export function precheckInstall(): Promise<UpdatePrecheckResult> {
  return client.updatePrecheck();
}
