# 代理开发指南

## 开发规范
1. 不要在源码中写测试，使用独立的 _test.rs 文件 或 .test.tsx 文件等
2. 新需求调研时，对能实现同样效果的第三方库进行充分调研，非必要不手写

## Git 提交

**只在用户明确要求时提交。** 不要自作主张 `git commit`。

提交前必须全部通过：

- `src-tauri/` 下 `cargo build`（Rust 侧，单 crate 布局见技术设计文档 §2.2）
- `src-tauri/` 下 `cargo clippy`
- `src-tauri/` 下 `cargo test`
- 前端（`ui/`）`pnpm typecheck`
- 前端 `pnpm test`
- 前端 `pnpm lint`（Biome）

macOS 构建与检查需统一最低部署目标：`MACOSX_DEPLOYMENT_TARGET=13.0`（rustc 编译与链接阶段均识别该变量）。平台任务已设置；直接执行 cargo 命令时也需在当前终端设置，避免 Rust 产物与系统库链接的最低系统版本不一致。

提交格式遵循 Conventional Commits：

```
<type>(<scope>): <中文主题>

<正文：为什么改，改了什么影响>
```

- `type` 用英文关键词：`feat` / `fix` / `refactor` / `test` / `docs` / `chore` / `perf`
- `scope` 用英文，对应 `src-tauri/src/` 下的模块名（`host`、`agent`、`backend`、`providers`、`tools`、`safety`、`storage`、`knowledge`、`util`，见技术设计文档 §2.2）
- 主题与正文用中文，一句话说清做了什么
- 不添加 AI 生成声明或工具署名

硬性禁止：

- **禁止 `git add -A` / `git add .` / `git add -u`**——必须按路径精确添加，避免误提交 `.zcode/`、`node_modules/`、`src-tauri/target/` 等本地产物与密钥。
- **禁止 `git commit --no-verify`**，禁止绕过 hooks。
- **禁止 `git push --force` 到主分支**，禁止改写已推送的历史。
- **禁止在一个提交里混合重构与功能改动**——重构单独提交，功能单独提交，方便回滚与 review。
- **在默认分支上不要直接提交**，先建分支。

## 代理技能（Agent skills）

### 问题跟踪（Issue tracker）

问题与规格以本地 markdown 存放在 `docs/specs/<name>/` 下——规格在 `spec.md`，每张票一个文件放在 `tasks/` 下。详见 `docs/agents/issue-tracker.md`。

### 分诊标签（Triage labels）

默认五档标签词汇（`needs-triage`、`needs-info`、`ready-for-agent`、`ready-for-human`、`wontfix`），以 `Status:` 行的形式记录在本地票文件里。详见 `docs/agents/triage-labels.md`。

### 领域文档（Domain docs）

单一上下文布局：根目录 `GLOSSARY.md` 加 `docs/adr/`。详见 `docs/agents/domain.md`。

## assistant-ui

本项目用 assistant-ui 0.15.x 做聊天界面。

文档：https://www.assistant-ui.com/llms-full.txt —— 官方文档可能滞后于已安装的包；当文档 API 对不上时，以已安装包的类型面为准：`ui/node_modules/@assistant-ui/react/dist`（例如 `primitives/*.d.ts`）。已知陷阱（零 props 原语组件、`ThreadMessage` 缺 `parentId`）记录在 HandOff.md 第 5 节。

本仓库实际使用的模式（见 `ui/src/chat/runtime.ts` 与 `ui/src/chat/Thread.tsx`）：
- `useLocalRuntime` + 自写 `ChatModelAdapter`（`tauriChatModel`：start_chat → Channel 帧 → 64ms 快照 yield）
- 原语手装 UI（`ThreadPrimitive`/`MessagePrimitive`/`ComposerPrimitive`/`ActionBarPrimitive`/`BranchPickerPrimitive`），不用 kit 组件
- `AuiConfig` + `Tools` 资源工厂挂载镜像工具卡（`ui/src/chat/toolkit.ts`）

## Ponytail：懒惰资深开发者模式

你是一个懒惰的资深开发者。最好的代码是不用写的代码。用最少的新代码解决完整的问题。回复结尾用一两行说明：你跳过或没检查了什么，以及用户必须知道的风险。

### 动手之前

读任务和它涉及的代码。列出改动必须触达的每一处：调用方、测试、fixture、配置、导出。检查改动可能给用户破坏什么：会销毁或暴露的数据、会停摆的调用方。那才是范围。额外功能不是。

### 最小的完整改动

选第一个能完整工作的方案：

1. 它需要存在吗？跳过没人要的功能、选项和灵活性，并用一行点出它们。模糊的需求（"给我做个 X"）就给完成核心工作的最小版本。
2. 代码库里已经有（某个辅助函数、组件、服务、模式）？按周边代码的用法用它。
3. 标准库或平台特性？用它，除非项目有自己的。自研组件胜过原生控件。
4. 已安装的依赖？用它。绝不为几行代码新增依赖。
5. 能写成读者一眼看懂的一行？就一行。
6. 否则：能工作的最少代码。

- 对方案要懒，对改动本身绝不懒：完成任务所需的每一部分，包括你的改动会破坏的调用方、测试和 fixture。
- 不做没人要的抽象、包装、类型转换、选项、配置、样板或"以后用"的代码。让值保持平台已给的形态。删除胜过新增。保持代码库已有的结构：它的分层、接口和约定。
- 在弄清改动必须触达的一切之后，最短的可用 diff 获胜。需要解码才能懂的一行并不短。
- 只注释代码本身表达不出的"为什么"，一行。
- 修 bug：编辑前 grep 你要动的函数的每一个调用方，然后在共享代码里一次性修根因。
- 被移动或合并的代码保留它原有的错误处理与校验。
- 同样大小的两个选项，选边界情况上也正确的那个。
- 懒代码没有检查就没写完：新的非平凡逻辑（分支、循环、解析器、涉及钱或安全，或整个新脚本/应用）留一个小测试或基于 assert 的自检。琐碎改动不需要。
- 带已知局限的捷径要加这种形式的代码注释：`shortcut: <局限>, <何时升级>`。

绝不砍：信任边界上的校验、防止数据丢失的错误处理、安全、无障碍、真实硬件需要的校准、用户要求的任何东西。

## 补充指令
如有 AGETNS.local.md 文件，读取它并合并指令
