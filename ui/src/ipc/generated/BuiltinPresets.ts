// 本文件由 cargo test 生成（src-tauri/src/ipc_manifest_test.rs），勿手改。
// 权威来源：src-tauri/src/providers/presets.rs 的 builtin_presets()（预设单源）。
import type { ProviderPreset } from "./ProviderPreset";

/** 内置供应商预设全集（顺序即设置页「从预设添加」的展示顺序）。 */
export const BUILTIN_PRESETS: ProviderPreset[] = [
  {
    "slug": "openai",
    "name": "OpenAI",
    "apiFormat": "openai_chat",
    "baseUrl": "https://api.openai.com/v1",
    "models": [
      {
        "model": "gpt-4.1",
        "contextWindow": 1047576,
        "maxTokens": 32768,
        "vision": true
      },
      {
        "model": "gpt-4o-mini",
        "contextWindow": 128000,
        "maxTokens": 16384,
        "vision": true
      }
    ],
    "local": false
  },
  {
    "slug": "anthropic",
    "name": "Anthropic",
    "apiFormat": "anthropic_messages",
    "baseUrl": "https://api.anthropic.com",
    "models": [
      {
        "model": "claude-sonnet-4-5",
        "contextWindow": 200000,
        "maxTokens": 64000,
        "vision": true
      },
      {
        "model": "claude-haiku-4-5",
        "contextWindow": 200000,
        "maxTokens": 64000,
        "vision": true
      }
    ],
    "local": false
  },
  {
    "slug": "deepseek",
    "name": "DeepSeek",
    "apiFormat": "openai_chat",
    "baseUrl": "https://api.deepseek.com",
    "models": [
      {
        "model": "deepseek-chat",
        "contextWindow": 128000,
        "maxTokens": 8192,
        "vision": false
      },
      {
        "model": "deepseek-reasoner",
        "contextWindow": 128000,
        "maxTokens": 8192,
        "vision": false
      }
    ],
    "local": false
  },
  {
    "slug": "moonshot",
    "name": "Kimi (Moonshot)",
    "apiFormat": "openai_chat",
    "baseUrl": "https://api.moonshot.cn/v1",
    "models": [
      {
        "model": "kimi-k2-0905-preview",
        "contextWindow": 256000,
        "maxTokens": 8192,
        "vision": false
      }
    ],
    "local": false
  },
  {
    "slug": "zhipu",
    "name": "智谱 GLM",
    "apiFormat": "openai_chat",
    "baseUrl": "https://open.bigmodel.cn/api/paas/v4",
    "models": [
      {
        "model": "glm-5.3",
        "contextWindow": 200000,
        "maxTokens": 128000,
        "vision": true
      },
      {
        "model": "glm-5.3-flash",
        "contextWindow": 200000,
        "maxTokens": 32768,
        "vision": true
      },
      {
        "model": "glm-4.6",
        "contextWindow": 200000,
        "maxTokens": 128000,
        "vision": true
      }
    ],
    "local": false
  },
  {
    "slug": "openrouter",
    "name": "OpenRouter",
    "apiFormat": "openai_chat",
    "baseUrl": "https://openrouter.ai/api/v1",
    "models": [
      {
        "model": "openrouter/auto",
        "contextWindow": 128000,
        "maxTokens": 32768,
        "vision": false
      }
    ],
    "local": false
  },
  {
    "slug": "ollama",
    "name": "Ollama（本地）",
    "apiFormat": "openai_chat",
    "baseUrl": "http://127.0.0.1:11434/v1",
    "models": [
      {
        "model": "qwen3:8b",
        "contextWindow": 128000,
        "maxTokens": 32768,
        "vision": false
      },
      {
        "model": "llama3.1:8b",
        "contextWindow": 128000,
        "maxTokens": 32768,
        "vision": false
      }
    ],
    "local": true
  },
  {
    "slug": "llamacpp",
    "name": "llama.cpp server（本地）",
    "apiFormat": "openai_chat",
    "baseUrl": "http://127.0.0.1:8080/v1",
    "models": [
      {
        "model": "local-model",
        "contextWindow": 32000,
        "maxTokens": 8192,
        "vision": false
      }
    ],
    "local": true
  },
  {
    "slug": "lmstudio",
    "name": "LM Studio（本地）",
    "apiFormat": "openai_chat",
    "baseUrl": "http://127.0.0.1:1234/v1",
    "models": [
      {
        "model": "local-model",
        "contextWindow": 32000,
        "maxTokens": 8192,
        "vision": false
      }
    ],
    "local": true
  }
];
