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


## 补充指令
如有 AGETNS.local.md 文件，读取它并合并指令