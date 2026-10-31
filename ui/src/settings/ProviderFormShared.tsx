/**
 * 供应商表单共享层（30 号票首启引导抽取）：字段校验、凭据（newKey）策略、
 * 连接测试状态机与全部表单控件的单源实现——设置页 ProvidersPage 与首启
 * 引导向导的供应商步共用同一份，避免两处表单漂移（票面红线：抽取后
 * ProvidersPage 既有用例必须全绿）。
 *
 * 数据面口径与原 ProvidersPage 一致：
 *  - 表单形态（ProviderForm）：baseUrl 必填字符串、headers 摊开成行、
 *    apiKey 为可选展示值（编辑态"保留现有 Key"时不回显）；
 *  - 凭据策略（CredentialState）：新增恒录入新 Key；编辑 keep / replace 两选；
 *  - 连接测试（ConnectionTestState）：idle/testing/ok/error 四态，
 *    error 为 §3.6 E_PROVIDER_* 七类分类（probeErrorLabel 人话映射）。
 */

import { Button } from "@/components/ui/button";
import { cn } from "@/components/ui/cn";
import { Input } from "@/components/ui/input";
import { NumberInput } from "@/components/ui/number-input";
import { Select } from "@/components/ui/select";
import { CheckableTag, Tag } from "@/components/ui/tag";
import { t } from "@/i18n/core";
import { testProvider } from "@/ipc/client";
import type { ApiFormat, ProbeError, ProviderConfig, ProviderModel } from "@/ipc/types";
import { EyeIcon, LockIcon, PencilIcon, PlusIcon, TrashIcon } from "@/settings/icons";
import { newDraftId } from "@/settings/presets";

/** API 格式下拉的选项表（label 展示 / value 存入配置）。 */
export const API_FORMAT_OPTIONS: { label: string; value: ApiFormat }[] = [
  { label: "OpenAI Chat", value: "openai_chat" },
  { label: "Anthropic Messages", value: "anthropic_messages" },
  { label: "OpenAI Responses", value: "openai_responses" },
];

// 已知官方默认端点：切协议时仅在这些值之间同步；自定义 URL 不受影响（与 CodeWave 同规则）
export const DEFAULT_BASE_URLS: Record<ApiFormat, string> = {
  openai_chat: "https://api.openai.com/v1",
  anthropic_messages: "https://api.anthropic.com",
  openai_responses: "https://api.openai.com/v1",
};

/** API 格式枚举值 → 下拉展示名（卡片标签用）。 */
export function apiFormatLabel(v: ApiFormat): string {
  return API_FORMAT_OPTIONS.find((o) => o.value === v)?.label ?? v;
}

/** 请求头行（表单内表示；提交时转回 Record） */
export type HeaderRow = { name: string; value: string };

/** 连接测试状态机（FR-02-09，19 号票）：idle 未测 / testing 进行中 /
 *  ok 已连通（延迟 + 模型数）/ error 失败（七类分类文案）。 */
export type ConnectionTestState =
  | { state: "idle" }
  | { state: "testing" }
  | { state: "ok"; latencyMs: number; modelCount: number }
  | { state: "error"; error: ProbeError };

/** 探测错误分类（§3.6 E_PROVIDER_* 七类）→ 人话文案映射表。 */
export function probeErrorLabel(code: string): string {
  const labels: Record<string, string> = {
    E_PROVIDER_AUTH: "settings.probeAuth",
    E_PROVIDER_BILLING: "settings.probeBilling",
    E_PROVIDER_RATE_LIMITED: "settings.probeRateLimited",
    E_PROVIDER_BAD_REQUEST: "settings.probeBadRequest",
    E_PROVIDER_SERVER: "settings.probeServer",
    E_PROVIDER_PROTOCOL: "settings.probeProtocol",
    E_PROVIDER_NETWORK: "settings.probeNetwork",
  };
  return labels[code] ? t(labels[code]) : t("settings.probeUnknown", { code });
}

/** 表单内的供应商表示（baseUrl 必填语义、headers 行式） */
export type ProviderForm = Omit<ProviderConfig, "baseUrl" | "headers" | "apiKey"> & {
  baseUrl: string;
  apiKey?: string;
  headerRows: HeaderRow[];
};

