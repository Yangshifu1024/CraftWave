//! 会话树存取引擎单元测试（独立测试文件，源码零测试）。
//!
//! 覆盖 语义：save/load 往返一致（roundtrip，指保存后读回逐字段相等）、
//! 仅追加缺失节点、幂等全量同步、revision 乐观并发冲突、孤儿节点/未知活动叶/
//! 非法会话 id 参数校验、NotFound；以及 的 canonical 追加、崩溃恢复
//!（interrupted marker 修复）、索引与 toolres 边车往返、的会话删除清理、
//! 的 compaction 条目与 升级备份守卫。同型用例在单个 #[test] 内
//! 表驱动合并，断言消息带用例标识。


