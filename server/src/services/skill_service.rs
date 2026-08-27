//! v5 技能图谱与能力雷达计算引擎 (ADR-0017 §3.1)

use sqlx::PgPool;
use crate::error::AppError;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct SkillRow {
    pub id: i64,
    pub user_id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
    pub path: String,
    pub icon: Option<String>,
    pub visibility: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SkillTreeNode {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
    pub path: String,
    pub icon: Option<String>,
    pub question_count: i64,
    pub proficiency: i32,
    pub weakness_index: i32,
    pub avg_score: Option<f64>,
    pub children: Vec<SkillTreeNode>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RadarDimension {
    pub key: String,
    pub name: String,
    pub score: i32,
    pub question_count: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SkillGraphData {
    pub tree: Vec<SkillTreeNode>,
    pub radar: Vec<RadarDimension>,
    pub total_skills: i64,
    pub total_tagged_questions: i64,
    pub overall_proficiency: i32,
}

pub const TOP_LEVEL_DOMAINS: &[(&str, &str, &str)] = &[
    ("专业技术与硬技能", "/hard-skills", "Code"),
    ("系统设计与架构思考", "/architecture", "Graph"),
    ("业务理解与行业实操", "/business-ops", "Briefcase"),
    ("工程落地与质量调优", "/engineering", "Cpu"),
    ("项目协作与团队管理", "/leadership", "UsersThree"),
    ("问题分析与通用素养", "/general-competence", "Brain"),
];

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MergeSkillResult {
    pub source_id: i64,
    pub target_id: i64,
    pub remapped_questions: u64,
    pub remapped_children: u64,
}

/// 映射名称到 6 大系统顶级领域之一（绝对禁止创建未受管的顶级领域）
pub fn match_to_system_top_domain(name: &str) -> &'static str {
    if let Some((n, _, _)) = TOP_LEVEL_DOMAINS.iter().find(|(n, _, _)| *n == name) {
        return n;
    }
    if name.contains("语言") || name.contains("存储") || name.contains("数据") || name.contains("技术") || name.contains("代码") || name.contains("算法") || name.contains("Java") || name.contains("Rust") || name.contains("Go") || name.contains("前端") || name.contains("后端") {
        "专业技术与硬技能"
    } else if name.contains("架构") || name.contains("设计") || name.contains("微服务") || name.contains("分布式") || name.contains("系统") {
        "系统设计与架构思考"
    } else if name.contains("工程") || name.contains("基建") || name.contains("网络") || name.contains("调优") || name.contains("运维") || name.contains("Linux") || name.contains("Docker") || name.contains("K8s") {
        "工程落地与质量调优"
    } else if name.contains("管理") || name.contains("协作") || name.contains("团队") || name.contains("项目") || name.contains("沟通") {
        "项目协作与团队管理"
    } else if name.contains("素养") || name.contains("分析") || name.contains("思维") || name.contains("通用") || name.contains("解决") || name.contains("排障") {
        "问题分析与通用素养"
    } else {
        "业务理解与行业实操"
    }
}

/// 确保用户的 6 大系统顶级领域存在，并将任何非系统顶级根节点自动归入最合适的顶级领域（平滑自愈迁移）
pub async fn ensure_system_top_level_domains(pool: &PgPool, uid: i64) -> Result<(), AppError> {
    let existing_roots: Vec<SkillRow> = sqlx::query_as(
        "SELECT id, user_id, parent_id, name, path, icon, visibility, created_at, updated_at
         FROM skills WHERE user_id=$1 AND parent_id IS NULL ORDER BY id ASC"
    )
    .bind(uid)
    .fetch_all(pool)
    .await?;

    let mut system_domain_ids: std::collections::HashMap<&'static str, i64> = std::collections::HashMap::new();

    for (name, path, icon) in TOP_LEVEL_DOMAINS {
        if let Some(r) = existing_roots.iter().find(|r| r.name == *name) {
            system_domain_ids.insert(*name, r.id);
        } else {
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO skills (user_id, parent_id, name, path, icon) VALUES ($1, NULL, $2, $3, $4) RETURNING id"
            )
            .bind(uid)
            .bind(*name)
            .bind(*path)
            .bind(*icon)
            .fetch_one(pool)
            .await?;
            system_domain_ids.insert(*name, id);
        }
    }

    for r in &existing_roots {
        if !TOP_LEVEL_DOMAINS.iter().any(|(name, _, _)| *name == r.name) {
            let target_top_name = match_to_system_top_domain(&r.name);

            if let Some(&top_id) = system_domain_ids.get(target_top_name) {
                let top_path = TOP_LEVEL_DOMAINS.iter().find(|(n, _, _)| *n == target_top_name).unwrap().1;
                let new_path = format!("{}/{}", top_path, r.name.to_lowercase().replace(' ', "-"));
                let _ = sqlx::query("UPDATE skills SET parent_id=$1, path=$2, updated_at=now() WHERE id=$3")
                    .bind(top_id)
                    .bind(&new_path)
                    .bind(r.id)
                    .execute(pool)
                    .await;
            }
        }
    }

    heal_tree_depth_and_duplicates(pool, uid).await?;
    Ok(())
}

/// 获取当前用户的完整技能图谱数据（3 层精简树 + 全景能力雷达 + 掌握度指标）
pub async fn get_skill_graph(pool: &PgPool, uid: i64) -> Result<SkillGraphData, AppError> {
    seed_default_skills(pool, uid).await?;
    heal_tree_depth_and_duplicates(pool, uid).await?;

    let rows: Vec<SkillRow> = sqlx::query_as(
        "SELECT id, user_id, parent_id, name, path, icon, visibility, created_at, updated_at
         FROM skills WHERE user_id=$1 ORDER BY path ASC, id ASC"
    )
    .bind(uid)
    .fetch_all(pool)
    .await?;

    if rows.is_empty() {
        return Ok(SkillGraphData {
            tree: Vec::new(),
            radar: Vec::new(),
            total_skills: 0,
            total_tagged_questions: 0,
            overall_proficiency: 0,
        });
    }

    #[derive(sqlx::FromRow)]
    struct QuestionSkillStat {
        question_id: i64,
        skill_id: Option<i64>,
        score: Option<i32>,
        last_result: Option<String>,
    }

    let stats: Vec<QuestionSkillStat> = sqlx::query_as(
        r#"
        SELECT
            q.id AS question_id,
            COALESCE(qs.skill_id, q.skill_id) AS skill_id,
            a.score,
            rr.last_result
        FROM questions q
        LEFT JOIN question_skills qs ON qs.question_id = q.id
        LEFT JOIN analyses a ON a.id = (
            SELECT a2.id FROM analyses a2 WHERE a2.question_id = q.id ORDER BY a2.created_at DESC, a2.id DESC LIMIT 1
        )
        LEFT JOIN review_records rr ON rr.question_id = q.id
        WHERE q.user_id = $1 AND q.parent_id IS NULL
        "#
    )
    .bind(uid)
    .fetch_all(pool)
    .await?;

    use std::collections::{HashMap, HashSet};
    use crate::services::skill_query::is_in_subtree;

    let skill_map: HashMap<i64, (i64, String)> = rows.iter().map(|r| (r.id, (r.id, r.path.clone()))).collect();

    struct QInfo {
        score: Option<i32>,
        last_result: Option<String>,
        skills: Vec<(i64, String)>,
    }
    let mut q_info_map: HashMap<i64, QInfo> = HashMap::new();
    for s in stats {
        let entry = q_info_map.entry(s.question_id).or_insert_with(|| QInfo {
            score: s.score,
            last_result: s.last_result.clone(),
            skills: Vec::new(),
        });
        if let Some(sid) = s.skill_id {
            if let Some(skill_info) = skill_map.get(&sid) {
                if !entry.skills.iter().any(|(id, _)| *id == sid) {
                    entry.skills.push(skill_info.clone());
                }
            }
        }
    }

    let mut flat_nodes: Vec<SkillTreeNode> = rows.iter().map(|r| {
        let sid = r.id;
        let spath = &r.path;

        let mut matched_q_ids = HashSet::new();
        let mut scores = Vec::new();
        let mut pass_count = 0;
        let mut fail_count = 0;

        for (qid, qinfo) in &q_info_map {
            let in_subtree = qinfo.skills.iter().any(|(s_id, s_path)| {
                is_in_subtree(sid, spath, *s_id, s_path)
            });
            if in_subtree && matched_q_ids.insert(*qid) {
                if let Some(sc) = qinfo.score {
                    scores.push(sc);
                }
                if let Some(ref res) = qinfo.last_result {
                    if res == "remembered" {
                        pass_count += 1;
                    } else if res == "forgotten" {
                        fail_count += 1;
                    }
                }
            }
        }

        let q_cnt = matched_q_ids.len() as i64;
        let avg_score = if !scores.is_empty() {
            Some(scores.iter().sum::<i32>() as f64 / scores.len() as f64)
        } else {
            None
        };

        let calculated_prof = if let Some(avg) = avg_score {
            let total_rev = pass_count + fail_count;
            let pass_ratio = if total_rev > 0 {
                pass_count as f64 / total_rev as f64
            } else {
                0.5
            };
            let p = avg * 0.7 + (pass_ratio * 100.0) * 0.3;
            p.round() as i32
        } else if (pass_count + fail_count) > 0 {
            let total_rev = pass_count + fail_count;
            let pass_ratio = pass_count as f64 / total_rev as f64;
            (pass_ratio * 100.0).round() as i32
        } else {
            0
        };

        let weakness = if q_cnt == 0 {
            100
        } else {
            (100 - calculated_prof).max(0)
        };

        SkillTreeNode {
            id: r.id,
            parent_id: r.parent_id,
            name: r.name.clone(),
            path: r.path.clone(),
            icon: r.icon.clone(),
            question_count: q_cnt,
            proficiency: calculated_prof,
            weakness_index: weakness,
            avg_score,
            children: Vec::new(),
        }
    }).collect();

    let total_skills = flat_nodes.len() as i64;
    let total_tagged_questions: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT question_id) FROM question_skills qs JOIN questions q ON q.id=qs.question_id WHERE q.user_id=$1"
    )
    .bind(uid)
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    let tree = build_tree_and_roll_up(&mut flat_nodes);

    let radar: Vec<RadarDimension> = tree.iter().map(|root| {
        RadarDimension {
            key: root.path.clone(),
            name: root.name.clone(),
            score: root.proficiency,
            question_count: root.question_count,
        }
    }).collect();

    let overall_proficiency = if !radar.is_empty() {
        (radar.iter().map(|r| r.score as i64).sum::<i64>() / radar.len() as i64) as i32
    } else {
        0
    };

    Ok(SkillGraphData {
        tree,
        radar,
        total_skills,
        total_tagged_questions,
        overall_proficiency,
    })
}

