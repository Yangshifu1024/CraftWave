# Agent develo guide

## Git 提交

**只在用户明确要求时提交。** 不要自作主张 `git commit`。

提交前必须全部通过：

- `go build $(go list ./... | grep -v '/build/')` —— **不是** `go build ./...`
- `go vet ./...`
- `go test ./...`
- 前端 `pnpm typecheck`
- 前端 `pnpm test`
- 前端 `pnpm lint`（Biome）

macOS 构建与检查需统一目标版本：`MACOSX_DEPLOYMENT_TARGET=13.0`、`CGO_CFLAGS="-O2 -g -mmacosx-version-min=13.0"`、`CGO_LDFLAGS="-O2 -g -mmacosx-version-min=13.0"`。平台任务已设置；直接执行 Go 命令时也需在当前终端设置，避免 Go 与 C 编译、链接的最低系统版本不一致。

提交格式遵循 Conventional Commits：

```
<type>(<scope>): <中文主题>

<正文：为什么改，改了什么影响>
```

- `type` 用英文关键词：`feat` / `fix` / `refactor` / `test` / `docs` / `chore` / `perf`
- `scope` 用英文，对应 `internal/` 下的模块名（`agent`、`tools`、`session`、`sandbox`、`context`、`permission`、`provider`、`ui`）
- 主题与正文用中文，一句话说清做了什么
- 不添加 AI 生成声明或工具署名

硬性禁止：

- **禁止 `git add -A` / `git add .` / `git add -u`**——必须按路径精确添加，避免误提交 `.scratch/`、`.workwave/`、密钥、构建产物。
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

This project uses assistant-ui for chat interfaces.

Documentation: https://www.assistant-ui.com/llms-full.txt

Key patterns:
- Use AssistantRuntimeProvider at the app root
- Thread component for full chat interface
- AssistantModal for floating chat widget
- useChatRuntime hook with AI SDK transp