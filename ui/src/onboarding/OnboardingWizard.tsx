/**
 * 首启引导向导（30 号票，FR-13-18 / §10.7）：全新用户第一次打开 CraftWave
 * 不用自己摸设置——四步向导把「能开始干活」的最小配置走完：
 *
 *   语言 → 供应商与 Key（含连通测试）→ 项目 → 权限档与安全简介
 *
 * 行为口径（票面）：
 *  - 每一步都可跳过（右上「跳过」单步前进）；顶部 × 整包关闭 = 跳过未完成，
 *    记录当前步，下次从该步续走；completed 后不再自动弹、横幅消失；
 *  - 引导绝不阻塞临时会话直接开聊：覆盖层可随时关闭，主界面在底层可用；
 *  - 第 2 步复用共享供应商表单（settings/ProviderFormShared.tsx，与设置页
 *    ProvidersPage 单源）+ 预设 chips + test_provider 连通测试；测试失败
 *    红字展示分类文案，「仍要保存」与「跳过」并存（已存项保留）；
 *  - test_provider 不可用（未注册等 reject）时按钮降级为「稍后在设置中
 *    测试」，不阻塞向导（软依赖）；
 *  - 第 4 步纯静态说明（四档权限 Shift+Tab 循环语义 / 审批弹窗 / 计划门 /
 *    设置入口），仅一个「完成」钮，无写操作；
 *  - 键盘可达（NFR-20）：全流程 Tab 聚焦 / Enter 提交 / Esc 整包关闭；
 *  - 记账：完成、单步跳过、整包关闭均即时经 sessions store 的
 *    setOnboarding → 既有 300ms 防抖 set_ui_state 通道落盘。
 */

import { Check, FolderPlus, X } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { cn } from "@/components/ui/cn";
import { Dialog } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { CheckableTag } from "@/components/ui/tag";
import { setLang, useT } from "@/i18n/core";
import { saveProject, selectDir } from "@/ipc/client";
import type { Language, ProviderModel } from "@/ipc/types";
import {
  afterCloseAll,
  afterComplete,
  afterStep,
  clampStep,
  ONBOARDING_TOTAL_STEPS,
  type OnboardingState,
  shouldShowResumeBanner,
} from "@/onboarding/onboarding-state";
import {
  allFieldsValid,
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
} from "@/settings/ProviderFormShared";
import { BUILTIN_PRESETS, newProviderFromPreset } from "@/settings/presets";
import { useSessionsStore } from "@/stores/sessions";
import { useSettingsStore } from "@/stores/settings";
import { pushToast } from "@/stores/toast";

/** 四步定义（stepper 展示面；step 序号即数组下标；label 为 i18n 键）。 */
const STEP_DEFS: { key: string; label: string }[] = [
  { key: "language", label: "onboarding.stepLanguage" },
  { key: "provider", label: "onboarding.stepProvider" },
  { key: "project", label: "onboarding.stepProject" },
  { key: "security", label: "onboarding.stepSecurity" },
];

/** 向导组件 props：open 是否显示；initialStep 续走步序（越界自动夹取）；
 *  onClose 关闭回调（父层卸载覆盖层）。 */
export interface OnboardingWizardProps {
  initialStep?: number;
  onClose: () => void;
}

/**
 * 首启引导向导：全屏覆盖层 + 顶部 stepper。父层以条件渲染挂载（每次打开
 * 全新挂载，步序从 initialStep 起算），关闭即卸载——主界面在底层始终可用。
 */
