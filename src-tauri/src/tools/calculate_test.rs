//! calculate 工具测试：四则/括号/浮点链/幂/比较/常量、除零与语法错误位置、
//! 整数溢出、Python 对照的格式化保真（票面测试计划逐项，矩阵化合并）。

use serde_json::json;

// 测试入口：calculate 不依赖工作区，直接对求值实现断言（dispatch 包装面
// run(ctx, args) 只做转调）。
use super::eval_tool as run;
use super::{CalculateArgs, evaluate};

/// 测试辅助：执行并取成功结果。
fn ok(expression: &str) -> serde_json::Value {
    let outcome = run(&CalculateArgs {
        expression: expression.to_string(),
    });
    assert!(outcome.ok, "{expression} 应求值成功：{outcome:?}");
    outcome.data.expect("data")
}

/// 测试辅助：执行并断言失败，返回（错误消息, uiSummary）。
fn err(expression: &str) -> String {
    let outcome = run(&CalculateArgs {
        expression: expression.to_string(),
    });
    assert!(!outcome.ok, "{expression} 应失败：{outcome:?}");
    outcome.error.expect("error").message
}

// 断言：求值成功面矩阵——四则与括号（整数通道精确）、运算优先级、除法永远
// 浮点（Python / 语义）、浮点链与 Python repr 逐字对照、幂（右结合）与负
// 指数、比较运算与布尔相等、pi/e 常量、run 的 data 形状与 uiSummary、
// evaluate 纯函数入口与 run 一致。
#[test]
fn evaluation_success_matrix() {
    // 四则与括号、优先级。
    assert_eq!(ok("2 + 3 * 4")["result"], json!(14));
    assert_eq!(ok("(2 + 3) * 4")["result"], json!(20));
    assert_eq!(ok("100 - 20 - 30")["result"], json!(50), "左结合");
    assert_eq!(ok("2 * 3 ^ 2")["result"], json!(18), "幂优先于乘");
    assert_eq!(ok("(2 * 3) ^ 2")["result"], json!(36));
    assert_eq!(ok("-3 + 5")["result"], json!(2));
    assert_eq!(ok("--3")["result"], json!(3), "一元负号可叠");
    assert_eq!(ok("7 % 3")["result"], json!(1));

    // 除法永远浮点；浮点链与 Python repr 逐字对照（最短往返表示）。
    let data = ok("0.1 + 0.2");
    assert_eq!(data["formatted"], json!("0.30000000000000004"));
    assert_eq!(ok("7 / 2")["formatted"], json!("3.5"));
    assert_eq!(ok("10 / 4")["formatted"], json!("2.5"));
    assert_eq!(ok("1 / 3")["formatted"], json!("0.3333333333333333"));
    // 整数相除再乘回：舍入回整值（与 Python `10/3*3 == 10.0` 一致）。
    assert_eq!(ok("10 / 3 * 3")["formatted"], json!("10"));
    // 整值浮点显示不带小数尾巴（Rust 最短往返）。
    assert_eq!(ok("6 / 2")["formatted"], json!("3"));
    // 科学计数法。
    assert_eq!(ok("1.5e3")["result"], json!(1500.0));
    assert_eq!(ok("2.5e-1")["result"], json!(0.25));

    // 幂（右结合）、负指数、比较运算、pi/e 常量。
    assert_eq!(ok("2 ^ 10")["result"], json!(1024));
    assert_eq!(ok("2 ^ 3 ^ 2")["result"], json!(512), "右结合：2^(3^2)");
    assert_eq!(ok("2 ^ -1")["formatted"], json!("0.5"), "负指数转浮点");
    assert_eq!(ok("(-2) ^ 2")["result"], json!(4));
    assert_eq!(ok("3 > 2")["result"], json!(true));
    assert_eq!(ok("3 <= 3")["result"], json!(true));
    assert_eq!(
        ok("0.1 + 0.2 == 0.3")["result"],
        json!(false),
        "浮点直接相等与 Python 一致"
    );
    assert_eq!(ok("1 != 2")["result"], json!(true));
    assert_eq!(ok("(1 > 2) == false")["result"], json!(true));
    // pi * 2 与 Python `math.pi * 2` 同为 6.283185307179586。
    assert_eq!(ok("pi * 2")["formatted"], json!("6.283185307179586"));
    assert_eq!(ok("e")["formatted"], json!("2.718281828459045"));

    // 整数溢出边界：浮点改写后可求值（舍入到 2^63；Rust Display 无指数形式，
    // 数值与 Python `9.223372036854776e+18` 相等）。
    let data = ok("9223372036854775807.0 + 1.0");
    assert_eq!(data["formatted"], json!("9223372036854776000"));

    // run 的 data 形状与 uiSummary。
    let outcome = run(&CalculateArgs {
        expression: "1 + 2".to_string(),
    });
    let data = outcome.data.expect("data");
    assert_eq!(data["expression"], json!("1 + 2"));
    assert_eq!(data["result"], json!(3));
    assert_eq!(data["formatted"], json!("3"));
    assert_eq!(outcome.ui_summary, "1 + 2 = 3");

    // evaluate 纯函数入口与 run 一致。
    assert_eq!(evaluate("1 + 2").expect("ok"), super::Value::Int(3));
    assert!(evaluate("1 /").is_err());
}

