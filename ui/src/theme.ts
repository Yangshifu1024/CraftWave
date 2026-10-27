/**
 * 主题应用（FR-13-03，17 号票）：light / dark / system 三态 →
 * `documentElement.classList` 的 `dark` 开关（styles.css 的 :root/.dark
 * 两套设计令牌据此翻转）。
 *
 * system 态跟随操作系统外观（matchMedia 监听，变化即时生效）。
 * 调用方：App 启动（读 config）、设置页（草稿即时预览，丢弃回滚）。
 */

export type ThemeMode = "light" | "dark" | "system";

let mediaQuery: MediaQueryList | null = null;
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