export function OnboardingWizard({ initialStep = 0, onClose }: OnboardingWizardProps) {
  const t = useT();
  const [step, setStep] = useState(() => clampStep(initialStep));

  /** 前进一步并记账（skipped 区分右上「跳过」与正常前进）；末步之后整包收口并关闭。 */
  function advance(skipped: boolean) {
    const store = useSessionsStore.getState();
    const current = store.onboarding ?? { completed: false, step };
    const next = afterStep(current, skipped, Date.now());
    store.setOnboarding(next);
    if (next.completed) {
      onClose();
      return;
    }
    setStep(next.step);
  }

  /** 顶部 × 整包关闭：记录当前步（completed=false），下次从该步续走。 */
  function closeAll() {
    const store = useSessionsStore.getState();
    store.setOnboarding(afterCloseAll(step, store.onboarding));
    onClose();
  }

  /** Esc 整包关闭（NFR-20；输入法组合键中的 Esc 不算）。 */
  function onKeyDown(e: React.KeyboardEvent) {
    if (e.key === "Escape" && !e.nativeEvent.isComposing) {
      e.preventDefault();
      closeAll();
    }
  }

  return (
    // 覆盖层带 role="dialog" 语义，键盘路径（Esc 关闭 / Tab 焦点循环由内部原生控件承担）
    <div
      className="fixed inset-0 z-50 overflow-y-auto bg-black/40"
      role="dialog"
      aria-modal="true"
      aria-label={t("onboarding.wizardAria")}
      data-testid="onboarding-wizard"
      onKeyDown={onKeyDown}
    >
      <div className="mx-auto mt-[6vh] mb-10 w-full max-w-2xl rounded-xl border border-[var(--border)] bg-[var(--cw-bg-main)] shadow-2xl">
        {/* 头部：stepper 进度 + 右上「跳过」（末步纯展示无跳过）与整包关闭 × */}
        <div className="flex items-center gap-3 border-b border-[var(--border)] px-5 py-3">
          <ol className="flex min-w-0 flex-1 items-center gap-1.5">
            {STEP_DEFS.map((def, index) => (
              <li key={def.key} className="flex min-w-0 items-center gap-1.5">
                <span
                  aria-current={index === step ? "step" : undefined}
                  className={cn(
                    "flex size-5 shrink-0 items-center justify-center rounded-full text-ui-caption font-medium",
                    index < step && "bg-[var(--primary)] text-[var(--primary-foreground)]",
                    index === step && "bg-[var(--foreground)] text-[var(--background)]",
                    index > step && "bg-[var(--muted)] text-[var(--muted-foreground)]",
                  )}
                >
                  {index < step ? <Check size={12} aria-hidden /> : index + 1}
                </span>
                <span
                  className={cn(
                    "hidden truncate text-ui-sm sm:inline",
                    index === step
                      ? "font-medium text-[var(--foreground)]"
                      : "text-[var(--muted-foreground)]",
                  )}
                >
                  {t(def.label)}
                </span>
                {index < STEP_DEFS.length - 1 && (
                  <span aria-hidden className="h-px w-4 shrink-0 bg-[var(--border)]" />
                )}
              </li>
            ))}
          </ol>
          {step < ONBOARDING_TOTAL_STEPS - 1 && (
            <button
              type="button"
              data-testid="onboarding-skip-step"
              className="shrink-0 rounded px-2 py-1 text-ui-sm text-[var(--muted-foreground)] hover:bg-[var(--accent)] hover:text-[var(--foreground)]"
              onClick={() => advance(true)}
            >
              {t("onboarding.skip")}
            </button>
          )}
          <button
            type="button"
            aria-label={t("onboarding.closeWizard")}
            data-testid="onboarding-close"
            className="flex size-6 shrink-0 items-center justify-center rounded text-[var(--muted-foreground)] hover:bg-[var(--accent)] hover:text-[var(--foreground)]"
            onClick={closeAll}
          >
            <X size={14} aria-hidden />
          </button>
        </div>

        {/* 步内容 */}
        <div className="px-5 py-4">
          {step === 0 && <LanguageStep onNext={() => advance(false)} />}
          {step === 1 && (
            <ProviderStep onForward={() => advance(false)} onSkip={() => advance(true)} />
          )}
          {step === 2 && <ProjectStep onNext={() => advance(false)} />}
          {step === 3 && (
            <SecurityStep
              onFinish={() => {
                useSessionsStore.getState().setOnboarding(afterComplete());
                onClose();
              }}
            />
          )}
        </div>
      </div>
    </div>
  );
}

