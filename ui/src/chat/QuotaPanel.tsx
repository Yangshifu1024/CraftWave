/**
 * Inspector 配额页（21 号票，FR-02-15/§10.7 M2 五页之一）。
 *
 * 布局：首屏 = **当前会话所用 provider** 的大卡（解析自会话覆盖模型 >
 * 全局 activeModelId 的归属供应商，quota-store.providerOfModel）；其余
 * 供应商为折叠列表（unsupported >3 家折叠为一行汇总）。
 *
 * 行态渲染（CodeWave QuotaSection 口径，antd → 现有 Tailwind 组件）：
 * - ok = 名称 + 剩余% + 进度条，点整行展开窗口明细；
 * - error / rejected = 警示/灰行，展开原因 + 重试按钮 + lastOkAt 时间行
 *   （entries 非空时一并显示最近一次成功明细并标注时点——离线兜底）；
 * - no_key = 标签行 + 「去设置」（跳设置页供应商页）；
 * - unsupported = 灰行「暂不支持查询」占位（不报错）。
 *
 * 刷新策略（FR-02-15 逐字）：配额页可见时每 5min 周期 + 手动刷新按钮 +
 * 切换 provider 即时；不可见即停（本组件由 Inspector 按条件渲染，卸载即
 * 清理定时器）。查询失败静默降级——后端六态行承载一切错误面，本组件不
 * 弹任何提示，绝不影响聊天。
 */

import { LoaderCircle, RefreshCw } from "lucide-react";
import { useEffect, useState } from "react";
import {
  providerOfModel,
  QUOTA_REFRESH_INTERVAL_MS,
  useQuotaStore,
  worstRemainingPercent,
} from "@/chat/quota-store";
import { useSessionModelStore } from "@/chat/session-model";
import { t, useT } from "@/i18n/core";
import { getConfig } from "@/ipc/client";
import type { AppConfig, QuotaEntry, QuotaSnapshot, QuotaStatus } from "@/ipc/types";

/** unsupported 行折叠为一行汇总的阈值（票面：>3 家折叠）。 */
const UNSUPPORTED_COLLAPSE_LIMIT = 3;

/** 剩余百分比低于该值视为「余额不足前有感知」的警示档（进度条转红）。 */
const LOW_REMAINING_PERCENT = 20;

/** 六态行的人读标签（i18n 键面，渲染时查词——31 号票）。 */
const STATE_LABEL_KEYS: Record<QuotaStatus, string> = {
  ok: "inspector.quotaStateOk",
  error: "inspector.quotaStateError",
  invalid: "inspector.quotaStateInvalid",
  rejected: "inspector.quotaStateRejected",
  no_key: "inspector.quotaStateNoKey",
  unsupported: "inspector.quotaStateUnsupported",
};

