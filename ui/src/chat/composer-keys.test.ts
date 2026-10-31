// composer-keys 纯逻辑测试（@ 触发检测与插入；/ 技能触发
// 检测、技能插入形态与清单过滤；$ 角色触发符；@ 引号路径）。
import { describe, expect, it } from "vitest";
import {
  detectDollarQuery,
  filterRoles,
  insertDollarInvocation,
  SUBAGENT_ROLES,
} from "@/chat/composer-keys";
import type { SkillListEntry } from "@/ipc/types";
import {
  detectAtQuery,
  detectSlashQuery,
  filterSkills,
  insertAtCompletion,
  insertSkillInvocation,
} from "./composer-keys";

describe("触发符检测（@ / $ 同构光标锚定；含引号路径）", () => {
  it("@ 检测：未闭合触发、间隔空白终止、取最近触发位、引号路径回扫", () => {
    // 光标前未闭合的 @ 返回插入位与查询词（光标锚定）
    const text = "看一下 @src/ma";
    expect(detectAtQuery(text, text.length)).toEqual({ at: 4, query: "src/ma" });
    // @ 与光标间隔空白 → 非触发态
    expect(detectAtQuery("看看 @src 之后", 9)).toBeNull();
    // 无 @ 或光标在 @ 前 → null
    expect(detectAtQuery("普通文本", 4)).toBeNull();
    expect(detectAtQuery("@src", 0)).toBeNull();
    // 多个触发位时取最近的 @
    expect(detectAtQuery("@a 起 @b", 7)).toEqual({ at: 5, query: "b" });

    // ---- 引号路径：未闭合引号回扫 ----
    // 引号内空白不终止触发（`@"a b/c` 输入中）
    expect(detectAtQuery('看 @"a b/c', 10)).toEqual({ at: 2, query: '"a b/c' });
    // 闭合引号即止：完整引用形态后不再触发
    expect(detectAtQuery('看 @"a b/c.txt"', 15)).toBeNull();
    // 完整引用后的后续输入照常触发
    expect(detectAtQuery('看 @"a b/c.txt" 然后 @', 21)).toEqual({ at: 18, query: "" });
    // 引号内的 @ 不嵌套触发（`@"a@b/c.txt"` 光标在闭合引号后）
    expect(detectAtQuery('看 @"a@b/c.txt"', 15)).toBeNull();
    // 引号内空白判定对多行文本同样成立（换行在引号内不终止）
    expect(detectAtQuery('@"a\nb/c', 7)).toEqual({ at: 0, query: '"a\nb/c' });
    // 引号外空白照常终止（普通路径输入不受引号升级影响）
    expect(detectAtQuery("看看 @src 之后", 9)).toBeNull();
    // 光标落在引用中间（未闭合段内）保持触发
    expect(detectAtQuery('看 @"a b/c.txt" 尾部', 7)).toEqual({ at: 2, query: '"a b' });
  });

  it("/ 与 $ 检测：行首与空白后触发、句中斜杠与 cost$ 不触发、三符互斥（更近触发符优先）", () => {
    // / 的正常触发形态
    expect(detectSlashQuery("/hel", 4)).toEqual({ at: 0, query: "hel" });
    expect(detectSlashQuery("请用 /dem", 9)).toEqual({ at: 3, query: "dem" });
    // 句中斜杠（路径文本）不触发——前一字符非空白
    expect(detectSlashQuery("读 src/main.rs 的第", 11)).toBeNull();
    // / 与光标间隔空白 → 非触发态；刚敲下 / 返回空查询词
    expect(detectSlashQuery("/hel 之后", 8)).toBeNull();
    expect(detectSlashQuery("/", 1)).toEqual({ at: 0, query: "" });

    // $ 的正常触发形态（与 @ / 同构）
    expect(detectDollarQuery("$ex", 3)).toEqual({ at: 0, query: "ex" });
    expect(detectDollarQuery("帮我看看 $", 10)).toEqual({ at: 5, query: "" });
    // 非空白后的 $ 不触发（排除 cost$100 误触）；间隔空白非触发态
    expect(detectDollarQuery("cost$100", 8)).toBeNull();
    expect(detectDollarQuery("$ex plore", 9)).toBeNull();

    // 三符互斥：更近的触发符优先
    const atNear = "用 /he 看 @a";
    expect(detectSlashQuery(atNear, atNear.length)).toBeNull();
    expect(detectAtQuery(atNear, atNear.length)).toEqual({ at: 8, query: "a" });
    const slashNear = "@a 结束 /he";
    // / 与光标之间无空白，@ 检测判定非触发态
    expect(detectSlashQuery(slashNear, slashNear.length)).toEqual({ at: 6, query: "he" });
    expect(detectAtQuery(slashNear, slashNear.length)).toBeNull();
    // 更近的 @ / / 触发归各自菜单，$ 不抢
    expect(detectDollarQuery("@src/", 6)).toBeNull();
    expect(detectDollarQuery("$a @b", 5)).toBeNull();
  });

  it("插入形态：/ 技能展开为指令、@ 路径补全（空格路径引号包裹）、$ 角色替换", () => {
    // / 技能选中插入：展开为明确指令文本，衔接用户后续输入
    // （光标在查询结束处——后续输入原样保留）
    expect(insertSkillInvocation("/hel 帮我入门", 0, 4, "hello")).toBe(
      "请用 skill 工具加载技能 hello 并按其指引行动 帮我入门",
    );
    // 无后续输入时以空格收尾
    expect(insertSkillInvocation("/demo", 0, 5, "demo")).toBe(
      "请用 skill 工具加载技能 demo 并按其指引行动 ",
    );

    // @ 补全：替换查询词为完整路径并留尾空格
    const text = "读 @src/ma";
    expect(insertAtCompletion(text, 2, text.length, "src/main.rs")).toBe("读 @src/main.rs ");
    // 路径含空格 → 插入 @"path" 引号形态；无空格 → 维持裸路径
    expect(insertAtCompletion("看 @", 2, 3, "a b/c.txt")).toBe('看 @"a b/c.txt" ');
    expect(insertAtCompletion("看 @", 2, 3, "src/main.rs")).toBe("看 @src/main.rs ");
    // 替换范围含查询中已敲出的开引号（`@"a b` 选中 c.txt → 完整形态）
    expect(insertAtCompletion('看 @"a b', 2, 7, "a b/c.txt")).toBe('看 @"a b/c.txt" ');

    // $ 角色插入：替换查询词为 $角色名 + 尾空格，保留后续输入
    expect(insertDollarInvocation("$ex 帮我找入口", 0, 3, "explore")).toBe("$explore 帮我找入口");
    // 后续文本以空白开头时剥掉首段空白（避免双空格）
    expect(insertDollarInvocation("$reviewer   审查一下", 0, 9, "reviewer")).toBe(
      "$reviewer 审查一下",
    );
  });

  it("清单过滤（filterSkills / filterRoles）：前缀命中排前、包含命中排后（大小写不敏感）", () => {
    const skills: SkillListEntry[] = [
      {
        name: "hello-craftwave",
        source: "builtin",
        enabled: true,
        description: "入门示例",
        trustStatus: "pending",
      },
      {
        name: "pdf-toolkit",
        source: "global",
        enabled: true,
        description: "PDF 处理",
        trustStatus: "pending",
      },
      {
        name: "demo-hello",
        source: "project",
        enabled: true,
        description: "演示",
        trustStatus: "pending",
      },
    ];
    // 空查询返回全部（保持清单序）；无命中返回空表
    expect(filterSkills(skills, "")).toHaveLength(3);
    expect(filterSkills(skills, "missing")).toEqual([]);
    expect(filterSkills(skills, "HEL").map((s) => s.name)).toEqual([
      "hello-craftwave",
      "demo-hello",
    ]);
    // $ 角色：空查询返回全部 7 角色（保持清单序）
    expect(filterRoles(SUBAGENT_ROLES, "")).toHaveLength(7);
    expect(filterRoles(SUBAGENT_ROLES, "DEV").map((r) => r.name)).toEqual([
      "frontend-dev",
      "backend-dev",
    ]);
  });
});