/** 步骨架：标题 + 说明 + 正文 + 底部按钮行（各步共用布局）。 */
function StepFrame({
  title,
  description,
  children,
  footerLeft,
  footerRight,
}: {
  title: string;
  description?: string;
  children: React.ReactNode;
  footerLeft?: React.ReactNode;
  footerRight: React.ReactNode;
}) {
  return (
    <div>
      <h2 className="text-ui-lg font-semibold text-[var(--foreground)]">{title}</h2>
      {description && (
        <p className="mt-1 text-ui-sm text-[var(--muted-foreground)]">{description}</p>
      )}
      <div className="mt-4">{children}</div>
      <div className="mt-5 flex items-center justify-between gap-2">
        <span>{footerLeft}</span>
        <span className="flex items-center gap-2">{footerRight}</span>
      </div>
    </div>
  );
}

/** 语言选择按钮卡（设置页 ChoiceCard 的向导简化版：两选一）。 */
function LanguageCard({
  active,
  title,
  description,
  onClick,
}: {
  active: boolean;
  title: string;
  description: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      data-testid={`onboarding-language-${title}`}
      aria-pressed={active}
      onClick={onClick}
      className={cn(
        "flex-1 rounded-lg border px-4 py-3 text-left transition-colors",
        active
          ? "border-[var(--cw-focus)] bg-[var(--accent)]"
          : "border-[var(--border)] hover:bg-[var(--accent)]",
      )}
    >
      <span className="block text-ui-base font-medium text-[var(--foreground)]">{title}</span>
      <span className="mt-0.5 block text-ui-sm text-[var(--muted-foreground)]">{description}</span>
    </button>
  );
}

/** 第 1 步：语言。zh-CN / en-US 单选 → save_config 写 config.ui.language。
 *  31 号票 i18n 落地前界面文案维持中文，本步只落配置值。 */
function LanguageStep({ onNext }: { onNext: () => void }) {
  const t = useT();
  const config = useSettingsStore((s) => s.config);
  const [language, setLanguageState] = useState<Language>(config?.ui.language ?? "zh-CN");
  const [saving, setSaving] = useState(false);
  // 31 号票联动（§10.7 注记）：选择即切 i18n 内存态——向导自身与底层主界面
  // 立即换语言，落盘仍随「下一步」的 save_config 通道。
  function chooseLanguage(lang: Language) {
    setLanguageState(lang);
    setLang(lang);
  }

  async function saveAndNext() {
    setSaving(true);
    try {
      // 配置未加载（设置页从未打开过）先拉一次，再按草稿局部覆写整份提交
      const store = useSettingsStore.getState();
      if (!store.config) await store.load();
      const current = useSettingsStore.getState().config;
      if (!current) throw new Error(t("onboarding.configReadFailed"));
      useSettingsStore.getState().patchConfig({ ui: { ...current.ui, language } });
      const ok = await useSettingsStore.getState().saveAll();
      if (!ok) throw new Error(useSettingsStore.getState().error ?? t("onboarding.saveFailed"));
      onNext();
    } catch (err) {
      // 29 号票 Toast 已落地：失败直接轻提示（票面 console.warn 锚点收编）
      console.warn("[onboarding] 语言设置保存失败", err);
      pushToast(
        "error",
        t("toasts.languageSaveFailed"),
        err instanceof Error ? err.message : String(err),
      );
    } finally {
      setSaving(false);
    }
  }

  return (
    <StepFrame
      title={t("onboarding.langTitle")}
      description={t("onboarding.langDescription")}
      footerRight={
        <Button
          data-testid="onboarding-language-next"
          disabled={saving}
          onClick={() => void saveAndNext()}
        >
          {saving ? t("onboarding.saving") : t("onboarding.next")}
        </Button>
      }
    >
      <div className="flex flex-col gap-2 sm:flex-row">
        <LanguageCard
          active={language === "zh-CN"}
          title={t("onboarding.langZhName")}
          description={t("onboarding.langZhDesc")}
          onClick={() => chooseLanguage("zh-CN")}
        />
        <LanguageCard
          active={language === "en-US"}
          title={t("onboarding.langEnName")}
          description={t("onboarding.langEnDesc")}
          onClick={() => chooseLanguage("en-US")}
        />
      </div>
    </StepFrame>
  );
}

