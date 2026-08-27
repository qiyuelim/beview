//! 会话上下文装配点（V6-M5a，ADR-0023 D5）。
//!
//! **边界**：仅会话型出口（drill 对话流）经此装配；批量分析/单题分析/判卷/解析等
//! 无状态契约维持 contracts 层直连——强行统一是无历史的仪式感。
//! 全仓唯一实现：会话上下文拼接只允许出现在本模块（验收含 grep 断言）。
//!
//! ## 装配与裁剪分层（逼近上限时自下而上砍）
//! 1. 人设前缀（不可裁）
//! 2. 面试官笔记（不可裁）
//! 3. 最近 3 轮完整对话
//! 4. 更早轮次规则压缩（每轮一行问答摘要；V6 不做 LLM 摘要）
//! 5. dossier / 自有题库候选 / 参考内容（同层，按序砍）
//! 6. JD 片段（≤JD_TOKEN_BUDGET）
//!
//! ## 预算模型（三条独立上限，互不混算——修订自初版单一梯子的歧义）
//! - 人设+笔记 ≤PREFIX_TOKEN_BUDGET（不可裁层，预算仅观测口径）
//! - 历史窗（最近3轮完整+压缩行） ≤HISTORY_TOKEN_BUDGET
//! - JD ≤JD_TOKEN_BUDGET；中低块（题本/题库/参考）合计 ≤CONTEXT_TOKEN_BUDGET，
//!   超限按 references → bank → dossier 顺序丢弃（组装序即优先级逆序）
//!
//! token 估算采用 `chars/2` 的中文经验近似（1 token ≈ 1.5~2 汉字），确定性可测。

/// system_prefix token 预算（人设+笔记；不可裁层，预算仅作观测口径）
use serde_json::Value;

use crate::error::AppError;

pub const PREFIX_TOKEN_BUDGET: usize = 4000;
/// 历史窗口 token 预算（仅约束对话轮次：最近3轮完整 + 更早轮次压缩行）
pub const HISTORY_TOKEN_BUDGET: usize = 8000;
/// JD 片段 token 预算
pub const JD_TOKEN_BUDGET: usize = 1000;
/// 中低优先级块（题本+题库候选+参考内容）合计 token 预算
pub const CONTEXT_TOKEN_BUDGET: usize = 3000;
/// 最近 N 轮完整保留（ADR-0023 D5 分层第 3 层）
pub const RECENT_FULL_TURNS: usize = 3;

/// 对话轮次（ADR-0023 D5 规范结构）。历史经压缩处理后按时间序输出。
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    /// 原始角色（"ai" | "user"）；渲染层据此格式化为【面试官提问】/【候选人回答】
    pub role: String,
    pub content: String,
}

/// 粗略 token 估算：非空白字符数 ÷ 2（中文经验近似；确定性、无外部依赖）。
fn estimate_tokens(s: &str) -> usize {
    (s.chars().filter(|c| !c.is_whitespace()).count() + 1) / 2
}

/// 会话装配输入：各来源的原始块
#[derive(Default)]
pub struct SessionInput<'a> {
    /// 人设前缀（persona.persona_prompt + focus_tags 渲染）；None = 经典模式默认开场
    pub persona_prefix: Option<&'a str>,
    /// 面试官笔记（interview_state JSONB 原文），渲染为四段摘要
    pub notes: Option<&'a serde_json::Value>,
    pub dossier: Option<&'a serde_json::Value>,
    pub references: Option<&'a str>,
    pub bank_lines: &'a [String],
    pub jd_text: Option<&'a str>,
    /// 对话历史轮次（按时间序；role = ai/user）
    pub turns: &'a [Turn],
}

/// 装配产物（ADR-0023 D5 规范结构）：稳定前缀 + 处理后轮次
#[derive(Debug, Clone, PartialEq)]
pub struct SessionContext {
    pub system_prefix: String,
    pub turns: Vec<Turn>,
    /// 观测：各段实际 token 估算（OTel/日志用）
    pub tokens_prefix: usize,
    pub tokens_history: usize,
}

