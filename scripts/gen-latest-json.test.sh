#!/usr/bin/env bash
# gen-latest-json.sh 的本地单测（33 号票，票面「latest.json 组装脚本本地单测」
# 承载面：fixture 产物名/.sig → JSON 形态、三平台 key 断言、URL 拼接）。
#
# 运行：bash scripts/gen-latest-json.test.sh
# 依赖：bash + jq（未装 jq 时全部用例跳过并明示——CI 的 ubuntu runner 与
# macOS runner 均预装 jq；本测试不在七项检查链内，发布面改动时手动跑）。

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GEN="$SCRIPT_DIR/gen-latest-json.sh"

if ! command -v jq > /dev/null 2>&1; then
  echo "SKIP：jq 未安装，gen-latest-json 单测全部跳过（需 bash + jq）"
  exit 0
fi

pass=0
fail=0

# 断言助手：$1 = 用例名；$2 = 实际值；$3 = 期望值。
assert_eq() {
  if [ "$2" = "$3" ]; then
    pass=$((pass + 1))
  else
    fail=$((fail + 1))
    echo "FAIL：$1"
    echo "  期望：$3"
    echo "  实际：$2"
  fi
}

# 造一份 fixture bundle 目录：$1 = 平台目录名；$2 = 产物文件名（产物与 .sig
# 同名成对；.sig 内容 = "signature-of-$2"）。
make_fixture() {
  local platform_dir="$1" name="$2" root="$3"
  mkdir -p "$root/src-tauri/target/release/bundle/$platform_dir"
  echo "fake-payload" > "$root/src-tauri/target/release/bundle/$platform_dir/$name"
  echo "signature-of-$name" > "$root/src-tauri/target/release/bundle/$platform_dir/$name.sig"
}

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# ---- 用例 1：macOS 产物 → darwin-aarch64 条目，URL 拼接与 signature 内容 ----
root1="$tmp/case1"
make_fixture "macos" "CraftWave.app.tar.gz" "$root1"
cd "$root1"
out1="$(GITHUB_REPOSITORY=Yangshifu1024/CraftWave bash "$GEN" platform-entry v0.1.1)"
assert_eq "用例1 平台键" "$(echo "$out1" | jq -r 'keys[0]')" "darwin-aarch64"
assert_eq "用例1 产物 URL" \
  "$(echo "$out1" | jq -r '."darwin-aarch64".url')" \
  "https://github.com/Yangshifu1024/CraftWave/releases/download/v0.1.1/CraftWave.app.tar.gz"
assert_eq "用例1 signature 内容" \
  "$(echo "$out1" | jq -r '."darwin-aarch64".signature')" \
  "signature-of-CraftWave.app.tar.gz"

# ---- 用例 2：Windows nsis 产物 → windows-x86_64 条目 ----
root2="$tmp/case2"
make_fixture "nsis" "CraftWave_0.1.1_x64-setup.exe" "$root2"
cd "$root2"
out2="$(GITHUB_REPOSITORY=acme/widget bash "$GEN" platform-entry 0.1.1)"
assert_eq "用例2 平台键" "$(echo "$out2" | jq -r 'keys[0]')" "windows-x86_64"
assert_eq "用例2 产物 URL" \
  "$(echo "$out2" | jq -r '."windows-x86_64".url')" \
  "https://github.com/acme/widget/releases/download/0.1.1/CraftWave_0.1.1_x64-setup.exe"

# ---- 用例 3：Linux AppImage 产物 → linux-x86_64 条目 ----
root3="$tmp/case3"
make_fixture "appimage" "craftwave_0.1.1_amd64.AppImage" "$root3"
cd "$root3"
out3="$(bash "$GEN" platform-entry v0.1.1)"
assert_eq "用例3 平台键（GITHUB_REPOSITORY 未设时用脚本缺省 repo）" \
  "$(echo "$out3" | jq -r '."linux-x86_64".url' | cut -d/ -f4-5)" \
  "Yangshifu1024/CraftWave"

# ---- 用例 4：产物缺 .sig → 报错退出（签名缺失不能组装出可校验条目）----
root4="$tmp/case4"
mkdir -p "$root4/src-tauri/target/release/bundle/macos"
echo "fake" > "$root4/src-tauri/target/release/bundle/macos/CraftWave.app.tar.gz"
cd "$root4"
if bash "$GEN" platform-entry v0.1.1 > /dev/null 2>&1; then
  fail=$((fail + 1))
  echo "FAIL：用例4 缺 .sig 应报错退出，实际成功"
else
  pass=$((pass + 1))
fi

# ---- 用例 5：merge 三平台条目 → 完整 latest.json 形态与版本号剥离 ----
root5="$tmp/case5"
mkdir -p "$root5/meta"
echo '{"darwin-aarch64": {"signature": "sig-mac", "url": "https://github.com/o/r/releases/download/v0.1.1/CraftWave.app.tar.gz"}}' > "$root5/meta/macos-latest.json"
echo '{"windows-x86_64": {"signature": "sig-win", "url": "https://github.com/o/r/releases/download/v0.1.1/CraftWave_0.1.1_x64-setup.exe"}}' > "$root5/meta/windows-latest.json"
echo '{"linux-x86_64": {"signature": "sig-linux", "url": "https://github.com/o/r/releases/download/v0.1.1/craftwave_0.1.1_amd64.AppImage"}}' > "$root5/meta/ubuntu-latest.json"
cd "$root5"
out5="$(bash "$GEN" merge v0.1.1 meta)"
assert_eq "用例5 version 剥离 v 前缀" "$(echo "$out5" | jq -r .version)" "0.1.1"
assert_eq "用例5 notes 缺省值" "$(echo "$out5" | jq -r .notes)" "CraftWave v0.1.1"
assert_eq "用例5 pub_date 形态（ISO 8601 UTC）" \
  "$(echo "$out5" | jq -r .pub_date | grep -cE '^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$')" \
  "1"
assert_eq "用例5 三平台 key 齐全" \
  "$(echo "$out5" | jq -r '.platforms | keys | sort | join(",")')" \
  "darwin-aarch64,linux-x86_64,windows-x86_64"
assert_eq "用例5 平台条目透传" \
  "$(echo "$out5" | jq -r '."platforms"."darwin-aarch64".signature')" \
  "sig-mac"

# ---- 用例 6：merge 缺平台条目 → 报错退出（三平台缺一不可，防漏传产物）----
root6="$tmp/case6"
mkdir -p "$root6/meta"
echo '{"darwin-aarch64": {"signature": "s", "url": "u"}}' > "$root6/meta/macos-latest.json"
cd "$root6"
if bash "$GEN" merge v0.1.1 meta > /dev/null 2>&1; then
  fail=$((fail + 1))
  echo "FAIL：用例6 缺平台条目应报错退出，实际成功"
else
  pass=$((pass + 1))
fi

# ---- 用例 7：notes 经 RELEASE_NOTES 覆盖（CI 传 release body）----
out7="$(cd "$root5" && RELEASE_NOTES="修复说明正文" bash "$GEN" merge v0.2.0 meta)"
assert_eq "用例7 notes 覆盖" "$(echo "$out7" | jq -r .notes)" "修复说明正文"
assert_eq "用例7 version 随 tag 更新" "$(echo "$out7" | jq -r .version)" "0.2.0"

echo
echo "结果：$pass 通过，$fail 失败"
[ "$fail" -eq 0 ]