/** 弹框内模型编辑器状态（向导只有新增表单，target 恒为新增）。 */
type WizardModelEditorState = { value: ProviderModel; isNew: boolean; idError: boolean };

/**
 * 第 2 步：供应商与 Key。预设 chips + 共享供应商表单（与设置页单源）；
 * 「保存并测试」= save_provider（newKey 通道）→ test_provider 连通探测：
 *  - 连通 → 显示成功，主按钮转「下一步」；
 *  - 探测失败 → 红字分类文案 +「仍要保存并继续」与「跳过此步」并存
 *    （配置已落盘保留，对应验收"保存过一次再跳过 → 已存项保留"）；
 *  - test_provider 不可用（未注册等 reject）→ 降级提示「稍后在设置中测试」，
 *    不阻塞向导（软依赖）。
 */
function ProviderStep({ onForward, onSkip }: { onForward: () => void; onSkip: () => void }) {
  const t = useT();
  const config = useSettingsStore((s) => s.config);
  const [form, setForm] = useState<ProviderForm>(blankProvider);
  const [newKey, setNewKey] = useState("");
  const [touched, setTouched] = useState<Partial<Record<FieldKey, boolean>>>({});
  const [submitTried, setSubmitTried] = useState(false);
  const [modelEditor, setModelEditor] = useState<WizardModelEditorState | null>(null);
  // 连接测试三面：test 状态机 / unavailable（test_provider 不可用降级）/ saving 提交中
  const [test, setTest] = useState<ConnectionTestState>({ state: "idle" });
  const [testUnavailable, setTestUnavailable] = useState(false);
  const [saving, setSaving] = useState(false);
  // 向导只有新增语义：恒录入新 Key（凭据策略单源在共享层）
  const credential: CredentialState = { isNew: true, mode: "replace", newKey };
  const hasProviders = (config?.providers.length ?? 0) > 0;

  /** 字段错误（touched / 提交尝试后显示；模型与请求头仅由提交尝试触发）。 */
  function fieldError(key: FieldKey): string | undefined {
    const issue = fieldIssue(form, key, credential);
    if (!issue) return undefined;
    if (key === "models" || key === "headers") return submitTried ? issue : undefined;
    return touched[key] || submitTried ? issue : undefined;
  }

  /** 预设快捷填充（与设置页同源预设清单）：名称/端点/模型/本地标记一次填入。 */
  function applyPreset(slug: string) {
    const provider = newProviderFromPreset(slug);
    if (!provider) return;
    setForm((prev) => ({
      ...prev,
      id: provider.id,
      name: provider.name,
      apiFormat: provider.apiFormat,
      baseUrl: provider.baseUrl ?? "",
      models: provider.models,
      local: provider.local ?? false,
    }));
    setTouched((tp) => ({ ...tp, name: true, baseUrl: true, keys: true }));
  }

  /** 模型编辑器提交：模型 ID 空则亮红；写回表单。 */
  function commitModel() {
    if (!modelEditor) return;
    const model = { ...modelEditor.value, model: modelEditor.value.model.trim() };
    if (!model.model) {
      setModelEditor({ ...modelEditor, idError: true });
      return;
    }
    setForm((prev) => ({
      ...prev,
      models: modelEditor.isNew
        ? [...prev.models, model]
        : prev.models.map((x) => (x.id === model.id ? model : x)),
    }));
    setModelEditor(null);
  }

  /**
   * 保存并测试：校验 → upsertProvider（settings store，save_provider 的
   * newKey 通道即时落盘）→ test_provider。本地供应商免 Key（newKey 不传）。
   */
  async function saveAndTest() {
    setSubmitTried(true);
    if (!allFieldsValid(form, credential)) return;
    setSaving(true);
    const provider = formToProvider(form, credential);
    const ok = await useSettingsStore
      .getState()
      .upsertProvider(provider, form.local ? undefined : credential.newKey.trim() || undefined);
    if (!ok) {
      setSaving(false);
      const message = useSettingsStore.getState().error ?? t("onboarding.unknownError");
      console.warn("[onboarding] 供应商保存失败", message);
      pushToast("error", t("toasts.providerSaveFailed"), message);
      return;
    }
    // 探测失败以 ok=false 返回（写 error 槽）；reject（test_provider 未注册
    // 等参数面错误）→ onReject 接管为「稍后在设置中测试」降级，不阻塞向导
    await runConnectionTest(provider.id, undefined, setTest, () => {
      setTest({ state: "idle" }); // 复位探测态，避免停格在「测试中…」
      setTestUnavailable(true);
      return true;
    });
    setSaving(false);
  }

  const tested = test.state === "ok" || test.state === "error" || testUnavailable;

  return (
    <StepFrame
      title={t("onboarding.providerTitle")}
      description={t("onboarding.providerDescription")}
      footerLeft={
        tested ? undefined : (
          <span className="text-ui-sm text-[var(--muted-foreground)]">
            {t("onboarding.providerTestNote")}
          </span>
        )
      }
      footerRight={
        <>
          {test.state === "error" && (
            <Button variant="outline" data-testid="onboarding-provider-keep" onClick={onSkip}>
              {t("onboarding.providerSkipStep")}
            </Button>
          )}
          {test.state === "ok" || testUnavailable ? (
            <Button data-testid="onboarding-provider-next" onClick={onForward}>
              {t("onboarding.next")}
            </Button>
          ) : (
            <Button
              data-testid="onboarding-provider-save"
              disabled={saving}
              onClick={() => void saveAndTest()}
            >
              {saving
                ? t("onboarding.saving")
                : test.state === "error"
                  ? t("onboarding.providerSaveRetest")
                  : t("onboarding.providerSaveAndTest")}
            </Button>
          )}
        </>
      }
    >
      {hasProviders && (
        <p
          className="mb-3 text-ui-sm text-[var(--muted-foreground)]"
          data-testid="onboarding-provider-existing"
        >
          {t("onboarding.providerExisting", { count: config?.providers.length ?? 0 })}
        </p>
      )}
      {/* 预设快捷填充（19 号票同源清单）：本地三家标注「本地 / 离线可用」 */}
      <div className="mb-4 flex flex-wrap items-center gap-1.5">
        <span className="text-ui-sm text-[var(--cw-text-2)]">
          {t("onboarding.providerFromPreset")}
        </span>
        {BUILTIN_PRESETS.map((preset) => (
          <CheckableTag
            key={preset.slug}
            checked={form.name === preset.name && form.baseUrl === preset.baseUrl}
            onChange={() => applyPreset(preset.slug)}
          >
            {preset.name}
            {preset.local ? t("onboarding.presetLocal") : ""}
          </CheckableTag>
        ))}
      </div>
      <ProviderFields
        form={form}
        credential={credential}
        errors={{
          name: fieldError("name"),
          baseUrl: fieldError("baseUrl"),
          keys: fieldError("keys"),
          models: fieldError("models"),
          headers: fieldError("headers"),
        }}
        onPatch={(patch) => {
          setForm((prev) => ({ ...prev, ...patch }));
          for (const key of ["name", "baseUrl", "keys"] as const) {
            if (key in patch) setTouched((tp) => ({ ...tp, [key]: true }));
          }
        }}
        onNewKey={(value) => {
          setNewKey(value);
          setTouched((tp) => ({ ...tp, keys: true }));
        }}
      >
        <ModelListSection
          models={form.models}
          activeModelId={undefined}
          onAdd={() =>
            setModelEditor({ value: providerModelDefaults(), isNew: true, idError: false })
          }
          onEdit={(model) => setModelEditor({ value: { ...model }, isNew: false, idError: false })}
          onRemove={(model) =>
            setForm((prev) => ({
              ...prev,
              models: prev.models.filter((x) => x.id !== model.id),
            }))
          }
        />
      </ProviderFields>

      {/* 测试结果与降级提示（19 号票口径：七类分类人话文案） */}
      <div className="mt-2">
        <ConnectionTestResult test={test} />
        {testUnavailable && (
          <p
            className="py-1 text-ui-sm text-[var(--muted-foreground)]"
            data-testid="onboarding-provider-unavailable"
          >
            {t("onboarding.providerTestUnavailable")}
          </p>
        )}
      </div>

      {/* 模型编辑器弹框（同设置页：同框两态切换） */}
      <Dialog
        open={modelEditor !== null}
        title={modelEditor?.isNew ? t("onboarding.addModel") : t("onboarding.editModel")}
        onClose={() => setModelEditor(null)}
        width={640}
        actions={[
          {
            label: t("onboarding.backToProvider"),
            variant: "ghost",
            onClick: () => setModelEditor(null),
          },
          { label: t("onboarding.saveModel"), onClick: commitModel },
        ]}
      >
        {modelEditor && (
          <ModelEditor
            value={modelEditor.value}
            idError={modelEditor.idError}
            onChange={(patch) =>
              setModelEditor((prev) =>
                prev ? { ...prev, value: { ...prev.value, ...patch } } : prev,
              )
            }
          />
        )}
      </Dialog>
    </StepFrame>
  );
}

