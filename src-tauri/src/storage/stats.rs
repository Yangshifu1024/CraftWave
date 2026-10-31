//! 用量统计存取（M2 增量两行）：日/模型匿名聚合 + run/会话
//! 四口径明细，两个权威文件面。
//!
//! 名词解释：「四口径」指 input / output / cacheRead / cacheWrite 四项 token
//! 计数（UsageTotal）；「匿名聚合」指按天按模型汇总的统计文件里不含
//! 任何会话标识——数字是真实消耗，但无法反查出自哪个会话（删除保留
//! 口径的存储侧定案）；「落账」指把一次模型调用的用量增量记入统计文件。
//!
//! 两个文件面（字段级）：
//!
//! 1. `stats/<YYYY-MM-DD>.json`（日/模型聚合，90 天保留）：
//!    `{ version: 1, day, models: { modelId: { modelName, 四口径, runs } } }`。
//! - 每次模型响应返回 usage 时按**当时本地日期**固定记账，跨午夜
//!   run 可分属两天，之后切时区不重划历史（记账时点决定归属，恢复/查询
//!   不改写已落日期）。
//! - 主代理、子代理与辅助调用（suggest/压缩）都进聚合，只计一次；
//!   `runs` 只计主 run（含队列自动发起），子 run 与辅助调用不增加。
//!    - **匿名聚合**：不含 sessionId——删除会话不改写本文件，聚合面永不
//!      暴露已删会话身份（字段级定案）。
//!    - `util::atomic` 原子写（合并 = 读整文件 → 增量累加 → 整文件重写）；
//!      解析失败先把原文件备份为 `<name>.corrupt` 再按空重建（宁缺勿炸）。
//!
//! 2. `sessions/<sid>.usage.json`（run/会话四口径明细，权威面）：
//!    `{ version: 1, sessionTotal, runs: { runId: { kind, startedAt,
//!    endedAt?, byModel: { modelId: { modelName, 四口径 } } } } }`。
//!    - sessionTotal = 全部 runs 之和（主/子/辅助，会话累计权威，与
//!      tokens:update 全量同源同构）。
//!    - run 内切模型按拍归属各自 modelId 桶（真实归属，不全算给结束模型）。
//! - 读取契约： `session_usage`；旧数据无明细文件时回退 index.json
//!   的 totalTokens 标量（不伪造四口径拆分）。
//! - 会话删除随目录边界删除本文件。
//!
//! 线程纪律：两个存取器与 SessionIndex 同款——进程内 Mutex 串行化读改写，
//! 幂等（同一增量重复落账 = 重复计数，落账方保证一次：usage 帧差分增量在
//! 帧出口处产生，attach 重放不经落账链，天然不重复）。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::backend::frames::UsageTotal;
use crate::util::atomic::write_json;

/// 日/模型统计目录名（数据根下）。
pub const STATS_DIR: &str = "stats";

/// 日/模型统计保留天数（90 天；按文件名日期惰性清理）。
pub const STATS_RETENTION_DAYS: u32 = 90;

/// 聚合文件与明细文件的格式版本（结构不兼容时升版并写迁移）。
pub const STATS_FORMAT_VERSION: u32 = 1;

/// 单模型的当日聚合（stats/<day>.json 的 models 元素；BTreeMap 键序稳定，
/// 文件内容跨次写不抖动）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDayStats {
    /// 该模型当日最近一次调用时的显示名快照（改名/删除配置不影响历史）。
    #[serde(default)]
    pub model_name: String,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    /// 当日该模型的主 run 次数（仅主 run 含队列发起；子 run 与辅助
    /// 调用不增加）。
    #[serde(default)]
    pub runs: u64,
}

/// stats/<YYYY-MM-DD>.json 文件载荷（匿名聚合；**无会话身份字段**）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyStats {
    pub version: u32,
    /// 该文件承载的本地日期（`YYYY-MM-DD`，与文件名一致）。
    pub day: String,
    /// 按模型 id 聚合的当日统计。
    #[serde(default)]
    pub models: BTreeMap<String, ModelDayStats>,
}

/// run 明细的单模型归属桶（usage.json 的 byModel 元素）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRunUsage {
    /// 调用当时的模型显示名快照。
    #[serde(default)]
    pub model_name: String,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

