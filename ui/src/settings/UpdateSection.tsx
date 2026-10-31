/**
 * 设置页「关于」页的检查更新区（33 号票，FR-16-03「检查可开关」+ 2026-10-29
 * 用户决策的运行与队列门）。
 *
 * 交互链：按钮 → checkForUpdate() 三态——
 * - 有新版 → Dialog（新版本号 + release notes + 「下载并安装」）；
 *   安装前先 updatePrecheck 后端权威门（活跃 run / 待审批 / 未暂停队列阻止，
 *   阻止原因逐行展示；安装前重新检查——下载完成后新出现的 run 仍能阻止）；
 *   放行 → downloadAndInstall 进度态（下载中/安装中）→ 「重启以完成更新」
 *   → relaunch；
 * - 已最新 / 失败 → Toast（29 号票反馈层）。
 *
 * 禁用兜底（票内 Answer）：config.update.enabled=false（用户关）或
 * update_status.pubkeyConfigured=false（HITL 密钥未配，tauri.conf.json 的
 * pubkey 占位空串期间 updater 校验必败）任一命中 → 入口禁用并显示原因。
 *
 * IPC 依赖全部可注入（测试换假实现）；真实 GUI 走查留用户验收面。
 */

import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { Dialog } from "@/components/ui/dialog";
import { useT } from "@/i18n/core";
import * as client from "@/ipc/client";
import {
  checkForUpdate,
  discardPendingUpdate,
  downloadAndInstallUpdate,
  type InstallProgress,
  relaunchForUpdate,
  type UpdateCheckState,
} from "@/ipc/updater";
import { pushToast } from "@/stores/toast";

/** 组件的 IPC 依赖面（全部可注入——测试换假实现）。 */
export interface UpdateSectionDeps {
  getConfig: typeof client.getConfig;
  updateStatus: typeof client.updateStatus;
  updatePrecheck: typeof client.updatePrecheck;
  checkForUpdate: typeof checkForUpdate;
  downloadAndInstallUpdate: typeof downloadAndInstallUpdate;
  relaunchForUpdate: typeof relaunchForUpdate;
  pushToast: typeof pushToast;
}

/** 生产依赖：ipc/client 与插件 API 真实出口。 */
export const defaultUpdateSectionDeps: UpdateSectionDeps = {
  getConfig: client.getConfig,
  updateStatus: client.updateStatus,
  updatePrecheck: client.updatePrecheck,
  checkForUpdate,
  downloadAndInstallUpdate,
  relaunchForUpdate,
  pushToast,
};

/** 区块内状态机：入口门控 + 检查三态 + 安装进度。 */
type Phase =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "available"; version: string; notes?: string }
  | { kind: "failed"; message: string }
  | { kind: "installing" }
  | { kind: "readyToRelaunch" };