/** 第 3 步：打开/创建项目。select_dir 原生选目录 + 项目名（留空取目录名
 *  兜底）→ save_project；跳过 = 先用临时会话（数据目录 workspace/）。 */
function ProjectStep({ onNext }: { onNext: () => void }) {
  const t = useT();
  const [name, setName] = useState("");
  const [homeDir, setHomeDir] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function pick() {
    try {
      const dir = await selectDir();
      if (dir) {
        setHomeDir(dir);
        setError(null);
      }
    } catch (err) {
      console.warn("[onboarding] 目录选择失败", err);
    }
  }

  async function saveAndNext() {
    if (homeDir === null || saving) return;
    setSaving(true);
    setError(null);
    const fallback = homeDir.split("/").filter(Boolean).at(-1) ?? homeDir;
    try {
      await saveProject({ project: { name: name.trim() || fallback, homeDir } });
      // 左栏列表刷新（与左栏新建项目同款），让注册结果立即可见
      await useSessionsStore.getState().refresh();
      onNext();
    } catch (err) {
      console.warn("[onboarding] 项目注册失败", err);
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  }

  return (
    <StepFrame
      title={t("onboarding.projectTitle")}
      description={t("onboarding.projectDescription")}
      footerLeft={
        error && (
          <span className="text-ui-sm text-[var(--cw-err)]" data-testid="onboarding-project-error">
            {error}
          </span>
        )
      }
      footerRight={
        <Button
          data-testid="onboarding-project-save"
          disabled={homeDir === null || saving}
          onClick={() => void saveAndNext()}
        >
          {saving ? t("onboarding.saving") : t("onboarding.projectSaveAndContinue")}
        </Button>
      }
    >
      <div className="mb-4">
        <p className="mb-1.5 text-ui-base font-medium text-[var(--foreground)]">
          {t("onboarding.projectName")}
        </p>
        <Input
          data-testid="onboarding-project-name"
          value={name}
          placeholder={t("onboarding.projectNamePlaceholder")}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.nativeEvent.isComposing) {
              e.preventDefault();
              void saveAndNext();
            }
          }}
        />
      </div>
      <button
        type="button"
        data-testid="onboarding-project-pick"
        onClick={() => void pick()}
        className="flex w-full flex-col items-center gap-2 rounded-lg border border-dashed border-[var(--input)] px-4 py-6 transition-colors hover:border-[var(--cw-text-2)]"
      >
        <span className="flex size-12 items-center justify-center rounded-lg bg-[var(--muted)] text-[var(--muted-foreground)]">
          <FolderPlus size={20} />
        </span>
        {homeDir === null ? (
          <span className="text-ui-base font-medium text-[var(--foreground)]">
            {t("onboarding.chooseFolder")}
          </span>
        ) : (
          <span className="max-w-full truncate font-mono text-ui-sm text-[var(--foreground)]">
            {homeDir}
          </span>
        )}
        <span className="text-ui-sm text-[var(--muted-foreground)]">
          {homeDir === null ? t("onboarding.folderHint") : t("onboarding.chooseAgain")}
        </span>
      </button>
    </StepFrame>
  );
}

