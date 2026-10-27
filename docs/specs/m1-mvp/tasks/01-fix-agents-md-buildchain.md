# 修正 AGENTS.md 构建检查段（Go 残留 → Tauri 工具链）

Type: task
Status: resolved

## Question

根目录 AGENTS.md 的「Git 提交」段是 Go 项目模板残留（`go build $(go list ./...)`、`go vet`、`go test`，scope 对应 `internal/` 下的 Go 模块名），与本项目 Tauri 2 + Rust + pnpm 技术栈完全不符，会误导后续每个会话的提交前检查。

请修正为真实约束本项目的检查链，要求：

1. Go 三件套替换为 `cargo build` + `cargo clippy` + `cargo test`（以仓库实际布局为准，注明 `src-tauri/`）。
2. 保留前端三项：`pnpm typecheck` / `pnpm test` / `pnpm lint`（Biome）。
3. `MACOSX_DEPLOYMENT_TARGET=13.0` 保留（rustc 链接阶段同样识别）；`CGO_CFLAGS`/`CGO_LDFLAGS` 为 Go 专属概念，删除，如有必要替换为等价的 Rust/macOS 最低部署目标说明。
4. Conventional Commits 的 scope 说明改为 `src-tauri/src/` 下实际模块名（host/agent/backend/providers/tools/safety/storage/knowledge/util，见技术设计文档 §2.2）。
5. 其余规则原样保留：仅用户明确要求时提交、按路径精确 add、禁止绕过 hooks、默认分支不直提、中文主题。

完成标准：AGENTS.md 与仓库实际工具链一致，不含本项目不存在的命令或目录。

## Answer

AGENTS.md「Git 提交」段已按五项要求修正，并额外清掉一处同类模板残留：

1. Go 三件套 → `src-tauri/` 下 `cargo build` / `cargo clippy` / `cargo test`。仓库当前尚无代码（骨架属 04 号票），布局按技术设计文档 §2.2 单 crate 结构书写；04 号票落地骨架时若布局有出入，需回头同步此段。
2. 前端三项保留，注明前端根为 `ui/`（技术设计文档 §2.4）。
3. 保留 `MACOSX_DEPLOYMENT_TARGET=13.0`，注明 rustc 编译与链接阶段均识别该变量；`CGO_CFLAGS`/`CGO_LDFLAGS` 删除，无 Rust 等价物需要补。
4. scope 说明改为 `src-tauri/src/` 九模块：`host`、`agent`、`backend`、`providers`、`tools`、`safety`、`storage`、`knowledge`、`util`（附 §2.2 引用）。
5. 其余规则原样保留；另将「禁止 add -A」理由里的 `.scratch/`、`.workwave/`（同为旧模板目录，本仓库不存在）替换为本仓库真实风险路径 `.zcode/`、`node_modules/`、`src-tauri/target/`，以符合「不含本项目不存在的目录」的完成标准。顺手修复标题 typo「develo」→「development」。

验证：grep 确认全文无 `go build/vet/test`、`CGO_*`、`internal/` 残留。改动未提交（AGENTS.md 规定仅用户明确要求时提交）。