/** 检查更新区（设置页关于页挂载）。 */
export function UpdateSection({
  deps = defaultUpdateSectionDeps,
}: {
  deps?: UpdateSectionDeps;
} = {}) {
  const t = useT();
  const [gate, setGate] = useState<string | null>(null);
  const [phase, setPhase] = useState<Phase>({ kind: "idle" });
  const [dialogOpen, setDialogOpen] = useState(false);
  const [blockReasons, setBlockReasons] = useState<string[]>([]);
  const [installProgress, setInstallProgress] = useState<InstallProgress>({ kind: "idle" });
  const [currentVersion, setCurrentVersion] = useState("");

  // 当前运行版本（对话框展示「当前 → 新版」对照；取不到省略该段）
  useEffect(() => {
    let cancelled = false;
    void readCurrentVersion().then((version) => {
      if (!cancelled && version) setCurrentVersion(version);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  // 挂载时拉一次门控面（enabled 与 pubkey 双门；配置改动后重开设置页生效）
  useEffect(() => {
    let cancelled = false;
    void deps
      .getConfig()
      .then((config) => {
        if (cancelled) return;
        if (!config.update?.enabled) {
          setGate(t("settings.updateDisabledByConfig"));
          return;
        }
        return deps.updateStatus().then((status) => {
          if (cancelled) return;
          if (!status.pubkeyConfigured) {
            setGate(t("settings.updateDisabledByPubkey"));
          }
        });
      })
      .catch(() => {
        // 读配置/状态失败（纯浏览器环境等）：保守禁用并说明
        if (!cancelled) setGate(t("settings.updateStatusUnavailable"));
      });
    return () => {
      cancelled = true;
    };
  }, [deps, t]);

  // 卸载时释放暂存的 Rust 侧更新资源（不安装就走 = 不悬挂句柄）
  useEffect(() => () => discardPendingUpdate(), []);

  const startCheck = () => {
    setPhase({ kind: "checking" });
    void deps
      .checkForUpdate()
      .then((state: UpdateCheckState) => {
        if (state.kind === "upToDate") {
          setPhase({ kind: "idle" });
          deps.pushToast("info", t("settings.updateUpToDate"));
        } else if (state.kind === "failed") {
          setPhase({ kind: "idle" });
          deps.pushToast("error", t("settings.updateCheckFailed"), state.message);
        } else {
          setPhase({ kind: "available", version: state.version, notes: state.notes });
          setBlockReasons([]);
          setInstallProgress({ kind: "idle" });
          setDialogOpen(true);
        }
      })
      .catch((err: unknown) => {
        setPhase({ kind: "idle" });
        deps.pushToast("error", t("settings.updateCheckFailed"), String(err));
      });
  };

  const startInstall = () => {
    // 安装门前置（后端权威面）：活跃 run / 待审批 / 未暂停队列阻止
    void deps
      .updatePrecheck()
      .then((result) => {
        if (result.blocked) {
          setBlockReasons(result.reasons);
          return;
        }
        setBlockReasons([]);
        setPhase({ kind: "installing" });
        setInstallProgress({ kind: "downloading", received: 0 });
        void deps
          .downloadAndInstallUpdate((progress) => setInstallProgress(progress))
          .then(() => {
            setPhase({ kind: "readyToRelaunch" });
          })
          .catch((err: unknown) => {
            setPhase({
              kind: "available",
              version: phase.kind === "available" ? phase.version : "",
            });
            setInstallProgress({ kind: "idle" });
            deps.pushToast("error", t("settings.updateInstallFailed"), String(err));
          });
      })
      .catch((err: unknown) => {
        deps.pushToast("error", t("settings.updateCheckFailed"), String(err));
      });
  };

  const closeDialog = () => {
    setDialogOpen(false);
    if (phase.kind !== "installing") {
      // 安装中途不允许关闭（没有取消通道——下载继续，界面只读）；其余态关闭即复位
      setPhase({ kind: "idle" });
    }
  };

  const busy = phase.kind === "checking";
  const installing = phase.kind === "installing";

  return (
    <div className="settings-section-card" data-testid="update-section">
      <div className="flex items-center justify-between gap-3 px-5 py-4">
        <div className="flex min-w-0 flex-col gap-0.5">
          <span className="text-ui-base font-medium">{t("settings.updateCheckTitle")}</span>
          <span className="text-ui-sm text-[var(--muted-foreground)]">
            {gate ?? t("settings.updateCheckDesc")}
          </span>
        </div>
        <Button
          variant="outline"
          size="sm"
          data-testid="update-check-button"
          disabled={gate !== null || busy || installing}
          onClick={startCheck}
        >
          {busy ? t("settings.updateChecking") : t("settings.updateCheckNow")}
        </Button>
      </div>

      <Dialog
        open={dialogOpen}
        title={t("settings.updateDialogTitle")}
        onClose={closeDialog}
        actions={[
          {
            label: t("settings.updateCancel"),
            variant: "ghost",
            disabled: installing,
            onClick: closeDialog,
          },
          phase.kind === "readyToRelaunch"
            ? {
                label: t("settings.updateRelaunch"),
                variant: "default",
                onClick: () => void deps.relaunchForUpdate(),
              }
            : {
                label:
                  installProgress.kind === "downloading"
                    ? t("settings.updateDownloading")
                    : installProgress.kind === "installing"
                      ? t("settings.updateInstalling")
                      : t("settings.updateInstallNow"),
                variant: "default",
                disabled: installing,
                onClick: startInstall,
                testId: "update-install-button",
              },
        ]}
      >
        <div className="flex flex-col gap-3" data-testid="update-dialog-body">
          {phase.kind === "available" && (
            <>
              <p className="text-ui-base" data-testid="update-available-line">
                {t("settings.updateAvailableLine", { version: phase.version })}
              </p>
              {currentVersion && (
                <p className="text-ui-sm text-[var(--muted-foreground)]">
                  {t("settings.updateCurrentVersion", { version: currentVersion })}
                </p>
              )}
              {phase.notes ? (
                <pre className="max-h-40 overflow-auto rounded-lg border border-[var(--border)] bg-[var(--muted)]/50 p-3 font-mono text-ui-sm whitespace-pre-wrap">
                  {phase.notes}
                </pre>
              ) : null}
            </>
          )}
          {installing && (
            <p className="text-ui-base" data-testid="update-progress">
              {installProgress.kind === "downloading"
                ? t("settings.updateDownloading")
                : t("settings.updateInstalling")}
            </p>
          )}
          {phase.kind === "readyToRelaunch" && (
            <p className="text-ui-base" data-testid="update-ready">
              {t("settings.updateReadyToRelaunch")}
            </p>
          )}
          {blockReasons.length > 0 && (
            <div
              className="flex flex-col gap-1 rounded-lg border border-[var(--warning)]/50 bg-[var(--muted)]/60 p-3"
              data-testid="update-block-reasons"
            >
              <span className="text-ui-sm font-medium text-[var(--warning)]">
                {t("settings.updateBlockedTitle")}
              </span>
              {blockReasons.map((reason) => (
                <span key={reason} className="text-ui-sm text-[var(--foreground)]">
                  {reason}
                </span>
              ))}
            </div>
          )}
        </div>
      </Dialog>
    </div>
  );
}

/** 当前运行版本（@tauri-apps/api/app 的 getVersion；纯浏览器环境回退空串，
 *  界面据此省略「当前版本」段）。 */
async function readCurrentVersion(): Promise<string> {
  try {
    const { getVersion } = await import("@tauri-apps/api/app");
    return await getVersion();
  } catch {
    return "";
  }
}