/** 四档权限档一句话说明（§8.5 矩阵从严到宽；与 Composer Shift+Tab 循环一致）。
 *  label/text 为 i18n 键（31 号票），渲染时查词。 */
const TIER_INTRO: { label: string; text: string }[] = [
  { label: "onboarding.tierPlanLabel", text: "onboarding.tierPlanText" },
  { label: "onboarding.tierConfirmEachLabel", text: "onboarding.tierConfirmEachText" },
  { label: "onboarding.tierAutoEditLabel", text: "onboarding.tierAutoEditText" },
  { label: "onboarding.tierFullAccessLabel", text: "onboarding.tierFullAccessText" },
];

/** 第 4 步：权限档与安全模式简介（纯静态说明，仅一个「完成」钮，无写操作）。 */
function SecurityStep({ onFinish }: { onFinish: () => void }) {
  const t = useT();
  return (
    <StepFrame
      title={t("onboarding.securityTitle")}
      description={t("onboarding.securityDescription")}
      footerRight={
        <Button data-testid="onboarding-finish" onClick={onFinish}>
          {t("onboarding.finish")}
        </Button>
      }
    >
      <div className="flex flex-col gap-4 text-ui-base text-[var(--foreground)]">
        <section>
          <h3 className="mb-1.5 text-ui-base font-medium">{t("onboarding.tierSectionTitle")}</h3>
          <ul className="flex flex-col gap-1">
            {TIER_INTRO.map((tier) => (
              <li key={tier.label} className="text-ui-sm text-[var(--muted-foreground)]">
                <span className="font-medium text-[var(--foreground)]">{t(tier.label)}</span>
                {" — "}
                {t(tier.text)}
              </li>
            ))}
          </ul>
        </section>
        <section>
          <h3 className="mb-1.5 text-ui-base font-medium">
            {t("onboarding.approvalSectionTitle")}
          </h3>
          <p className="text-ui-sm text-[var(--muted-foreground)]">
            {t("onboarding.approvalSectionBody")}
          </p>
        </section>
        <section>
          <h3 className="mb-1.5 text-ui-base font-medium">
            {t("onboarding.planGateSectionTitle")}
          </h3>
          <p className="text-ui-sm text-[var(--muted-foreground)]">
            {t("onboarding.planGateSectionBody")}
          </p>
        </section>
        <section>
          <h3 className="mb-1.5 text-ui-base font-medium">
            {t("onboarding.adjustableSectionTitle")}
          </h3>
          <p className="text-ui-sm text-[var(--muted-foreground)]">
            {t("onboarding.adjustableSectionBody")}
          </p>
        </section>
      </div>
    </StepFrame>
  );
}

