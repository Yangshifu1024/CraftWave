/**
 * 设置页 MCP 管理组：双作用域服务器管理。
 *
 * 三个区块：
 *  - 服务器：配置合并视图（global + project 同名覆盖）逐行展示——名称 /
 *    作用域徽标 / 启停按钮（mcp_connect / mcp_disconnect）/ 状态徽标
 *    （挂载时 mcp_snapshot 拉全量，订阅 mcp:status 实时迁移）/ 工具数 /
 *    信任状态列（已记住 / 待决策 / 已撤销）；
 *  - 配置编辑：全局 / 项目两作用域的 mcp.json 原文编辑（JSON 文本域——
 *    编辑的就是原始文件内容，未知字段天然不丢；保存经 mcp_save_config
 *    原子落盘，坏条目在后端校验报错）；project 作用域无项目上下文时
 *    禁用（projectAvailable=false）；
 *  - 测试连接：按草稿条目调 mcp_test（initialize + tools/list，总预算
 *    60 秒；不进池、不写信任）。
 *
 * 草稿态说明：mcp.json 不属 AppConfig（无 mcp 段），保存走
 * 独立的 mcp_save_config 命令、生命周期独立于设置页 saveAll——草稿态放
 * 组件内 useState，不入 stores/settings.ts（那里管 AppConfig 草稿与基线
 * 差集，混入会破坏「脏判定切片」模型）。
 *
 * 事件消费：mcp:status 事件在本组件订阅——设置页打开期间
 * 连接状态迁移（starting/ready/stopped/error）实时刷新对应服务器行的
 * 状态徽标，无需轮询。
 */

import { CircleAlert, LoaderCircle } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { Alert } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { cn } from "@/components/ui/cn";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { useT } from "@/i18n/core";
import {
  EVENT_KEYS,
  listenEvent,
  mcpConnect,
  mcpDisconnect,
  mcpListConfig,
  mcpSaveConfig,
  mcpSnapshot,
  mcpTest,
} from "@/ipc/client";
import type {
  McpConfigFile,
  McpListConfigResult,
  McpScope,
  McpServerConfig,
  McpSnapshotResult,
  McpStatusEvent,
  McpTestResult,
} from "@/ipc/types";

/** 作用域的中文徽标文案。 */
const SCOPE_LABEL_KEYS: Record<McpScope, string> = {
  global: "settings.scopeGlobal",
  project: "settings.scopeProject",
};

/** 连接状态徽标（四态 + idle=池无条目）的文案与配色。 */
const STATE_LABEL: Record<string, string> = {
  idle: "settings.connIdle",
  starting: "settings.connStarting",
  ready: "settings.connReady",
  stopped: "settings.connStopped",
  error: "settings.connError",
};

/** 信任状态列（mcp_snapshot 补充面）的文案。 */
const TRUST_LABEL: Record<string, string> = {
  trusted: "settings.trustTrusted",
  pending: "settings.trustPending",
  revoked: "settings.trustRevoked",
};

/** 快照行的页面形状（mcp_snapshot 行 + 事件驱动的实时状态覆盖）。 */
interface ServerRow {
  name: string;
  scope: McpScope;
  state: string;
  toolCount: number;
  detail?: string;
  fingerprint: string;
  trustStatus: string;
}

/** 从 mcp_list_config 的双作用域配置合并出服务器行骨架（配置面：名称 +
 *  作用域 + enabled；状态面由快照与事件补齐）。同名项目条目覆盖全局。 */
function mergeRows(config: McpListConfigResult): { name: string; scope: McpScope }[] {
  const rows: { name: string; scope: McpScope }[] = [];
  const seen = new Set<string>();
  for (const [name] of Object.entries(config.global.mcpServers ?? {})) {
    if (!seen.has(name)) {
      seen.add(name);
      rows.push({ name, scope: "global" });
    }
  }
  for (const [name] of Object.entries(config.project.mcpServers ?? {})) {
    // 同名覆盖：项目条目压掉全局行
    if (seen.has(name)) {
      const idx = rows.findIndex((r) => r.name === name);
      if (idx >= 0) rows[idx] = { name, scope: "project" };
    } else {
      seen.add(name);
      rows.push({ name, scope: "project" });
    }
  }
  return rows;
}

/** 序列化一份作用域配置为编辑用 JSON 文本（键序 = 文件书写序，preserve_order）。 */
function configToText(config: McpConfigFile): string {
  return JSON.stringify(config, null, 2);
}

/**
 * MCP 管理页（设置页 mcp 分节的内容体）。
 */