fn build_tree_and_roll_up(nodes: &mut [SkillTreeNode]) -> Vec<SkillTreeNode> {
    use std::collections::HashMap;
    let mut by_id: HashMap<i64, SkillTreeNode> = nodes.iter().map(|n| (n.id, n.clone())).collect();
    let mut children_map: HashMap<Option<i64>, Vec<i64>> = HashMap::new();

    for n in nodes.iter() {
        children_map.entry(n.parent_id).or_default().push(n.id);
    }

    fn assemble(node_id: i64, by_id: &mut HashMap<i64, SkillTreeNode>, children_map: &HashMap<Option<i64>, Vec<i64>>) -> SkillTreeNode {
        let child_ids = children_map.get(&Some(node_id)).cloned().unwrap_or_default();
        let mut children = Vec::new();
        for cid in child_ids {
            children.push(assemble(cid, by_id, children_map));
        }

        let mut node = by_id.get(&node_id).cloned().unwrap();
        node.children = children;
        node
    }

    let root_ids = children_map.get(&None).cloned().unwrap_or_default();
    let mut roots: Vec<SkillTreeNode> = root_ids.into_iter().map(|rid| assemble(rid, &mut by_id, &children_map)).collect();
    roots.sort_by_key(|r| {
        TOP_LEVEL_DOMAINS.iter().position(|(name, _, _)| *name == r.name).unwrap_or(999)
    });
    roots
}

