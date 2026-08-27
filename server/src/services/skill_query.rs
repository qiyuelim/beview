//! 子树查询构造器 (ADR-0018 D3)
//! 提供全仓唯一的"按技能节点过滤其子树题目集合" SQL 谓词构造与子树归属判定。
//! 确保题库列表筛选、能力矩阵单元格、靶向圈题、图谱关联计数四处口径 100% 恒等。

/// 内部共享骨架：构造 UNION 关联题目全部技能并向上匹配子树的统一 EXISTS 谓词
fn build_subtree_exists_sql(q_alias: &str, user_param: &str, dom_filter_clause: &str) -> String {
    format!(
        r#"EXISTS(
            SELECT 1 FROM skills dom
            JOIN (
                SELECT skill_id FROM question_skills qs WHERE qs.question_id = {q_alias}.id
                UNION
                SELECT {q_alias}.skill_id WHERE {q_alias}.skill_id IS NOT NULL
            ) q_all_skills ON true
            JOIN skills s ON s.id = q_all_skills.skill_id
            WHERE dom.user_id = {user_param}
              AND ({dom_filter_clause})
              AND (s.id = dom.id OR s.path LIKE dom.path || '/%')
        )"#
    )
}

/// 构造统一的单技能/单名称子树匹配 SQL EXISTS 谓词片段
/// 
/// - `q_alias`: 题目表别名 (如 "q")
/// - `user_param`: 用户 ID 参数占位符 (如 "$9" 或 "$1")
/// - `dom_id_param`: 技能 ID 参数占位符 (如 "$10", 若无传 "NULL")
/// - `dom_name_param`: 技能名称/标签参数占位符 (如 "$6", 若无传 "NULL")
pub fn subtree_condition_sql(
    q_alias: &str,
    user_param: &str,
    dom_id_param: &str,
    dom_name_param: &str,
) -> String {
    let filter = format!(
        "({dom_id_param}::bigint IS NULL OR dom.id = {dom_id_param}) \
         AND ({dom_name_param}::text IS NULL OR dom.name = {dom_name_param} OR dom.name ILIKE '%'||{dom_name_param}||'%')"
    );
    build_subtree_exists_sql(q_alias, user_param, &filter)
}

/// 构造统一的多技能/多名称/多数组匹配子树 SQL EXISTS 谓词片段 (专供靶向圈题等复合圈选场景)
///
/// - `dom_ids_array_param`: 技能 ID 数组参数占位符 (如 "$4", 对应 bigint[], 若无传 "NULL")
/// - `dom_names_array_param`: 技能名称/标签数组参数占位符 (如 "$3", 对应 text[], 若无传 "NULL")
pub fn subtree_multi_condition_sql(
    q_alias: &str,
    user_param: &str,
    dom_id_param: &str,
    dom_ids_array_param: &str,
    dom_names_array_param: &str,
) -> String {
    let filter = format!(
        "({dom_id_param}::bigint IS NOT NULL AND dom.id = {dom_id_param}) \
         OR ({dom_ids_array_param}::bigint[] IS NOT NULL AND dom.id = ANY({dom_ids_array_param})) \
         OR ({dom_names_array_param}::text[] IS NOT NULL AND dom.name = ANY({dom_names_array_param}))"
    );
    build_subtree_exists_sql(q_alias, user_param, &filter)
}

/// 判定给定技能节点 `s_path` / `s_id` 是否属于目标节点 `dom_path` / `dom_id` 的子树
pub fn is_in_subtree(dom_id: i64, dom_path: &str, s_id: i64, s_path: &str) -> bool {
    if dom_id == s_id {
        return true;
    }
    if dom_path.is_empty() || s_path.is_empty() {
        return false;
    }
    let prefix = if dom_path.ends_with('/') {
        dom_path.to_string()
    } else {
        format!("{dom_path}/")
    };
    s_path.starts_with(&prefix)
}