/** 凭据策略状态：isNew = 新增（恒录入新 Key）；编辑时 keep（保留）/ replace（替换）两选。 */
export interface CredentialState {
  isNew: boolean;
  mode: "keep" | "replace";
  newKey: string;
}

/** 自定义请求头保留名（与后端校验口径一致）：不允许被覆盖 */
const RESERVED_HEADER_NAMES = new Set([
  "content-type",
  "authorization",
  "x-api-key",
  "anthropic-version",
  "host",
  "content-length",
]);
/** HTTP 头名的合法字符集（RFC 定义的 token：字母/数字/部分符号，不含空格）。 */
const HTTP_TOKEN_RE = /^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/;

/** 校验请求头行：头名合法 token / 非保留名 / 无重名；整行全空视为待填占位跳过 */
function headersValid(rows: HeaderRow[]): boolean {
  const seen = new Set<string>();
  for (const row of rows) {
    const name = row.name.trim();
    if (!name && !row.value.trim()) continue;
    if (!name || !HTTP_TOKEN_RE.test(name)) return false;
    const lower = name.toLowerCase();
    if (RESERVED_HEADER_NAMES.has(lower) || seen.has(lower)) return false;
    if (/[\r\n]/.test(row.value) || !/^[\x20-\x7E]*$/.test(row.value)) return false;
    seen.add(lower);
  }
  return true;
}

/** 可校验的五个字段键（名称 / 端点 / 凭据 / 模型 / 请求头）。 */
export type FieldKey = "name" | "baseUrl" | "keys" | "models" | "headers";

/** 字段校验（结构化问题 → 文案），返回该字段错误文本（undefined = 通过） */
export function fieldIssue(
  form: ProviderForm,
  key: FieldKey,
  credential: CredentialState,
): string | undefined {
  if (key === "name" && form.name.trim() === "") return t("settings.required");
  if (key === "baseUrl") {
    const url = form.baseUrl.trim();
    if (url === "") return t("settings.required");
    if (!/^https?:\/\/[^\s/]+/i.test(url)) return t("settings.invalidUrl");
  }
  if (key === "keys") {
    // 本地供应商（FR-02-04，19 号票）：免 API Key（本地服务不校验凭据）
    if (form.local) return undefined;
    const hasKey = credential.isNew
      ? credential.newKey.trim() !== ""
      : credential.mode === "replace"
        ? credential.newKey.trim() !== ""
        : !!form.apiKey;
    if (!hasKey) return t("settings.requiredWithKey");
  }
  if (key === "models" && form.models.length === 0) return t("settings.atLeastOneModel");
  if (key === "headers" && !headersValid(form.headerRows)) {
    return t("settings.headerInvalid");
  }
  return undefined;
}

/** 全字段校验（提交前的总闸）：五个字段全部通过才允许入草稿。 */
export function allFieldsValid(form: ProviderForm, credential: CredentialState): boolean {
  const keys: FieldKey[] = ["name", "baseUrl", "keys", "models", "headers"];
  return keys.every((key) => fieldIssue(form, key, credential) === undefined);
}

/** 配置形态 → 表单形态（baseUrl 转必填字符串、headers 对象摊开成行）。 */
export function toForm(provider: ProviderConfig): ProviderForm {
  return {
    ...provider,
    baseUrl: provider.baseUrl ?? "",
    apiKey: provider.apiKey,
    headerRows: Object.entries(provider.headers ?? {}).map(([name, value]) => ({ name, value })),
  };
}

/** 空白新增表单：默认 OpenAI Chat 格式及其官方端点，无凭据无模型。 */
export function blankProvider(): ProviderForm {
  return {
    id: newDraftId(),
    name: "",
    apiFormat: "openai_chat",
    baseUrl: DEFAULT_BASE_URLS.openai_chat,
    models: [],
    headerRows: [],
    local: false,
  };
}

