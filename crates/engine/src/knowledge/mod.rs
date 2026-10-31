//! knowledge 模块：知识域资源——系统提示面与技能/记忆（域）。
//!
//! 本模块承载技能系统（`skills.rs`）与技能命令（`commands.rs`）；
//! M1 的系统提示组装面此前散在 backend/wire（`with_system_prefix_layers`）
//! 与 host/commands/chat.rs（注入次序），不迁址——本域只收编新资源。
//! memory/RAG 同域资源随 M3 落地（04）。
//!
//! 依赖方向（单向）：knowledge 只依赖 util（原子写/错误码）与存储形状，
//! 不依赖 tools/agent/host；tools 经 [`crate::knowledge::skills::SkillRegistry`]
//! 进程级单例反查 skill 工具的动态 description（与 tools→mcp 反查同精神）。

// 项目指令（AGENTS.md/CRAFTWAVE.md/CLAUDE.md）的信任核验与注入
// （信任挂接点）。
pub mod instructions;
// 会话级信任集（allow_session 内存面；起技能与项目指令共用）。
pub mod session_trust;
pub mod skills;
pub mod trust;

/// 技能测试的进程级单例互斥面（cfg(test)）：SkillRegistry::global() 是
/// 进程唯一实例，skills_test / commands_test / chat_test 并行时会互相
/// 改写三来源根——知识域测试以本锁串行化（chat_test 不经本锁，但其
/// configure 只影响 skill_spec 的 description 文本，无断言依赖）。
#[cfg(test)]
#[path = "skills_test.rs"]
mod skills_test;

#[cfg(test)]
#[path = "instructions_test.rs"]
mod instructions_test;