pub async fn seed_default_skills(pool: &PgPool, uid: i64) -> Result<(), AppError> {
    ensure_system_top_level_domains(pool, uid).await?;

    struct SubDomain {
        top_name: &'static str,
        name: &'static str,
        path_suffix: &'static str,
        icon: &'static str,
        leaves: &'static [(&'static str, &'static str, &'static str)],
    }

    let presets = [
        SubDomain {
            top_name: "专业技术与硬技能",
            name: "编程语言与核心基础",
            path_suffix: "/languages",
            icon: "Code",
            leaves: &[
                ("Rust 核心与并发", "/languages/rust", "FileCode"),
                ("Go 并发与运行时", "/languages/go", "FileCode"),
                ("Java 核心与 JVM", "/languages/java", "FileCode"),
                ("数据结构与算法", "/languages/dsa", "TreeStructure"),
            ],
        },
        SubDomain {
            top_name: "专业技术与硬技能",
            name: "存储与数据系统",
            path_suffix: "/storage",
            icon: "Database",
            leaves: &[
                ("MySQL 索引与事务", "/storage/mysql", "Database"),
                ("Redis 缓存与底层结构", "/storage/redis", "HardDrive"),
                ("PostgreSQL 与高级特性", "/storage/postgres", "Database"),
                ("分布式存储与事务", "/storage/distributed-tx", "ShareNetwork"),
            ],
        },
        SubDomain {
            top_name: "系统设计与架构思考",
            name: "高可用与微服务架构",
            path_suffix: "/microservices",
            icon: "Buildings",
            leaves: &[
                ("高并发微服务设计", "/microservices/design", "Buildings"),
                ("消息队列与事件驱动", "/microservices/mq", "ChatDots"),
                ("领域驱动设计 (DDD)", "/microservices/ddd", "Cube"),
                ("缓存一致性与高可用", "/microservices/cache", "ShieldCheck"),
            ],
        },
        SubDomain {
            top_name: "业务理解与行业实操",
            name: "业务链路与领域建模",
            path_suffix: "/domain-modeling",
            icon: "Cube",
            leaves: &[
                ("业务状态机与流程闭环", "/domain-modeling/state-machine", "GitBranch"),
                ("核心业务指标与对账体系", "/domain-modeling/metrics", "ChartLineUp"),
                ("行业垂直场景与痛点攻坚", "/domain-modeling/industry-scenarios", "Briefcase"),
            ],
        },
        SubDomain {
            top_name: "工程落地与质量调优",
            name: "基础设施与运维调优",
            path_suffix: "/infra",
            icon: "Cpu",
            leaves: &[
                ("Linux 操作系统与内核", "/infra/linux", "TerminalWindow"),
                ("计算机网络 (TCP/HTTP)", "/infra/network", "Globe"),
                ("Docker & Kubernetes", "/infra/k8s", "Cloud"),
                ("可观测性与性能调优", "/infra/observability", "Gauge"),
            ],
        },
        SubDomain {
            top_name: "项目协作与团队管理",
            name: "敏捷研发与协作交付",
            path_suffix: "/agile",
            icon: "UsersThree",
            leaves: &[
                ("研发流程与代码规范", "/agile/standards", "CheckSquare"),
                ("团队协同与跨组沟通", "/agile/collaboration", "Users"),
                ("项目排期与风险控制", "/agile/risk-management", "Clock"),
            ],
        },
        SubDomain {
            top_name: "问题分析与通用素养",
            name: "复杂问题分析与决策",
            path_suffix: "/problem-solving",
            icon: "Brain",
            leaves: &[
                ("生产紧急故障排障", "/problem-solving/troubleshooting", "Warning"),
                ("技术选型与权衡分析", "/problem-solving/tradeoffs", "Scales"),
                ("系统复盘与演进沉淀", "/problem-solving/retrospective", "ArrowsClockwise"),
            ],
        },
    ];

    for p in presets {
        let top_id: i64 = sqlx::query_scalar("SELECT id FROM skills WHERE user_id=$1 AND parent_id IS NULL AND name=$2")
            .bind(uid)
            .bind(p.top_name)
            .fetch_one(pool)
            .await?;

        let top_path = TOP_LEVEL_DOMAINS.iter().find(|(n, _, _)| *n == p.top_name).unwrap().1;
        let domain_path = format!("{}{}", top_path, p.path_suffix);
        
        let existing_sub_id: Option<i64> = sqlx::query_scalar("SELECT id FROM skills WHERE user_id=$1 AND parent_id=$2 AND name=$3")
            .bind(uid)
            .bind(top_id)
            .bind(p.name)
            .fetch_optional(pool)
            .await?;

        let sub_id = if let Some(sid) = existing_sub_id {
            sid
        } else {
            sqlx::query_scalar(
                "INSERT INTO skills (user_id, parent_id, name, path, icon) VALUES ($1, $2, $3, $4, $5) RETURNING id"
            )
            .bind(uid)
            .bind(top_id)
            .bind(p.name)
            .bind(&domain_path)
            .bind(p.icon)
            .fetch_one(pool)
            .await?
        };

        for (l_name, l_suffix, l_icon) in p.leaves {
            let leaf_path = format!("{}/{}", domain_path, l_suffix.trim_start_matches('/'));
            let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM skills WHERE user_id=$1 AND parent_id=$2 AND name=$3")
                .bind(uid)
                .bind(sub_id)
                .bind(*l_name)
                .fetch_optional(pool)
                .await?;
            if exists.is_none() {
                sqlx::query(
                    "INSERT INTO skills (user_id, parent_id, name, path, icon) VALUES ($1, $2, $3, $4, $5)"
                )
                .bind(uid)
                .bind(sub_id)
                .bind(*l_name)
                .bind(&leaf_path)
                .bind(*l_icon)
                .execute(pool)
                .await?;
            }
        }
    }

    Ok(())
}

