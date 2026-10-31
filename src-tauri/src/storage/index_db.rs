//! 派生索引库 `cache/index.db`（M2 建库承载 sessions/stats 缓存，
//! 落地 stats_cache 面）。
//!
//! 名词解释：「派生索引库」指从权威数据（stats/*.json 等文件）算出来的
//! 查询加速库——SQLite 单文件数据库；「真相」指唯一权威数据源，本库对
//! stats 而言只是缓存，删掉可随时从 stats/*.json 重建（三铁律）。
//!
//! 生命周期（定稿）：
//! - **启动校验**：`PRAGMA quick_check`（页面级完整性检查）+ `PRAGMA
//!   user_version` 比对（本仓 schema 版本 = [`INDEX_DB_VERSION`]）；任一
//!   不符 → 弃库（删文件）重建（建表 DDL 逐字）。
//! - **维护**：stats 落账成功后 upsert 对应 (day, model_id) 行；失败仅记
//!   日志、不阻断落账主链路（"异步"语义的实质 = 不影响主链路时序，
//!   进程内同步执行）。库被清空/重建后由查询路径发现缺日回填。
//! - **降级**：库打开失败/重建失败（只读盘、权限、磁盘满等）→ `degraded`
//!   态，统计查询直读 stats/*.json（真相面），应用其余功能不受影响。
//!
//! 线程纪律：rusqlite `Connection` 非 Sync，包 `Mutex` 串行化（写入低频：
//! 每 turn 一次落账 upsert + 低频查询）。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::Connection;

use crate::backend::frames::UsageTotal;

/// 本仓 index.db 的 schema 版本（DDL 变化时升版触发重建）。
pub const INDEX_DB_VERSION: i32 = 1;

/// 库文件路径：`<root>/cache/index.db`。
pub fn index_db_path(root: &Path) -> PathBuf {
    root.join("cache").join("index.db")
}

/// DDL（逐字；stats_cache 的 runs 列为 回写增补）。
const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS sessions (
  id TEXT PRIMARY KEY, project_id TEXT, title TEXT NOT NULL,
  created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
  revision INTEGER NOT NULL, message_count INTEGER NOT NULL DEFAULT 0,
  total_tokens INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS stats_cache (
  day TEXT NOT NULL, model_id TEXT NOT NULL,
  input INTEGER NOT NULL, output INTEGER NOT NULL,
  cache_read INTEGER NOT NULL, cache_write INTEGER NOT NULL,
  runs INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (day, model_id));
";

/// 单个 (day, model) 聚合行（stats_cache 的读写面，与 DailyStats 的
/// models 元素同构 + runs）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatsCacheRow {
    pub day: String,
    pub model_id: String,
    pub usage: UsageTotal,
    pub runs: u64,
}

/// 派生索引库句柄。`db = None` 即降级态（库不可用，统计直读 stats/*.json）。
pub struct IndexDb {
    path: PathBuf,
    /// 库连接（Mutex 串行化；None = 降级）。
    db: Mutex<Option<Connection>>,
}

/// 打开（或创建）index.db：不存在则建目录建表；quick_check / user_version
/// 校验不符则删文件重建。全部失败 → 降级态（db=None），永不 panic。
pub fn open(root: &Path) -> IndexDb {
    let path = index_db_path(root);
    let db = try_open(&path);
    IndexDb {
        path,
        db: Mutex::new(db),
    }
}

/// 尝试打开并校验库；返回 None = 不可用（降级）。
fn try_open(path: &Path) -> Option<Connection> {
    match open_fresh(path) {
        Ok(conn) => Some(conn),
        Err(e) => {
            #[cfg(debug_assertions)]
            eprintln!("[craftwave] index.db 打开失败（降级直读 stats/*.json）：{e}");
            let _ = e;
            None
        }
    }
}

/// 打开/重建的单一尝试：开连接 → 校验（quick_check + user_version）→
/// 校验不符删文件重开 → 建表 + 写版本。
fn open_fresh(path: &Path) -> Result<Connection, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("建 cache/ 目录失败：{e}"))?;
    }
    let conn = Connection::open(path).map_err(|e| format!("打开失败：{e}"))?;
    let checked = (|| -> Result<bool, String> {
        let quick: String = conn
            .query_row("PRAGMA quick_check", [], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        Ok(quick == "ok" && version == INDEX_DB_VERSION)
    })()
    .inspect_err(|e| {
        #[cfg(debug_assertions)]
        eprintln!("[craftwave] index.db 校验异常：{e}");
    })
    .unwrap_or(false);
    if !checked {
        // 弃库重建：关连接 → 删文件 → 重开
        drop(conn);
        if let Err(e) = std::fs::remove_file(path) {
            // 文件不存在 = 首次创建路径，不算失败
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(format!("弃库失败（{e}）"));
            }
        }
        let conn = Connection::open(path).map_err(|e| format!("重建打开失败：{e}"))?;
        return init_schema(conn);
    }
    Ok(conn)
}