/** 新模型的默认值（上下文 128k、输出 32k，纯文本；对应常见模型的保守缺省）。 */
export function providerModelDefaults(): ProviderModel {
  return {
    id: newDraftId(),
    model: "",
    maxTokens: 32768,
    contextWindow: 128000,
    vision: false,
  };
}

/** 表单 → 配置载荷：trim、空请求头行丢弃、凭据按策略落到 apiKey。
 *  凭据落点 = 新增或"替换为新 Key"时取 newKey（trim 后空 → undefined，
 *  由后端校验兜底拒绝）；"保留现有 Key"时沿用表单携带的 apiKey。 */
export function formToProvider(form: ProviderForm, credential: CredentialState): ProviderConfig {
  const headers = Object.fromEntries(
    form.headerRows
      .map((row) => ({ name: row.name.trim(), value: row.value.trim() }))
      .filter((row) => row.name !== "")
      .map((row) => [row.name, row.value]),
  );
  const apiKey =
    credential.isNew || credential.mode === "replace"
      ? credential.newKey.trim() === ""
        ? undefined
        : credential.newKey.trim()
      : form.apiKey;
  return {
    ...form,
    name: form.name.trim(),
    baseUrl: form.baseUrl.trim(),
    apiKey,
    headers: Object.keys(headers).length > 0 ? headers : undefined,
  };
}

/**
 * 连接测试（FR-02-09）：对已保存供应商发探测（草稿态编辑时以表单当前
 * baseUrl 做 baseUrlOverride 覆写——"未保存草稿直接测试"，不落库）。
 * 结果按 { state, latency, modelCount / error } 写入调用方给的槽。
 * reject（test_provider 未注册 / 供应商不存在等参数面错误）时先交给
 * onReject 接管——返回 true 表示调用方已处理（如 30 号票向导的
 * 「稍后在设置中测试」降级），不再写 error 槽；未接管则兜底为
 * protocol 分类展示（设置页现状）。
 */
export async function runConnectionTest(
  providerId: string,
  baseUrlOverride: string | undefined,
  slot: (next: ConnectionTestState) => void,
  onReject?: (err: unknown) => boolean,
): Promise<void> {
  slot({ state: "testing" });
  try {
    const result = await testProvider({
      providerId,
      baseUrlOverride: baseUrlOverride || undefined,
    });
    if (result.ok) {
      slot({
        state: "ok",
        latencyMs: result.latencyMs ?? 0,
        modelCount: result.models?.length ?? 0,
      });
    } else if (result.error) {
      slot({ state: "error", error: result.error });
    } else {
      slot({
        state: "error",
        error: {
          code: "E_PROVIDER_PROTOCOL",
          message: "probe result missing error details",
        },
      });
    }
  } catch (err) {
    if (onReject?.(err)) return;
    // 参数面错误（供应商不存在等）：兜底展示为 protocol 分类
    const message = err instanceof Error ? err.message : String(err);
    slot({ state: "error", error: { code: "E_PROVIDER_PROTOCOL", message } });
  }
}

/** 表单行内带错误/说明的控件包装（复刻 Form.Item 的 label + help + extra 结构）。
 *  props：error 错误文案（优先显示）；hint 无错误时的说明文案；wide 占满控件列。 */
export function FieldControl({
  error,
  hint,
  children,
  wide,
}: {
  error?: string;
  hint?: string;
  children: React.ReactNode;
  wide?: boolean;
}) {
  return (
    <div
      className={cn(
        "flex w-full flex-col items-end gap-1",
        wide && "settings-provider-control-wide",
      )}
    >
      {children}
      {error ? <span className="settings-field-error">{error}</span> : null}
      {!error && hint ? <span className="settings-field-hint">{hint}</span> : null}
    </div>
  );
}

/**
 * 供应商字段区（新增/编辑共用，顺序复刻：名称 / API 格式 / Base URL / API Key / 自定义请求头）。
 * props：form 表单数据；credential 凭据策略；errors 各字段错误文案（undefined = 通过）；
 * onPatch 表单局部更新；onCredentialMode / onNewKey 凭据交互回调；children 插入模型列表区。
 */