/// 技能节点合并：将源节点的所有题目关联和子节点转移到目标节点，并删除源节点（解决同义重复裂变）
pub async fn merge_skill_node(
    pool: &PgPool,
    uid: i64,
    source_id: i64,
    target_id: i64,
) -> Result<MergeSkillResult, AppError> {
    if source_id == target_id {
        return Err(AppError::BadRequest("源节点与目标节点不能相同".to_string()));
    }

    let source: Option<SkillRow> = sqlx::query_as("SELECT * FROM skills WHERE id=$1 AND user_id=$2")
        .bind(source_id)
        .bind(uid)
        .fetch_optional(pool)
        .await?;
    let target: Option<SkillRow> = sqlx::query_as("SELECT * FROM skills WHERE id=$1 AND user_id=$2")
        .bind(target_id)
        .bind(uid)
        .fetch_optional(pool)
        .await?;

    let (Some(src), Some(_tgt)) = (source, target) else {
        return Err(AppError::NotFound);
    };

    // 系统顶级节点不能作为源节点被合并/删除
    if src.parent_id.is_none() && TOP_LEVEL_DOMAINS.iter().any(|(name, _, _)| *name == src.name) {
        return Err(AppError::BadRequest("系统顶级知识域不可被合并或删除".to_string()));
    }

    let mut tx = pool.begin().await?;

    // 1. 将源节点的题目关联迁移到目标节点（去重）
    let q_ids: Vec<i64> = sqlx::query_scalar("SELECT question_id FROM question_skills WHERE skill_id=$1")
        .bind(source_id)
        .fetch_all(&mut *tx)
        .await?;

    let mut remapped_q = 0;
    for qid in q_ids {
        let res = sqlx::query("INSERT INTO question_skills (question_id, skill_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(qid)
            .bind(target_id)
            .execute(&mut *tx)
            .await?;
        if res.rows_affected() > 0 {
            remapped_q += 1;
        }
    }
    sqlx::query("DELETE FROM question_skills WHERE skill_id=$1")
        .bind(source_id)
        .execute(&mut *tx)
        .await?;

    // 2. 将源节点的子节点迁移至目标节点
    let remapped_children = sqlx::query("UPDATE skills SET parent_id=$1, updated_at=now() WHERE parent_id=$2 AND user_id=$3")
        .bind(target_id)
        .bind(source_id)
        .bind(uid)
        .execute(&mut *tx)
        .await?
        .rows_affected();

    // 3. 删除源节点
    sqlx::query("DELETE FROM skills WHERE id=$1 AND user_id=$2")
        .bind(source_id)
        .bind(uid)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    Ok(MergeSkillResult {
        source_id,
        target_id,
        remapped_questions: remapped_q,
        remapped_children,
    })
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UnmappedTag {
    pub tag: String,
    pub question_count: i64,
}

/// 自动根据标签匹配用户的技能树节点并建立关联
pub async fn auto_bind_skills_by_tags(
    pool: &PgPool,
    uid: i64,
    qid: i64,
    tags: &[String],
) -> Result<Vec<i64>, AppError> {
    if tags.is_empty() {
        return Ok(Vec::new());
    }

    let skills: Vec<(i64, String)> = sqlx::query_as("SELECT id, name FROM skills WHERE user_id=$1")
        .bind(uid)
        .fetch_all(pool)
        .await?;

    if skills.is_empty() {
        return Ok(Vec::new());
    }

    let mut matched_sids = Vec::new();
    for (sid, sname) in &skills {
        let s_lower = sname.to_lowercase();
        for t in tags {
            let t_lower = t.trim().to_lowercase();
            if t_lower.is_empty() {
                continue;
            }
            if s_lower.contains(&t_lower) || t_lower.contains(&s_lower) {
                matched_sids.push(*sid);
                break;
            }
        }
    }

    // 评审 P3 整改：一条语句批量建立关联（此前逐条 INSERT 的 N+1）
    if !matched_sids.is_empty() {
        sqlx::query(
            "INSERT INTO question_skills (question_id, skill_id) SELECT $1, x FROM UNNEST($2) AS x ON CONFLICT DO NOTHING",
        )
        .bind(qid)
        .bind(&matched_sids)
        .execute(pool)
        .await?;
    }

    Ok(matched_sids)
}

/// 获取当前用户题库中尚未建树的标签列表及题数统计
pub async fn get_unmapped_tags(pool: &PgPool, uid: i64) -> Result<Vec<UnmappedTag>, AppError> {
    #[derive(sqlx::FromRow)]
    struct TagCount {
        name: String,
        q_count: i64,
    }

    let rows: Vec<TagCount> = sqlx::query_as(
        r#"
        SELECT t.name, count(DISTINCT qt.question_id) AS q_count
        FROM tags t
        JOIN question_tags qt ON qt.tag_id = t.id
        JOIN questions q ON q.id = qt.question_id AND q.user_id = $1
        WHERE NOT EXISTS (
            SELECT 1 FROM skills s
            WHERE s.user_id = $1
              AND (LOWER(s.name) = LOWER(t.name) OR LOWER(s.name) LIKE '%'||LOWER(t.name)||'%' OR LOWER(t.name) LIKE '%'||LOWER(s.name)||'%')
        )
        GROUP BY t.name
        HAVING count(DISTINCT qt.question_id) > 0
        ORDER BY q_count DESC, t.name ASC
        LIMIT 50
        "#
    )
    .bind(uid)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|r| UnmappedTag {
        tag: r.name,
        question_count: r.q_count,
    }).collect())
}

