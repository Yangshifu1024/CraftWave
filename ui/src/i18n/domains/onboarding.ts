/**
 * onboarding 域字典：首启引导向导（四步向导）的全部
 * 用户可见文案——stepper 步名、跳过/关闭、语言步、供应商步（含预设与
 * 测试降级提示）、项目步、安全简介步、续走横幅。语言步选择即经 setLang
 * 即时切换界面语言（与设置页语言设置的联动点）。
 */

const zh = {
  wizardAria: "初始配置向导",
  skip: "跳过",
  closeWizard: "关闭向导",
  stepLanguage: "语言",
  stepProvider: "供应商与 Key",
  stepProject: "项目",
  stepSecurity: "安全简介",
  next: "下一步",
  saving: "保存中…",
  configReadFailed: "配置读取失败",
  saveFailed: "保存失败",
  // ---- 语言步 ----
  langTitle: "选择界面语言",
  langDescription: "设置界面与提示文案的显示语言；后续可在设置中随时更改。",
  langZhName: "简体中文",
  langZhDesc: "界面与提示使用中文。",
  langEnName: "English",
  langEnDesc: "Use English for the interface.",
  // ---- 供应商步 ----
  providerTitle: "配置 AI 供应商",
  providerDescription: "填一个模型端点与 API Key 即可开始对话；也可以先跳过，稍后在设置中配置。",
  providerTestNote: "保存时会用你填的 Key 实测一次连通性。",
  providerSkipStep: "跳过此步",
  providerSaveRetest: "仍要保存并重测",
  providerSaveAndTest: "保存并测试",
  providerExisting: "已配置 {{count}} 个供应商，可直接跳过本步。",
  providerFromPreset: "从预设填充：",
  presetLocal: " · 本地",
  providerTestUnavailable: "连接测试暂不可用，供应商已保存；可稍后在设置中测试。",
  addModel: "添加模型",
  editModel: "编辑模型",
  backToProvider: "返回供应商",
  saveModel: "保存模型",
  unknownError: "未知错误",
  // ---- 项目步 ----
  projectTitle: "打开或创建项目",
  projectDescription:
    "把一个文件夹注册为项目，AI 在其中读写文件；跳过则先用临时会话（数据目录内置工作区）。",
  projectSaveAndContinue: "保存并继续",
  projectName: "项目名称",
  projectNamePlaceholder: "留空则取文件夹名",
  chooseFolder: "选择文件夹",
  folderHint: "CraftWave 可读取和编辑此文件夹",
  chooseAgain: "点击重新选择",
  // ---- 安全简介步 ----
  securityTitle: "权限与安全简介",
  securityDescription: "花 30 秒了解 CraftWave 如何守安全；全部设置稍后都可在设置页调整。",
  finish: "完成",
  tierSectionTitle: "权限档（输入框处按 Shift+Tab 循环切换）",
  tierPlanLabel: "计划",
  tierPlanText: "AI 只产出计划不动手，你批准后才执行。",
  tierConfirmEachLabel: "逐项确认",
  tierConfirmEachText: "每个工具操作都先询问你，逐项放行。",
  tierAutoEditLabel: "自动编辑",
  tierAutoEditText: "默认档：读写项目内文件自动执行，命令等敏感操作仍先询问。",
  tierFullAccessLabel: "完全信任",
  tierFullAccessText: "全部操作自动执行，仅灾难级命令仍被硬拦。",
  approvalSectionTitle: "审批弹窗",
  approvalSectionBody:
    "执行命令、写项目外文件等敏感操作会弹出审批卡：可以允许一次、本会话始终允许或拒绝；常用只读命令可勾选「加入白名单」免审批。关闭应用前未决审批不会自动执行。",
  planGateSectionTitle: "计划门",
  planGateSectionBody:
    "「计划」档位下 AI 先给出任务计划，你点击批准后它才动手；计划里声明的文件范围会被记住，超出范围再写会重新询问。设置页可开启严格计划模式（无批准计划时拒绝写入）。",
  adjustableSectionTitle: "随时可调",
  adjustableSectionBody:
    "左栏底部「设置」可更改语言、供应商与 Key、网络代理、安全策略、MCP 与技能等全部配置。",
  // ---- 续走横幅 ----
  bannerPending: "初始配置还没完成。",
  bannerResume: "继续初始配置 →",
};