/// 建表 + 写 schema 版本（新库/重建共用）。
fn init_schema(conn: Connection) -> Result<Connection, String> {
    conn.execute_batch(SCHEMA_SQL)
        .map_err(|e| format!("建表失败：{e}"))?;
    conn.execute_batch(&format!("PRAGMA user_version = {INDEX_DB_VERSION};"))
        .map_err(|e| format!("写 schema 版本失败：{e}"))?;
    Ok(conn)
}

impl IndexDb {
    /// 库文件路径。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 是否处于降级态（库不可用；查询直读 stats/*.json）。
    pub fn degraded(&self) -> bool {
        self.db
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_none()
    }

    /// upsert 一批 stats_cache 行（stats 落账成功后的写穿面；失败仅记日志，
    /// 不上抛—— 维护行）。降级态静默跳过。
    pub fn upsert_stats(&self, row: &StatsCacheRow) {
        let guard = self
            .db
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(conn) = guard.as_ref() else {
            return; // 降级态：真相面直读，缓存写失败不重试
        };
        let result = conn.execute(
            "INSERT INTO stats_cache (day, model_id, input, output, cache_read, cache_write, runs)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(day, model_id) DO UPDATE SET
               input = excluded.input, output = excluded.output,
               cache_read = excluded.cache_read, cache_write = excluded.cache_write,
               runs = excluded.runs",
            rusqlite::params![
                row.day,
                row.model_id,
                row.usage.input as i64,
                row.usage.output as i64,
                row.usage.cache_read as i64,
                row.usage.cache_write as i64,
                row.runs as i64
            ],
        );
        if let Err(e) = result {
            #[cfg(debug_assertions)]
            eprintln!("[craftwave] stats_cache upsert 失败（仅记日志）：{e}");
        }
    }

    /// 范围查询 stats_cache（升序按 day）：返回 None = 库不可用（调用方
    /// 降级直读）；Some(rows) = 缓存命中面。`min_rows` 为期望最少行数——
    /// 缓存空/缺日（重建后首次查询）时调用方回填后重查。
    pub fn query_stats_range(&self, from: &str, to: &str) -> Option<Vec<StatsCacheRow>> {
        let mut guard = self
            .db
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let conn = guard.as_mut()?;
        let mut stmt = conn
            .prepare(
                "SELECT day, model_id, input, output, cache_read, cache_write, runs
                 FROM stats_cache WHERE day >= ?1 AND day <= ?2 ORDER BY day, model_id",
            )
            .ok()?;
        let rows = stmt
            .query_map([from, to], |row| {
                Ok(StatsCacheRow {
                    day: row.get(0)?,
                    model_id: row.get(1)?,
                    usage: UsageTotal {
                        input: row.get::<_, i64>(2)?.max(0) as u64,
                        output: row.get::<_, i64>(3)?.max(0) as u64,
                        cache_read: row.get::<_, i64>(4)?.max(0) as u64,
                        cache_write: row.get::<_, i64>(5)?.max(0) as u64,
                    },
                    runs: row.get::<_, i64>(6)?.max(0) as u64,
                })
            })
            .ok()?
            .collect::<Result<Vec<_>, _>>()
            .ok()?;
        Some(rows)
    }

    /// 全量回填 stats_cache（库重建/清空后的恢复面：调用方从 stats/*.json
    /// 真相面读出全部 (day, model) 行传入）。降级态静默跳过；失败仅记日志。
    pub fn backfill_stats(&self, rows: &[StatsCacheRow]) {
        let guard = self
            .db
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(conn) = guard.as_ref() else {
            return;
        };
        let result = (|| -> rusqlite::Result<()> {
            conn.execute("DELETE FROM stats_cache", [])?;
            for row in rows {
                conn.execute(
                    "INSERT OR REPLACE INTO stats_cache
                     (day, model_id, input, output, cache_read, cache_write, runs)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    rusqlite::params![
                        row.day,
                        row.model_id,
                        row.usage.input as i64,
                        row.usage.output as i64,
                        row.usage.cache_read as i64,
                        row.usage.cache_write as i64,
                        row.runs as i64
                    ],
                )?;
            }
            Ok(())
        })();
        if let Err(e) = result {
            #[cfg(debug_assertions)]
            eprintln!("[craftwave] stats_cache 回填失败（仅记日志）：{e}");
        }
    }

    /// 丢弃当前库并重建（测试与手动恢复面；重建失败保持降级态）。
    pub fn rebuild(&self) {
        {
            let mut guard = self
                .db
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard = None; // 先降级，再尝试重开
        }
        let reconnected = try_open(&self.path);
        *self
            .db
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = reconnected;
    }
}

#[cfg(test)]
#[path = "index_db_test.rs"]
mod index_db_test;
