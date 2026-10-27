/**
 * 数字输入（shadcn Input 的数值变体）：type=number + min/step，
 * 失焦归位越界值；onChange 恒给 number（空输入回退 fallback）。
 */
import type { ChangeEvent } from "react";
import { cn } from "./cn";
import { Input } from "./input";

export interface NumberInputProps {
  value: number;
  onValueChange: (value: number) => void;
  min?: number;
  step?: number;
  fallback: number;
  "aria-label"?: string;
  className?: string;
}

export function NumberInput({
  value,
  onValueChange,
  min,
  step,
  fallback,
  className,
  ...aria
}: NumberInputProps) {
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
