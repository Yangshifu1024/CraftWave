/** 只改变字阶基准；根字号、间距、图标和圆角保持原有尺寸。 */
export function applyUiFontSize(size = 14): void {
  const normalized = Number.isFinite(size) ? Math.min(18, Math.max(12, Math.round(size))) : 14;
  document.documentElement.style.setProperty("--ui-font-size", `${normalized}px`);
}
