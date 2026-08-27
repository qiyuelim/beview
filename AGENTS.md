# AGENTS.md

Development spec and file hub for Beview. 这是**如何在该代码库上开发**的
事实来源；用户向导入门见 [README.md](README.md)。

## Project

**Beview**（Be Ready, Review Better.）是个人求职工作台：投递跟踪、简历、AI 陪练、面试复盘、复习强化一条闭环。
面试域按 公司 → 岗位 → 投递 → 轮次 → 题目 组织；陪练沉淀仍可走内部 session 容器。
多用户（管理员开账号，行级隔离，无自助注册）；局域网 HTTP 可访问，需登录。

当前主线：投递枢纽、JD/押题 AiJob、面试官人格、哨兵流式陪练、FSRS 复习、技能图谱、
积分与 ICS 日历。LLM 只走 OpenAI Responses API（`/responses`）。

- 技术栈：Rust axum（JSON API）+ PostgreSQL + React-TS (Vite) SPA。
- 单仓库 monorepo：`server/`（Cargo workspace member）+ `frontend/`（Vite React-TS）。
- Edition 2024。迁移用 sqlx migrate。DSN / 端口 / OTLP 走 gitignored `server/config.toml`，
  **管理员账号绝不用环境变量引导**。
- 对外介绍：[docs/architecture-c4.html](docs/architecture-c4.html)。ADR、规划、词表、交接
  仅本机 `docs/`（不入库）；改架构先在本机落 ADR，再改代码与 C4 图。

## 开发基准（Development baseline）

> 开发任何功能前先读这里：这是产品方向与体验约束，与下方不变量同等重要。

1. **文档先行。** 架构级改动先落本机 ADR（`docs/adr/`），对外架构介绍同步
   [docs/architecture-c4.html](docs/architecture-c4.html)。代码与对外文档不同步 = 未完成。
2. **核心闭环优先。** 改动必须落在投递 / 简历 / 陪练 / 复盘 / 复习这条主线上；不要顺手展开
   未立项的能力（自助注册、公网多租户、默认 HTTPS、批量静默 LLM 等）。
3. **LLM 按需触发、绝不在后台静默调用。** 只有用户点分析 / 陪练 / 讲解 / 押题等才发请求；
   所有 LLM 出口必须带 OTel span（记 model / 耗时 / token），并计入 `/api/metrics`。
4. **评分量纲唯一。** 综合分 0–100（正确性 50 / 完整性 30 / 表达清晰度 20）、难度 1–5；
   prompt 与代码共用同一量纲，禁止另起一套。
5. **认证与数据安全。** 局域网可访问 → 除 login / setup / health / calendar.ics 外全部 API
   需登录（含 metrics）；密码 argon2 哈希；API key 设置页掩码、库内 AES-256-GCM；备份用
   `scripts/backup.sh`（必须连带 `server/.master_key`）。
6. **首启建管理员，不走环境变量。** users 表为空时前端进 `/setup` 创建管理员；环境变量 /
   `.env` / 配置文件只用于运维配置（DB DSN、端口、OTLP endpoint），**绝不用于安全凭据引导**。
7. **体验词表以本机 `docs/context.md` 为准。** 新增状态 / 概念先写进词表再用，避免全仓库叫法分裂。
8. **UI 设计语言以本机 ADR-0015 为准。** 数据驾驶舱方向（素雅灰主色 / 琥珀强调 / 数据密集）：
   颜色一律经语义 token（Tailwind `@theme` CSS variables，映射本机
   `docs/design-system/MASTER.md`）、禁止裸色值；双主题（亮默认）成对定义；**灰字纪律白名单制**
   ——muted 文本仅限 placeholder / 错误警告 / disabled / 时间来源等元数据 / 必须的字段说明 /
   tooltip / 确有价值的非核心 metadata，白名单外升级或删除；表单一经 FormField 渲染；
   提示文案必须与真实状态一致；每页验收含 375 / 768 / 1280 三断点与双主题目检。违反即
   Standards 轴发现。

## Module layout

- `server/` — axum 后端 crate（二进制名 `beview`）：
  - `src/main.rs` 启动（配置 / 路由装配 / 静态托管）
  - `src/lib.rs` `build_api`（与集成测试共用）
  - `src/auth.rs` 登录 / 会话 / argon2；`src/crypto.rs` API key 加解密
  - `src/config.rs` `config.toml`；`src/db.rs` 连接与迁移；`src/state.rs` AppState
  - `src/routes/` HTTP 层（19 个业务模块 + `mod.rs` 认证中间件 / setup / admin）
  - `src/services/` 领域服务（context_manager、answer_flow、memory_model、skill_*、job_queue 等）
  - `src/contracts/` AI 契约（12 个登记出口，`contracts::execute` 唯一咽喉）
  - `src/llm.rs` Responses API 引擎；`src/prompts.rs` 提示词注册表（14 个 key）
  - `src/observe.rs` 日志 / metrics / span；`src/points.rs` 积分账本
  - `migrations/` sqlx 迁移；`tests/` 集成测试 + `tests/golden/` 快照
- `frontend/` — Vite + React-TS：`src/pages/` 25 个模块、`src/api/` 客户端、
  `src/components/`（Layout 主导航 10 项 + 设计原语）、`src/ai/jobs.ts` AiJob 事件。
- `scripts/backup.sh`、`scripts/reset_data.sh`

## Commands

```
cp server/config.example.toml server/config.toml   # 首次：填写 DSN
cd server && cargo run                             # 后端，端口见 config.toml
cd frontend && npm run dev                         # Vite，/api 代理到后端
cd server && cargo test                            # 后端测试
cd frontend && npm run build                       # 产物写入 server/static
./scripts/backup.sh                                # pg_dump + .master_key
```

## File hub

| Path | Role | Touch when |
|---|---|---|
| `README.md` | 用户入口 | 能力、启动方式、安全边界变化时 |
| `AGENTS.md` | 开发规范 | 基准、布局、命令变化时 |
| `docs/architecture-c4.html` | 对外架构介绍 | 容器 / 组件 / 主数据流变化时 |
| `server/config.example.toml` | 配置样例 | 新增运维配置项时 |

## No Negative Echo

生成最终产物及其包装时，包括标题、文件名、正文、注释、标签、commit、
PR 和交付说明，只描述最终采用的状态，假设读者没看过本次会话。

- 会话里的否决、中间尝试和措辞纠正，只当作控制信息，不要让它们成为最终产物的命名或叙述中心。
- 对每个交付面分别判断：不知道本次会话的读者需要这条信息吗？省略会不会导致不准确、不安全、误导或兼容性信息缺失？它是不是任务开始时已提交或用户确认状态中的真实变化，而且当前交付面需要解释它？
- 「不要提 X」不是让你写「无 X」。标题、文件名、开篇和标签应从正向目标重新生成，不要逐词修改被否文案。
- 保留真实的基线变化、已经执行的外部操作，以及必要的技术名称、诊断、测试和快照。任务开始前已有的用户改动不算被否内容。
- 不要把与本任务无关的改动写进本次 commit、PR 或交付说明。对比、引用、审计和迁移说明，只在用户要求或当前交付面确实需要时保留。
- 写完后通读全部用户可见内容及其包装，包括文件名、元数据和 hook 改写。内容发生变化后重新检查，不要另加「已清理」或「无残留」类声明。
