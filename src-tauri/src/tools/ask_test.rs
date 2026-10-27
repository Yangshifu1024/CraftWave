//! ask 工具纯逻辑单测（§8.3 ask 行）：问卷校验与 AskPayload 构造。

use super::ask::{AskArgs, AskQuestionArg, build_payload};

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

#[test]
fn builds_question_payload() {
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
}

#[test]
fn empty_title_or_questions_rejected() {
    let err = build_payload(&AskArgs {
        title: " ".to_owned(),
        questions: vec![question("q1", None, true)],
    })
    .unwrap_err();
    assert_eq!(err.code, "E_INVALID_ARG");

    let err = build_payload(&AskArgs {
        title: "t".to_owned(),
        questions: vec![],
    })
    .unwrap_err();
    assert!(err.message.contains("至少一个问题"));
}

#[test]
fn no_options_no_free_text_rejected() {
    let err = build_payload(&AskArgs {
        title: "t".to_owned(),
        questions: vec![question("q1", None, false)],
    })
    .unwrap_err();
    assert!(err.message.contains("自由文本"), "{}", err.message);
}

#[test]
fn empty_or_duplicate_option_ids_rejected() {
    let err = build_payload(&AskArgs {
        title: "t".to_owned(),
        questions: vec![question("q1", Some(vec![]), true)],
    })
    .unwrap_err();
    assert!(err.message.contains("空列表"));

    let err = build_payload(&AskArgs {
        title: "t".to_owned(),
        questions: vec![question("q1", Some(vec![("a", "A"), ("a", "B")]), true)],
    })
    .unwrap_err();
    assert!(err.message.contains("重复"));
}
