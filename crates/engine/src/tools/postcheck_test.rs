//! 写后完整性自检测试：正常写、故障注入（重读不一致）、
//! CRLF 保持、拒绝写二进制、错误回喂构造（矩阵化合并）。

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

// 断言：verify_written 校验矩阵——内容一致（Any/LF 保持）通过；故障注入
// 磁盘不一致卡哈希阶段；目标缺失卡重读阶段；NUL 内容卡 UTF-8 阶段；
// CRLF 保持被破坏（裸 LF）与 LF 被引入 CRLF 均卡 EOL 阶段；失败构造的
// ToolError 带 E_INTERNAL 码与路径、校验阶段。
#[test]
fn verify_written_pass_and_failure_matrix() {
    // 正常写：Any 与 LF 保持（Preserve(false)）都通过。
    let path = write_tmp("ok", b"hello craftwave\n");
    verify_written(&path, b"hello craftwave\n", EolExpect::Any).expect("正常写应通过");
    verify_written(&path, b"hello craftwave\n", EolExpect::Preserve(false)).expect("LF 保持应通过");

    // 故障注入：磁盘实际内容与写入预期不符（等价坏块/篡改场景）→ 哈希阶段。
    let path = write_tmp("mismatch", b"actual bytes on disk");
    let failure = assert_fail(
        verify_written(&path, b"expected bytes", EolExpect::Any),
        "哈希",
    );
    assert!(
        failure.message.contains("不一致"),
        "描述应指明不一致：{failure:?}"
    );

    // 注入：目标不存在（重读失败）→ 重读阶段。
    let missing = Path::new("/nonexistent/cw-postcheck-probe.txt");
    let failure = assert_fail(verify_written(missing, b"x", EolExpect::Any), "重读");
    assert!(failure.message.contains("重读失败"), "{failure:?}");

    // 拒绝写二进制：NUL 字节在哈希一致的前提下被 UTF-8 阶段拦下。
    let binary: &[u8] = b"ok\x00\xff\xfe binary";
    let path = write_tmp("binary", binary);
    let failure = assert_fail(verify_written(&path, binary, EolExpect::Any), "UTF-8");
    assert!(failure.message.contains("UTF-8"), "{failure:?}");

    // CRLF 文件（写回保持 CRLF）→ 通过。
    let crlf: &[u8] = b"a\r\nb\r\n";
    let path = write_tmp("crlf-ok", crlf);
    verify_written(&path, crlf, EolExpect::Preserve(true)).expect("CRLF 保持应通过");

    // 故障注入：EOL 还原被破坏——「哈希一致但风格错」（预期与磁盘同为含
    // 裸 LF 的内容），Preserve(true) 要求下 EOL 阶段拦下。
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

    // 错误回喂构造：E_INTERNAL 码 + 路径 + 校验阶段。
    let failure = PostcheckFailure {
        stage: "哈希",
        message: "内容不一致".to_string(),
    };
    let err = postcheck_error("src/main.rs", &failure);
    assert_eq!(err.code, "E_INTERNAL", "错误码用 E_INTERNAL（通用兜底）");
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