/// 纯装配核心（无 IO，便于单测）：分层裁剪并组装
pub fn assemble_session(input: &SessionInput<'_>) -> SessionContext {
    // ---------- ① 人设前缀 + 笔记（不可裁层） ----------
    let mut prefix = String::from("【本场信息】\n");
    // 每场陪练都有人格归属（未传时后端解析「经典面试官」内置种子）；空字符串不渲染
    if let Some(p) = input.persona_prefix.filter(|s| !s.trim().is_empty()) {
        prefix.push_str(&format!("【面试官人设】{}\n", p.trim()));
    }
    if let Some(notes) = render_notes_block(input.notes) {
        prefix.push_str(&notes);
    }

    // ---------- ② 历史窗（仅约束轮次）：最近 3 轮完整 + 更早轮次一行压缩 ----------
    let (older, recent): (&[Turn], &[Turn]) = if input.turns.len() > RECENT_FULL_TURNS {
        input.turns.split_at(input.turns.len() - RECENT_FULL_TURNS)
    } else {
        (&[], input.turns)
    };
    let mut compressed: Vec<String> = older.iter().map(|t| compress_turn(t)).collect();
    let recent_tokens: usize = recent.iter().map(|t| estimate_tokens(&t.content)).sum();

    // 压缩行合计超预算时从最旧的开始整体丢弃
    while estimate_tokens(&compressed.join("\n")) > HISTORY_TOKEN_BUDGET.saturating_sub(recent_tokens)
        && !compressed.is_empty()
    {
        compressed.remove(0);
    }
    let tokens_history = recent_tokens + estimate_tokens(&compressed.join("\n"));

    // ---------- ③ JD：恒定封顶在自身预算内 ----------
    let mut jd_block = String::new();
    if let Some(j) = input.jd_text.map(str::trim).filter(|s| !s.is_empty()) {
        let cap_chars = JD_TOKEN_BUDGET * 2; // 与 chars/2 估算对称的字符上限
        let cut = truncate_chars_cap(j, cap_chars);
        jd_block = format!("【目标岗位 JD】（出题与追问以此为准）：\n{cut}\n");
    }

    // ---------- ④ 中低块：合计 ≤CONTEXT_TOKEN_BUDGET，超限按组装序逆序丢弃 ----------
    let mut mid_blocks: Vec<(String, usize)> = Vec::new(); // (渲染文本, token 数)，push 序即优先级降序
    if let Some(d) = input.dossier {
        let rendered = render_dossier_block(d);
        if !rendered.is_empty() {
            let t = estimate_tokens(&rendered);
            mid_blocks.push((rendered, t));
        }
    }
    if !input.bank_lines.is_empty() {
        let rendered = format!(
            "自有题库候选（出题可优先复用或改编）：\n{}\n",
            input.bank_lines.join("\n")
        );
        let t = estimate_tokens(&rendered);
        mid_blocks.push((rendered, t));
    }
    if let Some(r) = input.references.map(str::trim).filter(|s| !s.is_empty()) {
        let rendered = format!("参考内容（岗位要求/面经/参考题，出题请优先参考）：\n{r}\n");
        let t = estimate_tokens(&rendered);
        mid_blocks.push((rendered, t));
    }
    // 组装序末尾 = 最低优先级（references 最后 push，最先被丢）；极端情况下 dossier 也可能被丢
    while mid_blocks.iter().map(|(_, t)| *t).sum::<usize>() > CONTEXT_TOKEN_BUDGET && !mid_blocks.is_empty() {
        mid_blocks.pop();
    }

    // ---------- 组装输出 ----------
    let mut turns_out: Vec<Turn> = Vec::new();
    for line in &compressed {
        turns_out.push(Turn { role: "user".into(), content: format!("（历史摘要）{line}") });
    }
    turns_out.extend(recent.iter().cloned());

    let mut system_prefix = prefix;
    for (text, _) in &mid_blocks {
        system_prefix.push_str(text);
    }
    system_prefix.push_str(&jd_block);

    let tokens_prefix = estimate_tokens(&system_prefix);
    SessionContext { system_prefix, turns: turns_out, tokens_prefix, tokens_history }
}

/// 面试官笔记 → 四段摘要块（不可裁层；缺段跳过）
fn render_notes_block(notes: Option<&serde_json::Value>) -> Option<String> {
    let n = notes?;
    let mut out = String::from("【面试官笔记 (Interviewer Notes)】（课前备课结论，本场提问与追问以此为导向）：\n");
    let mut any = false;
    for (label, key) in [
        ("岗位要求", "job_requirements"),
        ("候选人事实", "candidate_facts"),
        ("风险信号", "risk_signals"),
        ("建议追问", "next_followups"),
    ] {
        if let Some(arr) = n.get(key).and_then(|v| v.as_array()) {
            let items: Vec<&str> = arr.iter().filter_map(|x| x.as_str()).filter(|s| !s.trim().is_empty()).collect();
            if !items.is_empty() {
                out.push_str(&format!("- {}：{}\n", label, items.join("；")));
                any = true;
            }
        }
    }
    if !any {
        return None;
    }
    out.push_str("请围绕笔记中的「风险信号」与「建议追问」定向考察。\n");
    Some(out)
}

