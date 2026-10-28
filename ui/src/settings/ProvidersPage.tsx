/**
 * 供应商页（交互与排版复刻 CodeWave——本项目的交互对标参考项目）。
 *
 * 结构与状态机一一对应 CodeWave：
 *  - 列表视图：section 标题右上「添加供应商」主按钮 + 供应商卡片网格
 *    （卡 = 名称 + API 格式标签 / API 端点块 / 模型列表块[当前标记] /
 *    actions 底栏[编辑|删除确认]）；
 *  - 单一宽弹框（860px）承担新增/编辑，内部两态切换：供应商表单 ↔ 模型编辑器
 *    （同一弹框、footer 与标题随态切换，不嵌套第二层弹框）；
 *  - 字段校验展示时机：新增 = touched（字段改过）或提交尝试后才亮红字；编辑 = 实时
 *    （进入时值本就有效，只有破坏后才见红）；
 *  - 保存语义 = 只写页面草稿（providers / activeModelId），统一走页头「保存」提交；
 *    删除供应商时 activeModelId 悬空自动回退第一个模型（firstModelId 同口径）。
 *
 * 与 CodeWave 的数据面差异映射：
 *  - Key 池 → 单 apiKey：编辑时凭据策略两选（保留现有 / 替换为新 Key）+ 输入框联动；
 *  - headers: Record ↔ 行式编辑（提交时空行丢弃、转回对象）；
 *  - video 能力字段不存在，输入类型只保留 文本(锁定) + 图像(可切换)；
 *  - 推理力度含「未设置」空档（原生 select 无 allowClear）。
 */

import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { cn } from "@/components/ui/cn";
import { ConfirmDialog } from "@/components/ui/confirm";
import { Dialog } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { NumberInput } from "@/components/ui/number-input";
import { Select } from "@/components/ui/select";
import { CheckableTag, Tag } from "@/components/ui/tag";
import type { ApiFormat, ProviderConfig, ProviderModel } from "@/ipc/types";
import { EyeIcon, LockIcon, PencilIcon, PlusIcon, TrashIcon } from "@/settings/icons";
import { useSettingsStore } from "@/stores/settings";

/** API 格式下拉的选项表（label 展示 / value 存入配置）。 */
const API_FORMAT_OPTIONS: { label: string; value: ApiFormat }[] = [
  { label: "OpenAI Chat", value: "openai_chat" },
  { label: "Anthropic Messages", value: "anthropic_messages" },
  { label: "OpenAI Responses", value: "openai_responses" },
];

// 已知官方默认端点：切协议时仅在这些值之间同步；自定义 URL 不受影响（与 CodeWave 同规则）
const DEFAULT_BASE_URLS: Record<ApiFormat, string> = {
  openai_chat: "https://api.openai.com/v1",
  anthropic_messages: "https://api.anthropic.com",
  openai_responses: "https://api.openai.com/v1",
};

/** API 格式枚举值 → 下拉展示名（卡片标签用）。 */
function apiFormatLabel(v: ApiFormat): string {
  return API_FORMAT_OPTIONS.find((o) => o.value === v)?.label ?? v;
}

/** 请求头行（表单内表示；提交时转回 Record） */
type HeaderRow = { name: string; value: string };

/** 表单内的供应商表示（baseUrl 必填语义、headers 行式） */
type ProviderForm = Omit<ProviderConfig, "baseUrl" | "headers" | "apiKey"> & {
  baseUrl: string;
  apiKey?: string;
  headerRows: HeaderRow[];
};

/** 生成草稿层新 id：优先用 crypto.randomUUID，老环境回退随机串。 */
function newDraftId(): string {
  return typeof crypto !== "undefined" && "randomUUID" in crypto
    ? crypto.randomUUID()
    : `tmp-${Math.random().toString(36).slice(2)}${Date.now().toString(36)}`;
}

/** 配置形态 → 表单形态（baseUrl 转必填字符串、headers 对象摊开成行）。 */
function toForm(provider: ProviderConfig): ProviderForm {
  return {
    ...provider,
    baseUrl: provider.baseUrl ?? "",
    apiKey: provider.apiKey,
    headerRows: Object.entries(provider.headers ?? {}).map(([name, value]) => ({ name, value })),
  };
}

