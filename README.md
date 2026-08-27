# Beview 
> Be Ready, Review Better.
>
> 复盘每一次面试，成就下一个 Offer。

个人求职工作台：投递跟踪、简历、AI 陪练、面试复盘、复习强化一条闭环。局域网可访问；多用户由管理员开账号，数据按用户行级隔离。

把求职过程沉淀成可检索、可评分、可复习的个人资产：

- **投递枢纽**：公司 → 岗位 → 投递 → 轮次 → 题目；岗位 JD 解读与押题、投递状态机、轮次通过标记与复盘报告。
- **简历**：原文解析为结构化资产，支持多版本留档、AI 变更集审批、Markdown 导出。
- **AI 陪练**：面试官人格、SSE 流式多轮、即时判分、题目自动沉淀进题库与复习队列。
- **复习强化**：FSRS 排程卡片流（记得 / 模糊 / 忘了）、今日队列、错题本、AI 讲解。
- **题库与图谱**：真题录入与分析（标签 / 参考答案 / 难度 / 综合评分 / 中文点评）、三层技能图谱、能力雷达。
- **数据与日历**：漏斗与活动流、积分账本、ICS 订阅（面试日程 + 复习到期）。

LLM 仅在你点击分析、陪练、讲解等操作时调用；后台不静默消耗 token。

## 技术栈

| 层 | 选型 |
|---|---|
| 前端 | React 18 + TypeScript (Vite)，25 个页面模块，主导航 10 项 |
| 后端 | Rust axum（JSON API；生产期托管 `server/static`） |
| 数据库 | PostgreSQL（sqlx migrate；DSN 写在 gitignored `server/config.toml`） |
| LLM | OpenAI Responses API（`POST {base_url}/responses`）。chat/completions 网关不可用 |
| 可观测 | tracing 结构化日志 + OpenTelemetry span（stdout 默认，OTLP 可选）+ `/api/metrics`（需登录） |

主导航：求职台 / 投递 / 陪练 / 简历 / 企业 / 题库 / 图谱 / 数据 / 积分 / 设置。

## 快速开始

```bash
# 1. 配置
cp server/config.example.toml server/config.toml
# 编辑 database_url 指向你的 PostgreSQL，并建库 beview

# 2. 开发（双进程）
mkdir -p logs
cd server && cargo run                      # 默认 0.0.0.0:8765，日志目录 logs/
cd frontend && npm install && npm run dev   # Vite :5173，/api 代理到后端

# 3. 生产（单进程：axum 托管 API + 前端静态）
cd frontend && npm run build
cd server && cargo build --release
./target/release/beview

# 备份（需 pg_dump；会连带复制 server/.master_key）
./scripts/backup.sh
```

首次打开浏览器 → users 表为空时进入 `/setup` 创建管理员 → 登录 → 设置页配置 LLM Provider / Model。模型可声明「结构化输出」「联网搜索」；高级参数含温度、top_p、思考强度、store、extra_body。默认端口 8765。

## 安全边界（务必阅读）

- **仅限可信局域网**：服务为纯 HTTP，密码与会话 cookie 明文传输。**不要直接暴露到公网**。远程访问请经反向代理终止 TLS，并自行为会话 cookie 补 `Secure`（`server/src/auth.rs`）。
- 密码 argon2 哈希；登录同一用户名 60 秒内失败 5 次返回 429（进程内存，重启清零）。
- 除 `/api/health`、`/api/setup`、`/api/setup/status`、`/api/login`、`/api/calendar.ics` 外，全部 API 需登录。`/api/metrics` 需登录。
- 日历订阅用 per-user token（日历 App 无法走 cookie）；泄露可在设置页重新生成吊销。
- LLM `api_key` 落库为 AES-256-GCM 密文（`enc:v1:`），主密钥 `server/.master_key`（gitignored，权限 0600）。**备份数据库必须连同备份该文件**。
- 管理员账号只通过 `/setup` 或管理员开户创建。

## 日志与排障

后端结构化 JSON 日志写到 `logs/`（按 interface / remote / db / error / app 分类）以及 stdout。排查：先看 `http request` 的 `status` → 用 `traceparent` 或 `x-trace-id` 串同一次请求的 LLM / SQL span。

常见情况：

- 判卷或分析慢：看 `llm.*` span 的 `duration_ms` 与指标 `llm_duration_seconds`
- LLM 报错：设置页「测试连接」；确认端点是 `/responses` 而不是 `/chat/completions`
- 401：会话在内存，重启后端需重新登录
- 忘记管理员密码：`cd server && cargo run --example reset_password -- '新密码'`（argv 传入，不落盘）

## 文档

| 文档 | 内容 |
|---|---|
| [AGENTS.md](AGENTS.md) | 开发规范与不变量（开发前必读） |
| [docs/architecture-c4.html](docs/architecture-c4.html) | 交互式 C4 架构图 |

## 状态

多用户底座、投递/岗位枢纽、JD 与押题任务化、面试官人格与哨兵流式陪练、FSRS 复习、技能图谱。