export function ProviderFields({
  form,
  credential,
  errors,
  onPatch,
  onCredentialMode,
  onNewKey,
  onTestDraft,
  draftTest,
  children,
}: {
  form: ProviderForm;
  credential: CredentialState;
  errors: Partial<Record<FieldKey, string | undefined>>;
  onPatch: (patch: Partial<ProviderForm>) => void;
  /** 凭据策略切换（编辑态 radio 用；新增态恒录新 Key 可缺省——30 号票向导） */
  onCredentialMode?: (mode: "keep" | "replace") => void;
  onNewKey: (value: string) => void;
  /** 草稿试连回调（编辑态注入；缺省 = 新增态不显示测试按钮） */
  onTestDraft?: () => void;
  /** 草稿试连结果（与 onTestDraft 配对） */
  draftTest?: ConnectionTestState;
  children: React.ReactNode;
}) {
  function patchHeader(index: number, patch: Partial<HeaderRow>) {
    onPatch({ headerRows: form.headerRows.map((h, i) => (i === index ? { ...h, ...patch } : h)) });
  }

  return (
    <>
      <div className="settings-section-card settings-provider-form">
        <div className="settings-row">
          <div className="settings-row-copy">
            <span className="settings-row-title">
              {t("settings.fieldName")} <span className="text-[var(--cw-err)]">*</span>
            </span>
          </div>
          <div className="settings-row-control">
            <FieldControl error={errors.name} wide>
              <Input
                className={cn(errors.name && "settings-field-error-input")}
                value={form.name}
                placeholder={t("settings.namePlaceholder")}
                onChange={(e) => onPatch({ name: e.target.value })}
              />
            </FieldControl>
          </div>
        </div>

        <div className="settings-row">
          <div className="settings-row-copy">
            <span className="settings-row-title">
              {t("settings.fieldApiFormat")} <span className="text-[var(--cw-err)]">*</span>
            </span>
          </div>
          <div className="settings-row-control">
            <FieldControl wide>
              <Select
                className="w-full"
                value={form.apiFormat}
                aria-label={t("settings.fieldApiFormat")}
                onChange={(e) => {
                  const next = e.target.value as ApiFormat;
                  // 切协议同步默认端点：仅当前值为空或已知官方默认时替换；自定义 URL 不受影响
                  const cur = form.baseUrl.trim();
                  const patch: Partial<ProviderForm> = { apiFormat: next };
                  if (!cur || Object.values(DEFAULT_BASE_URLS).includes(cur)) {
                    patch.baseUrl = DEFAULT_BASE_URLS[next];
                  }
                  onPatch(patch);
                }}
              >
                {API_FORMAT_OPTIONS.map((option) => (
                  <option key={option.value} value={option.value}>
                    {option.label}
                  </option>
                ))}
              </Select>
            </FieldControl>
          </div>
        </div>

        <div className="settings-row">
          <div className="settings-row-copy">
            <span className="settings-row-title">
              Base URL <span className="text-[var(--cw-err)]">*</span>
            </span>
            <span className="settings-row-description">{t("settings.draftTestNote")}</span>
          </div>
          <div className="settings-row-control">
            <FieldControl error={errors.baseUrl} wide>
              <div className="flex w-full items-start gap-2">
                <Input
                  className={cn(
                    "font-mono text-ui-sm",
                    errors.baseUrl && "settings-field-error-input",
                  )}
                  value={form.baseUrl}
                  placeholder="https://api.example.com/v1"
                  onChange={(e) => onPatch({ baseUrl: e.target.value })}
                />
                {/* 草稿试连（19 号票 FR-02-09）：仅编辑态可用——已保存供应商
                    的 baseUrl 覆写探测；新增态还没有可探测的配置主体 */}
                {onTestDraft && (
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={
                      (draftTest?.state ?? "idle") === "testing" || form.baseUrl.trim() === ""
                    }
                    onClick={onTestDraft}
                  >
                    {draftTest?.state === "testing"
                      ? t("settings.testing")
                      : t("settings.testConnectionShort")}
                  </Button>
                )}
              </div>
              {onTestDraft && (
                <div className="w-full">
                  <ConnectionTestResult test={draftTest ?? { state: "idle" }} />
                </div>
              )}
            </FieldControl>
          </div>
        </div>

        <div className="settings-row">
          <div className="settings-row-copy">
            <span className="settings-row-title">
              {form.local ? (
                <>API Key</>
              ) : (
                <>
                  API Key <span className="text-[var(--cw-err)]">*</span>
                </>
              )}
            </span>
            <span className="settings-row-description">
              {form.local
                ? t("settings.localNoKeyNote")
                : credential.isNew || credential.mode === "replace"
                  ? t("settings.keyPlainNote")
                  : t("settings.keyKeepNote")}
            </span>
          </div>
          <div className="settings-row-control">
            <FieldControl error={form.local ? undefined : errors.keys} wide>
              {form.local ? (
                // 本地预设（FR-02-04，19 号票）：免 Key——Key 输入隐藏
                <div className="flex w-full items-center gap-2 text-ui-sm text-[var(--cw-text-2)]">
                  <Tag>{t("settings.localTag")}</Tag>
                  <span>{t("settings.noApiKeyNeeded")}</span>
                </div>
              ) : credential.isNew ? (
                <Input
                  className={cn(
                    "font-mono text-ui-sm",
                    errors.keys && "settings-field-error-input",
                  )}
                  autoComplete="off"
                  value={credential.newKey}
                  placeholder="sk-…"
                  onChange={(e) => onNewKey(e.target.value)}
                />
              ) : (
                <div className="settings-credential-options">
                  <span className="text-ui-sm text-[var(--cw-text-2)]">
                    {form.apiKey ? t("settings.keyConfigured") : t("settings.keyNotConfigured")}
                  </span>
                  <label>
                    <input
                      type="radio"
                      name="credential-mode"
                      checked={credential.mode === "keep"}
                      onChange={() => onCredentialMode?.("keep")}
                    />
                    {t("settings.keepExistingKey")}
                  </label>
                  <label>
                    <input
                      type="radio"
                      name="credential-mode"
                      checked={credential.mode === "replace"}
                      onChange={() => onCredentialMode?.("replace")}
                    />
                    {t("settings.replaceWithNewKey")}
                  </label>
                  {credential.mode === "replace" && (
                    <Input
                      className={cn(
                        "font-mono text-ui-sm",
                        errors.keys && "settings-field-error-input",
                      )}
                      autoComplete="off"
                      value={credential.newKey}
                      placeholder="sk-…"
                      onChange={(e) => onNewKey(e.target.value)}
                    />
                  )}
                </div>
              )}
            </FieldControl>
          </div>
        </div>

        <div className="settings-row">
          <div className="settings-row-copy">
            <span className="settings-row-title">{t("settings.headersTitle")}</span>
            <span className="settings-row-description">{t("settings.headersNote")}</span>
          </div>
          <div className="settings-row-control">
            <FieldControl error={errors.headers} wide>
              <div className="flex w-full flex-col gap-2">
                {form.headerRows.map((header, index) => (
                  // biome-ignore lint/suspicious/noArrayIndexKey: 请求头行允许全空占位且可重名前无稳定标识，顺序即身份
                  <div className="settings-header-row" key={index}>
                    <Input
                      value={header.name}
                      placeholder="x-opencode-session"
                      onChange={(e) => patchHeader(index, { name: e.target.value })}
                    />
                    <Input
                      value={header.value}
                      placeholder="${session_id}"
                      onChange={(e) => patchHeader(index, { value: e.target.value })}
                    />
                    <Button
                      variant="outline"
                      size="sm"
                      aria-label={t("settings.deleteHeader")}
                      onClick={() =>
                        onPatch({ headerRows: form.headerRows.filter((_, i) => i !== index) })
                      }
                    >
                      <TrashIcon className="h-3.5 w-3.5" />
                    </Button>
                  </div>
                ))}
                <button
                  type="button"
                  className="settings-dashed-button self-start"
                  onClick={() =>
                    onPatch({ headerRows: [...form.headerRows, { name: "", value: "" }] })
                  }
                >
                  <PlusIcon className="h-3.5 w-3.5" />
                  {t("settings.addHeader")}
                </button>
              </div>
            </FieldControl>
          </div>
        </div>
      </div>

      <div className="settings-divider">{t("settings.modelList")}</div>
      {errors.models && <div className="mb-2 text-ui-sm text-[var(--cw-err)]">{errors.models}</div>}
      {children}
    </>
  );
}