/** en 全量翻译：键集合编译期同构。 */
const en: Record<keyof typeof zh, string> = {
  wizardAria: "Initial setup wizard",
  skip: "Skip",
  closeWizard: "Close wizard",
  stepLanguage: "Language",
  stepProvider: "Provider & Key",
  stepProject: "Project",
  stepSecurity: "Security intro",
  next: "Next",
  saving: "Saving…",
  configReadFailed: "Failed to read configuration",
  saveFailed: "Failed to save",
  langTitle: "Choose interface language",
  langDescription:
    "The display language for the interface and messages; you can change it anytime in Settings.",
  langZhName: "简体中文",
  langZhDesc: "界面与提示使用中文。",
  langEnName: "English",
  langEnDesc: "Use English for the interface.",
  providerTitle: "Configure an AI provider",
  providerDescription:
    "Enter a model endpoint and API key to start chatting; you can also skip and configure later in Settings.",
  providerTestNote: "Saving performs a real connectivity test with the key you entered.",
  providerSkipStep: "Skip this step",
  providerSaveRetest: "Save and retest anyway",
  providerSaveAndTest: "Save and test",
  providerExisting: "{{count}} provider(s) already configured; you can skip this step.",
  providerFromPreset: "Fill from preset:",
  presetLocal: " · local",
  providerTestUnavailable:
    "Connection testing is unavailable; the provider was saved — test later in Settings.",
  addModel: "Add model",
  editModel: "Edit model",
  backToProvider: "Back to provider",
  saveModel: "Save model",
  unknownError: "unknown error",
  projectTitle: "Open or create a project",
  projectDescription:
    "Register a folder as a project for the AI to read and write in; skip to start with a temporary session (built-in workspace in the data directory).",
  projectSaveAndContinue: "Save and continue",
  projectName: "Project name",
  projectNamePlaceholder: "Leave empty to use the folder name",
  chooseFolder: "Choose a folder",
  folderHint: "CraftWave can read and edit this folder",
  chooseAgain: "Click to choose again",
  securityTitle: "Permissions & safety intro",
  securityDescription:
    "Take 30 seconds to see how CraftWave stays safe; everything can be adjusted later in Settings.",
  finish: "Finish",
  tierSectionTitle: "Permission tiers (Shift+Tab in the composer to cycle)",
  tierPlanLabel: "Plan",
  tierPlanText: "The AI only produces a plan and waits; it executes after your approval.",
  tierConfirmEachLabel: "Confirm each",
  tierConfirmEachText: "Every tool operation asks you first, one by one.",
  tierAutoEditLabel: "Auto edit",
  tierAutoEditText:
    "Default: reading and writing files inside the project runs automatically; sensitive operations like commands still ask first.",
  tierFullAccessLabel: "Full trust",
  tierFullAccessText:
    "Everything runs automatically; only catastrophic commands remain hard-blocked.",
  approvalSectionTitle: "Approval dialogs",
  approvalSectionBody:
    'Sensitive operations (running commands, writing outside the project) pop up an approval card: allow once, always allow this session, or deny; common read-only commands can be ticked "Add to allowlist" to skip approval. Pending approvals are not auto-executed before the app closes.',
  planGateSectionTitle: "Plan gate",
  planGateSectionBody:
    'In the "Plan" tier the AI first presents a task plan and only acts after you approve it; the file scope declared in the plan is remembered, and writes beyond it ask again. Strict plan mode (reject writes without an approved plan) can be enabled in Settings.',
  adjustableSectionTitle: "Adjustable anytime",
  adjustableSectionBody:
    'The "Settings" entry at the bottom of the sidebar changes language, providers and keys, network proxy, security policy, MCP, skills, and every other setting.',
  bannerPending: "Initial setup is not finished yet.",
  bannerResume: "Resume setup →",
};

export const onboardingDomain = { name: "onboarding", zh, en };
