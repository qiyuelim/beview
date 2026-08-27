//! 领域服务层（ADR-0014 §25 原则四）：状态机与跨实体业务规则的唯一归属地。
//! UI 与 HTTP 路由不拥有这些规则——路由只做参数解析与响应组装。
pub mod application_service;
pub mod answer_flow;
pub mod context_manager;
pub mod job_queue;
pub mod memory_model;
pub mod skill_query;
pub mod skill_service;
pub mod system_containers;
