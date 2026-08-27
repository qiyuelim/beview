-- interview_review 当前 schema 基线。
-- 新库一次建齐；已有库只改 _sqlx_migrations 记录，不重跑本文件。

-- ---------- 函数 ----------
CREATE FUNCTION normalize_question_content(t TEXT) RETURNS TEXT
IMMUTABLE PARALLEL SAFE LANGUAGE SQL AS $norm$
  SELECT lower(regexp_replace(
    translate(COALESCE(t, ''),
      '！＂＃＄％＆＇（）＊＋，－．／０１２３４５６７８９：；＜＝＞？＠ＡＢＣＤＥＦＧＨＩＪＫＬＭＮＯＰＱＲＳＴＵＶＷＸＹＺ［＼］＾＿｀ａｂｃｄｅｆｇｈｉｊｋｌｍｎｏｐｑｒｓｔｕｖｗｘｙｚ｛｜｝～　、。〈〉《》〔〕〖〗・…—―‘’“”·',
      '!"#$%&''()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\]^_`abcdefghijklmnopqrstuvwxyz{|}~ ,,,,,,,,,,,,,,,,,,,'),
    '[[:space:][:punct:]]', '', 'g'))
$norm$;

-- ---------- 账号 ----------
CREATE TABLE users (
    id BIGSERIAL PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    role TEXT NOT NULL DEFAULT 'admin',
    row_status TEXT NOT NULL DEFAULT 'active'
        CHECK (row_status IN ('active', 'disabled')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE settings (
    user_id BIGINT NOT NULL REFERENCES users(id),
    key TEXT NOT NULL,
    value JSONB NOT NULL,
    PRIMARY KEY (user_id, key)
);

-- ---------- 公司 / 岗位 / 投递 ----------
CREATE TABLE companies (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id),
    name TEXT NOT NULL,
    description TEXT,
    is_system BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, name)
);
CREATE INDEX idx_companies_user ON companies(user_id);

CREATE TABLE positions (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id),
    company_id BIGINT NOT NULL REFERENCES companies(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    location TEXT,
    department TEXT,
    jd_text TEXT,
    jd_interpret JSONB,
    predict_result JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, company_id, title)
);
CREATE INDEX idx_positions_company ON positions(company_id);

CREATE TABLE resumes (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id),
    name TEXT NOT NULL DEFAULT '我的简历',
    version_name VARCHAR(120) NOT NULL DEFAULT '工作副本',
    raw_text TEXT NOT NULL DEFAULT '',
    parsed JSONB,
    is_active BOOLEAN NOT NULL DEFAULT true,
    is_archived BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_resumes_user_archived ON resumes(user_id, is_archived, updated_at DESC);

CREATE TABLE applications (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id),
    position_id BIGINT NOT NULL,
    resume_id BIGINT REFERENCES resumes(id) ON DELETE SET NULL,
    channel TEXT,
    applied_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    status TEXT NOT NULL DEFAULT 'applied',
    salary TEXT,
    note TEXT,
    jd_interpret JSONB,
    jd_match JSONB,
    overall_analysis JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_applications_user ON applications(user_id);
CREATE INDEX idx_applications_status ON applications(status);
CREATE INDEX idx_applications_resume_id ON applications(resume_id);

CREATE TABLE application_events (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id),
    application_id BIGINT NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    kind TEXT NOT NULL DEFAULT 'status',
    from_status TEXT,
    to_status TEXT NOT NULL,
    source TEXT NOT NULL DEFAULT 'manual',
    note TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_app_events_app ON application_events(application_id);

