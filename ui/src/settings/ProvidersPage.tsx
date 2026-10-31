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
 *
 * 注：表单字段区、字段校验、凭据（newKey）策略、连接测试状态机与
 * 模型编辑器已抽取为共享层 `settings/ProviderFormShared.tsx`（首启引导的
 * 供应商步复用同一份，防两处表单漂移）；本组件只保留列表/弹框编排。
 */

import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { ConfirmDialog } from "@/components/ui/confirm";
import { Dialog } from "@/components/ui/dialog";
import { CheckableTag, Tag } from "@/components/ui/tag";
import { useT } from "@/i18n/core";
import type { ProviderConfig, ProviderModel } from "@/ipc/types";
import { PencilIcon, PlusIcon, TrashIcon } from "@/settings/icons";
import {
  allFieldsValid,
  apiFormatLabel,
  blankProvider,
  ConnectionTestResult,
  type ConnectionTestState,
  type CredentialState,
  type FieldKey,
  fieldIssue,
  formToProvider,
  ModelEditor,
  ModelListSection,
  ProviderFields,
  type ProviderForm,
  providerModelDefaults,
  runConnectionTest,
  toForm,
} from "@/settings/ProviderFormShared";
import { BUILTIN_PRESETS, newProviderFromPreset } from "@/settings/presets";
import { useSettingsStore } from "@/stores/settings";

