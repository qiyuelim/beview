//! M4 对话控制流/报告流分离（ADR-0023 D2）：answer 单次流式调用的输出协议层。
//!
//! 协议（纯输出约定，非工具契约）：
//! ```text
//! [追问元数据行（仅当本次输出为追问）]
//! <<<PROBE>>>{"anchor_keyword":"…","reason":"depth_probe|clarification|edge_case|contradiction|breadth_pivot"}
//! [续接正文：追问题干 或 新主问题正文 或 全场复盘 Markdown]
//! <<<REPORT>>>
//! {"tags":["…"],"difficulty":3,"ref_answer":"…","score":85,"feedback":"…"}   （仅当上一题考核结束）
//! ```
//!
//! 语义：模型基于回答原文自主决定「追问 or 切新题」；REPORT 段仅在切新题/收尾时出现，
//! 承载上一题的合并判分。评分落库时序不变——解析后的字段走与 grade_and_record 相同的
//! 持久化通道（见 drills.rs record_inline_analysis），错题本/积分/统计零影响。

use serde::Serialize;
use serde_json::Value;

/// 追问理由封闭枚举：直接映射前端徽章语义色/图标，并为「被追问维度」统计预置口径。
pub const PROBE_REASONS: [&str; 5] =
    ["depth_probe", "clarification", "edge_case", "contradiction", "breadth_pivot"];

pub const PROBE_REASON_LABELS: [(&str, &str); 5] = [
    ("depth_probe", "深挖"),
    ("clarification", "澄清"),
    ("edge_case", "边界"),
    ("contradiction", "矛盾"),
    ("breadth_pivot", "拓展"),
];

pub const SENTINEL_PROBE: &str = "<<<PROBE>>>";
pub const SENTINEL_REPORT: &str = "<<<REPORT>>>";

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ProbeMeta {
    pub anchor_keyword: String,
    pub reason: String,
}

