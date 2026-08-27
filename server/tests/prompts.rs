//! 提示词处处可编辑（设置页）：清单返回全部注册 prompt；自定义值真实到达 LLM 请求边界；
//! 空值恢复内置默认；未知 key 拒绝。

mod common;

use axum::http::Method;
use common::llm_mock::LlmMock;
use common::TestApp;
use serde_json::{json, Value};

fn find_prompt<'a>(v: &'a Value, key: &str) -> &'a Value {
    v["prompts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["key"] == key)
        .unwrap_or_else(|| panic!("应注册 {key}"))
}

#[tokio::test]
async fn prompts_list_returns_all_registered_defaults() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    let (_, v) = app.req(Method::GET, "/api/settings/prompts", None).await;
    let arr = v["prompts"].as_array().unwrap();
    assert_eq!(arr.len(), 14, "应登记 14 个 LLM 出口提示词（V6.1 票 01 退役 paper_generate/paper_grade 后）");
    assert!(arr.iter().all(|p| p["is_custom"] == false), "初始全部为内置默认");
    assert!(
        arr.iter().all(|p| p["value"].as_str().map(|s| s.len() > 10).unwrap_or(false)),
        "每个 prompt 都应有非空默认值"
    );
    for key in [
        "prompt_drill_interview",
        "prompt_question_ref",
        "prompt_answer_evaluate",
        "prompt_question_full",
        "prompt_resume_parse",
        "prompt_jd_interpret",
        "prompt_jd_match",
        "prompt_retrospective",
        "prompt_application_overall",
        "prompt_position_predict",
        "prompt_tag_cleanup",
        "prompt_resume_optimize",
        "prompt_application_insights",
    ] {
        assert!(arr.iter().any(|p| p["key"] == key), "缺少 {key}");
    }
}