/** 配额页本体（Inspector 第三 tab；visible 门控 = 条件渲染）。 */
export function QuotaPanel({
  sessionId,
  onOpenSettings,
}: {
  sessionId: string;
  onOpenSettings: () => void;
}) {
  const t = useT();
  const rows = useQuotaStore((s) => s.rows);
  const loading = useQuotaStore((s) => s.loading);
  const load = useQuotaStore((s) => s.load);
  // 会话覆盖模型（undefined = 跟随全局）——变化即触发「切 provider 即时刷新」
  const sessionModel = useSessionModelStore((s) => s.bySession[sessionId]);
  // 配置缓存（providers + activeModelId；挂载时拉一次）
  const [config, setConfig] = useState<AppConfig | null>(null);
  // unsupported 汇总的展开态（>3 家折叠为一行汇总，点击展开全部灰行）
  const [unsupportedExpanded, setUnsupportedExpanded] = useState(false);

  // 打开配额页（挂载）拉配置 + 快照；会话覆盖模型变化（切 provider）→
  // 即时刷新（FR-02-15 刷新策略：「切 provider 即时」与「打开即拉」同入口）。
  // sessionModel 刻意作为触发依赖（其变化即「切换 provider」，驱动即时重拉），
  // effect 内不直接引用其值——归属解析在渲染期由 providerOfModel 完成
  // biome-ignore lint/correctness/useExhaustiveDependencies: sessionModel 为触发依赖（切 provider 即时刷新），effect 内不读值
  useEffect(() => {
    let cancelled = false;
    void getConfig()
      .then((loaded) => {
        if (!cancelled) setConfig(loaded);
      })
      .catch(() => {
        // 配置读不到（异常态）：快照行仍可展示（不依赖配置）
      });
    void load();
    return () => {
      cancelled = true;
    };
  }, [sessionModel, load]);

  // 周期刷新（FR-02-15）：配额页可见时每 5 分钟；不可见即停（本组件被
  // Inspector 条件渲染，卸载走本清理）
  useEffect(() => {
    const timer = setInterval(() => {
      void load();
    }, QUOTA_REFRESH_INTERVAL_MS);
    return () => clearInterval(timer);
  }, [load]);

  const currentProviderId = config === null ? undefined : providerOfModel(config, sessionModel);
  const current = rows?.find((row) => row.providerId === currentProviderId) ?? null;
  const others = rows?.filter((row) => row.providerId !== currentProviderId) ?? [];
  // unsupported 汇总折叠（票面：>3 家折叠为一行汇总，点击展开）
  const unsupported = others.filter((row) => row.state === "unsupported");
  const collapseUnsupported =
    unsupported.length > UNSUPPORTED_COLLAPSE_LIMIT && !unsupportedExpanded;
  const listedOthers = collapseUnsupported
    ? others.filter((row) => row.state !== "unsupported")
    : others;

  return (
    <div
      className="flex min-h-0 flex-1 flex-col overflow-y-auto p-3 text-ui-sm text-[var(--muted-foreground)]"
      data-testid="quota-panel"
    >
      {/* 标题行 + 手动刷新（FR-02-15 刷新策略三入口之一） */}
      <div className="mb-2 flex items-center">
        <h3 className="text-ui-sm text-[var(--cw-dim)]">{t("inspector.quotaTitle")}</h3>
        <button
          type="button"
          data-testid="quota-refresh"
          aria-label={t("inspector.quotaRefreshAria")}
          title={t("inspector.refresh")}
          disabled={loading}
          onClick={() => void load()}
          className="ml-auto rounded p-1 text-[var(--muted-foreground)] hover:bg-[var(--accent)] hover:text-[var(--foreground)] disabled:opacity-50"
        >
          {loading ? <LoaderCircle size={14} className="animate-spin" /> : <RefreshCw size={14} />}
        </button>
      </div>
      {rows === null ? (
        <p className="text-ui-sm" data-testid="quota-loading">
          {t("inspector.quotaRefreshing")}
        </p>
      ) : (
        <>
          {/* 首屏：当前会话所用 provider 大卡 */}
          {current !== null ? (
            <QuotaCard row={current} highlight onOpenSettings={onOpenSettings} detailOpenDefault />
          ) : (
            <p className="mb-2 text-ui-sm text-[var(--cw-dim)]" data-testid="quota-no-active">
              {t("inspector.quotaNoProvider")}
            </p>
          )}
          {/* 其余供应商折叠列表 */}
          {collapseUnsupported && (
            <button
              type="button"
              data-testid="quota-unsupported-summary"
              className="mb-1 w-full rounded-lg border border-transparent px-2 py-1.5 text-left text-ui-sm text-[var(--cw-dim)] hover:border-[var(--border)]"
              onClick={() => setUnsupportedExpanded(true)}
            >
              {t("inspector.quotaUnsupportedCount", { count: unsupported.length })}
            </button>
          )}
          <ul className="space-y-1" data-testid="quota-list">
            {listedOthers.map((row) => (
              <li key={row.providerId}>
                <QuotaRow row={row} onOpenSettings={onOpenSettings} />
              </li>
            ))}
          </ul>
        </>
      )}
    </div>
  );
}

/** 首屏大卡（当前会话所用 provider）：行渲染复用 QuotaRow，缺省展开明细。 */
function QuotaCard({
  row,
  highlight,
  onOpenSettings,
  detailOpenDefault,
}: {
  row: QuotaSnapshot;
  highlight: boolean;
  onOpenSettings: () => void;
  detailOpenDefault: boolean;
}) {
  return (
    <div className={highlight ? "mb-3" : ""} data-testid="quota-current-card">
      <QuotaRow row={row} onOpenSettings={onOpenSettings} detailOpenDefault={detailOpenDefault} />
    </div>
  );
}

