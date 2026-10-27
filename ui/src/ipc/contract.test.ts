/**
 * 契约测试（§12.3/NFR-12，前端侧）：消费 src-tauri/target/ipc-manifest.json
 * （由 `cargo test` 的 export_ipc_manifest 测试产出），做双向扫描：
 *
 * 1. client.ts（唯一 invoke 出口）注册的每个命令名 ∈ manifest.commands；
 * 2. manifest 中 provider/config 切片的命令在 client.ts 全部接线；
 * 3. ui/src 全源码不得出现绕过唯一出口的 `invoke("…")` 未知字面量；
 * 4. manifest.frames 覆盖 §3.4 P0 全集 9 帧。
 *
 * CI 顺序：cargo test 产出 manifest artifact → 本测试消费，不齐即红。
 */

import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { beforeAll, describe, expect, it } from "vitest";
import { COMMANDS } from "./client";

interface ManifestCommand {
  name: string;
  argsType: string;
  retType: string;
}
interface Manifest {
  commands: ManifestCommand[];
  frames: string[];
  events: string[];
  tools: string[];
}

const manifestPath = fileURLToPath(
  new URL("../../../src-tauri/target/ipc-manifest.json", import.meta.url),
);
const uiSrcDir = fileURLToPath(new URL("..", import.meta.url));

let manifest: Manifest;

beforeAll(() => {
  let raw: string;
  try {
    raw = readFileSync(manifestPath, "utf-8");
  } catch {
    throw new Error(
      `找不到 ${manifestPath}——先在 src-tauri/ 下跑 cargo test 产出 ipc-manifest.json（§12.3 CI 顺序）`,
    );
  }
  manifest = JSON.parse(raw) as Manifest;
});

describe("IPC manifest 契约", () => {
  it("client.ts 注册的命令名全部存在于 manifest.commands", () => {
    const manifestNames = new Set(manifest.commands.map((c) => c.name));
    for (const name of Object.values(COMMANDS)) {
      expect(manifestNames, `命令 ${name} 未在 manifest 中注册`).toContain(name);
    }
  });

  it("provider/config 切片命令在 client.ts 全部接线（双向）", () => {
    const wired = new Set(Object.values(COMMANDS));
    const slice = ["get_config", "save_config", "save_provider", "delete_provider"];
    for (const name of slice) {
      expect(wired, `manifest 命令 ${name} 未在 client.ts 接线（禁止只注册不使用）`).toContain(
        name as (typeof COMMANDS)[keyof typeof COMMANDS],
      );
    }
  });

  it("ui/src 全源码无绕过唯一出口的 invoke 字面量", () => {
    const manifestNames = new Set(manifest.commands.map((c) => c.name));
    const offenders: string[] = [];
    const walk = (dir: string) => {
      for (const entry of readdirSync(dir, { withFileTypes: true })) {
        const full = path.join(dir, entry.name);
        if (entry.isDirectory()) {
          walk(full);
        } else if (/\.(ts|tsx)$/.test(entry.name) && !entry.name.endsWith(".test.ts")) {
          const source = readFileSync(full, "utf-8");
          for (const match of source.matchAll(/invoke[<(]\s*["'`]([^"'`]+)["'`]/g)) {
            const name = match[1];
            if (!manifestNames.has(name)) {
              offenders.push(`${path.relative(uiSrcDir, full)}: ${name}`);
            }
          }
        }
      }
    };
    walk(uiSrcDir);
    expect(offenders, `未知 invoke 命令：\n${offenders.join("\n")}`).toEqual([]);
  });

  it("manifest.frames 覆盖 §3.4 P0 全集 9 帧", () => {
    expect(manifest.frames).toHaveLength(9);
    for (const frame of [
      "runStarted",
      "stepStarted",
      "textDelta",
      "reasoningDelta",
      "toolStarted",
      "toolArgs",
      "toolResult",
      "usage",
      "runEnded",
    ]) {
      expect(manifest.frames).toContain(frame);
    }
  });
});
