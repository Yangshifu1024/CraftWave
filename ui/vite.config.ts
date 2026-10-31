import { writeFile } from "node:fs/promises";
import path from "node:path";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import type { Plugin } from "vite";
import { defineConfig } from "vite";

/**
 * NFR-03 压测报告回传（34 号票，dev-only）：压测页跑完后把帧采样报告
 * POST 到 /__stress_report，写入 src-tauri/target/stress-report.json
 * （target/ 已 gitignore，属测试产物）。`apply: "serve"` 让插件只在
 * dev server 生效——生产构建完全不包含此中间件。
 */
function stressReportPlugin(): Plugin {
  return {
    name: "craftwave-stress-report",
    apply: "serve",
    configureServer(server) {
      server.middlewares.use("/__stress_report", (req, res) => {
        if (req.method !== "POST") {
          res.statusCode = 405;
          res.end("POST only");
          return;
        }
        const chunks: Buffer[] = [];
        req.on("data", (chunk: Buffer) => chunks.push(chunk));
        req.on("end", () => {
          const body = Buffer.concat(chunks).toString("utf8");
          const outPath = path.resolve(
            import.meta.dirname,
            "../src-tauri/target/stress-report.json",
          );
          writeFile(outPath, body, "utf8")
            .then(() => {
              res.statusCode = 200;
              res.end("ok");
            })
            .catch((err: unknown) => {
              res.statusCode = 500;
              res.end(String(err));
            });
        });
      });
    },
  };
}

export default defineConfig({
  plugins: [react(), tailwindcss(), stressReportPlugin()],
  resolve: {
    alias: {
      "@": path.resolve(import.meta.dirname, "src"),
    },
  },
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
  },
  build: {
    target: "es2022",
  },
});