/// 考官题本块（圈定出题范围）
fn render_dossier_block(d: &serde_json::Value) -> String {
    let mut out = String::new();
    if let Some(summary) = d.get("summary").and_then(|v| v.as_str()) {
        out.push_str(&format!("考核侧重：{}\n", summary));
    }
    if let Some(qs) = d.get("questions").and_then(|v| v.as_array()) {
        out.push_str("重点考核题目与标准参考：\n");
        for (i, q) in qs.iter().enumerate() {
            let q_text = q.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let ref_ans = q.get("ref_answer").and_then(|v| v.as_str()).unwrap_or("");
            out.push_str(&format!("  {}. 题目：{}\n", i + 1, q_text));
            if !ref_ans.is_empty() {
                out.push_str(&format!("     标准参考：{}\n", ref_ans));
            }
        }
    }
    out
}

/// 更早轮次的一行压缩：保留问答骨架与角色标签，整体截断到 ~80 字符（复用统一截断器）
fn compress_turn(t: &Turn) -> String {
    let tag = if t.role == "ai" { "【面试官】" } else { "【候选人】" };
    truncate_chars_cap(&format!("{tag}{}", t.content.trim()), 80)
}

fn truncate_chars_cap(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

// ==================== DB 装配收口（ADR-0023 D5 规范 trait）====================

/// ADR-0023 D5 规范 trait：会话上下文装配的唯一入口。
/// `up_to_msg` 限定历史轮次的消息 id 上界（幂等重放/回溯场景使用；常规调用传当前最大消息 id）。
pub trait SessionContextAssembler {
    /// DB 装配需要 IO：签名在 ADR 基础上 async 化（Rust 2024 原生 AFIT），其余与规范一致。
    fn assemble(&self, drill_id: i64, up_to_msg: i64) -> impl std::future::Future<Output = std::result::Result<SessionContext, AppError>> + Send;
}

/// drill 会话装配器：负责全部 DB 取数，纯装配核心在 `assemble_session`。
pub struct DrillSessionAssembler {
    pub pool: sqlx::PgPool,
    pub uid: i64,
}

#[derive(sqlx::FromRow)]
struct PersonaEngineRow {
    persona_prompt: Option<String>,
    focus_tags_str: Option<String>,
}

impl SessionContextAssembler for DrillSessionAssembler {
    async fn assemble(&self, drill_id: i64, up_to_msg: i64) -> std::result::Result<SessionContext, AppError> {
        let drill: (Option<Value>, Option<Value>, Option<String>, Option<String>, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT d.interview_state, d.dossier, d.ref_content, p.jd_text, d.position, d.direction
             FROM drills d
             LEFT JOIN applications a ON a.id = d.application_id
             LEFT JOIN positions p ON p.id = a.position_id
             WHERE d.id=$1 AND d.user_id=$2",
        )
        .bind(drill_id)
        .bind(self.uid)
        .fetch_one(&self.pool)
        .await?;
        let (interview_state, dossier, references, jd_text, position, direction) = drill;

        // 人格行：温度提示 + 人设 + focus tags（经典模式全空）
        let persona: Option<PersonaEngineRow> = sqlx::query_as(
            r#"SELECT ip.persona_prompt AS persona_prompt,
                      array_to_string(ip.focus_tags, '、') AS focus_tags_str
               FROM drills d JOIN interviewer_personas ip ON ip.id = d.persona_id
               WHERE d.id=$1 AND d.user_id=$2"#,
        )
        .bind(drill_id)
        .bind(self.uid)
        .fetch_optional(&self.pool)
        .await?;

        let bank_lines: Vec<String> =
            load_bank_samples(&self.pool, self.uid, &position, &direction).await;

        let turn_rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT role, content FROM drill_messages WHERE drill_id=$1 AND id<=$2 AND ((role='ai' AND kind IN ('question','probe')) OR (role='user' AND kind='answer')) ORDER BY id ASC",
        )
        .bind(drill_id)
        .bind(up_to_msg)
        .fetch_all(&self.pool)
        .await?;
        let turns: Vec<Turn> = turn_rows
            .into_iter()
            .map(|(role, content)| {
                let content = if role == "ai" {
                    format!("【面试官提问】{content}")
                } else {
                    format!("【候选人回答】{content}")
                };
                Turn { role, content }
            })
            .collect();

        let persona_prefix: Option<String> = persona.as_ref().map(|p| {
            let mut s = p.persona_prompt.clone().unwrap_or_default();
            if let Some(focus) = p.focus_tags_str.as_deref().filter(|f| !f.is_empty()) {
                s.push_str(&format!("\n考察侧重：{focus}"));
            }
            s
        });

        Ok(assemble_session(&SessionInput {
            persona_prefix: persona_prefix.as_deref(),
            notes: interview_state.as_ref(),
            dossier: dossier.as_ref(),
            references: references.as_deref(),
            bank_lines: &bank_lines,
            jd_text: jd_text.as_deref(),
            turns: &turns,
        }))
    }
}

