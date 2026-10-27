/**
 * 数字输入（shadcn Input 的数值变体）：type=number + min/step。
 * 失焦归位越界值（低于下限抬回下限）；onChange 恒给 number（空输入回退 fallback）。
 */
import type { ChangeEvent } from "react";
import { cn } from "./cn";
import { Input } from "./input";

/** NumberInput 的 props：value 当前数值；onValueChange 数值回调；
 *  min 下限（失焦校验）；step 步进；fallback 空输入回退值；aria-label 无障碍名。 */
export interface NumberInputProps {
  value: number;
  onValueChange: (value: number) => void;
  min?: number;
  step?: number;
  fallback: number;
  "aria-label"?: string;
  className?: string;
}

/** 渲染数字输入框：把原生的字符串事件翻译成稳定的 number 回调。 */
export function NumberInput({
  value,
  onValueChange,
  min,
  step,
  fallback,
  className,
  ...aria
}: NumberInputProps) {
  /** 输入变化：空串给 fallback；可解析的数字才上抛（保持回调签名恒为 number）。 */
  function handleChange(e: ChangeEvent<HTMLInputElement>) {
    const raw = e.target.value;
    if (raw === "") {
      onValueChange(fallback);
      return;
    }
    const parsed = Number(raw);
    if (Number.isFinite(parsed)) onValueChange(parsed);
  }
  return (
    <Input
      type="number"
      inputMode="numeric"
      className={cn("w-[220px] max-w-full", className)}
      value={value}
      min={min}
      step={step}
      onChange={handleChange}
      onBlur={(e) => {
        // 失焦归位：低于下限的值抬回下限（与 CodeWave InputNumber min 行为一致）
        const parsed = Number(e.target.value);
        if (min !== undefined && Number.isFinite(parsed) && parsed < min) onValueChange(min);
      }}
      {...aria}
    />
  );
}