/// 一键将未建树标签沉淀为指定父节点下的新技能，并自动关联所有打过该标签的题目
pub async fn ingest_unmapped_tag(
    pool: &PgPool,
    uid: i64,
    tag: &str,
    parent_id: Option<i64>,
) -> Result<i64, AppError> {
    let name = tag.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("标签名称不能为空".to_string()));
    }

    let path = if let Some(pid) = parent_id {
        let parent_path: Option<String> = sqlx::query_scalar("SELECT path FROM skills WHERE id=$1 AND user_id=$2")
            .bind(pid)
            .bind(uid)
            .fetch_optional(pool)
            .await?;
        let Some(p) = parent_path else {
            return Err(AppError::BadRequest("父技能节点不存在".to_string()));
        };
        format!("{}/{}", p.trim_end_matches('/'), name.to_lowercase().replace(' ', "-"))
    } else {
        format!("/{}", name.to_lowercase().replace(' ', "-"))
    };

    let sid: i64 = sqlx::query_scalar(
        "INSERT INTO skills (user_id, parent_id, name, path, icon)
         VALUES ($1, $2, $3, $4, 'BookmarkSimple')
         ON CONFLICT (user_id, COALESCE(parent_id, 0), name) DO UPDATE SET name=EXCLUDED.name
         RETURNING id"
    )
    .bind(uid)
    .bind(parent_id)
    .bind(name)
    .bind(&path)
    .fetch_one(pool)
    .await?;

    // 自动将所有打了该 tag 的用户题目关联至此新节点
    sqlx::query(
        r#"
        INSERT INTO question_skills (question_id, skill_id)
        SELECT qt.question_id, $2
        FROM question_tags qt
        JOIN tags t ON t.id = qt.tag_id AND t.user_id = $1
        JOIN questions q ON q.id = qt.question_id AND q.user_id = $1
        WHERE t.name = $3
        ON CONFLICT DO NOTHING
        "#
    )
    .bind(uid)
    .bind(sid)
    .bind(name)
    .execute(pool)
    .await?;

    Ok(sid)
}

/// 标签合并结果
pub struct TagMergeResult {
    /// 重挂的题目-标签关联数
    pub remapped: u64,
    /// 被吸收删除的别名标签数
    pub removed_tags: u64,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct TagMergeGroupInput {
    pub canonical: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub target_skill_id: Option<i64>,
}

/// 标签聚合清洗·应用阶段（用户裁决 3：LLM 建议 + 人工确认后执行，支持 merge-to-skill 迁移）。
/// 每组 = canonical 规范名 + aliases 别名列表 + target_skill_id 目标技能节点：
/// 1. 把别名的所有题目关联改挂到规范名，然后删除别名标签；
/// 2. 若指定 target_skill_id（或同名技能节点存在），把该规范名标签下的所有题目挂靠至技能节点（question_skills 与 questions.skill_id）。
/// 3. question_tags 历史数据零丢失，仅作为卡片展示标签。
/// 幂等：重复应用无副作用。
pub async fn apply_tag_merges(
    pool: &PgPool,
    uid: i64,
    groups: &[TagMergeGroupInput],
) -> Result<TagMergeResult, AppError> {
    let mut result = TagMergeResult { remapped: 0, removed_tags: 0 };
    for group in groups {
        let canonical = group.canonical.trim();
        if canonical.is_empty() {
            continue;
        }
        // 规范名标签不存在则创建
        let canonical_id: i64 = sqlx::query_scalar(
            "INSERT INTO tags(user_id, name) VALUES($1,$2) \
             ON CONFLICT (user_id, name) DO UPDATE SET name=EXCLUDED.name RETURNING id",
        )
        .bind(uid)
        .bind(canonical)
        .fetch_one(pool)
        .await?;

        for alias in &group.aliases {
            let alias = alias.trim();
            if alias.is_empty() || alias == canonical {
                continue;
            }
            let Some(alias_id): Option<i64> =
                sqlx::query_scalar("SELECT id FROM tags WHERE user_id=$1 AND name=$2")
                    .bind(uid)
                    .bind(alias)
                    .fetch_optional(pool)
                    .await?
            else {
                continue; // 别名不存在（已并过/写错），跳过
            };
            // 题目关联迁移：已同时拥有两标签的题目会撞唯一约束 → 逐行 ON CONFLICT DO NOTHING 兜底
            let moved = sqlx::query(
                "INSERT INTO question_tags(question_id, tag_id) \
                 SELECT qt.question_id, $2 FROM question_tags qt WHERE qt.tag_id=$1 \
                 ON CONFLICT DO NOTHING",
            )
            .bind(alias_id)
            .bind(canonical_id)
            .execute(pool)
            .await?
            .rows_affected();
            sqlx::query("DELETE FROM question_tags WHERE tag_id=$1")
                .bind(alias_id)
                .execute(pool)
                .await?;
            sqlx::query("DELETE FROM tags WHERE id=$1")
                .bind(alias_id)
                .execute(pool)
                .await?;
            result.removed_tags += 1;
            result.remapped += moved;
        }

        // merge-to-skill: 如果指定了目标技能节点或同名技能节点存在，将该标签下的题目批量挂到技能树
        let skill_id = match group.target_skill_id {
            Some(sid) => {
                let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM skills WHERE id=$1 AND user_id=$2)")
                    .bind(sid)
                    .bind(uid)
                    .fetch_one(pool)
                    .await?;
                if valid { Some(sid) } else { None }
            }
            None => {
                sqlx::query_scalar("SELECT id FROM skills WHERE user_id=$1 AND name=$2 LIMIT 1")
                    .bind(uid)
                    .bind(canonical)
                    .fetch_optional(pool)
                    .await?
            }
        };

        if let Some(sid) = skill_id {
            // 批量绑定题目与技能 (question_skills)
            sqlx::query(
                "INSERT INTO question_skills(question_id, skill_id) \
                 SELECT qt.question_id, $1 FROM question_tags qt WHERE qt.tag_id=$2 \
                 ON CONFLICT DO NOTHING"
            )
            .bind(sid)
            .bind(canonical_id)
            .execute(pool)
            .await?;

            // 同步 questions.skill_id（若主列为空）
            sqlx::query(
                "UPDATE questions SET skill_id=$1 WHERE skill_id IS NULL AND id IN (SELECT question_id FROM question_tags WHERE tag_id=$2)"
            )
            .bind(sid)
            .bind(canonical_id)
            .execute(pool)
            .await?;
        }
    }
    Ok(result)
}