impl DrillSessionAssembler {
    /// 温度即人格（M5a）：该场次人格的采样温度提示（None = 经典模式/未设置）
    pub async fn persona_temperature_hint(&self, drill_id: i64) -> std::result::Result<Option<f64>, AppError> {
        let t: Option<Option<f64>> = sqlx::query_scalar(
            "SELECT ip.temperature_hint::float8 FROM drills d JOIN interviewer_personas ip ON ip.id = d.persona_id WHERE d.id=$1 AND d.user_id=$2",
        )
        .bind(drill_id)
        .bind(self.uid)
        .fetch_optional(&self.pool)
        .await?
        .map(|(v,)| v);
        Ok(t.flatten())
    }
}

/// 题库优先抽取（ADR-0008 §3）：按岗位/方向关键词取最近已分析题（含参考答案），供 AI 复用/改编。
/// 会话装配域的取数逻辑——从 drills.rs 迁入（M5a 收口）。
pub(crate) async fn load_bank_samples(
    pool: &sqlx::PgPool,
    uid: i64,
    position: &Option<String>,
    direction: &Option<String>,
) -> Vec<String> {
    let limit: i64 = 3;
    let kws: Vec<String> = [position, direction]
        .iter()
        .filter_map(|s| s.as_ref().filter(|s| !s.is_empty()).map(|s| s.to_string()))
        .collect();
    let mut out = Vec::new();
    async fn fetch(
        pool: &sqlx::PgPool,
        sql: &str,
        uid: i64,
        kw: Option<&String>,
        limit: i64,
    ) -> Vec<(String, Option<String>)> {
        let mut b = sqlx::query_as::<_, (String, Option<String>)>(sql);
        if let Some(k) = kw {
            b = b.bind(k);
        }
        b.bind(limit).bind(uid).fetch_all(pool).await.unwrap_or_default()
    }
    let sql_kw = r#"
        SELECT q.content, a.ref_answer FROM questions q
        JOIN analyses a ON a.id = (SELECT a2.id FROM analyses a2 WHERE a2.question_id=q.id ORDER BY a2.created_at DESC, a2.id DESC LIMIT 1)
        WHERE q.user_id=$3 AND q.source='manual' AND (q.content ILIKE '%'||$1||'%' OR COALESCE(a.tags::text,'') ILIKE '%'||$1||'%')
        ORDER BY q.created_at DESC LIMIT $2"#;
    let sql_all = r#"
        SELECT q.content, a.ref_answer FROM questions q
        JOIN analyses a ON a.id = (SELECT a2.id FROM analyses a2 WHERE a2.question_id=q.id ORDER BY a2.created_at DESC, a2.id DESC LIMIT 1)
        WHERE q.user_id=$2 AND q.source='manual' ORDER BY q.created_at DESC LIMIT $1"#;
    for k in &kws {
        if out.len() >= limit as usize {
            break;
        }
        for (c, r) in fetch(pool, sql_kw, uid, Some(k), limit).await {
            out.push(format!("- 题目：{c}\n  参考答案：{}", r.unwrap_or_default()));
        }
    }
    if out.len() < limit as usize {
        for (c, r) in fetch(pool, sql_all, uid, None, limit).await {
            out.push(format!("- 题目：{c}\n  参考答案：{}", r.unwrap_or_default()));
        }
    }
    out.truncate(limit as usize);
    out
}