/** 模型列表区：行 = wire id + 类型标签 + 窗口/输出摘要 + 当前标记；空态虚线框 + 添加按钮。
 *  props：models 模型列表；activeModelId 当前默认模型（打"当前"标记）；
 *  onAdd / onEdit / onRemove 三个操作回调。 */
export function ModelListSection({
  models,
  activeModelId,
  onAdd,
  onEdit,
  onRemove,
}: {
  models: ProviderModel[];
  activeModelId: string | undefined;
  onAdd: () => void;
  onEdit: (model: ProviderModel) => void;
  onRemove: (model: ProviderModel) => void;
}) {
  if (models.length === 0) {
    return (
      <div className="settings-models-empty">
        <span className="text-ui-sm text-[var(--cw-text-2)]">{t("settings.noModelsHint")}</span>
        <button type="button" className="settings-dashed-button" onClick={onAdd}>
          <PlusIcon className="h-3.5 w-3.5" />
          {t("settings.addModel")}
        </button>
      </div>
    );
  }
  return (
    <>
      <div className="settings-collection-rows">
        {models.map((model) => (
          <div className="settings-collection-row" key={model.id}>
            <span className="settings-collection-row-info">
              <code>{model.model}</code>
              {model.vision && <Tag>{t("settings.modelVisionTag")}</Tag>}
              <span className="text-ui-sm text-[var(--muted-foreground)]">
                {Math.round(model.contextWindow / 1000)}k · {Math.round(model.maxTokens / 1000)}k
                {t("settings.maxOutput")}
              </span>
              {activeModelId === model.id && (
                <Tag className="settings-provider-active-tag">{t("settings.modelActiveTag")}</Tag>
              )}
            </span>
            <span className="flex items-center gap-1">
              <Button variant="ghost" size="sm" onClick={() => onEdit(model)}>
                <PencilIcon className="h-3.5 w-3.5" />
                {t("settings.editModel")}
              </Button>
              <Button
                variant="ghost"
                size="sm"
                className="text-[var(--destructive)] hover:bg-[var(--destructive)]/10"
                onClick={() => onRemove(model)}
              >
                <TrashIcon className="h-3.5 w-3.5" />
                {t("settings.delete")}
              </Button>
            </span>
          </div>
        ))}
      </div>
      <button type="button" className="settings-dashed-button mt-2 w-full" onClick={onAdd}>
        <PlusIcon className="h-3.5 w-3.5" />
        {t("settings.addModel")}
      </button>
    </>
  );
}

