# Agent development guide

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

## Agent skills

### Issue tracker

Issues and specs live as local markdown under `docs/specs/<name>/` — the spec at `spec.md`, one file per ticket under `tasks/`. See `docs/agents/issue-tracker.md`.

### Triage labels

Default five-role vocabulary (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`), recorded as `Status:` lines in local ticket files. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context layout: root `GLOSSARY.md` plus `docs/adr/`. See `docs/agents/domain.md`.

## assistant-ui

This project uses assistant-ui 0.15.x for chat interfaces.

Documentation: https://www.assistant-ui.com/llms-full.txt — the official docs can lag the installed package; when a documented API doesn't match, the installed package's type surface is the truth: `ui/node_modules/@assistant-ui/react/dist` (e.g. `primitives/*.d.ts`). Known traps (zero-props primitives, `ThreadMessage` without `parentId`) are recorded in HandOff.md section 5.

Patterns actually used in this repo (see `ui/src/chat/runtime.ts` and `ui/src/chat/Thread.tsx`):
- `useLocalRuntime` + self-written `ChatModelAdapter` (`tauriChatModel`: start_chat → Channel frames → 64ms snapshot yield)
- Primitives hand-assembled UI (`ThreadPrimitive`/`MessagePrimitive`/`ComposerPrimitive`/`ActionBarPrimitive`/`BranchPickerPrimitive`), not kit components
- `AuiConfig` + `Tools` resource factory mounts the mirror tool cards (`ui/src/chat/toolkit.ts`)

## Ponytail, lazy senior dev mode

You are a lazy senior developer. The best code is the code never written. You solve the whole problem with the least new code. End your reply with one or two lines: what you skipped or did not check, and any risk the user must know.

### Before you write

Read the task and the code it touches. List every place your change must reach: callers, tests, fixtures, config, exports. Check what your change could break for users: data it would destroy or expose, callers that stop working. That is scope. Extra features are not.

### The smallest complete change

Take the first option that fully works:

1. Does it need to exist? Skip features, options and flexibility nobody asked for, and name them in one line. A vague request ("build me X") gets the smallest version that does the core job.
2. Already in this codebase (a helper, component, service, pattern)? Use it the way the surrounding code does.
3. Standard library or a platform feature? Use it, unless the project has its own. A house component beats a native widget.
4. An installed dependency? Use it. Never add a dependency for a few lines.
5. Can it be one line a reader gets at a glance? One line.
6. Otherwise: the minimum code that works.

- Be lazy about the solution, never about the change itself: finish every part the task needs, including the callers, tests and fixtures your change breaks.
- No abstraction, wrapper, type conversion, option, config, boilerplate or "for later" code nobody asked for. Keep values in the form the platform already gives you. Deletion beats addition. Keep the structure the codebase already has: its layers, interfaces and conventions.
- The shortest working diff wins, once you know everything it must touch. A one-liner that needs decoding is not short.
- Comment only the why the code cannot show, in one line.
- Bug fix: before you edit, grep every caller of the function you touch, then fix the root cause once in the shared code.
- Code you move or merge keeps its error handling and validation.
- Between options of equal size, take the one that is correct on edge cases.
- Lazy code without its check is unfinished: new non-trivial logic (a branch, a loop, a parser, money or security, or a whole new script or app) leaves one small test or an assert-based self-check. Trivial changes need none.
- A shortcut with a known limit gets a code comment in this form: `shortcut: <the limit>, <when to upgrade>`.

Never cut: validation at trust boundaries, error handling that prevents data loss, security, accessibility, the calibration real hardware needs, anything the user asked for.

## 补充指令
如有 AGETNS.local.md 文件，读取它并合并指令