// 断言：错误面矩阵——除零（整除/浮点除/取余）带出错位置、浮点溢出 inf/NaN
// 被非有限兜底拦截、语法错误（意外结束/多余内容/无法识别字符/赋值/未知名称/
// 函数调用拒绝/未闭合括号/空表达式）带位置、整数 64 位 checked 溢出提示浮点
// 改写、布尔值不参与算术与链式比较。
#[test]
fn error_reporting_matrix() {
    // 除零：除号在第 3 个字符（1 空格 / → 字节位置 2 → 字符位置 3）。
    let message = err("1 / 0");
    assert!(message.contains("第 3 个字符"), "{message}");
    assert!(message.contains("除零"), "{message}");
    let message = err("1.5 / 0.0");
    assert!(message.contains("除零"), "{message}");
    let message = err("5 % (3 - 3)");
    assert!(message.contains("除零"), "{message}");

    // 浮点溢出产生 inf 的表达式被非有限结果兜底拦截（独立于除零面）。
    let outcome = run(&CalculateArgs {
        expression: "1e308 * 10".to_string(),
    });
    assert!(!outcome.ok, "inf 结果应拦截");
    assert_eq!(outcome.error.expect("error").code, "E_INVALID_ARG");
    assert!(
        outcome.ui_summary.contains("不是有限数"),
        "{}",
        outcome.ui_summary
    );
    // 0.0/0.0 在除法入口就被「除零」拦下，到不了非有限兜底。
    let outcome = run(&CalculateArgs {
        expression: "0.0 / 0.0".to_string(),
    });
    assert!(!outcome.ok);

    // 语法错误带出错位置。
    // "2 +"：意外结束，位置指向尾部（第 4 个字符附近）。
    let message = err("2 +");
    assert!(message.contains("第 4 个字符"), "{message}");
    // "1 2"：多余内容在第二个操作数（第 3 个字符）。
    let message = err("1 2");
    assert!(message.contains("第 3 个字符"), "{message}");
    assert!(message.contains("多余"), "{message}");
    // 无法识别的字符。
    let message = err("2 @ 3");
    assert!(message.contains("第 3 个字符"), "{message}");
    assert!(message.contains("无法识别"), "{message}");
    // 单个 = 按「不支持赋值」拒绝（比较相等须 ==）。
    let message = err("1 = 2");
    assert!(message.contains("赋值"), "{message}");
    // 未知名称在词法期拒绝（含 pi/e 之外的任何标识符）。
    let message = err("x + 1");
    assert!(message.contains("不支持的名称"), "{message}");
    // 函数调用形态显式拒绝（防任意函数调用面），位置在标识符起始。
    let message = err("sin(1)");
    assert!(message.contains("sin"), "{message}");
    assert!(message.contains("无函数调用"), "{message}");
    assert!(message.contains("第 1 个字符"), "{message}");
    // 未闭合括号。
    let message = err("(1 + 2");
    assert!(message.contains("第 1 个字符"), "{message}");
    // 空表达式。
    let message = err("   ");
    assert!(message.contains("表达式为空"), "{message}");

    // 整数 64 位通道 checked 溢出报错并提示浮点改写；超范围字面量词法期报错。
    let message = err("9223372036854775807 + 1");
    assert!(message.contains("溢出"), "{message}");
    assert!(message.contains("浮点"), "{message}");
    let message = err("9223372036854775807 * 9223372036854775807");
    assert!(message.contains("溢出"), "{message}");
    let message = err("99999999999999999999");
    assert!(message.contains("超出 64 位"), "{message}");

    // 布尔值不能参与算术/大小比较（链式比较禁止）。
    let message = err("(1 > 2) + 1");
    assert!(message.contains("布尔"), "{message}");
    let message = err("1 < 2 < 3");
    assert!(message.contains("布尔"), "{message}");
}