/// 单个 run 的用量明细（usage.json 的 runs 元素）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunUsage {
    /// run 类别：main = 主 run（含队列自动发起）；sub = 子代理 run；
    /// aux = 辅助调用（suggest/压缩等后台小调用，计入会话不计轮次）。
    pub kind: String,
    /// run/调用开始时刻（Unix 毫秒）。
    #[serde(default)]
    pub started_at: i64,
    /// 收口时刻（Unix 毫秒；运行中缺省）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
    /// 按模型分桶的用量（run 内切模型按拍归属真实模型）。
    #[serde(default)]
    pub by_model: BTreeMap<String, ModelRunUsage>,
}

/// sessions/<sid>.usage.json 文件载荷（run/会话四口径权威明细）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUsage {
    pub version: u32,
    /// 会话累计（主/子/辅助总和；与 tokens:update 全量同源同构）。
    #[serde(default)]
    pub session_total: UsageTotal,
    /// 按 runId 的明细。
    #[serde(default)]
    pub runs: BTreeMap<String, RunUsage>,
}

/// 手工 Default：version 恒为当前格式版本（derive 的数字默认值 0 不是
/// 合法版本——空明细也应声明自己属于哪个格式，写入即合法文件）。
impl Default for SessionUsage {
    fn default() -> Self {
        Self {
            version: STATS_FORMAT_VERSION,
            session_total: UsageTotal::default(),
            runs: BTreeMap::new(),
        }
    }
}

/// 落账请求（调用方给足身份面，存取器负责文件合并）。
#[derive(Debug, Clone)]
pub struct UsageRecord<'a> {
    /// 记账日（调用时刻的本地日期，`YYYY-MM-DD`；由调用方传入——
    /// 存取器不做时钟依赖，测试可注入任意日期）。
    pub day: &'a str,
    /// 实际执行模型的配置 id。
    pub model_id: &'a str,
    /// 调用当时的模型显示名快照。
    pub model_name: &'a str,
    /// 本次调用的贡献增量（累计 usage 帧须由调用方差分为单次贡献，
    /// 不能把累计值重复相加）。
    pub delta: UsageTotal,
    /// 是否为主 run 首次落账（true = 当日该模型 runs +1；子 run/辅助调用
    /// 与后续帧传 false）。
    pub counts_run: bool,
}