/** 六态行渲染（CodeWave QuotaSection 口径移植）：主体行 + 可展开明细区。 */
function QuotaRow({
  row,
  onOpenSettings,
  detailOpenDefault = false,
}: {
  row: QuotaSnapshot;
  onOpenSettings: () => void;
  detailOpenDefault?: boolean;
}) {
  const t = useT();
  const [detailOpen, setDetailOpen] = useState(detailOpenDefault);
  const load = useQuotaStore((s) => s.load);
  const data = row.data;
  const name = data?.displayName ?? row.providerId;
  const expandable = row.state === "ok" || row.state === "error" || row.state === "rejected";
  const toggle = expandable ? () => setDetailOpen((open) => !open) : undefined;
  const remaining = data ? worstRemainingPercent(data.entries) : null;

  return (
    <div>
      {/* 主体行：ok/error/rejected 整行可点展开；no_key/unsupported 静态灰行。
          role/aria 按行态条件生效（可展开行 = button 语义），Biome 静态分析
          不识别条件表达式——a11y 实际面由 expandable 三态保证，同 SessionWorkspace
          行组件的 ignore 先例。 */}
      {/* biome-ignore lint/a11y/useAriaPropsSupportedByRole: aria-expanded 随行态条件挂载（仅可展开行），见上 */}
      {/* biome-ignore lint/a11y/noStaticElementInteractions: 可展开行带 button 语义（role/tabIndex/键盘），见上 */}
      <div
        role={toggle !== undefined ? "button" : undefined}
        tabIndex={toggle !== undefined ? 0 : undefined}
        data-testid={`quota-row-${row.providerId}`}
        data-quota-state={row.state}
        aria-expanded={toggle !== undefined ? detailOpen : undefined}
        onClick={toggle}
        onKeyDown={
          toggle !== undefined
            ? (event) => {
                if (event.key === "Enter" || event.key === " ") {
                  event.preventDefault();
                  toggle();
                }
              }
            : undefined
        }
        className={`w-full rounded-lg border px-2 py-1.5 text-left ${
          toggle !== undefined
            ? "cursor-default border-transparent hover:border-[var(--border)]"
            : "border-transparent"
        } ${row.state === "ok" ? "" : "opacity-80"}`}
      >
        <div className="flex items-center gap-2">
          <span className="min-w-0 truncate text-ui-sm text-[var(--foreground)]">{name}</span>
          <span className="ml-auto shrink-0 tabular-nums">{rowMainValue(row, remaining)}</span>
        </div>
        {/* 进度条：ok 行有百分比窗口时展示；低于警示档转红（余额不足前有感知） */}
        {row.state === "ok" && remaining !== null && (
          <div className="mt-1 h-1.5 w-full overflow-hidden rounded bg-[var(--muted)]">
            <div
              className={`h-full ${remaining < LOW_REMAINING_PERCENT ? "bg-[var(--destructive)]" : "bg-[var(--primary)]"}`}
              style={{ width: `${Math.min(100, Math.max(0, remaining))}%` }}
            />
          </div>
        )}
        {/* 六态副文案：错误/占位行的人读标签（不报错，灰行示警） */}
        {row.state !== "ok" && (
          <p
            className={`mt-0.5 text-ui-sm ${
              row.state === "error" || row.state === "rejected"
                ? "text-amber-600 dark:text-amber-400"
                : "text-[var(--cw-dim)]"
            }`}
          >
            {t(STATE_LABEL_KEYS[row.state])}
          </p>
        )}
        {/* no_key 行尾「去设置」（跳设置页供应商页） */}
        {row.state === "no_key" && (
          <button
            type="button"
            data-testid={`quota-goto-settings-${row.providerId}`}
            className="mt-1 rounded px-1.5 py-0.5 text-ui-sm text-[var(--foreground)] hover:bg-[var(--accent)]"
            onClick={(event) => {
              // 不冒泡给行（no_key 行不可展开，防双触发）
              event.stopPropagation();
              onOpenSettings();
            }}
          >
            {t("inspector.quotaGoSettings")}
          </button>
        )}
      </div>
      {/* 展开区：窗口明细 / 错误原因 + 重试 + 时点标注 */}
      {detailOpen && data !== null && <QuotaDetail row={row} onRetry={() => void load()} />}
    </div>
  );
}

/** 行主数值：ok 行 = 最紧张窗口剩余%（无数值窗时回落明细首行的文本）；
 *  error 行带 lastOkAt 时标注「上次成功」；其余态空（副文案已表达）。 */
