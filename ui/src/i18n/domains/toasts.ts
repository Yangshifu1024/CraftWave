/**
 * toasts 域字典（31 号票）：顶部 Toast 反馈层（29 号票）的用户可见文案。
 * pushToast 调用点多在非组件场景（事件桥/纯函数/回调），直接用模块级
 * t() 查词——入队时求值，语言切换后的新 Toast 即新语言。
 */

const zh = {
  /** 导出（27 号票，左栏右键菜单入口）。 */
  exportMarkdownDone: "已导出 Markdown",
  exportMarkdownFailed: "导出 Markdown 失败",
  exportJsonlDone: "已导出 JSONL",
  exportJsonlFailed: "导出 JSONL 失败",
  /** 全局热键冲突（29 号票，hotkey.ts）。 */
  hotkeyOccupied: "快捷键被其他应用占用",
  hotkeyOccupiedDetail: "{{accelerator}}（{{message}}）；可改用其他组合键或关闭开关",
  /** 设置保存（SettingsPage）。 */
  settingsSaved: "设置已保存",
  settingsSaveFailed: "设置保存失败",
  /** 上下文压缩失败（04 号票 → 29 号票 Toast 收敛，event-bridge.ts）。 */
  compactFailed: "上下文压缩失败",
  compactFailedDetail: "本次对话已继续（如可容纳）；可在右侧面板重试压缩，或开启新会话",
  /** start_chat 命令失败（runtime.ts）。 */
  sendFailed: "发送失败",
  /** 首启向导保存失败（OnboardingWizard）。 */
  languageSaveFailed: "语言设置保存失败",
  providerSaveFailed: "供应商保存失败",
};

/** en 全量翻译：键集合编译期同构。 */
const en: Record<keyof typeof zh, string> = {
  exportMarkdownDone: "Exported Markdown",
  exportMarkdownFailed: "Markdown export failed",
  exportJsonlDone: "Exported JSONL",
  exportJsonlFailed: "JSONL export failed",
  hotkeyOccupied: "Shortcut is taken by another app",
  hotkeyOccupiedDetail: "{{accelerator}} ({{message}}); choose another combination or turn it off",
  settingsSaved: "Settings saved",
  settingsSaveFailed: "Failed to save settings",
  compactFailed: "Context compaction failed",
  compactFailedDetail:
    "This conversation continued (if it fits); retry compaction from the right panel, or start a new session",
  sendFailed: "Failed to send",
  languageSaveFailed: "Failed to save language setting",
  providerSaveFailed: "Failed to save provider",
};

export const toastsDomain = { name: "toasts", zh, en };