/** 空白新增表单：默认 OpenAI Chat 格式及其官方端点，无凭据无模型。 */
function blankProvider(): ProviderForm {
  return {
    id: newDraftId(),
    name: "",
    apiFormat: "openai_chat",
    baseUrl: DEFAULT_BASE_URLS.openai_chat,
    models: [],
    headerRows: [],
  };
}

/** 新模型的默认值（上下文 128k、输出 32k，纯文本；对应常见模型的保守缺省）。 */
function providerModelDefaults(): ProviderModel {
  return {
    id: newDraftId(),
    model: "",
    maxTokens: 32768,
    contextWindow: 128000,
    vision: false,
  };
}

/** 取供应商列表里第一个模型的 id（activeModelId 悬空时的回退目标）。 */
function firstModelId(providers: ProviderConfig[]): string | undefined {
  return providers.flatMap((p) => p.models)[0]?.id;
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
type FieldKey = "name" | "baseUrl" | "keys" | "models" | "headers";

/** 凭据策略状态：isNew = 新增（恒录入新 Key）；编辑时 keep（保留）/ replace（替换）两选。 */
interface CredentialState {
  isNew: boolean;
  mode: "keep" | "replace";
  newKey: string;
}

/** 字段校验（结构化问题 → 文案），返回该字段错误文本（undefined = 通过） */
function fieldIssue(
  form: ProviderForm,
  key: FieldKey,
  credential: CredentialState,
): string | undefined {
  if (key === "name" && form.name.trim() === "") return "必填";
  if (key === "baseUrl") {
    const url = form.baseUrl.trim();
    if (url === "") return "必填";
    if (!/^https?:\/\/[^\s/]+/i.test(url)) return "必须是合法 http(s) 地址";
  }
  if (key === "keys") {
    const hasKey = credential.isNew
      ? credential.newKey.trim() !== ""
      : credential.mode === "replace"
        ? credential.newKey.trim() !== ""
        : !!form.apiKey;
    if (!hasKey) return "必填（供应商调用需要 API Key）";
  }
  if (key === "models" && form.models.length === 0) return "至少配置一个模型";
  if (key === "headers" && !headersValid(form.headerRows)) {
    return "头名非法、保留、重名，或值含不可见字符";
  }
  return undefined;
}

/** 全字段校验（提交前的总闸）：五个字段全部通过才允许入草稿。 */
function allFieldsValid(form: ProviderForm, credential: CredentialState): boolean {
  const keys: FieldKey[] = ["name", "baseUrl", "keys", "models", "headers"];
  return keys.every((key) => fieldIssue(form, key, credential) === undefined);
}

/** 弹框内模型编辑器状态（target 区分新增表单与既有供应商） */
type ModelEditorState = {
  target: "add-form" | { providerId: string };
  value: ProviderModel;
  isNew: boolean;
  idError: boolean;
};

/** 视图状态：供应商列表，或在同一弹框中新增 / 编辑供应商 */
type View = { kind: "list" } | { kind: "add" } | { kind: "edit"; providerId: string };

/**
 * 供应商页组件：列表视图 + 新增/编辑弹框（内含模型编辑器两态切换）。
 * 所有改动只写设置草稿（patchConfig），统一由设置页页头「保存」提交。
 */
export function ProvidersPage() {
  const config = useSettingsStore((s) => s.config);
  const patchConfig = useSettingsStore((s) => s.patchConfig);

  const [view, setView] = useState<View>({ kind: "list" });
  // 凭据策略：新增恒为 replace（直接录入）；编辑 keep / replace 两选
  const [credentialMode, setCredentialMode] = useState<"keep" | "replace">("keep");
  const [newKey, setNewKey] = useState("");
  // 新增供应商的本地表单（提交前不并入草稿）；编辑也在弹框内暂存，完成时才写入草稿
  const [addForm, setAddForm] = useState<ProviderForm>(blankProvider);
  const [editForm, setEditForm] = useState<ProviderForm | null>(null);
  // 字段校验展示时机（字段变更后或点击提交才出红字；打开即红太吵）
  const [touched, setTouched] = useState<Partial<Record<FieldKey, boolean>>>({});
  const [submitTried, setSubmitTried] = useState(false);
  const [modelEditor, setModelEditor] = useState<ModelEditorState | null>(null);
  // 删除确认（供应商 / 模型两形态）
  const [confirmState, setConfirmState] = useState<
    | { kind: "provider"; provider: ProviderConfig }
    | { kind: "model"; form: "add" | "edit"; model: ProviderModel }
    | null
  >(null);

  if (!config) return null;
  // 判空收窄不传播进下方闭包：以别名固定非空类型
  const activeConfig = config;
  const editing = view.kind === "edit" && editForm?.id === view.providerId ? editForm : null;

  const credential: CredentialState = {
    isNew: view.kind === "add",
    mode: view.kind === "add" ? "replace" : credentialMode,
    newKey,
  };

  /** 新增表单字段错误（touched / 提交尝试后显示；模型与请求头仅由提交尝试触发） */
  function addFieldError(key: FieldKey): string | undefined {
    const issue = fieldIssue(addForm, key, credential);
    if (!issue) return undefined;
    if (key === "models" || key === "headers") return submitTried ? issue : undefined;
    return touched[key] || submitTried ? issue : undefined;
  }

  /** 编辑表单字段错误（实时显示：进入时值本就有效，只有破坏后才见红字） */
  function editFieldError(key: FieldKey): string | undefined {
    return editing ? fieldIssue(editing, key, credential) : undefined;
  }

  /** 从草稿移除供应商；若它持有当前默认模型，则回退到剩余列表的第一个模型。 */
  function removeProvider(provider: ProviderConfig) {
    const providers = activeConfig.providers.filter((x) => x.id !== provider.id);
    const removed = new Set(provider.models.map((m) => m.id));
    const activeValid =
      activeConfig.activeModelId != null && !removed.has(activeConfig.activeModelId);
    patchConfig({
      providers,
      activeModelId: activeValid ? activeConfig.activeModelId : firstModelId(providers),
    });
    if (view.kind === "edit" && view.providerId === provider.id) setView({ kind: "list" });
  }

  /** 进入新增态：重置全部表单状态后切到 add 视图。 */
  function startAddProvider() {
    setCredentialMode("replace");
    setNewKey("");
    setAddForm(blankProvider());
    setEditForm(null);
    setTouched({});
    setSubmitTried(false);
    setModelEditor(null);
    setView({ kind: "add" });
  }

  /** 进入编辑态：把配置转成表单形态暂存，凭据策略默认"保留现有 Key"。 */
  function startEditProvider(provider: ProviderConfig) {
    setCredentialMode("keep");
    setNewKey("");
    setEditForm(toForm(provider));
    setModelEditor(null);
    setView({ kind: "edit", providerId: provider.id });
  }

  /** 表单 → 配置载荷：trim、空请求头行丢弃、凭据按策略落到 apiKey */
  function formToProvider(form: ProviderForm): ProviderConfig {
    const headers = Object.fromEntries(
      form.headerRows
        .map((row) => ({ name: row.name.trim(), value: row.value.trim() }))
        .filter((row) => row.name !== "")
        .map((row) => [row.name, row.value]),
    );
    const apiKey =
      view.kind === "add" || credentialMode === "replace"
        ? newKey.trim() === ""
          ? undefined
          : newKey.trim()
        : form.apiKey;
    return {
      ...form,
      name: form.name.trim(),
      baseUrl: form.baseUrl.trim(),
      apiKey,
      headers: Object.keys(headers).length > 0 ? headers : undefined,
    };
  }

  /** 新增提交：先亮出全部字段错误再校验拦截；通过后并入草稿并关弹框。 */
  function submitAdd() {
    setSubmitTried(true); // 先亮出全部字段错误，再做有效性拦截
    if (!allFieldsValid(addForm, credential)) return;
    const provider = formToProvider(addForm);
    patchConfig({
      providers: [...activeConfig.providers, provider],
      activeModelId: activeConfig.activeModelId ?? provider.models[0]?.id,
    });
    closeProviderModal();
  }

  /** 编辑完成：仅在内容确有变化时写草稿；默认模型被编辑掉时同样回退第一个模型。 */
  function finishEdit() {
    if (!editing) return;
    const provider = formToProvider(editing);
    const original = activeConfig.providers.find((p) => p.id === editing.id);
    if (original && JSON.stringify(original) !== JSON.stringify(provider)) {
      const providers = activeConfig.providers.map((p) => (p.id === editing.id ? provider : p));
      const activeStillExists =
        activeConfig.activeModelId == null ||
        providers.some((p) => p.models.some((m) => m.id === activeConfig.activeModelId));
      patchConfig({
        providers,
        ...(activeStillExists ? {} : { activeModelId: firstModelId(providers) }),
      });
    }
    closeProviderModal();
  }

  /** 关闭供应商弹框：清空暂存与校验状态，视图回列表。 */
  function closeProviderModal() {
    setNewKey("");
    setCredentialMode("keep");
    setModelEditor(null);
    setEditForm(null);
    setTouched({});
    setSubmitTried(false);
    setView({ kind: "list" });
  }

  /** 模型编辑器提交：模型 ID 空则亮红；按新增/编辑写回对应目标表单。 */
  function commitModel() {
    const editor = modelEditor;
    if (!editor) return;
    const model = { ...editor.value, model: editor.value.model.trim() };
    if (!model.model) {
      setModelEditor({ ...editor, idError: true });
      return;
    }
    if (editor.target === "add-form") {
      setAddForm((prev) => ({
        ...prev,
        models: editor.isNew
          ? [...prev.models, model]
          : prev.models.map((x) => (x.id === model.id ? model : x)),
      }));
    } else if (editing && editing.id === editor.target.providerId) {
      setEditForm({
        ...editing,
        models: editor.isNew
          ? [...editing.models, model]
          : editing.models.map((x) => (x.id === model.id ? model : x)),
      });
    }
    setModelEditor(null);
  }

  // 弹框三态推导：新增态或编辑表单就绪时开；表单/模型编辑目标随之取对应值
  const modalOpen = view.kind === "add" || (view.kind === "edit" && editing != null);
  const modalForm = view.kind === "add" ? addForm : editing;
  const modelTarget: ModelEditorState["target"] | null =
    view.kind === "add" ? "add-form" : editing ? { providerId: editing.id } : null;

  return (
    <div className="setting-anchor" data-setting-id="providers">
      <section className="settings-section settings-provider-section">
        <div className="flex items-center justify-between gap-3">
          <h3 className="settings-section-title">供应商</h3>
          <Button onClick={startAddProvider}>
            <PlusIcon className="h-4 w-4" />
            添加供应商
          </Button>
        </div>
        <div className="settings-section-card">
          {activeConfig.providers.length === 0 ? (
            <div className="px-5 py-14 text-center text-body text-[var(--muted-foreground)]">
              暂无供应商
              <div className="mt-1 text-secondary">
                点右上「添加供应商」，配置端点与 API Key 后即可开始对话。
              </div>
            </div>
          ) : (
            <div className="settings-provider-cards">
              {activeConfig.providers.map((p) => (
                <Card
                  key={p.id}
                  title={
                    <>
                      <strong className="settings-provider-name">{p.name || p.id}</strong>
                      <Tag className="settings-provider-api-tag">{apiFormatLabel(p.apiFormat)}</Tag>
                    </>
                  }
                  actions={[
                    <Button
                      key="edit"
                      variant="ghost"
                      size="sm"
                      onClick={() => startEditProvider(p)}
                    >
                      <PencilIcon className="h-3.5 w-3.5" />
                      编辑
                    </Button>,
                    <Button
                      key="delete"
                      variant="ghost"
                      size="sm"
                      className="text-[var(--destructive)] hover:bg-[var(--destructive)]/10"
                      onClick={() => setConfirmState({ kind: "provider", provider: p })}
                    >
                      <TrashIcon className="h-3.5 w-3.5" />
                      删除
                    </Button>,
                  ]}
                >
                  <div className="settings-provider-card-content">
                    <div className="settings-provider-endpoint">
                      <span className="settings-provider-block-label">API 端点</span>
                      <span
                        className="settings-provider-endpoint-url"
                        title={p.baseUrl || undefined}
                      >
                        {p.baseUrl || "-"}
                      </span>
                    </div>
                    <div className="border-t border-[var(--border)] pt-3">
                      <span className="settings-provider-block-label">模型列表</span>
                      <div className="mt-1.5">
                        {p.models.length > 0 ? (
                          p.models.map((model) => (
                            <div className="settings-provider-model" key={model.id}>
                              <span className="settings-provider-model-name">{model.model}</span>
                              <span className="settings-provider-model-limits">
                                {Math.round(model.contextWindow / 1000)}k 上下文 ·{" "}
                                {Math.round(model.maxTokens / 1000)}k 输出
                              </span>
                              <span className="settings-provider-model-tags">
                                {model.vision && <Tag>图像</Tag>}
                                {activeConfig.activeModelId === model.id && (
                                  <Tag className="settings-provider-active-tag">当前</Tag>
                                )}
                              </span>
                            </div>
                          ))
                        ) : (
                          <span className="settings-provider-model-limits">暂无模型</span>
                        )}
                      </div>
                    </div>
                  </div>
                </Card>
              ))}
            </div>
          )}
        </div>
      </section>

      {/* 供应商弹框：供应商表单 ↔ 模型编辑器同框切换（标题与 footer 随态变） */}
      <Dialog
        open={modalOpen}
        title={
          modelEditor
            ? modelEditor.isNew
              ? "添加模型"
              : "编辑模型"
            : view.kind === "add"
              ? "添加供应商"
              : "编辑供应商"
        }
        onClose={closeProviderModal}
        width={860}
        actions={
          modelEditor
            ? [
                { label: "返回供应商", variant: "ghost", onClick: () => setModelEditor(null) },
                { label: "保存模型", onClick: commitModel },
              ]
            : [
                { label: "取消", variant: "ghost", onClick: closeProviderModal },
                {
                  label: view.kind === "add" ? "添加供应商" : "完成",
                  onClick: () => (view.kind === "add" ? submitAdd() : finishEdit()),
                },
              ]
        }
      >
        <div className="settings-provider-modal-body">
          {modelEditor ? (
            <ModelEditor
              value={modelEditor.value}
              idError={modelEditor.idError}
              onChange={(patch) =>
                setModelEditor((prev) =>
                  prev ? { ...prev, value: { ...prev.value, ...patch } } : prev,
                )
              }
            />
          ) : (
            modalForm &&
            modelTarget && (
              <>
                {view.kind === "add" && (
                  <p className="mb-4 text-secondary text-[var(--muted-foreground)]">
                    填写端点与凭据后添加；模型定义完全以你的输入为准（无内置目录）。
                  </p>
                )}
                <ProviderFields
                  form={modalForm}
                  credential={credential}
                  errors={
                    view.kind === "add"
                      ? {
                          name: addFieldError("name"),
                          baseUrl: addFieldError("baseUrl"),
                          keys: addFieldError("keys"),
                          models: addFieldError("models"),
                          headers: addFieldError("headers"),
                        }
                      : {
                          name: editFieldError("name"),
                          baseUrl: editFieldError("baseUrl"),
                          keys: editFieldError("keys"),
                          models: editFieldError("models"),
                          headers: editFieldError("headers"),
                        }
                  }
                  onPatch={(patch) => {
                    if (view.kind === "add") {
                      setAddForm((prev) => ({ ...prev, ...patch }));
                      for (const key of ["name", "baseUrl", "keys"] as const) {
                        if (key in patch) setTouched((tp) => ({ ...tp, [key]: true }));
                      }
                    } else if (editing) {
                      setEditForm((prev) =>
                        prev?.id === editing.id ? { ...prev, ...patch } : prev,
                      );
                    }
                  }}
                  onCredentialMode={setCredentialMode}
                  onNewKey={(value) => {
                    setNewKey(value);
                    setTouched((tp) => ({ ...tp, keys: true }));
                  }}
                >
                  <ModelListSection
                    models={modalForm.models}
                    activeModelId={activeConfig.activeModelId}
                    onAdd={() =>
                      setModelEditor({
                        target: modelTarget,
                        value: providerModelDefaults(),
                        isNew: true,
                        idError: false,
                      })
                    }
                    onEdit={(model) =>
                      setModelEditor({
                        target: modelTarget,
                        value: { ...model },
                        isNew: false,
                        idError: false,
                      })
                    }
                    onRemove={(model) =>
                      setConfirmState({
                        kind: "model",
                        form: view.kind === "add" ? "add" : "edit",
                        model,
                      })
                    }
                  />
                </ProviderFields>
              </>
            )
          )}
        </div>
      </Dialog>

      {/* 删除确认：供应商 / 模型两形态共用 */}
      <ConfirmDialog
        open={confirmState !== null}
        title={
          confirmState?.kind === "provider"
            ? `删除「${confirmState.provider.name || confirmState.provider.id}」?`
            : confirmState?.kind === "model"
              ? `删除「${confirmState.model.model}」?`
              : ""
        }
        confirmLabel="删除"
        onCancel={() => setConfirmState(null)}
        onConfirm={() => {
          if (confirmState?.kind === "provider") {
            removeProvider(confirmState.provider);
          } else if (confirmState?.kind === "model") {
            if (confirmState.form === "add") {
              setAddForm((prev) => ({
                ...prev,
                models: prev.models.filter((x) => x.id !== confirmState.model.id),
              }));
            } else if (editing) {
              setEditForm({
                ...editing,
                models: editing.models.filter((x) => x.id !== confirmState.model.id),
              });
            }
          }
          setConfirmState(null);
        }}
      />
    </div>
  );
}

/** 表单行内带错误/说明的控件包装（复刻 Form.Item 的 label + help + extra 结构）。
 *  props：error 错误文案（优先显示）；hint 无错误时的说明文案；wide 占满控件列。 */
function FieldControl({
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
function ProviderFields({
  form,
  credential,
  errors,
  onPatch,
  onCredentialMode,
  onNewKey,
  children,
}: {
  form: ProviderForm;
  credential: CredentialState;
  errors: Partial<Record<FieldKey, string | undefined>>;
  onPatch: (patch: Partial<ProviderForm>) => void;
  onCredentialMode: (mode: "keep" | "replace") => void;
  onNewKey: (value: string) => void;
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
              名称 <span className="text-[var(--cw-err)]">*</span>
            </span>
          </div>
          <div className="settings-row-control">
            <FieldControl error={errors.name} wide>
              <Input
                className={cn(errors.name && "settings-field-error-input")}
                value={form.name}
                placeholder="如 智谱 GLM Coding Plan"
                onChange={(e) => onPatch({ name: e.target.value })}
              />
            </FieldControl>
          </div>
        </div>

        <div className="settings-row">
          <div className="settings-row-copy">
            <span className="settings-row-title">
              API 格式 <span className="text-[var(--cw-err)]">*</span>
            </span>
          </div>
          <div className="settings-row-control">
            <FieldControl wide>
              <Select
                className="w-full"
                value={form.apiFormat}
                aria-label="API 格式"
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
          </div>
          <div className="settings-row-control">
            <FieldControl error={errors.baseUrl} wide>
              <Input
                className={cn(
                  "font-mono text-secondary",
                  errors.baseUrl && "settings-field-error-input",
                )}
                value={form.baseUrl}
                placeholder="https://api.example.com/v1"
                onChange={(e) => onPatch({ baseUrl: e.target.value })}
              />
            </FieldControl>
          </div>
        </div>

        <div className="settings-row">
          <div className="settings-row-copy">
            <span className="settings-row-title">
              API Key <span className="text-[var(--cw-err)]">*</span>
            </span>
            <span className="settings-row-description">
              {credential.isNew || credential.mode === "replace"
                ? "明文存于 config.json（文件权限 0600）。"
                : "现有 Key 保持在 config.json，不在界面回显。"}
            </span>
          </div>
          <div className="settings-row-control">
            <FieldControl error={errors.keys} wide>
              {credential.isNew ? (
                <Input
                  className={cn(
                    "font-mono text-secondary",
                    errors.keys && "settings-field-error-input",
                  )}
                  autoComplete="off"
                  value={credential.newKey}
                  placeholder="sk-…"
                  onChange={(e) => onNewKey(e.target.value)}
                />
              ) : (
                <div className="settings-credential-options">
                  <span className="text-secondary text-[var(--cw-text-2)]">
                    {form.apiKey ? "已配置 API Key" : "尚未配置 API Key"}
                  </span>
                  <label>
                    <input
                      type="radio"
                      name="credential-mode"
                      checked={credential.mode === "keep"}
                      onChange={() => onCredentialMode("keep")}
                    />
                    保留现有 Key
                  </label>
                  <label>
                    <input
                      type="radio"
                      name="credential-mode"
                      checked={credential.mode === "replace"}
                      onChange={() => onCredentialMode("replace")}
                    />
                    替换为新 Key
                  </label>
                  {credential.mode === "replace" && (
                    <Input
                      className={cn(
                        "font-mono text-secondary",
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
            <span className="settings-row-title">自定义请求头</span>
            <span className="settings-row-description">
              追加到每次模型请求；保留名（Authorization 等）不可覆盖。
            </span>
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
                      aria-label="删除请求头"
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
                  添加请求头
                </button>
              </div>
            </FieldControl>
          </div>
        </div>
      </div>

      <div className="settings-divider">模型列表</div>
      {errors.models && (
        <div className="mb-2 text-secondary text-[var(--cw-err)]">{errors.models}</div>
      )}
      {children}
    </>
  );
}

/** 模型列表区：行 = wire id + 类型标签 + 窗口/输出摘要 + 当前标记；空态虚线框 + 添加按钮。
 *  props：models 模型列表；activeModelId 当前默认模型（打"当前"标记）；
 *  onAdd / onEdit / onRemove 三个操作回调。 */
function ModelListSection({
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
        <span className="text-secondary text-[var(--cw-text-2)]">暂无模型</span>
        <button type="button" className="settings-dashed-button" onClick={onAdd}>
          <PlusIcon className="h-3.5 w-3.5" />
          添加模型
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
              {model.vision && <Tag>图像</Tag>}
              <span className="text-secondary text-[var(--muted-foreground)]">
                {Math.round(model.contextWindow / 1000)}k · {Math.round(model.maxTokens / 1000)}k
                输出
              </span>
              {activeModelId === model.id && (
                <Tag className="settings-provider-active-tag">当前</Tag>
              )}
            </span>
            <span className="flex items-center gap-1">
              <Button variant="ghost" size="sm" onClick={() => onEdit(model)}>
                <PencilIcon className="h-3.5 w-3.5" />
                编辑模型
              </Button>
              <Button
                variant="ghost"
                size="sm"
                className="text-[var(--destructive)] hover:bg-[var(--destructive)]/10"
                onClick={() => onRemove(model)}
              >
                <TrashIcon className="h-3.5 w-3.5" />
                删除
              </Button>
            </span>
          </div>
        ))}
      </div>
      <button type="button" className="settings-dashed-button mt-2 w-full" onClick={onAdd}>
        <PlusIcon className="h-3.5 w-3.5" />
        添加模型
      </button>
    </>
  );
}

/** 模型编辑区（两列 grid，模型 ID 跨两列；复刻 CodeWave ModelEditor 字段集）。
 *  props：value 编辑中的模型；idError 是否已尝试提交且 ID 为空；onChange 局部更新。 */
function ModelEditor({
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
            模型 ID <span className="text-[var(--cw-err)]">*</span>
          </span>
          <span className="settings-row-description">服务商文档中的 wire id。</span>
        </div>
        <div className="settings-row-control">
          <FieldControl error={idError && idEmpty ? "必填" : undefined}>
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
          <span className="settings-row-title">上下文窗口</span>
          <span className="settings-row-description">超过后触发历史压缩。</span>
        </div>
        <div className="settings-row-control">
          <NumberInput
            aria-label="上下文窗口"
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
          <span className="settings-row-title">最大输出</span>
          <span className="settings-row-description">单次回复的输出上限。</span>
        </div>
        <div className="settings-row-control">
          <NumberInput
            aria-label="最大输出"
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
          <span className="settings-row-title">输入类型</span>
        </div>
        <div className="settings-row-control">
          <div className="flex flex-wrap items-center gap-2">
            <CheckableTag checked locked>
              <LockIcon className="h-3 w-3" />
              文本
            </CheckableTag>
            <CheckableTag
              checked={value.vision ?? false}
              onChange={(checked) => onChange({ vision: checked })}
            >
              <EyeIcon className="h-3 w-3" />
              图像
            </CheckableTag>
          </div>
        </div>
      </div>

      <div className="settings-row">
        <div className="settings-row-copy">
          <span className="settings-row-title">输出类型</span>
        </div>
        <div className="settings-row-control">
          <div className="flex flex-wrap items-center gap-2">
            <CheckableTag checked locked>
              <LockIcon className="h-3 w-3" />
              文本
            </CheckableTag>
          </div>
        </div>
      </div>

      <div className="settings-row settings-provider-model-field-wide">
        <div className="settings-row-copy">
          <span className="settings-row-title">推理力度</span>
          <span className="settings-row-description">按模型支持情况生效；未设置用服务端默认。</span>
        </div>
        <div className="settings-row-control">
          <Select
            width="narrow"
            value={value.reasoningEffort ?? ""}
            aria-label="推理力度"
            onChange={(e) =>
              onChange({
                reasoningEffort: (e.target.value || undefined) as ProviderModel["reasoningEffort"],
              })
            }
          >
            <option value="">未设置</option>
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