/// 聚合文件的单文件读结果：内容 + 该文件是否损坏（损坏已备份并按空重建，
/// 调用方仅用于日志面）。
fn read_daily(path: &Path, day: &str) -> (DailyStats, bool) {
    match fs::read_to_string(path) {
        Ok(raw) => match serde_json::from_str::<DailyStats>(&raw) {
            Ok(stats) => (stats, false),
            Err(e) => {
                // 损坏：备份原文件（数据不丢，可人工恢复）后按空重建
                #[cfg(debug_assertions)]
                eprintln!("[craftwave] 日统计损坏，备份重建（{path:?}）：{e}");
                let backup = path.with_extension("json.corrupt");
                let _ = fs::rename(path, &backup);
                (
                    DailyStats {
                        version: STATS_FORMAT_VERSION,
                        day: day.to_owned(),
                        models: BTreeMap::new(),
                    },
                    true,
                )
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (
            DailyStats {
                version: STATS_FORMAT_VERSION,
                day: day.to_owned(),
                models: BTreeMap::new(),
            },
            false,
        ),
        Err(e) => {
            #[cfg(debug_assertions)]
            eprintln!("[craftwave] 日统计读取失败（{path:?}）：{e}");
            (
                DailyStats {
                    version: STATS_FORMAT_VERSION,
                    day: day.to_owned(),
                    models: BTreeMap::new(),
                },
                true,
            )
        }
    }
}

/// 聚合文件路径：`<root>/stats/<day>.json`。
fn daily_path(root: &Path, day: &str) -> PathBuf {
    root.join(STATS_DIR).join(format!("{day}.json"))
}

/// 日/模型聚合存取器（进程内 Mutex 串行化；不持有时钟——记账日由调用方
/// 按 传入）。
pub struct StatsStore {
    root: PathBuf,
    lock: Mutex<()>,
}

impl StatsStore {
    /// 数据根 → 统计存取器（目录在首次落账时惰性创建）。
    pub fn new(data_root: &Path) -> Self {
        Self {
            root: data_root.to_path_buf(),
            lock: Mutex::new(()),
        }
    }

    /// 数据根（测试与清理面用）。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 落一笔用量增量进当日聚合（原子重写）。
    /// 返回合并后的当日该模型条目（失败返回 IO 错误，调用方仅记日志）。
    pub fn record(&self, record: &UsageRecord) -> std::io::Result<ModelDayStats> {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let path = daily_path(&self.root, record.day);
        let (mut stats, _corrupt) = read_daily(&path, record.day);
        let updated = {
            let entry = stats.models.entry(record.model_id.to_owned()).or_default();
            entry.model_name = record.model_name.to_owned();
            entry.input += record.delta.input;
            entry.output += record.delta.output;
            entry.cache_read += record.delta.cache_read;
            entry.cache_write += record.delta.cache_write;
            if record.counts_run {
                entry.runs = entry.runs.saturating_add(1);
            }
            entry.clone()
        };
        let value = serde_json::to_value(&stats).map_err(std::io::Error::other)?;
        write_json(&path, &value)?;
        Ok(updated)
    }

    /// 读单日聚合（文件缺失 → 空；损坏 → 空 + 备份语义同 `record` 读取面）。
    pub fn read_day(&self, day: &str) -> DailyStats {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        read_daily(&daily_path(&self.root, day), day).0
    }

    /// 列出保留期内全部统计日（文件名升序；非 `YYYY-MM-DD.json` 命名的
    /// 文件忽略——含 .corrupt 备份）。
    pub fn list_days(&self) -> Vec<String> {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        list_days_unlocked(&self.root)
    }

    /// 90 天保留的惰性清理（落账时顺带执行，无后台任务）：文件名
    /// 升序遍历，早于 `today - retention_days` 的统计日文件整文件删除
    /// （今天 = `today` 参数，`YYYY-MM-DD`）。返回被删除的日期清单。
    pub fn prune_expired(&self, today: &str, retention_days: u32) -> Vec<String> {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(cutoff) = shift_day(today, -(i64::from(retention_days))) else {
            return Vec::new();
        };
        let mut removed = Vec::new();
        for day in list_days_unlocked(&self.root) {
            if day.as_str() < cutoff.as_str()
                && fs::remove_file(daily_path(&self.root, &day)).is_ok()
            {
                removed.push(day);
            }
        }
        removed
    }

    /// 范围聚合（降级直读面；真相在 stats/*.json）：`from`/`to`
    /// 为闭区间日期（`YYYY-MM-DD`），返回区间内逐日聚合（升序）。
    pub fn read_range(&self, from: &str, to: &str) -> Vec<DailyStats> {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        list_days_unlocked(&self.root)
            .into_iter()
            .filter(|day| day.as_str() >= from && day.as_str() <= to)
            .map(|day| read_daily(&daily_path(&self.root, &day), &day).0)
            .collect()
    }
}

/// 列出统计目录下合法命名的日期文件（`YYYY-MM-DD.json`，升序）。
fn list_days_unlocked(root: &Path) -> Vec<String> {
    let dir = root.join(STATS_DIR);
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut days: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let stem = name.strip_suffix(".json")?;
            is_valid_day(stem).then(|| stem.to_owned())
        })
        .collect();
    days.sort();
    days
}

/// `YYYY-MM-DD` 日期串的合法性（位数 + 分隔位 + 数字位；语义合法性由
/// chrono 解析面在调用方校验，此处只挡路径穿越与垃圾文件名）。
pub fn is_valid_day(day: &str) -> bool {
    let bytes = day.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[8..10].iter().all(u8::is_ascii_digit)
}

/// 日期平移（`YYYY-MM-DD` ± n 天；越界/非法返回 None——调用方按空结果
/// 处理，不 panic）。
pub fn shift_day(day: &str, delta_days: i64) -> Option<String> {
    use chrono::Datelike;
    let date = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?;
    let shifted = date + chrono::Duration::days(delta_days);
    Some(format!(
        "{:04}-{:02}-{:02}",
        shifted.year(),
        shifted.month(),
        shifted.day()
    ))
}

// ---- run/会话四口径明细（sessions/<sid>.usage.json，权威面）----

/// 明细文件路径：`sessions/<sessionId>.usage.json`。
pub fn usage_detail_path(root: &Path, session_id: &str) -> PathBuf {
    root.join("sessions")
        .join(format!("{session_id}.usage.json"))
}