function rowMainValue(row: QuotaSnapshot, remaining: number | null): string {
  if (row.state === "ok" && remaining !== null) return `${Math.round(remaining)}%`;
  if (row.state === "ok" && row.data?.entries.length === 1) {
    const text = row.data.entries[0].valueText;
    if (text !== undefined) return text;
  }
  if ((row.state === "error" || row.state === "rejected") && row.lastOkAt) {
    return t("inspector.quotaLastOkAt", { time: shortTime(row.lastOkAt) });
  }
  return "";
}

/** 展开明细区：ok 行 = 窗口明细（百分比/余额行各自形态）；error/rejected
 *  行 = 原因 + 重试按钮 + lastOkAt 时间行（entries 非空时一并显示——
 *  「数据获取时间」以 lastOkAt 标注，FR-02-15 离线兜底面）。 */
function QuotaDetail({ row, onRetry }: { row: QuotaSnapshot; onRetry: () => void }) {
  const t = useT();
  const data = row.data;
  if (data === undefined) return null;
  return (
    <div
      className="mt-1 rounded-lg border border-[var(--border)] bg-[var(--muted)] p-2"
      data-testid={`quota-detail-${row.providerId}`}
    >
      {/* 错误原因（已脱敏的后端文案）+ 重试 */}
      {data.error !== undefined && (
        <div className="mb-2">
          <p className="break-all text-ui-sm text-[var(--muted-foreground)]">{data.error}</p>
          <button
            type="button"
            data-testid={`quota-retry-${row.providerId}`}
            className="mt-1 rounded px-1.5 py-0.5 text-ui-sm text-[var(--foreground)] hover:bg-[var(--accent)]"
            onClick={onRetry}
          >
            {t("inspector.quotaRetry")}
          </button>
        </div>
      )}
      {/* unsupported 的原因码（no_adapter / empty_base_url） */}
      {data.reason !== undefined && (
        <p className="text-ui-sm text-[var(--cw-dim)]">
          {t("inspector.quotaReason", { reason: data.reason })}
        </p>
      )}
      {/* 窗口明细行 */}
      {data.entries.length > 0 && (
        <ul className="space-y-0.5">
          {data.entries.map((entry) => (
            <QuotaEntryLine key={entry.key} entry={entry} />
          ))}
        </ul>
      )}
      {/* 时点标注：明细为最近一次成功数据时（error 行带 entries），以
          lastOkAt 说明「数据获取时间」；fetchedAt 为本次查询时刻 */}
      {(row.state === "error" || row.state === "rejected") && data.entries.length > 0 && (
        <p className="mt-1 text-ui-sm text-[var(--cw-dim)]">
          {t("inspector.quotaDataAt", { time: shortTime(row.lastOkAt ?? row.fetchedAt) })}
        </p>
      )}
    </div>
  );
}

/** 单窗口明细行：百分比行 = 标签 + 已用/剩余 + 重置时间；数值行 = 原文。 */
function QuotaEntryLine({ entry }: { entry: QuotaEntry }) {
  const t = useT();
  const label = entry.label ?? entry.key;
  const value =
    entry.valueText !== undefined
      ? entry.valueText
      : [
          entry.usedPercent !== undefined
            ? t("workspace.quotaUsed", { percent: Math.round(entry.usedPercent) })
            : null,
          entry.remainingPercent !== undefined
            ? t("workspace.quotaRemaining", { percent: Math.round(entry.remainingPercent) })
            : null,
          entry.status !== undefined ? entry.status : null,
        ]
          .filter(Boolean)
          .join(" · ");
  return (
    <li className="flex items-center gap-2 text-ui-sm tabular-nums">
      <span className="min-w-0 truncate text-[var(--muted-foreground)]">{label}</span>
      <span className="ml-auto shrink-0 text-[var(--foreground)]">{value}</span>
      {entry.resetsAt !== undefined && (
        <span className="shrink-0 text-[var(--cw-dim)]">{shortTime(entry.resetsAt)}</span>
      )}
    </li>
  );
}

/** RFC3339 → 人读短时间（今天显示 HH:mm，更早显示 MM-DD HH:mm）。 */
export function shortTime(rfc3339: string): string {
  const date = new Date(rfc3339);
  if (Number.isNaN(date.getTime())) return rfc3339;
  const pad = (n: number) => String(n).padStart(2, "0");
  const hm = `${pad(date.getHours())}:${pad(date.getMinutes())}`;
  const now = new Date();
  if (date.toDateString() === now.toDateString()) return hm;
  return `${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${hm}`;
}
