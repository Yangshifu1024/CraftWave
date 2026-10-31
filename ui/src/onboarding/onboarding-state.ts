/**
 * 首启引导状态（30 号票，FR-13-18）：schema 定义、不可信数据清洗与判定
 * 纯函数——存放于 ui-state（§3.3 #30/#31，前端自管 JSON 域，无 Rust 改动）。
 *
 * 存储形态（票内即定义）：`onboarding: { completed, step, skippedAt? }`
 *  - completed：四步是否走完（含末步跳过——末步之后无续走目标，跳过即收口）；
 *  - step：下次续走应落的步序（0 起；四步 = 语言 0 / 供应商 1 / 项目 2 / 安全简介 3）；
 *  - skippedAt：最后一次单步跳过的时间戳（Unix 毫秒，诊断面，不参与判定）。
 *
 * 判定口径（票面）：
 *  - 无 onboarding 记录（全新数据目录）→ 自动弹向导；
 *  - completed=true → 不再自动弹，「继续配置」横幅消失；
 *  - completed=false → 横幅常驻，点击从记录 step 续走。
 * ui-state 是前端自管 JSON（形状不可信），读入前先经 sanitizeOnboarding 清洗。
 */

/** 向导总步数（语言 → 供应商与 Key → 项目 → 权限档与安全简介）。 */
export const ONBOARDING_TOTAL_STEPS = 4;

/** 首启引导持久化状态（ui-state.onboarding 字段的形状）。 */
export interface OnboardingState {
  completed: boolean;
  step: number;
  /** 最后一次单步跳过的时间戳（Unix 毫秒；诊断面，不参与判定）。 */
  skippedAt?: number;
}

/**
 * 清洗 ui-state 里读出的 onboarding 记录（前端自管 JSON，形状不可信）：
 * 字段缺失/类型不符整条丢弃（返回 null，等价无记录）；step 非数字回 0、
 * 越界夹取到合法范围；记录本体不存在（全新数据目录）返回 null（= 首启
 * 自动弹的判定输入）。
 */
export function sanitizeOnboarding(raw: unknown): OnboardingState | null {
  if (typeof raw !== "object" || raw === null) return null;
  const src = raw as Record<string, unknown>;
  if (typeof src.completed !== "boolean") return null;
  const step = typeof src.step === "number" && Number.isFinite(src.step) ? Math.trunc(src.step) : 0;
  const skippedAt =
    typeof src.skippedAt === "number" && Number.isFinite(src.skippedAt) ? src.skippedAt : undefined;
  return {
    completed: src.completed,
    step: clampStep(step),
    ...(skippedAt !== undefined ? { skippedAt } : {}),
  };
}

/** 步序夹取：0 ≤ step ≤ 总步数-1（负数回 0 从头走，超界夹到末步）。 */
export function clampStep(step: number): number {
  if (!Number.isFinite(step)) return 0;
  return Math.min(Math.max(Math.trunc(step), 0), ONBOARDING_TOTAL_STEPS - 1);
}

/** 是否应自动弹向导：无记录（全新数据目录）= 弹；其余只看 completed。 */
export function shouldAutoOpen(state: OnboardingState | null): boolean {
  return state === null;
}

/** 是否显示「继续初始配置」横幅：存在记录、未完成时显示。 */
export function shouldShowResumeBanner(state: OnboardingState | null): boolean {
  return state !== null && !state.completed;
}

/**
 * 前进一步后的下一状态（单步跳过与正常前进共用同一推进语义，skipped
 * 区分是否打跳过时间戳）；跳过/前进末步（其后无续走目标）即整包
 * completed=true——否则会落出"重开直接在末步且横幅常驻"的怪状态。
 */
export function afterStep(state: OnboardingState, skipped: boolean, now: number): OnboardingState {
  const nextStep = state.step + 1;
  const mark = skipped ? { skippedAt: now } : {};
  if (nextStep >= ONBOARDING_TOTAL_STEPS) {
    return { completed: true, step: ONBOARDING_TOTAL_STEPS - 1, ...mark };
  }
  return { completed: false, step: nextStep, ...mark };
}

/** 走完（末步「完成」钮）后的下一状态：completed 收口，横幅随之消失。 */
export function afterComplete(): OnboardingState {
  return { completed: true, step: ONBOARDING_TOTAL_STEPS - 1 };
}

/**
 * 整包关闭（顶部 ×，未走完即弃）后的下一状态：completed=false、step 保持
 * 当前步（重开续走）；已有记录的 skippedAt 保留，无记录则不带。
 */
export function afterCloseAll(step: number, prior: OnboardingState | null): OnboardingState {
  return {
    completed: false,
    step: clampStep(step),
    ...(prior?.skippedAt !== undefined ? { skippedAt: prior.skippedAt } : {}),
  };
}
