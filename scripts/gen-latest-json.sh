#!/usr/bin/env bash
# latest.json 组装脚本（33 号票，FR-16-03/技术设计文档 §12.4 更新器发布骨架）。
#
# tauri-plugin-updater 客户端从固定 endpoint 拉取 latest.json 判断新版本：
#   https://github.com/<repo>/releases/latest/download/latest.json
# 文件形态（updater 插件约定）：
#   {
#     "version": "0.1.1",                  # 新版本号（不带 v 前缀）
#     "notes": "...",                      # release notes 文本
#     "pub_date": "2026-10-30T12:00:00Z",  # ISO 8601 UTC
#     "platforms": {
#       "darwin-aarch64":  { "signature": "<.sig 文件内容>", "url": "https://github.com/<repo>/releases/download/<tag>/<产物名>" },
#       "windows-x86_64":  { ... },
#       "linux-x86_64":    { ... }
#     }
#   }
#
# 用法（两个子命令，供 GitHub Actions 三平台矩阵分阶段组装）：
#   gen-latest-json.sh platform-entry <tag>          # 在构建 runner 上执行：
#                                                    # 探测本平台 updater 产物与
#                                                    # .sig，stdout 输出单平台条目
#                                                    # 对象 {"<平台键>": {...}}
#   gen-latest-json.sh merge <tag> <meta目录>        # 在汇总 job 执行：把目录下
#                                                    # 三个平台条目 json 合并成完整
#                                                    # latest.json（notes 用第三个
#                                                    # 环境变量 RELEASE_NOTES 可覆盖，
#                                                    # 缺省 "CraftWave <tag>"）
#
# 平台键判定依据产物存在性（与矩阵 runner 对应）：
#   src-tauri/target/release/bundle/macos/*.app.tar.gz(.sig)    → darwin-aarch64
#   src-tauri/target/release/bundle/nsis/*.exe(.sig)            → windows-x86_64
#   src-tauri/target/release/bundle/appimage/*.AppImage(.sig)   → linux-x86_64
# 依赖：bash + jq（GitHub Actions runner 自带；macOS/Linux 均预装）。

set -euo pipefail

usage() {
  echo "用法：$0 platform-entry <tag>" >&2
  echo "      $0 merge <tag> <meta目录>" >&2
  exit 2
}

# 版本号：去掉 tag 的前导 v（v0.1.1 → 0.1.1；无前缀原样）。
strip_v() {
  local tag="$1"
  echo "${tag#v}"
}

# 取第一个匹配文件的基名（产物名）。$1 = glob；无匹配返回 1（临时开
# nullglob 防字面量误命中——未展开的 `*` 模式不能当作产物名）。
first_basename() {
  local glob="$1"
  local had_nullglob=0
  shopt -q nullglob && had_nullglob=1
  shopt -s nullglob
  # shellcheck disable=SC2206  # glob 刻意展开
  local files=($glob)
  [ "$had_nullglob" -eq 1 ] || shopt -u nullglob
  [ ${#files[@]} -gt 0 ] || return 1
  basename "${files[0]}"
}

# 组装单平台条目对象：{"<平台键>": {"signature": ..., "url": ...}}
# signature = .sig 文件完整原文（tauri updater 校验面，--rawfile 保真读入，
# 含 minisign 多行注释与 base64 体，不做任何加工）。
platform_object() {
  local key="$1" name="$2" sig="$3" tag="$4" repo="$5"
  local url
  url="https://github.com/${repo}/releases/download/${tag}/${name}"
  jq -n --arg key "$key" --rawfile signature "$sig" --arg url "$url" \
    '{ ($key): { signature: $signature, url: $url } }'
}

# 子命令一：探测本平台 updater 产物并输出单平台条目。
cmd_platform_entry() {
  local tag="$1"
  local repo="${GITHUB_REPOSITORY:-Yangshifu1024/CraftWave}"
  local bundle="src-tauri/target/release/bundle"

  local name sig
  if name="$(first_basename "${bundle}/macos/"*.app.tar.gz)"; then
    sig="$(first_basename "${bundle}/macos/"*.app.tar.gz.sig)" || {
      echo "错误：macOS updater 产物缺少 .sig（createUpdaterArtifacts 未生效？）" >&2
      exit 1
    }
    platform_object "darwin-aarch64" "$name" "${bundle}/macos/${sig}" "$tag" "$repo"
    return 0
  fi
  if name="$(first_basename "${bundle}/nsis/"*-setup.exe)"; then
    sig="$(first_basename "${bundle}/nsis/"*-setup.exe.sig)" || {
      echo "错误：Windows updater 产物缺少 .sig" >&2
      exit 1
    }
    platform_object "windows-x86_64" "$name" "${bundle}/nsis/${sig}" "$tag" "$repo"
    return 0
  fi
  if name="$(first_basename "${bundle}/appimage/"*.AppImage)"; then
    sig="$(first_basename "${bundle}/appimage/"*.AppImage.sig)" || {
      echo "错误：Linux updater 产物缺少 .sig" >&2
      exit 1
    }
    platform_object "linux-x86_64" "$name" "${bundle}/appimage/${sig}" "$tag" "$repo"
    return 0
  fi
  echo "错误：未找到本平台的 updater 产物（macos/*.app.tar.gz、nsis/*-setup.exe、appimage/*.AppImage 均缺失）" >&2
  exit 1
}

# 子命令二：合并目录下各平台条目 json 成完整 latest.json。
# $1 = tag；$2 = 条目文件目录（每文件一个 {"平台键": {...}} 对象）。
cmd_merge() {
  local tag="$1" dir="$2"
  local version notes pub_date
  version="$(strip_v "$tag")"
  notes="${RELEASE_NOTES:-CraftWave ${tag}}"
  pub_date="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

  local platforms="{}"
  local found=0
  for f in "${dir}"/*.json; do
    [ -e "$f" ] || continue
    found=1
    platforms="$(jq -c -s '.[0] * .[1]' <(echo "$platforms") "$f")"
  done
  [ "$found" -eq 1 ] || { echo "错误：条目目录 ${dir} 下没有 json" >&2; exit 1; }

  local count
  count="$(jq '. | length' <<< "$platforms")"
  [ "$count" -eq 3 ] || {
    echo "错误：latest.json 应含三平台条目，实际 ${count} 个：$(jq -r 'keys | join(", ")' <<< "$platforms")" >&2
    exit 1
  }
  jq -n --arg version "$version" --arg notes "$notes" --arg pub_date "$pub_date" \
    --argjson platforms "$platforms" \
    '{ version: $version, notes: $notes, pub_date: $pub_date, platforms: $platforms }'
}

case "${1:-}" in
  platform-entry)
    [ $# -eq 2 ] || usage
    cmd_platform_entry "$2"
    ;;
  merge)
    [ $# -eq 3 ] || usage
    cmd_merge "$2" "$3"
    ;;
  *)
    usage
    ;;
esac