/** 模型编辑区（两列 grid，模型 ID 跨两列；复刻 CodeWave ModelEditor 字段集）。
 *  props：value 编辑中的模型；idError 是否已尝试提交且 ID 为空；onChange 局部更新。 */
export function ModelEditor({
  value,
  idError,
  onChange,
}: {
  value: ProviderModel;
  idError: boolean;
  onChange: (patch: Partial<ProviderModel>) => void;
}) {
  const idEmpty = value.model.trim() === "";
  return (
    <div className="settings-provider-model-form">
      <div className="settings-row settings-provider-model-field-wide">
        <div className="settings-row-copy">
          <span className="settings-row-title">
            {t("settings.modelId")} <span className="text-[var(--cw-err)]">*</span>
          </span>
          <span className="settings-row-description">{t("settings.modelIdNote")}</span>
        </div>
        <div className="settings-row-control">
          <FieldControl error={idError && idEmpty ? t("settings.required") : undefined}>
            <Input
              className={cn(idError && idEmpty && "settings-field-error-input")}
              value={value.model}
              placeholder="glm-5.3 / claude-sonnet-4-5 / gpt-4o"
              onChange={(e) => onChange({ model: e.target.value })}
            />
          </FieldControl>
        </div>
      </div>

      <div className="settings-row">
        <div className="settings-row-copy">
          <span className="settings-row-title">{t("settings.contextWindow")}</span>
          <span className="settings-row-description">{t("settings.contextWindowNote")}</span>
        </div>
        <div className="settings-row-control">
          <NumberInput
            aria-label={t("settings.contextWindow")}
            min={1000}
            step={1000}
            fallback={128000}
            value={value.contextWindow}
            onValueChange={(v) => onChange({ contextWindow: v })}
          />
        </div>
      </div>

      <div className="settings-row">
        <div className="settings-row-copy">
          <span className="settings-row-title">{t("settings.maxOutput")}</span>
          <span className="settings-row-description">{t("settings.maxOutputNote")}</span>
        </div>
        <div className="settings-row-control">
          <NumberInput
            aria-label={t("settings.maxOutput")}
            min={256}
            step={256}
            fallback={32768}
            value={value.maxTokens}
            onValueChange={(v) => onChange({ maxTokens: v })}
          />
        </div>
      </div>

      <div className="settings-row">
        <div className="settings-row-copy">
          <span className="settings-row-title">{t("settings.inputType")}</span>
        </div>
        <div className="settings-row-control">
          <div className="flex flex-wrap items-center gap-2">
            <CheckableTag checked locked>
              <LockIcon className="h-3 w-3" />
              {t("settings.typeText")}
            </CheckableTag>
            <CheckableTag
              checked={value.vision ?? false}
              onChange={(checked) => onChange({ vision: checked })}
            >
              <EyeIcon className="h-3 w-3" />
              {t("settings.typeVision")}
            </CheckableTag>
          </div>
        </div>
      </div>

      <div className="settings-row">
        <div className="settings-row-copy">
          <span className="settings-row-title">{t("settings.outputType")}</span>
        </div>
        <div className="settings-row-control">
          <div className="flex flex-wrap items-center gap-2">
            <CheckableTag checked locked>
              <LockIcon className="h-3 w-3" />
              {t("settings.typeText")}
            </CheckableTag>
          </div>
        </div>
      </div>

      <div className="settings-row settings-provider-model-field-wide">
        <div className="settings-row-copy">
          <span className="settings-row-title">{t("settings.reasoningEffort")}</span>
          <span className="settings-row-description">{t("settings.reasoningEffortNote")}</span>
        </div>
        <div className="settings-row-control">
          <Select
            width="narrow"
            value={value.reasoningEffort ?? ""}
            aria-label={t("settings.reasoningEffort")}
            onChange={(e) =>
              onChange({
                reasoningEffort: (e.target.value || undefined) as ProviderModel["reasoningEffort"],
              })
            }
          >
            <option value="">{t("settings.effortUnset")}</option>
            {(["low", "medium", "high", "max"] as const).map((level) => (
              <option key={level} value={level}>
                {level}
              </option>
            ))}
          </Select>
        </div>
      </div>
    </div>
  );
}

/**
 * 连接测试结果行（FR-02-09，19 号票）：idle 不渲染；testing 进行中提示；
 * ok 带延迟与模型数；error 带七类分类的人话文案（原始 message 兜底展示
 * 在 title 里，方便排查精简网关的非标准响应）。
 */
export function ConnectionTestResult({ test }: { test: ConnectionTestState }) {
  if (test.state === "idle") return null;
  if (test.state === "testing") {
    return <div className="py-1 text-ui-sm text-[var(--cw-text-2)]">{t("settings.testing")}</div>;
  }
  if (test.state === "ok") {
    return (
      <div className="py-1 text-ui-sm" data-testid="connection-test-ok">
        {t("settings.testOkLatency", { latency: test.latencyMs })}
        {test.modelCount > 0 ? t("settings.testOkModels", { count: test.modelCount }) : ""}
      </div>
    );
  }
  return (
    <div
      className="py-1 text-ui-sm text-[var(--cw-err)]"
      data-testid="connection-test-error"
      title={test.error.message}
    >
      {probeErrorLabel(test.error.code)}
    </div>
  );
}
