//! ask 工具纯逻辑单测（§8.3 ask 行）：问卷校验与 AskPayload 构造。

use super::ask::{AskArgs, AskQuestionArg, build_payload};

/// 测试辅助：构造一个问题参数（options 为 (id, label) 列表或 None）。
fn question(id: &str, options: Option<Vec<(&str, &str)>>, free: bool) -> AskQuestionArg {
    AskQuestionArg {
        id: id.to_owned(),
        text: format!("问题 {id}"),
        options: options.map(|opts| {
            opts.iter()
                .map(|(oid, label)| super::ask::AskChoiceArg {
                    id: oid.to_string(),
                    label: label.to_string(),
                })
                .collect()
        }),
        allow_free_text: free,
    }
}

/// 测试辅助：断言问卷构造失败且消息含指定片段。
fn build_err(args: &AskArgs, expect_contains: &str) {
    let err = build_payload(args).expect_err("应校验失败");
    assert!(
        err.message.contains(expect_contains),
        "错误消息应含「{expect_contains}」：{}",
        err.message
    );
}

// 断言：合法问卷构造出 Question 形态的 AskPayload；空白标题、空问题列表、
// 既无选项又不许自由文本、空选项列表、重复选项 id 均被拒（校验矩阵）。
#[test]
fn payload_build_and_validation_matrix() {
    // 正常路径：字段与选项完整保留。
    let args = AskArgs {
        title: "确认方案".to_owned(),
        questions: vec![
            question("q1", Some(vec![("yes", "是"), ("no", "否")]), false),
            question("q2", None, true),
        ],
    };
    let payload = build_payload(&args).unwrap();
    let crate::backend::ask::AskPayload::Question { title, questions } = payload else {
        panic!("应为 Question 形态")
    };
    assert_eq!(title, "确认方案");
    assert_eq!(questions.len(), 2);
    assert_eq!(questions[0].options.as_ref().unwrap().len(), 2);
    assert!(questions[1].allow_free_text);

    // 校验矩阵：空白标题 / 空问题列表 / 无选项且无自由文本 / 空选项 / 重复选项 id。
    let invalid: Vec<(AskArgs, &str)> = vec![
        (
            AskArgs {
                title: " ".to_owned(),
                questions: vec![question("q1", None, true)],
            },
            "标题",
        ),
        (
            AskArgs {
                title: "t".to_owned(),
                questions: vec![],
            },
            "至少一个问题",
        ),
        (
            AskArgs {
                title: "t".to_owned(),
                questions: vec![question("q1", None, false)],
            },
            "自由文本",
        ),
        (
            AskArgs {
                title: "t".to_owned(),
                questions: vec![question("q1", Some(vec![]), true)],
            },
            "空列表",
        ),
        (
            AskArgs {
                title: "t".to_owned(),
                questions: vec![question("q1", Some(vec![("a", "A"), ("a", "B")]), true)],
            },
            "重复",
        ),
    ];
    for (args, fragment) in invalid {
        build_err(&args, fragment);
    }
}
