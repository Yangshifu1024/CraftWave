// composer-keys 纯逻辑测试（17 号票：@ 触发检测与插入；09 号票：/ 技能触发
// 检测、技能插入形态与清单过滤、@ 与 / 首字符互斥）。
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

describe("detectAtQuery（@ 路径触发检测，§10.6 光标锚定）", () => {
  it("光标前未闭合的 @ 返回插入位与查询词", () => {
    const text = "看一下 @src/ma";
    expect(detectAtQuery(text, text.length)).toEqual({ at: 4, query: "src/ma" });
  });

  it("@ 与光标间隔空白 → 非触发态", () => {
    const text = "看看 @src 之后";
    expect(detectAtQuery(text, text.length)).toBeNull();
  });

  it("无 @ 或光标在 @ 前 → null", () => {
    expect(detectAtQuery("普通文本", 4)).toBeNull();
    expect(detectAtQuery("@src", 0)).toBeNull();
  });

  it("取最近的 @（多个触发位时）", () => {
    const text = "@a 起 @b";
    expect(detectAtQuery(text, text.length)).toEqual({ at: 5, query: "b" });
  });
});

describe("detectSlashQuery（/ 技能触发检测，09 号票 FR-01-17 技能半边）", () => {
  it("行首 / 返回插入位与查询词", () => {
    const text = "/hel";
    expect(detectSlashQuery(text, text.length)).toEqual({ at: 0, query: "hel" });
  });

  it("空白后的 / 同样触发", () => {
    const text = "请用 /dem";
    expect(detectSlashQuery(text, text.length)).toEqual({ at: 3, query: "dem" });
  });

  it("句中斜杠（路径文本）不触发——前一字符非空白", () => {
    expect(detectSlashQuery("读 src/main.rs 的第", 11)).toBeNull();
  });

  it("/ 与光标间隔空白 → 非触发态", () => {
    expect(detectSlashQuery("/hel 之后", 8)).toBeNull();
  });

  it("更近的 @ 触发归 @ 菜单（首字符互斥，@ 胜出）", () => {
    const text = "用 /he 看 @a";
    // 光标往回先遇到 @（更近的触发符），/ 不抢菜单
    expect(detectSlashQuery(text, text.length)).toBeNull();
    expect(detectAtQuery(text, text.length)).toEqual({ at: 8, query: "a" });
  });

  it("/ 在 @ 之后仍归 /（更近触发符优先）", () => {
    const text = "@a 结束 /he";
    expect(detectSlashQuery(text, text.length)).toEqual({ at: 6, query: "he" });
    // / 与光标之间无空白，@ 检测判定非触发态
    expect(detectAtQuery(text, text.length)).toBeNull();
  });

  it("空查询（刚敲下 /）返回空查询词", () => {
    expect(detectSlashQuery("/", 1)).toEqual({ at: 0, query: "" });
  });
});

describe("insertSkillInvocation（技能选中插入，票面定案展开为明确指令文本）", () => {
  it("把 /查询 替换为明确指令并衔接用户后续输入", () => {
    const text = "/hel 帮我入门";
    // 光标在查询结束处（下标 4，空格前）——后续输入原样保留
    expect(insertSkillInvocation(text, 0, 4, "hello")).toBe(
      "请用 skill 工具加载技能 hello 并按其指引行动 帮我入门",
    );
  });

  it("无后续输入时以空格收尾", () => {
    expect(insertSkillInvocation("/demo", 0, 5, "demo")).toBe(
      "请用 skill 工具加载技能 demo 并按其指引行动 ",
    );
  });
});

describe("filterSkills（/ 菜单清单过滤）", () => {
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

  it("空查询返回全部（保持清单序）", () => {
    expect(filterSkills(skills, "")).toHaveLength(3);
  });

  it("前缀命中排前、包含命中排后（大小写不敏感）", () => {
    const hits = filterSkills(skills, "HEL");
    expect(hits.map((s) => s.name)).toEqual(["hello-craftwave", "demo-hello"]);
  });

  it("无命中返回空表", () => {
    expect(filterSkills(skills, "missing")).toEqual([]);
  });
});

describe("insertAtCompletion（补全项插入）", () => {
  it("替换查询词为完整路径并留尾空格", () => {
    const text = "读 @src/ma";
    expect(insertAtCompletion(text, 2, text.length, "src/main.rs")).toBe("读 @src/main.rs ");
  });
});

// ---- $ 角色触发符（FR-07-02，10 号票）----

describe("detectDollarQuery（$ 菜单触发检测，与 @ / 同构光标锚定）", () => {
  it("行首 $ 触发：返回插入位与查询词", () => {
    expect(detectDollarQuery("$ex", 3)).toEqual({ at: 0, query: "ex" });
  });

  it("空白后的 $ 触发（句子中段点名）", () => {
    const text = "帮我看看 $";
    expect(detectDollarQuery(text, text.length)).toEqual({ at: 5, query: "" });
  });

  it("非空白后的 $ 不触发（排除 cost$100 误触）", () => {
    expect(detectDollarQuery("cost$100", 8)).toBeNull();
  });

  it("$ 与光标间隔空白 → 非触发态", () => {
    expect(detectDollarQuery("$ex plore", 9)).toBeNull();
  });

  it("更近的 @ / / 触发归各自菜单（三符互斥）", () => {
    expect(detectDollarQuery("@src/", 6)).toBeNull();
    expect(detectDollarQuery("$a @b", 5)).toBeNull();
  });
});

describe("filterRoles（$ 菜单角色过滤）", () => {
  it("空查询返回全部 7 角色（保持清单序）", () => {
    expect(filterRoles(SUBAGENT_ROLES, "")).toHaveLength(7);
  });

  it("前缀命中排前、包含命中排后（大小写不敏感）", () => {
    const hits = filterRoles(SUBAGENT_ROLES, "DEV");
    expect(hits.map((r) => r.name)).toEqual(["frontend-dev", "backend-dev"]);
  });
});

describe("insertDollarInvocation（$ 角色插入）", () => {
  it("替换查询词为 $角色名 + 尾空格，保留后续输入", () => {
    expect(insertDollarInvocation("$ex 帮我找入口", 0, 3, "explore")).toBe("$explore 帮我找入口");
  });

  it("后续文本以空白开头时剥掉首段空白（避免双空格）", () => {
    expect(insertDollarInvocation("$reviewer   审查一下", 0, 9, "reviewer")).toBe(
      "$reviewer 审查一下",
    );
  });
});