/// 读会话用量明细（文件缺失/损坏 → 空明细兜底——明细面与 prefs 同款
/// 容错口径：绝不能阻塞会话加载；损坏时保留原文件不覆盖，等待下次写覆盖）。
pub fn load_session_usage(root: &Path, session_id: &str) -> SessionUsage {
    match fs::read_to_string(usage_detail_path(root, session_id)) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
        Err(_) => SessionUsage::default(),
    }
}

/// 原子写会话用量明细（调用方持锁面）。
pub fn save_session_usage(
    root: &Path,
    session_id: &str,
    usage: &SessionUsage,
) -> std::io::Result<()> {
    let value = serde_json::to_value(usage).map_err(std::io::Error::other)?;
    write_json(&usage_detail_path(root, session_id), &value)
}

/// 会话明细存取器（读改写合并 + Mutex 串行化；与 StatsStore 拆开——两者
/// 粒度不同：聚合按日、明细按会话）。
pub struct UsageDetailStore {
    root: PathBuf,
    lock: Mutex<()>,
}

/// 明细落账请求（比聚合多会话/run 身份面）。
pub struct RunUsageRecord<'a> {
    pub session_id: &'a str,
    /// run id（主 run id / 子 run id / 辅助调用 id 如 `{runId}:suggest`）。
    pub run_id: &'a str,
    /// run 类别（"main" | "sub" | "aux"）。
    pub kind: &'a str,
    /// 实际执行模型配置 id。
    pub model_id: &'a str,
    /// 调用当时的模型显示名快照。
    pub model_name: &'a str,
    /// 本次调用的贡献增量。
    pub delta: UsageTotal,
}

impl UsageDetailStore {
    /// 数据根 → 明细存取器。
    pub fn new(data_root: &Path) -> Self {
        Self {
            root: data_root.to_path_buf(),
            lock: Mutex::new(()),
        }
    }

    /// 数据根。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 落一笔 run 用量增量进会话明细（合并 byModel 桶 + 推进 sessionTotal；
    /// sessionTotal 恒 = 重算全量，杜绝明细与汇总漂移）。返回合并后的
    /// sessionTotal（会话口径：主/子/辅助总和）。
    pub fn record(&self, record: &RunUsageRecord) -> std::io::Result<UsageTotal> {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut usage = load_session_usage(&self.root, record.session_id);
        let run = usage.runs.entry(record.run_id.to_owned()).or_default();
        if run.kind.is_empty() {
            run.kind = record.kind.to_owned();
            run.started_at = now_ms();
        }
        let bucket = run.by_model.entry(record.model_id.to_owned()).or_default();
        bucket.model_name = record.model_name.to_owned();
        bucket.input += record.delta.input;
        bucket.output += record.delta.output;
        bucket.cache_read += record.delta.cache_read;
        bucket.cache_write += record.delta.cache_write;
        // 汇总恒重算：会话累计 = 全部 runs 之和（主/子/辅助）
        let mut total = UsageTotal::default();
        for run in usage.runs.values() {
            for bucket in run.by_model.values() {
                total.accumulate(&UsageTotal {
                    input: bucket.input,
                    output: bucket.output,
                    cache_read: bucket.cache_read,
                    cache_write: bucket.cache_write,
                });
            }
        }
        usage.session_total = total;
        save_session_usage(&self.root, record.session_id, &usage)?;
        Ok(total)
    }

    /// 读会话累计与明细（恢复读面，数据源）。
    pub fn load(&self, session_id: &str) -> SessionUsage {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        load_session_usage(&self.root, session_id)
    }

    /// 会话运行收口：补写 endedAt（终态只做收口，不搬移已记日期）。
    pub fn mark_run_ended(&self, session_id: &str, run_id: &str) -> std::io::Result<()> {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut usage = load_session_usage(&self.root, session_id);
        if let Some(run) = usage.runs.get_mut(run_id)
            && run.ended_at.is_none()
        {
            run.ended_at = Some(now_ms());
            save_session_usage(&self.root, session_id, &usage)?;
        }
        Ok(())
    }

    /// 删除会话明细（delete_session 联动；文件不存在静默成功）。
    pub fn forget_session(&self, session_id: &str) {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = fs::remove_file(usage_detail_path(&self.root, session_id));
    }
}

/// 当前时间的 Unix 毫秒时间戳（取不到兜底 0；仅明细展示字段）。
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "stats_test.rs"]
mod stats_test;
