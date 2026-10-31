/**
 * 主题应用：把 light（浅色）/ dark（深色）/ system（跟随系统）
 * 三种主题模式落到页面上。
 *
 * 实现方式：在 <html> 根元素上切换 dark 类名。styles.css 里准备了 :root 与
 * .dark 两套"设计令牌"——设计令牌（design token）指把颜色等视觉常量定义成
 * CSS 变量（如 --background、--card），切换类名后整套变量随之翻转，界面即换肤。
 *
 * system 态跟随操作系统外观：用 matchMedia 监听系统深浅色变化，变化即时生效。
 * 调用方：App 启动时（读取已保存配置并应用）、设置页（草稿即时预览，丢弃草稿
 * 时随基线回滚）。
 */

/** 主题三态：浅色 / 深色 / 跟随操作系统外观。 */
export type ThemeMode = "light" | "dark" | "system";

/** 系统深色偏好媒体查询的缓存（首次 applyTheme 时创建，之后复用同一个监听）。 */
let mediaQuery: MediaQueryList | null = null;
/** 当前已应用的模式（系统外观变化时用它重新走一遍 applyTheme 归位）。 */
let currentMode: ThemeMode = "system";

/** 解析当前模式下是否应启用暗色令牌。 */
function resolveDark(mode: ThemeMode): boolean {
  if (mode === "dark") return true;
  if (mode === "light") return false;
  return mediaQuery?.matches ?? false;
}

/** 应用主题模式（幂等；重复调用同值无副作用）。 */
export function applyTheme(mode: ThemeMode): void {
  currentMode = mode;
  if (mediaQuery === null && typeof matchMedia === "function") {
    mediaQuery = matchMedia("(prefers-color-scheme: dark)");
    // system 态下操作系统外观切换时保持跟随（其他态 listeners 触发后按 resolveDark 归位）
    mediaQuery.addEventListener("change", () => applyTheme(currentMode));
  }
  document.documentElement.classList.toggle("dark", resolveDark(mode));
}
