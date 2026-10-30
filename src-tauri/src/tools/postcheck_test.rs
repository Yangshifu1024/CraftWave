//! 写后完整性自检测试（22 号票）：正常写、故障注入（重读不一致）、
//! CRLF 保持、拒绝写二进制、错误回喂构造。

use std::path::Path;

use super::{EolExpect, PostcheckFailure, postcheck_error, verify_written};

/// 在临时目录写一份内容并返回路径（测试基准工具）。
fn write_tmp(tag: &str, bytes: &[u8]) -> std::path::PathBuf {
    let dir = tempfile::tempdir().expect("临时目录");
    // tempdir 作用域问题：把文件写到进程级共享临时区（测试尾清理交给系统），
    // 这里换用一次性目录 + 手动延迟删除不现实——直接用 tempdir 的 path
    // 写文件并 leak 目录（测试进程退出即回收，量级可忽略）。
    let leaked = dir.keep();
    let path = leaked.join(format!("postcheck-{tag}.txt"));
    std::fs::write(&path, bytes).expect("写测试文件");
    path
}

/// 断言校验失败且阶段名匹配。
fn assert_fail(result: Result<(), PostcheckFailure>, stage: &str) -> PostcheckFailure {
    let Err(f) = result else {
        panic!("应校验失败（阶段 {stage}）")
    };
    assert_eq!(f.stage, stage, "失败阶段不符：{f:?}");
    f
}

#[test]
fn normal_write_passes() {
    let path = write_tmp("ok", b"hello craftwave\n");
    verify_written(&path, b"hello craftwave\n", EolExpect::Any).expect("正常写应通过");
    // 内容一致时 Preserve(false) 也应通过（LF 文件无 CRLF）。
    verify_written(&path, b"hello craftwave\n", EolExpect::Preserve(false)).expect("LF 保持应通过");
}

#[test]
fn injected_mismatch_fails_at_hash_stage() {
    // 故障注入：磁盘实际内容与写入预期不符（等价坏块/篡改场景）。
    let path = write_tmp("mismatch", b"actual bytes on disk");
    let failure = assert_fail(
        verify_written(&path, b"expected bytes", EolExpect::Any),
        "哈希",
    );
    assert!(
        failure.message.contains("不一致"),
        "描述应指明不一致：{failure:?}"
    );
}

#[test]
fn injected_reread_failure_is_reported() {
    // 注入：目标不存在（重读失败）。
    let missing = Path::new("/nonexistent/cw-postcheck-probe.txt");
    let failure = assert_fail(verify_written(missing, b"x", EolExpect::Any), "重读");
    assert!(failure.message.contains("重读失败"), "{failure:?}");
}

#[test]
fn binary_content_is_rejected() {
    // 拒绝写二进制：内容含 NUL（非严格 UTF-8 时按 UTF-8 阶段拦截；
    // NUL 字节在哈希一致的前提下也应被 UTF-8 阶段拦下）。
    let binary: &[u8] = b"ok\x00\xff\xfe binary";
    let path = write_tmp("binary", binary);
    // 哈希一致（磁盘内容 = 预期）→ 落到 UTF-8 阶段失败。
    let failure = assert_fail(verify_written(&path, binary, EolExpect::Any), "UTF-8");
    assert!(failure.message.contains("UTF-8"), "{failure:?}");
}

#[test]
fn crlf_preserve_is_enforced() {
    // CRLF 文件（写回保持 CRLF）→ 通过。
    let crlf: &[u8] = b"a\r\nb\r\n";
    let path = write_tmp("crlf-ok", crlf);
    verify_written(&path, crlf, EolExpect::Preserve(true)).expect("CRLF 保持应通过");

    // 故障注入：EOL 还原被破坏——磁盘上混入裸 LF（与预期不一致也会先卡
    // 哈希，故此处构造「哈希一致但风格错」：预期与磁盘同为含裸 LF 的
    // 内容，Preserve(true) 要求下 EOL 阶段必须拦下）。
    let broken: &[u8] = b"a\nb\r\n";
    let path = write_tmp("crlf-broken", broken);
    let failure = assert_fail(
        verify_written(&path, broken, EolExpect::Preserve(true)),
        "EOL",
    );
    assert!(failure.message.contains("裸 LF"), "{failure:?}");

    // 反向：LF 文件被引入 CRLF → EOL 阶段拦下。
    let path = write_tmp("lf-broken", b"a\r\nb\n");
    let failure = assert_fail(
        verify_written(&path, b"a\r\nb\n", EolExpect::Preserve(false)),
        "EOL",
    );
    assert!(failure.message.contains("CRLF"), "{failure:?}");
}

#[test]
fn postcheck_error_carries_path_and_stage() {
    let failure = PostcheckFailure {
        stage: "哈希",
        message: "内容不一致".to_string(),
    };
    let err = postcheck_error("src/main.rs", &failure);
    assert_eq!(err.code, "E_INTERNAL", "错误码用 E_INTERNAL（§3.6 兜底）");
    assert!(
        err.message.contains("src/main.rs"),
        "message 含路径：{}",
        err.message
    );
    assert!(
        err.message.contains("哈希"),
        "message 含校验阶段：{}",
        err.message
    );
}