use crate::contracts::question::NewSkillItem;

/// 题目技能多对多关联与主列同步（声明式全量替换）
/// - 若传入 skill_ids: 清空现有关联，插入指定 skill_ids，并将首个技能同步至 questions.skill_id（空数组则置空为 NULL）
/// - 若传入 skill_id: 清空现有关联，插入该 skill_id，并更新 questions.skill_id
pub async fn sync_question_skills(
    pool: &PgPool,
    question_id: i64,
    skill_id: Option<i64>,
    skill_ids: Option<&[i64]>,
) -> Result<Option<i64>, AppError> {
    if let Some(sids) = skill_ids {
        sqlx::query("DELETE FROM question_skills WHERE question_id=$1")
            .bind(question_id)
            .execute(pool)
            .await?;
        for sid in sids {
            let _ = sqlx::query("INSERT INTO question_skills(question_id, skill_id) VALUES($1,$2) ON CONFLICT DO NOTHING")
                .bind(question_id)
                .bind(sid)
                .execute(pool)
                .await;
        }
        let primary_skill_id = sids.first().copied();
        sqlx::query("UPDATE questions SET skill_id=$2 WHERE id=$1")
            .bind(question_id)
            .bind(primary_skill_id)
            .execute(pool)
            .await?;
        Ok(primary_skill_id)
    } else if let Some(sid) = skill_id {
        sqlx::query("DELETE FROM question_skills WHERE question_id=$1")
            .bind(question_id)
            .execute(pool)
            .await?;
        let _ = sqlx::query("INSERT INTO question_skills(question_id, skill_id) VALUES($1,$2) ON CONFLICT DO NOTHING")
            .bind(question_id)
            .bind(sid)
            .execute(pool)
            .await;
        sqlx::query("UPDATE questions SET skill_id=$2 WHERE id=$1")
            .bind(question_id)
            .bind(Some(sid))
            .execute(pool)
            .await?;
        Ok(Some(sid))
    } else {
        Ok(None)
    }
}

/// 知识图谱三层不变量与同名自愈修复（保证全库恒为 ≤3 层：L1 根域 -> L2 子领域 -> L3 叶子技能）
/// 单次调用循环迭代直至完全收敛（彻底消灭多层脏链与嵌套同名节点）
pub async fn heal_tree_depth_and_duplicates(pool: &PgPool, uid: i64) -> Result<(), AppError> {
    loop {
        let mut changed = false;

        // 1. 消除相邻同名节点（child.name == parent.name）：将 child 的关联与子节点转移到 parent，并删除 child
        let same_name_pairs: Vec<(i64, i64)> = sqlx::query_as(
            r#"
            SELECT c.id, p.id
            FROM skills c
            JOIN skills p ON p.id = c.parent_id
            WHERE c.user_id = $1 AND p.user_id = $1 AND c.name = p.name
            "#
        )
        .bind(uid)
        .fetch_all(pool)
        .await?;

        if !same_name_pairs.is_empty() {
            changed = true;
            for (child_id, parent_id) in same_name_pairs {
                let _ = sqlx::query(
                    "INSERT INTO question_skills (question_id, skill_id)
                     SELECT question_id, $2 FROM question_skills WHERE skill_id = $1
                     ON CONFLICT DO NOTHING"
                )
                .bind(child_id)
                .bind(parent_id)
                .execute(pool)
                .await;

                let _ = sqlx::query("UPDATE questions SET skill_id = $2 WHERE skill_id = $1 AND user_id = $3")
                    .bind(child_id)
                    .bind(parent_id)
                    .bind(uid)
                    .execute(pool)
                    .await;

                let _ = sqlx::query("DELETE FROM question_skills WHERE skill_id = $1")
                    .bind(child_id)
                    .execute(pool)
                    .await;

                let _ = sqlx::query("UPDATE skills SET parent_id = $2, updated_at = now() WHERE parent_id = $1 AND user_id = $3")
                    .bind(child_id)
                    .bind(parent_id)
                    .bind(uid)
                    .execute(pool)
                    .await;

                let _ = sqlx::query("DELETE FROM skills WHERE id = $1 AND user_id = $2")
                    .bind(child_id)
                    .bind(uid)
                    .execute(pool)
                    .await;
            }
        }

        // 2. 深度折叠：找出深度 >= 4 的节点（祖先链长度 >= 3），将其收敛到其 L3 祖先
        let over_deep_nodes: Vec<(i64, i64)> = sqlx::query_as(
            r#"
            SELECT node.id, p.id
            FROM skills node
            JOIN skills p ON p.id = node.parent_id
            JOIN skills gp ON gp.id = p.parent_id
            WHERE node.user_id = $1 AND gp.parent_id IS NOT NULL
            "#
        )
        .bind(uid)
        .fetch_all(pool)
        .await?;

        if !over_deep_nodes.is_empty() {
            changed = true;
            for (deep_id, l3_id) in over_deep_nodes {
                let _ = sqlx::query(
                    "INSERT INTO question_skills (question_id, skill_id)
                     SELECT question_id, $2 FROM question_skills WHERE skill_id = $1
                     ON CONFLICT DO NOTHING"
                )
                .bind(deep_id)
                .bind(l3_id)
                .execute(pool)
                .await;

                let _ = sqlx::query("UPDATE questions SET skill_id = $2 WHERE skill_id = $1 AND user_id = $3")
                    .bind(deep_id)
                    .bind(l3_id)
                    .bind(uid)
                    .execute(pool)
                    .await;

                let _ = sqlx::query("DELETE FROM question_skills WHERE skill_id = $1")
                    .bind(deep_id)
                    .execute(pool)
                    .await;

                let _ = sqlx::query("UPDATE skills SET parent_id = $2, updated_at = now() WHERE parent_id = $1 AND user_id = $3")
                    .bind(deep_id)
                    .bind(l3_id)
                    .bind(uid)
                    .execute(pool)
                    .await;

                let _ = sqlx::query("DELETE FROM skills WHERE id = $1 AND user_id = $2")
                    .bind(deep_id)
                    .bind(uid)
                    .execute(pool)
                    .await;
            }
        }

        if !changed {
            break;
        }
    }

    Ok(())
}