/// 解析哨兵后的 `{…}` 元数据；括号配对扫描（容忍模型未换行直接接题干）。
/// reason 必须命中封闭枚举、anchor 非空，否则拒绝（返回 None）。
pub fn parse_probe_meta(after_sentinel: &str) -> Option<(ProbeMeta, String)> {
    let s = after_sentinel.trim();
    if !s.starts_with('{') {
        return None;
    }
    // 括号深度扫描提取首个平衡 JSON 对象
    let mut depth = 0usize;
    let mut in_str = false;
    let mut esc = false;
    let mut end = None;
    for (i, ch) in s.char_indices() {
        if esc {
            esc = false;
            continue;
        }
        match ch {
            '\\' if in_str => esc = true,
            '"' => in_str = !in_str,
            '{' if !in_str => depth += 1,
            '}' if !in_str => {
                depth -= 1;
                if depth == 0 {
                    end = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let json_end = end?;
    let v: Value = serde_json::from_str(&s[..=json_end]).ok()?;
    let anchor = v["anchor_keyword"].as_str()?.trim();
    let reason = v["reason"].as_str()?.trim();
    if anchor.is_empty() || !PROBE_REASONS.contains(&reason) {
        return None; // 枚举外输出被 schema 拒绝
    }
    let remaining = s[json_end + 1..].trim().to_string();
    Some((ProbeMeta { anchor_keyword: anchor.to_string(), reason: reason.to_string() }, remaining))
}

#[derive(Debug, Clone, PartialEq)]
pub enum Continuation {
    /// 追问（携带锚点+理由元数据；解析失败时降级为无徽章追问）
    Probe { meta: Option<ProbeMeta> },
    Question,
    Summary,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AnswerFlowOutput {
    pub continuation: Continuation,
    pub continuation_text: String,
    /// 上一题的合并判分报告（仅当模型决定切新题/收尾时出现）
    pub report: Option<Value>,
}

/// 拆分单次流式调用的完整输出文本。
///
/// - `expect_summary`：本轮为收尾轮（task 要求输出全场复盘）→ 续接按 Summary 处理；
///   若模型未输出 REPORT（漏评），report 为 None 由调用方兜底。
pub fn split_answer_output(text: &str, expect_summary: bool) -> AnswerFlowOutput {
    let (continuation_raw, report_raw) = match text.find(SENTINEL_REPORT) {
        Some(idx) => (&text[..idx], Some(text[idx + SENTINEL_REPORT.len()..].trim())),
        None => (text, None),
    };

    // 探测首部 PROBE 哨兵（容忍元数据与题干同行）；枚举外/畸形被 schema 拒绝：剥掉元数据，按普通追问处理
    let trimmed = continuation_raw.trim_start();
    let (continuation, body) = if trimmed.starts_with(SENTINEL_PROBE) {
        match parse_probe_meta(&trimmed[SENTINEL_PROBE.len()..]) {
            Some((meta, body)) => (Continuation::Probe { meta: Some(meta) }, body),
            None => {
                // 畸形元数据被拒绝：有换行则丢弃哨兵行，其余作为追问正文；无换行（同行粘连）
                // 无法可靠分离垃圾元数据与题干——保留全文按无徽章追问处理（提示真实：不伪造徽章）
                let after = &trimmed[SENTINEL_PROBE.len()..];
                let stripped = after.find('\n').map(|i| after[i + 1..].trim()).unwrap_or_else(|| after.trim());
                (Continuation::Probe { meta: None }, stripped.to_string())
            }
        }
    } else if expect_summary {
        (Continuation::Summary, trimmed.to_string())
    } else {
        (Continuation::Question, trimmed.to_string())
    };

    let report = report_raw.and_then(|r| {
        let v: Value = serde_json::from_str(r).ok()?;
        (v.get("score").is_some() || v.get("feedback").is_some()).then_some(v)
    });

    AnswerFlowOutput { continuation, continuation_text: body.to_string(), report }
}

/// 流式哨兵门：实时 delta 转发过滤器。
/// 保证任何以 `<<<` 开头的行不会泄漏到前端画面；哨兵之后的全部内容进入缓冲区，
/// 流结束时由 finish() 归还原始尾部供协议解析。
#[derive(Debug, PartialEq)]
enum GatePhase {
    /// 开局判定：等待首个完整行/可解析的 PROBE 元数据
    Head,
    /// 正文透传（直到 REPORT 哨兵出现）
    Body,
    /// REPORT 段之后：全部吞掉
    Sealed,
}

/// 流式哨兵门（M4）：保证哨兵行与 REPORT 段不泄漏进直播画面。
/// - Head 相位：若首行以 PROBE 哨兵开头，解析其元数据（括号配对容忍跨 delta），仅吞掉元数据部分，
///   其后的题干正文照常透传；首行无哨兵则整行透传进入 Body。
/// - Body 相位：透传完整行；一旦出现 REPORT 哨兵立即转入 Sealed。
/// - finish() 归还从未下发的原始尾巴，供协议解析使用完整文本。
pub struct SentinelGate {
    pending: String,
    phase: GatePhase,
}


impl Default for SentinelGate {
    fn default() -> Self {
        Self { pending: String::new(), phase: GatePhase::Head }
    }
}

impl SentinelGate {
    pub fn new() -> Self {
        Self { pending: String::new(), phase: GatePhase::Head }
    }

    /// 推入一个 delta，返回本次可安全下发给前端的增量文本（可能为空）。
    pub fn push(&mut self, delta: &str) -> String {
        self.pending.push_str(delta);
        self.drain()
    }

    /// 流结束：返回尚未下发给前端的「续接」尾巴（如无换行的最后一行）。
    /// 已进入 Sealed（REPORT 哨兵之后）的内容不属于续接，不下发——由调用方从完整原文解析。
    pub fn finish(mut self) -> String {
        if self.phase == GatePhase::Sealed {
            String::new()
        } else {
            std::mem::take(&mut self.pending)
        }
    }

    fn drain(&mut self) -> String {
        let mut out = String::new();
        loop {
            match self.phase {
                GatePhase::Body => {
                    if let Some(p) = s_find(&self.pending, SENTINEL_REPORT) {
                        out.push_str(&self.pending.drain(..p).collect::<String>());
                        self.phase = GatePhase::Sealed;
                    } else if let Some(nl) = self.pending.rfind('\n') {
                        out.push_str(&self.pending.drain(..=nl).collect::<String>());
                        break;
                    } else {
                        break;
                    }
                }
                GatePhase::Sealed => {
                    // REPORT 段保留在 pending 中：finish() 需归还完整尾巴供协议解析
                    break;
                }
                GatePhase::Head => {
                    // 尝试判定首行形态：需要至少一个换行或已可完成 PROBE 元数据解析
                    let trimmed = self.pending.trim_start();
                    if trimmed.starts_with(SENTINEL_PROBE) {
                        match parse_probe_meta(&trimmed[SENTINEL_PROBE.len()..]) {
                            Some((_, rest)) => {
                                // 元数据解析成功：吞掉元数据，剩余正文（可能为空）直接放行
                                if !rest.is_empty() {
                                    out.push_str(&rest);
                                    out.push('\n');
                                }
                                self.consume_through_first_line();
                                self.phase = GatePhase::Body;
                            }
                            None => {
                                // 无法解析：要么 JSON 未到齐（继续等），要么整行畸形且已有换行（剥掉该行转 Body）
                                let has_newline = trimmed.contains('\n');
                                let balanced = s_has_balanced_object(&trimmed[SENTINEL_PROBE.len()..]);
                                if !has_newline && !balanced {
                                    break; // 继续等 delta
                                }
                                if balanced && !has_newline {
                                    break; // 平衡但解析失败=枚举外等非法值：等待更多内容也无济于事，
                                           // 但为简单起见继续等换行后走剥离路径
                                }
                                self.consume_through_first_line();
                                self.phase = GatePhase::Body;
                            }
                        }
                    } else if trimmed.is_empty() {
                        break; // 只有空白，继续等
                    } else if let Some(nl) = self.pending.find('\n') {
                        let line: String = self.pending.drain(..=nl).collect();
                        out.push_str(&line);
                        self.phase = GatePhase::Body;
                    } else {
                        // 无换行且非哨兵开头：无法确定是否哨兵前缀，保守等待
                        // （真实题目正文为中文，不会以 '<' 起始；此处多为哨兵前缀碎片）
                        if !trimmed.starts_with('<') {
                            out.push_str(&self.pending);
                            self.pending.clear();
                            self.phase = GatePhase::Body;
                        }
                        break;
                    }
                }
            }
            if self.phase == GatePhase::Sealed {
                break;
            }
        }
        out
    }

    /// 消费掉 pending 中第一个完整行（含换行符）
    fn consume_through_first_line(&mut self) {
        if let Some(nl) = self.pending.find('\n') {
            self.pending.drain(..=nl);
        } else {
            self.pending.clear();
        }
    }
}

/// 在 pending 中查找 REPORT 哨兵位置
fn s_find(haystack: &str, needle: &str) -> Option<usize> {
    haystack.find(needle)
}

/// 判断哨兵后的花括号是否已达平衡（粗判：供流式 Head 相位决策——JSON 未到齐则继续等）
fn s_has_balanced_object(s: &str) -> bool {
    if !s.starts_with('{') {
        return false;
    }
    let mut depth = 0usize;
    let mut in_str = false;
    let mut esc = false;
    for ch in s.chars() {
        if esc {
            esc = false;
            continue;
        }
        match ch {
            '\\' if in_str => esc = true,
            '"' => in_str = !in_str,
            '{' if !in_str => depth += 1,
            '}' if !in_str => {
                depth -= 1;
                if depth == 0 {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_meta_parses_valid_enum() {
        let (m, rest) =
            parse_probe_meta(r#"{"anchor_keyword":"锁","reason":"depth_probe"}"#).unwrap();
        assert_eq!(m.anchor_keyword, "锁");
        assert_eq!(m.reason, "depth_probe");
        assert!(rest.is_empty());
    }

    #[test]
    fn probe_meta_rejects_out_of_enum_and_blank_anchor() {
        assert!(parse_probe_meta(r#"{"anchor_keyword":"锁","reason":"自由发挥"}"#).is_none());
        assert!(parse_probe_meta(r#"{"anchor_keyword":"","reason":"depth_probe"}"#).is_none());
        assert!(parse_probe_meta("not-json").is_none());
    }

    #[test]
    fn probe_meta_survives_same_line_body_and_nested_braces() {
        // 模型未换行直接接题干 / JSON 内含嵌套对象时仍可解析
        let text = r#"{"anchor_keyword":"扩容","reason":"depth_probe"}追问：并发扩容会怎样？"#;
        let (m, body) = parse_probe_meta(text).unwrap();
        assert_eq!(m.reason, "depth_probe");
        assert_eq!(body, "追问：并发扩容会怎样？");
        let nested = r#"{"anchor_keyword":"a","reason":"clarification"} tail {"x":1}"#;
        let (m2, _) = parse_probe_meta(nested).unwrap();
        assert_eq!(m2.anchor_keyword, "a");
    }

    #[test]
    fn split_probe_with_report_absent() {
        let out = split_answer_output(
            r#"<<<PROBE>>>{"anchor_keyword":"扩容","reason":"contradiction"}
追问：并发扩容下迭代器会发生什么？"#,
            false,
        );
        assert_eq!(
            out.continuation,
            Continuation::Probe {
                meta: Some(ProbeMeta { anchor_keyword: "扩容".into(), reason: "contradiction".into() })
            }
        );
        assert!(out.continuation_text.contains("并发扩容"));
        assert!(out.report.is_none());
    }

    #[test]
    fn split_question_with_report() {
        let out = split_answer_output(
            "下一题：请讲讲 B+ 树。\n<<<REPORT>>>\n{\"tags\":[\"索引\"],\"difficulty\":3,\"ref_answer\":\"要点\",\"score\":82,\"feedback\":\"不错\"}",
            false,
        );
        assert_eq!(out.continuation, Continuation::Question);
        assert!(out.continuation_text.starts_with("下一题"));
        let r = out.report.unwrap();
        assert_eq!(r["score"], 82);
        assert_eq!(r["difficulty"], 3);
    }

    #[test]
    fn summary_turn_without_sentinels() {
        let out = split_answer_output("# 🎯 全场复盘\n\n内容…", true);
        assert_eq!(out.continuation, Continuation::Summary);
        assert!(out.report.is_none());
    }

    #[test]
    fn malformed_probe_line_stripped_but_body_kept() {
        let out = split_answer_output("<<<PROBE>>>{oops}\n追问：这里为什么慢？", false);
        assert_eq!(out.continuation, Continuation::Probe { meta: None });
        assert!(out.continuation_text.contains("为什么慢"), "{}", out.continuation_text);
        // 同行粘连形态（无换行）
        let out2 = split_answer_output("<<<PROBE>>>{oops}追问：为什么？", false);
        assert_eq!(out2.continuation, Continuation::Probe { meta: None });
        assert!(out2.continuation_text.contains("为什么"), "{}", out2.continuation_text);
    }

    #[test]
    fn gate_never_leaks_sentinels_and_finish_flushes_continuation() {
        // 调用方累积原始全文（供协议解析）；gate 只保证直播画面干净
        let mut g = SentinelGate::new();
        let mut live = String::new();
        let mut raw = String::new();
        for d in ["第一段回答", "\n<<<RE", "PORT>>>\n{\"score\":90,\"feedback\":\"好\"}"] {
            raw.push_str(d);
            live.push_str(&g.push(d));
        }
        let cont_tail = g.finish(); // 未下发的续接尾巴（此处为空：正文已在换行处放行）
        assert!(!live.contains("<<<"), "哨兵不得泄漏进直播画面: {live}");
        assert!(cont_tail.is_empty(), "REPORT 段不属于续接，不得下发: {cont_tail}");
        // 完整原文可正常协议解析
        let out = split_answer_output(&raw, false);
        assert_eq!(out.continuation, Continuation::Question);
        assert_eq!(out.report.unwrap()["score"], 90);
    }

    #[test]
    fn gate_flushes_held_partial_line_as_continuation_tail() {
        let mut g = SentinelGate::new();
        let mut live = String::new();
        // PROBE 元数据行被吞；其后无换行的题干在流结束时作为续接尾巴归还
        for d in [
            r#"<<<PROBE>>>{"anchor_keyword":"锁","reason":"depth_probe"}
"#,
            "追问：并发下迭代器会怎样？",
        ] {
            raw_push(&mut live, &mut g, d);
        }
        let tail = g.finish();
        assert_eq!(tail, "追问：并发下迭代器会怎样？", "未下发的续接应作为尾巴归还");
        assert!(!live.contains("PROBE"));
    }

    fn raw_push(live: &mut String, g: &mut SentinelGate, d: &str) {
        live.push_str(&g.push(d));
    }

    #[test]
    fn gate_emits_complete_lines_without_sentinel() {
        let mut g = SentinelGate::new();
        let a = g.push("题目正文第一行\n");
        assert_eq!(a, "题目正文第一行\n");
        let b = g.push("第二行没有换行符");
        assert_eq!(b, "", "半行不下发");
        let c = g.push("\n后续\n");
        assert_eq!(c, "第二行没有换行符\n后续\n");
        let tail = g.finish();
        assert!(tail.is_empty());
    }
}