export function McpPage() {
  const t = useT();
  // 数据面：双作用域配置 + 快照行
  const [listing, setListing] = useState<McpListConfigResult | null>(null);
  const [snapshot, setSnapshot] = useState<McpSnapshotResult | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  // 操作反馈（连接/断开/保存/测试的行内错误与进行中标记）
  const [busyName, setBusyName] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  // 配置编辑草稿：作用域 → 文本（组件内草稿）
  const [editScope, setEditScope] = useState<McpScope>("global");
  const [drafts, setDrafts] = useState<Partial<Record<McpScope, string>>>({});
  const [saveError, setSaveError] = useState<string | null>(null);
  const [saveNotice, setSaveNotice] = useState<string | null>(null);

  // 测试连接草稿（独立于保存面：直接试连不落盘）
  const [testName, setTestName] = useState("");
  const [testCommand, setTestCommand] = useState("");
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<McpTestResult | null>(null);

  /** 拉全量（配置 + 快照）；失败给行内错误（不打断页面）。 */
  const reload = () => {
    void (async () => {
      try {
        const [listing, snap] = await Promise.all([mcpListConfig(), mcpSnapshot()]);
        setListing(listing);
        setSnapshot(snap);
        setLoadError(null);
        // 草稿未动过的作用域随重载刷新（编辑中则保留草稿不覆盖）
        setDrafts((prev) => ({
          global: prev.global ?? configToText(listing.global),
          project: prev.project ?? configToText(listing.project),
        }));
      } catch (err) {
        setLoadError(err instanceof Error ? err.message : String(err));
      }
    })();
  };

  // 挂载拉一次 + 订阅 mcp:status（状态徽标实时迁移；卸载退订）。
  // biome-ignore lint/correctness/useExhaustiveDependencies: 打开时拉一次全量，不随 reload 引用重挂订阅（SettingsPage 拉配置同款先例）
  useEffect(() => {
    reload();
    let disposed = false;
    let unlisten: (() => void) | null = null;
    void listenEvent<McpStatusEvent>(EVENT_KEYS.mcpStatus, (p) => {
      if (disposed) return;
      // 事件只带状态：就地更新对应行的 state/detail；工具数等全量面等
      // ready 事件后重拉一次快照（低频，事件是低频迁移信号）
      setSnapshot((prev) => {
        const rows = prev?.servers ?? [];
        const idx = rows.findIndex((r) => r.name === p.name && r.scope === p.scope);
        const next = [...rows];
        if (idx >= 0) {
          next[idx] = { ...next[idx], state: p.state, detail: p.detail };
        } else {
          next.push({
            name: p.name,
            scope: p.scope,
            state: p.state,
            toolCount: 0,
            detail: p.detail,
            fingerprint: "",
            trustStatus: "pending",
          });
        }
        return { servers: next };
      });
      if (p.state === "ready")
        void mcpSnapshot()
          .then(setSnapshot)
          .catch(() => {});
    }).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  /** 合并视图行：配置面（list_config）× 状态面（快照 + 事件覆盖）。 */
  const rows: ServerRow[] = useMemo(() => {
    if (!listing) return [];
    const snapIndex = new Map((snapshot?.servers ?? []).map((s) => [`${s.scope}:${s.name}`, s]));
    return mergeRows(listing).map(({ name, scope }) => {
      const snap = snapIndex.get(`${scope}:${name}`);
      return {
        name,
        scope,
        state: snap?.state ?? "idle",
        toolCount: snap?.toolCount ?? 0,
        detail: snap?.detail,
        fingerprint: snap?.fingerprint ?? "",
        trustStatus: snap?.trustStatus ?? "pending",
      };
    });
  }, [listing, snapshot]);

  /** 启停按钮：stopped/idle/error → connect；starting/ready → disconnect。 */
  async function toggle(row: ServerRow) {
    setActionError(null);
    setBusyName(`${row.scope}:${row.name}`);
    try {
      if (row.state === "starting" || row.state === "ready") {
        await mcpDisconnect(row.scope, row.name);
      } else {
        await mcpConnect(row.scope, row.name);
      }
      // 状态以 mcp:status 事件回推；这里补一次快照对账（错误态等即时面）
      void mcpSnapshot()
        .then(setSnapshot)
        .catch(() => {});
    } catch (err) {
      setActionError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusyName(null);
    }
  }

  /** 保存当前作用域草稿（JSON 解析失败行内提示；后端校验坏条目报错）。 */
  async function saveDraft() {
    setSaveError(null);
    setSaveNotice(null);
    const text = drafts[editScope];
    if (text === undefined) return;
    let parsed: McpConfigFile;
    try {
      parsed = JSON.parse(text) as McpConfigFile;
    } catch (err) {
      setSaveError(
        t("settings.mcpSaveFailedParse", {
          error: err instanceof Error ? err.message : String(err),
        }),
      );
      return;
    }
    try {
      await mcpSaveConfig({ scope: editScope, config: parsed });
      setSaveNotice(t("settings.mcpSaved"));
      // 保存后未知字段可能被归一（flatten round-trip 保序不丢）——清掉
      // 本作用域草稿并重拉，编辑框回到新基线，用户可核对未知字段仍在
      setDrafts((prev) => ({ ...prev, [editScope]: undefined }));
      reload();
    } catch (err) {
      setSaveError(err instanceof Error ? err.message : String(err));
    }
  }

  /** 测试连接：草稿条目 → mcp_test（不落盘、不进池）。 */
  async function runTest() {
    setActionError(null);
    setTesting(true);
    setTestResult(null);
    // 空命令不写字段（McpServerConfig 的 flatten index signature 不接受
    // undefined 属性值；后端对双空配置报「无法确定传输方式」）
    const command = testCommand.trim();
    const draft: McpServerConfig = command === "" ? { enabled: true } : { command, enabled: true };
    try {
      const result = await mcpTest({ draft });
      setTestResult(result);
    } catch (err) {
      setActionError(err instanceof Error ? err.message : String(err));
    } finally {
      setTesting(false);
    }
  }

  const editDisabled = editScope === "project" && listing?.projectAvailable === false;

  return (
    <div>
      {loadError && (
        <Alert
          variant="error"
          title={t("settings.mcpLoadFailed", { error: loadError })}
          className="mb-3"
        />
      )}
      {actionError && (
        <Alert
          variant="error"
          title={t("settings.mcpActionFailed", { error: actionError })}
          className="mb-3"
        />
      )}
      <section className="settings-section" data-testid="mcp-servers-section">
        <h3 className="settings-section-title">{t("settings.mcpServersSection")}</h3>
        <div className="settings-section-card">
          {rows.length === 0 ? (
            <p className="py-10 text-center text-ui-base text-[var(--muted-foreground)]">
              {t("settings.mcpEmptyHint")}
            </p>
          ) : (
            <div className="py-2">
              {rows.map((row) => {
                const busy = busyName === `${row.scope}:${row.name}`;
                const connected = row.state === "ready" || row.state === "starting";
                return (
                  <div
                    key={`${row.scope}:${row.name}`}
                    data-testid="mcp-server-row"
                    data-state={row.state}
                    className="flex items-center gap-3 border-b border-[var(--border)] py-3 first:pt-0 last:border-b-0 last:pb-0"
                  >
                    <span className="min-w-0 flex-1 truncate font-mono text-ui-sm text-[var(--foreground)]">
                      {row.name}
                    </span>
                    <Badge variant="outline">{t(SCOPE_LABEL_KEYS[row.scope])}</Badge>
                    <span
                      data-testid={`mcp-trust-${row.name}`}
                      title={
                        row.fingerprint
                          ? t("settings.fingerprintTitle", { fingerprint: row.fingerprint })
                          : undefined
                      }
                      className="shrink-0 text-ui-sm text-[var(--muted-foreground)]"
                    >
                      {TRUST_LABEL[row.trustStatus]
                        ? t(TRUST_LABEL[row.trustStatus])
                        : row.trustStatus}
                    </span>
                    <span
                      data-testid={`mcp-state-${row.name}`}
                      className={cn(
                        "flex shrink-0 items-center gap-1 text-ui-sm",
                        row.state === "ready" && "text-[var(--success)]",
                        row.state === "error" && "text-[var(--destructive)]",
                        (row.state === "idle" || row.state === "stopped") &&
                          "text-[var(--muted-foreground)]",
                      )}
                    >
                      {row.state === "starting" && (
                        <LoaderCircle size={12} aria-hidden className="animate-spin" />
                      )}
                      {row.state === "error" && (
                        <CircleAlert size={12} aria-hidden className="shrink-0" />
                      )}
                      {STATE_LABEL[row.state] ? t(STATE_LABEL[row.state]) : row.state}
                      {row.state === "ready" &&
                        t("settings.toolCountSuffix", { count: row.toolCount })}
                    </span>
                    <Button
                      variant="outline"
                      size="sm"
                      data-testid={`mcp-toggle-${row.name}`}
                      disabled={busy}
                      onClick={() => void toggle(row)}
                    >
                      {busy
                        ? t("settings.processing")
                        : connected
                          ? t("settings.stop")
                          : t("settings.connect")}
                    </Button>
                    {row.state === "error" && row.detail && (
                      <span
                        className="w-full shrink-0 basis-full text-ui-sm break-all text-[var(--destructive)]"
                        title={row.detail}
                      >
                        {row.detail}
                      </span>
                    )}
                  </div>
                );
              })}
            </div>
          )}
        </div>
      </section>

      <section className="settings-section" data-testid="mcp-config-section">
        <h3 className="settings-section-title">{t("settings.mcpConfigSection")}</h3>
        <div className="settings-section-card">
          <div className="flex items-center gap-3 py-3">
            <Select
              width="narrow"
              aria-label={t("settings.configScopeAria")}
              value={editScope}
              onChange={(e) => {
                setEditScope(e.target.value as McpScope);
                setSaveError(null);
                setSaveNotice(null);
              }}
            >
              <option value="global">{t("settings.scopeGlobalOption")}</option>
              <option value="project" disabled={!listing?.projectAvailable}>
                {t("settings.scopeProjectOption")}
              </option>
            </Select>
            {editDisabled && (
              <span className="text-ui-sm text-[var(--muted-foreground)]">
                {t("settings.scopeProjectUnavailable")}
              </span>
            )}
          </div>
          <textarea
            aria-label={t("settings.mcpEditorAria")}
            data-testid="mcp-config-editor"
            disabled={editDisabled}
            spellCheck={false}
            value={drafts[editScope] ?? ""}
            onChange={(e) => setDrafts((prev) => ({ ...prev, [editScope]: e.target.value }))}
            className="min-h-56 w-full rounded-[var(--cw-radius)] border border-[var(--cw-border)] bg-transparent p-3 font-mono text-ui-sm text-[var(--foreground)] outline-none focus:border-[var(--cw-focus)] disabled:opacity-50"
          />
          {saveError && (
            <p data-testid="mcp-save-error" className="mt-2 text-ui-sm text-[var(--destructive)]">
              {saveError}
            </p>
          )}
          {saveNotice && (
            <p data-testid="mcp-save-notice" className="mt-2 text-ui-sm text-[var(--success)]">
              {saveNotice}
            </p>
          )}
          <div className="flex items-center justify-end py-3">
            <Button
              size="sm"
              data-testid="mcp-save-config"
              disabled={editDisabled || drafts[editScope] === undefined}
              onClick={() => void saveDraft()}
            >
              {t("settings.saveConfig")}
            </Button>
          </div>
        </div>
      </section>

      <section className="settings-section" data-testid="mcp-test-section">
        <h3 className="settings-section-title">{t("settings.mcpTestSection")}</h3>
        <div className="settings-section-card">
          <div className="flex flex-wrap items-center gap-3 py-3">
            <Input
              aria-label={t("settings.serverNameAria")}
              data-testid="mcp-test-name"
              className="w-40"
              placeholder={t("settings.mcpNamePlaceholder")}
              value={testName}
              onChange={(e) => setTestName(e.target.value)}
            />
            <Input
              aria-label={t("settings.launchCommandAria")}
              data-testid="mcp-test-command"
              className="min-w-64 flex-1 font-mono"
              placeholder={t("settings.launchCommandPlaceholder")}
              value={testCommand}
              onChange={(e) => setTestCommand(e.target.value)}
            />
            <Button
              variant="outline"
              size="sm"
              data-testid="mcp-test-run"
              disabled={testing || testCommand.trim() === ""}
              onClick={() => void runTest()}
            >
              {testing ? t("settings.testing") : t("settings.testConnectionShort")}
            </Button>
          </div>
          {testResult && (
            <div data-testid="mcp-test-result" className="border-t border-[var(--border)] py-3">
              {testResult.ok ? (
                <p className="text-ui-sm text-[var(--success)]">
                  {t("settings.mcpTestOk", { count: testResult.tools.length })}
                  {testName.trim() && t("settings.mcpTestOkSuffix", { name: testName.trim() })}
                </p>
              ) : (
                <p className="text-ui-sm text-[var(--destructive)]">
                  {t("settings.mcpTestFailed", {
                    error: testResult.error ?? t("settings.unknownError"),
                  })}
                </p>
              )}
              {testResult.ok && testResult.tools.length > 0 && (
                <ul className="mt-2 space-y-1">
                  {testResult.tools.map((tool) => (
                    <li key={tool.name} className="text-ui-sm text-[var(--muted-foreground)]">
                      <code className="text-[var(--foreground)]">{tool.name}</code>
                      {tool.description && ` — ${tool.description}`}
                    </li>
                  ))}
                </ul>
              )}
            </div>
          )}
        </div>
      </section>
    </div>
  );
}