pub(crate) async fn find_or_create_child(
    pool: &PgPool,
    uid: i64,
    parent_id: i64,
    name: &str,
    path: &str,
    icon: &str,
) -> Result<i64, AppError> {
    let name = name.trim();
    if name.is_empty() {
        return Ok(parent_id);
    }
    let parent_info: Option<(Option<i64>, String, String)> = sqlx::query_as(
        "SELECT parent_id, name, path FROM skills WHERE id=$1 AND user_id=$2"
    )
    .bind(parent_id)
    .bind(uid)
    .fetch_optional(pool)
    .await?;

    let (grandparent_id, parent_name, parent_path) = match parent_info {
        Some(info) => info,
        None => return Ok(parent_id),
    };

    // 相邻层级同名自愈：若与父节点同名，直接复用父节点
    if parent_name == name {
        return Ok(parent_id);
    }

    // 深度守卫：若父节点已有祖父（即 parent 已是 L3），则直接就地收敛至 parent，杜绝创建 L4
    if let Some(gp_id) = grandparent_id {
        let great_grandparent: Option<Option<i64>> = sqlx::query_scalar(
            "SELECT parent_id FROM skills WHERE id=$1 AND user_id=$2"
        )
        .bind(gp_id)
        .bind(uid)
        .fetch_optional(pool)
        .await?;
        if let Some(Some(_)) = great_grandparent {
            return Ok(parent_id);
        }
    }

    if let Some(id) = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM skills WHERE user_id=$1 AND parent_id=$2 AND name=$3 LIMIT 1",
    )
    .bind(uid)
    .bind(parent_id)
    .bind(name)
    .fetch_optional(pool)
    .await?
    {
        Ok(id)
    } else {
        let safe_slug = name.to_lowercase().replace(' ', "-");
        let safe_path = if path.is_empty() { format!("{parent_path}/{safe_slug}") } else { path.to_string() };
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO skills (user_id, parent_id, name, path, icon) VALUES ($1, $2, $3, $4, $5) RETURNING id",
        )
        .bind(uid)
        .bind(parent_id)
        .bind(name)
        .bind(safe_path)
        .bind(icon)
        .fetch_one(pool)
        .await?;
        Ok(id)
    }
}