export type { ConnectionTestState } from "@/settings/ProviderFormShared";
// 探测错误分类 → 人话文案映射表由共享层单源提供；此处 re-export 维持
// 既有 import 面（ProvidersPage.test.tsx 从本模块取 probeErrorLabel）。
export { probeErrorLabel } from "@/settings/ProviderFormShared";

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
  const t = useT();
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
  // 连接测试：卡片按 providerId 一份、弹框草稿一份。
  // 测试是探测动作不落盘——状态不入 settings 草稿（对齐搜索连通
  // 测试的草稿试连定案）。
  const [cardTests, setCardTests] = useState<Record<string, ConnectionTestState>>({});
  const [draftTest, setDraftTest] = useState<ConnectionTestState>({ state: "idle" });
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

  /** 卡片上的测试（列表态：用已保存配置，无覆写）。 */
  function testSavedProvider(provider: ProviderConfig) {
    void runConnectionTest(provider.id, undefined, (next) =>
      setCardTests((prev) => ({ ...prev, [provider.id]: next })),
    );
  }

  /** 弹框内的草稿测试（编辑态：baseUrlOverride 传表单当前值）。 */
  function testDraftProvider() {
    if (!editing) return;
    void runConnectionTest(editing.id, editing.baseUrl, setDraftTest);
  }

  /** 预设快捷填充（新增态）：名称/端点/模型/本地标记一次填入，凭据留待
   *  用户填写（本地预设免 Key，local=true 即免）。 */
  function applyPreset(slug: string) {
    const provider = newProviderFromPreset(slug);
    if (!provider) return;
    setAddForm((prev) => ({
      ...prev,
      id: provider.id,
      name: provider.name,
      apiFormat: provider.apiFormat,
      baseUrl: provider.baseUrl ?? "",
      models: provider.models,
      local: provider.local ?? false,
    }));
    // 预设填充视为对名称/端点/凭据三字段的操作（校验红字随填充消隐）
    setTouched((tp) => ({ ...tp, name: true, baseUrl: true, keys: true }));
  }

  /** 取供应商列表里第一个模型的 id（activeModelId 悬空时的回退目标）。 */
  function firstModelId(providers: ProviderConfig[]): string | undefined {
    return providers.flatMap((p) => p.models)[0]?.id;
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

  /** 新增提交：先亮出全部字段错误再校验拦截；通过后并入草稿并关弹框。 */
  function submitAdd() {
    setSubmitTried(true); // 先亮出全部字段错误，再做有效性拦截
    if (!allFieldsValid(addForm, credential)) return;
    const provider = formToProvider(addForm, credential);
    patchConfig({
      providers: [...activeConfig.providers, provider],
      activeModelId: activeConfig.activeModelId ?? provider.models[0]?.id,
    });
    closeProviderModal();
  }

  /** 编辑完成：仅在内容确有变化时写草稿；默认模型被编辑掉时同样回退第一个模型。 */
  function finishEdit() {
    if (!editing) return;
    const provider = formToProvider(editing, credential);
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
          <h3 className="settings-section-title">{t("settings.providersSection")}</h3>
          <Button onClick={startAddProvider}>
            <PlusIcon className="h-4 w-4" />
            {t("settings.addProvider")}
          </Button>
        </div>
        <div className="settings-section-card">
          {activeConfig.providers.length === 0 ? (
            <div className="px-5 py-14 text-center text-ui-base text-[var(--muted-foreground)]">
              {t("settings.noProviders")}
              <div className="mt-1 text-ui-sm">{t("settings.noProvidersHint")}</div>
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
                      key="test"
                      variant="ghost"
                      size="sm"
                      // 探测三方言全接通：连接测试与
                      // 实际对话走同一 apiFormat 选择（openai_chat 与
                      // openai_responses 同 /models + Bearer，anthropic
                      // 用 x-api-key + anthropic-version），不再限制方言
                      onClick={() => testSavedProvider(p)}
                    >
                      {t("settings.testConnectionShort")}
                    </Button>,
                    <Button
                      key="edit"
                      variant="ghost"
                      size="sm"
                      onClick={() => startEditProvider(p)}
                    >
                      <PencilIcon className="h-3.5 w-3.5" />
                      {t("settings.edit")}
                    </Button>,
                    <Button
                      key="delete"
                      variant="ghost"
                      size="sm"
                      className="text-[var(--destructive)] hover:bg-[var(--destructive)]/10"
                      onClick={() => setConfirmState({ kind: "provider", provider: p })}
                    >
                      <TrashIcon className="h-3.5 w-3.5" />
                      {t("settings.delete")}
                    </Button>,
                  ]}
                >
                  <div className="settings-provider-card-content">
                    <div className="settings-provider-endpoint">
                      <span className="settings-provider-block-label">
                        {t("settings.apiEndpoint")}
                      </span>
                      <span
                        className="settings-provider-endpoint-url"
                        title={p.baseUrl || undefined}
                      >
                        {p.baseUrl || "-"}
                      </span>
                      {p.local && <Tag>{t("settings.localTag")}</Tag>}
                    </div>
                    <ConnectionTestResult test={cardTests[p.id] ?? { state: "idle" }} />
                    <div className="border-t border-[var(--border)] pt-3">
                      <span className="settings-provider-block-label">
                        {t("settings.modelList")}
                      </span>
                      <div className="mt-1.5">
                        {p.models.length > 0 ? (
                          p.models.map((model) => (
                            <div className="settings-provider-model" key={model.id}>
                              <span className="settings-provider-model-name">{model.model}</span>
                              <span className="settings-provider-model-limits">
                                {t("settings.modelWindowOutput", {
                                  window: Math.round(model.contextWindow / 1000),
                                  output: Math.round(model.maxTokens / 1000),
                                })}
                              </span>
                              <span className="settings-provider-model-tags">
                                {model.vision && <Tag>{t("settings.modelVisionTag")}</Tag>}
                                {activeConfig.activeModelId === model.id && (
                                  <Tag className="settings-provider-active-tag">
                                    {t("settings.modelActiveTag")}
                                  </Tag>
                                )}
                              </span>
                            </div>
                          ))
                        ) : (
                          <span className="settings-provider-model-limits">
                            {t("settings.noModels")}
                          </span>
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
              ? t("settings.addModel")
              : t("settings.editModel")
            : view.kind === "add"
              ? t("settings.addProviderTitle")
              : t("settings.editProviderTitle")
        }
        onClose={closeProviderModal}
        width={860}
        actions={
          modelEditor
            ? [
                {
                  label: t("onboarding.backToProvider"),
                  variant: "ghost",
                  onClick: () => setModelEditor(null),
                },
                { label: t("onboarding.saveModel"), onClick: commitModel },
              ]
            : [
                { label: t("settings.cancel"), variant: "ghost", onClick: closeProviderModal },
                {
                  label: view.kind === "add" ? t("settings.addProviderTitle") : t("settings.done"),
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
                  <>
                    <p className="mb-2 text-ui-sm text-[var(--muted-foreground)]">
                      {t("settings.providerAddHint")}
                    </p>
                    {/* 预设快捷填充：本地三家标注「本地 / 离线可用」，
                        免 API Key（选中后 Key 行替换为说明文案） */}
                    <div className="mb-4 flex flex-wrap items-center gap-1.5">
                      <span className="text-ui-sm text-[var(--cw-text-2)]">
                        {t("settings.fillFromPreset")}
                      </span>
                      {BUILTIN_PRESETS.map((preset) => (
                        <CheckableTag
                          key={preset.slug}
                          checked={
                            modalForm.name === preset.name && modalForm.baseUrl === preset.baseUrl
                          }
                          onChange={() => applyPreset(preset.slug)}
                        >
                          {preset.name}
                          {preset.local ? t("settings.presetLocal") : ""}
                        </CheckableTag>
                      ))}
                    </div>
                  </>
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
                  onTestDraft={view.kind === "edit" ? testDraftProvider : undefined}
                  draftTest={draftTest}
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
            ? t("settings.deleteConfirmProvider", {
                name: confirmState.provider.name || confirmState.provider.id,
              })
            : confirmState?.kind === "model"
              ? t("settings.deleteConfirmModel", { name: confirmState.model.model })
              : ""
        }
        confirmLabel={t("settings.delete")}
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