/// 系统公司「模拟面试」/ 批次「AI 训练」/ 轮次「AI 生成」（首次沉淀时自动建）
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn older_turns_compressed_recent_three_intact() {
        let turns: Vec<Turn> = (0..10)
            .flat_map(|i| {
                vec![
                    Turn { role: "ai".into(), content: format!("【面试官提问】问题{i} {}", "详".repeat(200)) },
                    Turn { role: "user".into(), content: format!("【候选人回答】回答{i}") },
                ]
            })
            .collect();
        let ctx = assemble_session(&SessionInput { turns: &turns, ..Default::default() });
        // 最近 3 行完整保留
        assert_eq!(ctx.turns.last().unwrap(), &turns[turns.len() - 1]);
        // 更早轮次带压缩标记且被截断到一行摘要
        let first = &ctx.turns[0];
        assert!(first.content.starts_with("（历史摘要）"), "{}", first.content);
        assert!(first.content.chars().count() < turns[0].content.chars().count());
    }

    #[test]
    fn jd_capped_independently_and_dossier_kept() {
        let big_jd = format!("岗位要求：{}", "长".repeat(6000)); // 远超 1K token 预算
        let dossier = json!({
            "summary": "侧重分布式一致性",
            "questions": [{ "content": "讲讲 Raft", "ref_answer": "多数派提交" }]
        });
        let ctx = assemble_session(&SessionInput {
            dossier: Some(&dossier),
            jd_text: Some(&big_jd),
            ..Default::default()
        });
        // JD 封顶在 ~1K token（约 2K 字符）：出现截断省略号
        assert!(ctx.system_prefix.contains("…"), "超预算 JD 应带截断标记");
        assert!(
            !ctx.system_prefix.contains(&"长".repeat(1999)),
            "JD 应被裁剪到 ~1K token（约 2K 字符）"
        );
        assert!(ctx.system_prefix.contains("侧重分布式一致性"), "dossier 应保留");
    }

    #[test]
    fn mid_blocks_capped_independently_dropping_lowest_priority_first() {
        // 中低块自身预算压力：references 最先丢、bank 其次、dossier 最后丢
        let dossier = json!({ "summary": "D-KEEP-MARKER" });
        let bank: Vec<String> = (0..80).map(|i| format!("题库候选{i}：{}", "候".repeat(260))).collect();
        let refs = format!("参考面经：{}", "参".repeat(600));
        let ctx = assemble_session(&SessionInput {
            dossier: Some(&dossier),
            references: Some(&refs),
            bank_lines: &bank,
            ..Default::default()
        });
        assert!(ctx.system_prefix.contains("D-KEEP-MARKER"), "最高优先级中低块应最后被丢");
        assert!(!ctx.system_prefix.contains("参考面经"), "refs 应最先被丢弃");
        assert!(!ctx.system_prefix.contains("题库候选59"), "bank 应在 dossier 之前被丢弃");
    }

    #[test]
    fn prefix_never_trimmed_and_notes_rendered() {
        let notes = json!({
            "job_requirements": ["高并发"],
            "risk_signals": ["RISK-MARKER"],
            "next_followups": ["FOLLOW-MARKER"]
        });
        let persona = "犀利交叉官人设 PERSONA-MARKER";
        let ctx = assemble_session(&SessionInput {
            persona_prefix: Some(persona),
            notes: Some(&notes),
            ..Default::default()
        });
        assert!(ctx.system_prefix.contains(persona));
        assert!(ctx.system_prefix.contains("RISK-MARKER"));
        assert!(ctx.system_prefix.contains("FOLLOW-MARKER"));
        assert!(ctx.system_prefix.contains("面试官笔记"));
        // 人设前缀字节稳定：同输入两次装配结果完全一致（缓存友好断言）
        assert_eq!(ctx, assemble_session(&SessionInput {
            persona_prefix: Some(persona),
            notes: Some(&notes),
            ..Default::default()
        }));
    }

    #[test]
    fn classic_mode_omits_persona_block() {
        let ctx = assemble_session(&SessionInput::default());
        // 经典模式：完全不注入人设块（与人格驱动模式互不影响，ADR-0023 D1）
        assert!(!ctx.system_prefix.contains("面试官人设"), "{}", ctx.system_prefix);
    }
}