/**
 * 「继续初始配置」横幅（主窗卡头上方常驻细条）：初始配置未完成时显示，
 * 点击从记录步续走向导；完成后随 completed=true 自动消失。
 * 渲染判定三态（票面测试口径）：无记录不渲染（首启走自动弹）、
 * 未完成渲染、已完成不渲染。
 */
export function OnboardingResumeBanner({
  state,
  onOpen,
}: {
  state: OnboardingState | null;
  onOpen: () => void;
}) {
  const t = useT();
  if (!shouldShowResumeBanner(state)) return null;
  return (
    <div
      data-testid="onboarding-resume-banner"
      className="flex shrink-0 items-center gap-2 border-b border-[var(--border)] bg-[var(--accent)]/50 px-4 py-1.5 text-ui-sm"
    >
      <span className="text-[var(--muted-foreground)]">{t("onboarding.bannerPending")}</span>
      <button
        type="button"
        data-testid="onboarding-resume-open"
        className="font-medium text-[var(--foreground)] underline-offset-2 hover:underline"
        onClick={onOpen}
      >
        {t("onboarding.bannerResume")}
      </button>
    </div>
  );
}

/** 续走步序（SessionWorkspace 打开向导时取）：记录存在取记录步（夹取），无记录从头。 */
export function resumeStep(state: OnboardingState | null): number {
  return state === null ? 0 : clampStep(state.step);
}