CREATE TABLE application_insights (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    payload JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_app_insights_user ON application_insights(user_id, created_at DESC);

-- ---------- 陪练容器 session / 轮次 / 题目 ----------
CREATE TABLE sessions (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id),
    company_id BIGINT NOT NULL REFERENCES companies(id) ON DELETE CASCADE,
    application_id BIGINT REFERENCES applications(id) ON DELETE SET NULL,
    department TEXT,
    position TEXT,
    started_at DATE,
    status TEXT NOT NULL DEFAULT 'ongoing',
    retrospective JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_sessions_user ON sessions(user_id);
CREATE INDEX idx_sessions_company ON sessions(company_id);
CREATE INDEX idx_sessions_application ON sessions(application_id);

CREATE TABLE rounds (
    id BIGSERIAL PRIMARY KEY,
    session_id BIGINT REFERENCES sessions(id) ON DELETE CASCADE,
    application_id BIGINT REFERENCES applications(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    sort_order INT NOT NULL DEFAULT 0,
    date DATE,
    form TEXT,
    passed TEXT NOT NULL DEFAULT 'pending',
    retrospective JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_rounds_session ON rounds(session_id);
CREATE INDEX idx_rounds_application ON rounds(application_id);

CREATE TABLE interviewer_personas (
    id BIGSERIAL PRIMARY KEY,
    owner_user_id BIGINT REFERENCES users(id) ON DELETE CASCADE,
    name VARCHAR(100) NOT NULL,
    title VARCHAR(100),
    persona_prompt TEXT NOT NULL DEFAULT '',
    difficulty_hint TEXT,
    temperature_hint NUMERIC(3,2)
        CHECK (temperature_hint IS NULL OR temperature_hint BETWEEN 0.3 AND 0.9),
    focus_tags TEXT[] NOT NULL DEFAULT '{}',
    builtin BOOLEAN NOT NULL DEFAULT false,
    deleted_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX uq_personas_owner_name
    ON interviewer_personas(owner_user_id, name)
    WHERE owner_user_id IS NOT NULL AND deleted_at IS NULL;
CREATE UNIQUE INDEX uq_personas_builtin_name
    ON interviewer_personas(name) WHERE builtin;

CREATE TABLE drills (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id),
    kind TEXT NOT NULL,
    title TEXT NOT NULL,
    position TEXT,
    direction TEXT,
    stages JSONB,
    status TEXT NOT NULL DEFAULT 'ongoing',
    score INT,
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ,
    target_questions INT NOT NULL DEFAULT 5,
    ref_content TEXT,
    grading TEXT,
    application_id BIGINT REFERENCES applications(id) ON DELETE SET NULL,
    llm_response_id TEXT,
    dossier JSONB,
    interview_state JSONB,
    persona_id BIGINT REFERENCES interviewer_personas(id) ON DELETE SET NULL
);
CREATE INDEX idx_drills_kind ON drills(kind);
CREATE INDEX idx_drills_user ON drills(user_id);

CREATE TABLE drill_messages (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id),
    drill_id BIGINT NOT NULL REFERENCES drills(id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    kind TEXT NOT NULL DEFAULT 'message',
    content TEXT NOT NULL,
    score INT,
    difficulty INT,
    feedback TEXT,
    intent VARCHAR(32),
    meta JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_drill_messages_drill ON drill_messages(drill_id, created_at);

CREATE TABLE skills (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    parent_id BIGINT REFERENCES skills(id) ON DELETE CASCADE,
    name VARCHAR(100) NOT NULL,
    path VARCHAR(500) NOT NULL,
    icon VARCHAR(50),
    visibility VARCHAR(20) NOT NULL DEFAULT 'private',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX idx_skills_user_parent_name
    ON skills(user_id, COALESCE(parent_id, 0), name);
CREATE INDEX idx_skills_user_path ON skills(user_id, path);

CREATE TABLE questions (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id),
    round_id BIGINT NOT NULL REFERENCES rounds(id) ON DELETE CASCADE,
    drill_id BIGINT REFERENCES drills(id) ON DELETE SET NULL,
    parent_id BIGINT REFERENCES questions(id) ON DELETE CASCADE,
    skill_id BIGINT REFERENCES skills(id) ON DELETE SET NULL,
    predicted_position_id BIGINT REFERENCES positions(id) ON DELETE SET NULL,
    content TEXT NOT NULL,
    content_normalized TEXT,
    my_answer TEXT,
    source TEXT NOT NULL DEFAULT 'manual',
    question_type TEXT NOT NULL DEFAULT 'professional_knowledge',
    difficulty INT NOT NULL DEFAULT 3,
    starred BOOLEAN NOT NULL DEFAULT false,
    asked_at DATE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_questions_round ON questions(round_id);
CREATE INDEX idx_questions_user ON questions(user_id);
CREATE INDEX idx_questions_parent ON questions(parent_id);
CREATE INDEX idx_questions_skill_id ON questions(skill_id);
CREATE INDEX idx_questions_type ON questions(question_type);
CREATE INDEX idx_questions_predicted_pos
    ON questions(user_id, predicted_position_id) WHERE predicted_position_id IS NOT NULL;
CREATE INDEX idx_questions_user_norm
    ON questions(user_id, content_normalized)
    WHERE parent_id IS NULL AND content_normalized IS NOT NULL;

CREATE TABLE tags (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id),
    name TEXT NOT NULL,
    UNIQUE (user_id, name)
);

CREATE TABLE question_tags (
    question_id BIGINT NOT NULL REFERENCES questions(id) ON DELETE CASCADE,
    tag_id BIGINT NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY (question_id, tag_id)
);
CREATE INDEX idx_question_tags_tag ON question_tags(tag_id);

CREATE TABLE question_skills (
    question_id BIGINT NOT NULL REFERENCES questions(id) ON DELETE CASCADE,
    skill_id BIGINT NOT NULL REFERENCES skills(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (question_id, skill_id)
);
CREATE INDEX idx_question_skills_skill ON question_skills(skill_id);

CREATE TABLE question_answers (
    id BIGSERIAL PRIMARY KEY,
    question_id BIGINT NOT NULL REFERENCES questions(id) ON DELETE CASCADE,
    source TEXT NOT NULL DEFAULT 'manual',
    content TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_question_answers_q ON question_answers(question_id, created_at DESC);

CREATE TABLE question_rounds (
    question_id BIGINT NOT NULL REFERENCES questions(id) ON DELETE CASCADE,
    round_id BIGINT NOT NULL REFERENCES rounds(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (question_id, round_id)
);
CREATE INDEX idx_question_rounds_round ON question_rounds(round_id);

CREATE TABLE analyses (
    id BIGSERIAL PRIMARY KEY,
    question_id BIGINT NOT NULL REFERENCES questions(id) ON DELETE CASCADE,
    provider TEXT,
    model TEXT,
    tags JSONB,
    difficulty INT,
    ref_answer TEXT,
    score INT,
    feedback TEXT,
    raw JSONB,
    answer_snapshot TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_analyses_question ON analyses(question_id);

CREATE TABLE comments (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id),
    question_id BIGINT REFERENCES questions(id) ON DELETE CASCADE,
    session_id BIGINT REFERENCES sessions(id) ON DELETE CASCADE,
    body TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (question_id IS NOT NULL OR session_id IS NOT NULL)
);
CREATE INDEX idx_comments_question ON comments(question_id);
CREATE INDEX idx_comments_session ON comments(session_id);

-- ---------- 复习 / 积分 / 任务 ----------
CREATE TABLE review_records (
    id BIGSERIAL PRIMARY KEY,
    question_id BIGINT NOT NULL UNIQUE REFERENCES questions(id) ON DELETE CASCADE,
    ease DOUBLE PRECISION NOT NULL DEFAULT 2.5,
    interval_days INT NOT NULL DEFAULT 1,
    next_review_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_result TEXT,
    review_count INT NOT NULL DEFAULT 0,
    last_reviewed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_review_next ON review_records(next_review_at);

CREATE TABLE review_logs (
    id BIGSERIAL PRIMARY KEY,
    question_id BIGINT NOT NULL REFERENCES questions(id) ON DELETE CASCADE,
    rating INT NOT NULL CHECK (rating BETWEEN 1 AND 4),
    reviewed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_review_logs_q_time ON review_logs(question_id, reviewed_at);

CREATE TABLE mall_items (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id),
    name TEXT NOT NULL,
    cost INT NOT NULL CHECK (cost > 0),
    emoji TEXT NOT NULL DEFAULT '🎁',
    sort_order INT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE points_ledger (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id),
    amount INT NOT NULL,
    category TEXT NOT NULL,
    ref_type TEXT,
    ref_id BIGINT,
    note TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_points_ledger_created ON points_ledger(created_at);
CREATE INDEX idx_points_ledger_cat ON points_ledger(category);
CREATE INDEX idx_points_ledger_user ON points_ledger(user_id);
CREATE UNIQUE INDEX idx_points_ledger_idem
    ON points_ledger(user_id, category, note)
    WHERE category IN ('daily_goal', 'streak7', 'milestone');

CREATE TABLE background_jobs (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    payload JSONB NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    attempts INT NOT NULL DEFAULT 0,
    max_attempts INT NOT NULL DEFAULT 3,
    claimed_at TIMESTAMPTZ,
    heartbeat_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    error TEXT,
    progress JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_bg_jobs_claim
    ON background_jobs(kind, created_at) WHERE status IN ('pending', 'running');
CREATE INDEX idx_bg_jobs_user ON background_jobs(user_id);

-- ---------- 内置面试官（ensure_builtins 同源，幂等） ----------
INSERT INTO interviewer_personas(
    owner_user_id, name, title, persona_prompt, difficulty_hint, temperature_hint, focus_tags, builtin
) VALUES
(
  NULL, '沉稳技术官', '资深后端架构师',
  E'你是一位沉稳内敛的资深后端架构师，语速平缓但问题扎实。你偏好从底层机制出发逐层深入：先确认候选人对基础数据结构与协议语义的理解，再推进到高并发与一致性权衡。你不接受模糊表述，会礼貌地要求候选人给出具体依据。',
  '注重原理深度与工程权衡', 0.35, ARRAY['系统设计','数据库','缓存'], true
),
(
  NULL, '犀利交叉官', '跨部门压力面试官',
  E'你是一位言辞犀利的交叉面考官，习惯连环追问并快速切换考点，专门检验候选人在压力下的思路稳定性。你会抓住回答中的矛盾点当场对质（contradiction），也常把问题抛到没准备过的边界场景。',
  '高压节奏 · 矛盾对质 · 快速切题', 0.60, ARRAY['场景设计','线上排障','项目深挖'], true
),
(
  NULL, '亲和HRBP', 'HR 业务伙伴',
  E'你是一位温和亲切的 HRBP，关注候选人的协作方式、成长动机与职业规划。你的提问开放而有层次，善于用追问帮助候选人展开行为面试的 STAR 结构，营造接近真实 HR 面的氛围。',
  '行为面试 · 协作与成长动机', 0.85, ARRAY['行为面','职业规划'], true
),
(
  NULL, '经典面试官', '通用技术面试',
  E'你是一位经验丰富的通用技术面试官，风格均衡而专业。你会根据候选人的岗位与方向灵活调整提问深度，既考察基础原理，也关注工程实践与系统思维。你的提问循序渐进，善于用追问帮助候选人展现真实水平。',
  '均衡全面 · 循序渐进', 0.5, ARRAY[]::text[], true
);
