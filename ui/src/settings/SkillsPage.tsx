/**
 * 设置页技能管理组（§10.7 M2 清单，09 号票）：skill_list 列表（名称 /
 * 来源徽标：内置·全局·项目 / 信任状态 / 启停开关 / 一句话描述）、「重新
 * 扫描」按钮（skill_reload）、行点击 skill_inspect 弹 SKILL.md 正文预览
 * （Markdown 只读，Dialog 承载）。24 号票增信任撤销：已「记住」的技能
 * 行内「撤销信任」按钮（skill_revoke_trust 写 revoke 记录，下次运行
 * 重新询问）。
 *
 * 数据面说明（票内定案，与 08 号票 McpPage 同精神）：技能清单不属 AppConfig
 * （无 config 段），生命周期独立于设置页 saveAll 草稿——数据放组件内
 * useState，启停经 skill_toggle 即时落盘（skills-state.json，后端权威）。
 */

import { LoaderCircle, ShieldOff, Zap } from "lucide-react";
import { useEffect, useState } from "react";
import { Markdown } from "@/chat/Markdown";
import { Alert } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Dialog } from "@/components/ui/dialog";
import { Switch } from "@/components/ui/switch";
import { skillInspect, skillList, skillReload, skillRevokeTrust, skillToggle } from "@/ipc/client";
import type { SkillListEntry } from "@/ipc/types";

/** 来源徽标（§7.7 覆盖优先级 内置 < 全局 < 项目；合并视图只显示胜出来源）。 */
const SOURCE_LABEL: Record<SkillListEntry["source"], string> = {
  builtin: "内置",
  global: "全局",
  project: "项目",
};

/** 来源徽标配色（内置=中性、全局=蓝、项目=绿；与 McpPage 徽标同视觉档）。 */
const SOURCE_CLASS: Record<SkillListEntry["source"], string> = {
  builtin: "border-[var(--border)] text-[var(--muted-foreground)]",
  global: "border-sky-500/40 text-sky-500",
  project: "border-emerald-500/40 text-emerald-500",
};

/**
 * 技能管理页（设置页 skills 分节的内容体）。
 */