/// 优先从现有树中查找匹配，若无且有 new_skill，自动建树并返回节点 id（恒为 ≤3 层：根域 -> 子领域 -> 叶子）
pub async fn resolve_or_create_skill(
    pool: &PgPool,
    uid: i64,
    path: Option<&str>,
    new_skill: Option<&NewSkillItem>,
) -> Result<Option<i64>, AppError> {
    if let Some(p) = path {
        let p_trimmed = p.trim().trim_matches('/');
        if !p_trimmed.is_empty() {
            let last_part = p_trimmed.split('/').last().unwrap_or(p_trimmed);
            let exact: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM skills WHERE user_id=$1 AND (path=$2 OR path=$3 OR name=$4) LIMIT 1"
            )
            .bind(uid)
            .bind(p_trimmed)
            .bind(format!("/{p_trimmed}"))
            .bind(last_part)
            .fetch_optional(pool)
            .await?;
            if let Some(id) = exact {
                return Ok(Some(id));
            }
        }
    }

    if let Some(ns) = new_skill {
        let l1 = ns.l1.trim();
        let l2 = ns.l2.trim();
        let l3 = ns.l3.trim();
        if !l1.is_empty() && !l2.is_empty() && !l3.is_empty() {
            ensure_system_top_level_domains(pool, uid).await?;

            // 1. 严格将 L1 映射至 6 大系统顶级领域之一
            let matched_top_domain = match_to_system_top_domain(l1);
            let top_row: (i64, String) = sqlx::query_as(
                "SELECT id, path FROM skills WHERE user_id=$1 AND parent_id IS NULL AND name=$2 LIMIT 1"
            )
            .bind(uid)
            .bind(matched_top_domain)
            .fetch_one(pool)
            .await?;
            let (top_id, top_path) = top_row;

            // 2. 构造严格 3 层结构：
            // L1 (根域) = matched_top_domain
            // L2 (子领域) = 若 l1==根域则取 l2；若 l1!=根域且 l1!=l2 则取 l2（或 l1）
            // L3 (叶子节点) = l3
            let mid_category = if l1 == matched_top_domain || l1.is_empty() {
                l2
            } else if l2 != l3 && !l2.is_empty() {
                l2
            } else {
                l1
            };

            let mid_slug = mid_category.to_lowercase().replace(' ', "-");
            let mid_path = format!("{top_path}/{mid_slug}");
            let mid_id = find_or_create_child(pool, uid, top_id, mid_category, &mid_path, "Folder").await?;

            // 3. L3 叶子挂在 L2 下
            let leaf_name = if l3 == mid_category && l2 != l3 && !l2.is_empty() {
                l2
            } else {
                l3
            };
            let leaf_slug = leaf_name.to_lowercase().replace(' ', "-");
            let leaf_path = format!("{mid_path}/{leaf_slug}");
            let leaf_id = find_or_create_child(pool, uid, mid_id, leaf_name, &leaf_path, "TreeStructure").await?;
            return Ok(Some(leaf_id));
        }
    }
    Ok(None)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MatrixCell {
    pub domain: String,
    pub question_type: String,
    pub count: i64,
    pub avg_score: f64,
    pub proficiency: i32,
    pub irt_theta: f64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SkillMatrixData {
    pub domains: Vec<String>,
    pub types: Vec<String>,
    pub cells: Vec<MatrixCell>,
    pub weakest_cell: Option<MatrixCell>,
}

/// 计算二维能力热力矩阵（技术大纲域 × 七类考察维度 + 加权能力指数，ADR-0022 D2：
/// 单用户样本量下不做真 IRT——启发式加权是针对个人规模的设计决策）
pub async fn get_capability_matrix(pool: &PgPool, uid: i64) -> Result<SkillMatrixData, AppError> {
    let tree = get_skill_graph(pool, uid).await?.tree;
    let domains: Vec<String> = tree.iter().map(|t| t.name.clone()).collect();
    let types = vec![
        "motivation_culture_fit".to_string(),
        "experience_track_record".to_string(),
        "professional_knowledge".to_string(),
        "scenario_case".to_string(),
        "practice_execution".to_string(),
        "problem_solving_resilience".to_string(),
        "collaboration".to_string(),
    ];

    #[derive(sqlx::FromRow)]
    struct RawMatrixRow {
        question_id: i64,
        skill_id: Option<i64>,
        question_type: Option<String>,
        score: Option<i32>,
        difficulty: i32,
        last_result: Option<String>,
    }

    let rows = sqlx::query_as::<_, RawMatrixRow>(
        r#"
        SELECT q.id AS question_id, COALESCE(qs.skill_id, q.skill_id) AS skill_id, q.question_type, a.score, q.difficulty, rr.last_result
        FROM questions q
        LEFT JOIN question_skills qs ON qs.question_id = q.id
        LEFT JOIN analyses a ON a.id = (SELECT a2.id FROM analyses a2 WHERE a2.question_id=q.id ORDER BY a2.created_at DESC, a2.id DESC LIMIT 1)
        LEFT JOIN review_records rr ON rr.question_id = q.id
        WHERE q.user_id = $1 AND q.parent_id IS NULL
        "#
    )
    .bind(uid)
    .fetch_all(pool)
    .await?;

    use std::collections::HashMap;
    use crate::services::skill_query::is_in_subtree;

    let all_skills: Vec<(i64, String, String)> = sqlx::query_as("SELECT id, name, path FROM skills WHERE user_id=$1")
        .bind(uid)
        .fetch_all(pool)
        .await?;

    let skill_map: HashMap<i64, (String, String)> = all_skills.into_iter().map(|(id, name, path)| (id, (name, path))).collect();

    struct QMatrixItem {
        question_type: String,
        score: Option<i32>,
        difficulty: i32,
        last_result: Option<String>,
        skills: Vec<(i64, String)>,
    }
    let mut q_items: HashMap<i64, QMatrixItem> = HashMap::new();
    for r in rows {
        let entry = q_items.entry(r.question_id).or_insert_with(|| {
            let q_type = r.question_type.as_deref().filter(|t| types.iter().any(|typ| typ == *t)).unwrap_or("professional_knowledge").to_string();
            QMatrixItem {
                question_type: q_type,
                score: r.score,
                difficulty: r.difficulty,
                last_result: r.last_result,
                skills: Vec::new(),
            }
        });
        if let Some(sid) = r.skill_id {
            if let Some((_, spath)) = skill_map.get(&sid) {
                if !entry.skills.iter().any(|(id, _)| *id == sid) {
                    entry.skills.push((sid, spath.clone()));
                }
            }
        }
    }

    struct Agg {
        count: i64,
        scores: Vec<i32>,
        difficulties: Vec<i32>,
        remembered: i64,
    }

    let mut map: HashMap<(String, String), Agg> = HashMap::new();

    for (_qid, item) in &q_items {
        let mut matched_domains = Vec::new();
        for root in &tree {
            let matches_root = item.skills.iter().any(|(sid, spath)| {
                is_in_subtree(root.id, &root.path, *sid, spath)
            });
            if matches_root {
                matched_domains.push(root.name.clone());
            }
        }
        if matched_domains.is_empty() {
            matched_domains.push("通用与其他".to_string());
        }

        for dom_name in matched_domains {
            let entry = map.entry((dom_name, item.question_type.clone())).or_insert_with(|| Agg {
                count: 0,
                scores: Vec::new(),
                difficulties: Vec::new(),
                remembered: 0,
            });
            entry.count += 1;
            if let Some(s) = item.score {
                entry.scores.push(s);
            }
            entry.difficulties.push(item.difficulty.clamp(1, 5));
            if let Some(ref res) = item.last_result {
                if res == "remembered" {
                    entry.remembered += 1;
                }
            }
        }
    }

    let mut cells = Vec::new();
    let mut weakest_cell: Option<MatrixCell> = None;
    let mut lowest_prof = 101;

    for d in &domains {
        for t in &types {
            if let Some(agg) = map.get(&(d.clone(), t.clone())) {
                let avg_s = if !agg.scores.is_empty() {
                    agg.scores.iter().sum::<i32>() as f64 / agg.scores.len() as f64
                } else {
                    0.0
                };
                let mem_rate = (agg.remembered as f64 / agg.count as f64) * 100.0;
                let prof = if avg_s > 0.0 { (avg_s * 0.7 + mem_rate * 0.3).round() as i32 } else { mem_rate.round() as i32 };
                
                let avg_diff = if !agg.difficulties.is_empty() { agg.difficulties.iter().sum::<i32>() as f64 / agg.difficulties.len() as f64 } else { 3.0 };
                let irt_theta = (avg_s * (0.8 + 0.1 * avg_diff)).clamp(0.0, 100.0);

                let cell = MatrixCell {
                    domain: d.clone(),
                    question_type: t.clone(),
                    count: agg.count,
                    avg_score: avg_s.round(),
                    proficiency: prof.clamp(0, 100),
                    irt_theta: (irt_theta * 10.0).round() / 10.0,
                };
                if prof < lowest_prof && agg.count > 0 {
                    lowest_prof = prof;
                    weakest_cell = Some(cell.clone());
                }
                cells.push(cell);
            } else {
                cells.push(MatrixCell {
                    domain: d.clone(),
                    question_type: t.clone(),
                    count: 0,
                    avg_score: 0.0,
                    proficiency: 0,
                    irt_theta: 0.0,
                });
            }
        }
    }

    Ok(SkillMatrixData {
        domains,
        types,
        cells,
        weakest_cell,
    })
}