export function SkillsPage() {
  const [skills, setSkills] = useState<SkillListEntry[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [reloading, setReloading] = useState(false);
  // 预览弹窗（skill_inspect 结果；loading 态防重复点击）
  const [preview, setPreview] = useState<{ name: string; markdown: string } | null>(null);
  const [previewLoading, setPreviewLoading] = useState<string | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);

  /** 拉清单（挂载与重扫共用）。 */
  const refresh = () => {
    setLoadError(null);
    skillList()
      .then((entries) => setSkills(entries))
      .catch((err: unknown) => {
        setSkills([]);
        setLoadError(err instanceof Error ? err.message : String(err));
      });
  };

  // biome-ignore lint/correctness/useExhaustiveDependencies: 挂载时拉一次清单（refresh 内部只用 setState，无闭包过期面）
  useEffect(() => {
    refresh();
  }, []);

  /** 启停切换（即时落盘，FR-14-04）：本地先行翻转 + 失败回滚。 */
  const toggle = (name: string, enabled: boolean) => {
    setSkills((prev) => prev?.map((s) => (s.name === name ? { ...s, enabled } : s)) ?? prev);
    skillToggle(name, enabled).catch(() => {
      // 失败回滚（按钮态以后端为权威，翻转回来）
      setSkills(
        (prev) => prev?.map((s) => (s.name === name ? { ...s, enabled: !enabled } : s)) ?? prev,
      );
    });
  };

  /** 信任撤销（24 号票，§9.8）：写 revoke 记录后刷新清单（信任状态列
   *  以后端为权威）。失败提示（撤销不生效会让用户误以为已脱险）。 */
  const revokeTrust = (name: string) => {
    skillRevokeTrust({ name })
      .then(refresh)
      .catch((err: unknown) => {
        setLoadError(
          `技能 ${name} 信任撤销失败：${err instanceof Error ? err.message : String(err)}`,
        );
      });
  };

  /** 重扫（skill_reload）：后端重建索引后重拉清单。 */
  const reload = () => {
    setReloading(true);
    skillReload()
      .then(refresh)
      .catch((err: unknown) => {
        setLoadError(err instanceof Error ? err.message : String(err));
      })
      .finally(() => setReloading(false));
  };

  /** 行点击预览（skill_inspect，SKILL.md 正文 Markdown 只读）。 */
  const inspect = (name: string) => {
    setPreviewLoading(name);
    setPreviewError(null);
    skillInspect({ name })
      .then((result) => setPreview({ name, markdown: result.markdown }))
      .catch((err: unknown) => {
        setPreviewError(
          `技能 ${name} 预览失败：${err instanceof Error ? err.message : String(err)}`,
        );
      })
      .finally(() => setPreviewLoading(null));
  };

  return (
    <div>
      <section className="settings-section">
        <div className="flex items-center justify-between px-1 pb-2">
          <p className="text-ui-sm text-[var(--muted-foreground)]">
            技能教 AI 干专项活：模型平时只看到一句话简介，用到才加载正文。目录格式兼容 Claude
            技能（一个技能一个目录一份 SKILL.md）；同名技能按 内置 &lt; 全局 &lt; 项目 覆盖。
          </p>
          <Button
            variant="outline"
            size="sm"
            data-testid="skill-reload"
            disabled={reloading}
            onClick={reload}
            className="ml-3 shrink-0"
          >
            {reloading && <LoaderCircle size={12} aria-hidden className="mr-1 animate-spin" />}
            重新扫描
          </Button>
        </div>
        {loadError && <Alert variant="error" title={`技能清单加载失败：${loadError}`} />}
        {skills === null ? (
          <div className="settings-section-card px-5 py-10 text-center text-ui-base text-[var(--muted-foreground)]">
            加载中…
          </div>
        ) : skills.length === 0 ? (
          <div className="settings-section-card px-5 py-10 text-center text-ui-base text-[var(--muted-foreground)]">
            还没有安装任何技能。把技能目录放到 <code>~/.craftwave/skills/</code>
            （全局）或项目 <code>&lt;项目主目录&gt;/.craftwave/skills/</code>
            （本项目），点「重新扫描」即可识别。
          </div>
        ) : (
          <div className="settings-section-card py-1" data-testid="skill-list">
            {skills.map((skill) => (
              <div
                key={skill.name}
                data-testid="skill-row"
                className="flex items-center gap-3 border-t border-[var(--border)] px-4 py-3 first:border-t-0"
              >
                <Zap size={14} aria-hidden className="shrink-0 text-[var(--cw-dim)]" />
                <div className="flex min-w-0 flex-1 flex-col gap-0.5">
                  <div className="flex min-w-0 items-center gap-2">
                    <button
                      type="button"
                      data-testid={`skill-name-${skill.name}`}
                      className="truncate text-ui-base font-medium text-[var(--foreground)] hover:underline"
                      onClick={() => inspect(skill.name)}
                      title="预览 SKILL.md 正文"
                    >
                      {skill.name}
                    </button>
                    <span
                      data-testid={`skill-source-${skill.name}`}
                      className={`shrink-0 rounded-full border px-1.5 text-ui-sm leading-4 ${SOURCE_CLASS[skill.source]}`}
                    >
                      {SOURCE_LABEL[skill.source]}
                    </span>
                    {previewLoading === skill.name && (
                      <LoaderCircle size={12} aria-hidden className="shrink-0 animate-spin" />
                    )}
                  </div>
                  <p className="truncate text-ui-sm text-[var(--muted-foreground)]">
                    {skill.description}
                  </p>
                </div>
                {skill.trustStatus === "trusted" && skill.source !== "builtin" && (
                  <button
                    type="button"
                    data-testid={`skill-revoke-trust-${skill.name}`}
                    title="撤销信任（下次运行重新询问）"
                    aria-label={`撤销技能 ${skill.name} 的信任`}
                    onClick={() => revokeTrust(skill.name)}
                    className="flex shrink-0 items-center gap-1 rounded-full border border-[var(--border)] px-2 py-0.5 text-ui-sm text-[var(--muted-foreground)] hover:border-[var(--destructive)] hover:text-[var(--destructive)]"
                  >
                    <ShieldOff size={12} aria-hidden />
                    撤销信任
                  </button>
                )}
                <Switch
                  aria-label={`启用技能 ${skill.name}`}
                  data-testid={`skill-toggle-${skill.name}`}
                  checked={skill.enabled}
                  onCheckedChange={(v) => toggle(skill.name, v)}
                />
              </div>
            ))}
          </div>
        )}
        {previewError && <Alert variant="error" title={previewError} />}
      </section>

      {/* 正文预览弹窗（SKILL.md，frontmatter 已由后端剥离） */}
      <Dialog
        open={preview !== null}
        title={`技能 ${preview?.name ?? ""} · SKILL.md`}
        onClose={() => setPreview(null)}
        actions={[{ label: "关闭", onClick: () => setPreview(null) }]}
      >
        <div
          data-testid="skill-preview"
          className="max-h-[60vh] min-w-0 overflow-auto rounded border border-[var(--border)] p-3"
        >
          <Markdown>{preview?.markdown ?? ""}</Markdown>
        </div>
      </Dialog>
    </div>
  );
}
