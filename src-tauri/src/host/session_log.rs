//! 会话级日志写入通道（32 号票，FR-15-01/FR-14-04/FR-14-07，§7.1 日志布局）。
//!
//! 轻量自写 logger（票面定案：**不引入 tracing 全家桶**——per-session 分
//! 文件 + JSON 行需要自定义 subscriber layer，成本高于收益；取舍记录于
//! 票内 Answer）。三个面：
//!
//! 1. **应用日志** `<logs>/app.log`：运行元数据（启动序/命令报错/panic
//!    捕获/落盘失败记账）。红线：不记录会话正文或图片（Q43——应用日志
//!    在会话删除后仍存活，不能成为内容副本）；>5MB 轮转 `.log.1`
//!    （保留 1 份，再满覆盖）。
//! 2. **会话日志** `<logs>/sessions/<sessionId>.jsonl`：每会话一文件，
//!    每行 `{ ts, level, kind, msg, data?, runId?, callId?, subId? }`，
//!    `kind ∈ run|llm|tool|approval|compact|subagent`；>10MB 轮转
//!    `.jsonl.1`（保留 1 份）。会话删除时关闭句柄并删除本会话日志与
//!    轮转副本（Q43：删会话同步删专属日志）。
//! 3. **级别过滤**：写前按 `config.log.level` 判定（Debug/Info/Warn/
//!    Error，**verbose = level 设为 Debug**）；每次写入都读配置缓存
//!    （`ConfigStore::load` 内存命中），save_config 热替换后下一行即按
//!    新级别过滤——级别修改即时生效，无需重启。
//!
//! 落盘纪律（与 NFR-06 会话数据通道严格区分）：append 打开、**不 fsync**
//! ——日志可丢、性能优先；轮转与写失败都吞掉（尽力而为，不反压业务）。

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Seek, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use chrono::SecondsFormat;
use serde_json::{Value, json};

use crate::providers::types::{AppConfig, LogLevel};
use crate::storage::config_store::ConfigStore;

/// 会话日志单文件轮转阈值（10MB，票面 §7.1 布局）。
const SESSION_ROTATE_BYTES: u64 = 10 * 1024 * 1024;
/// 应用日志单文件轮转阈值（5MB）。
const APP_ROTATE_BYTES: u64 = 5 * 1024 * 1024;

/// 会话日志条目的事件类别（票面 §7.1 布局定值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    /// run 生命周期（起止与终态）。
    Run,
    /// 模型请求/响应（Info 元数据 + Debug 全文，过脱敏闸）。
    Llm,
    /// 工具调用起止与错误。
    Tool,
    /// 审批/询问开关（ask 协议两端口）。
    Approval,
    /// 上下文压缩生命周期。
    Compact,
    /// 子代理生命周期。
    Subagent,
}

impl LogKind {
    /// 行内 kind 字符串（§7.1 布局小写定值）。
    fn as_str(self) -> &'static str {
        match self {
            LogKind::Run => "run",
            LogKind::Llm => "llm",
            LogKind::Tool => "tool",
            LogKind::Approval => "approval",
            LogKind::Compact => "compact",
            LogKind::Subagent => "subagent",
        }
    }
}

/// 单个日志文件的可复用写手：append 句柄 + 已写字节计数（轮转判定）。
struct LogWriter {
    file: File,
    bytes: u64,
}

impl LogWriter {
    /// 以追加模式打开（不存在即创建），记录当前大小。
    fn open(path: &std::path::Path) -> std::io::Result<Self> {
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        let bytes = file.seek(std::io::SeekFrom::End(0))?;
        Ok(Self { file, bytes })
    }
}

/// 会话级日志器（进程内单例，`AppState.session_log` 持有）。
pub struct SessionLogger {
    /// 日志根目录（§7.1：`~/.craftwave/logs/`）。
    root: PathBuf,
    /// 配置缓存句柄（级别热生效的数据源：每次写入时 load 内存缓存）。
    config: Arc<ConfigStore<AppConfig>>,
    /// 会话写手表（sessionId → 句柄）。会话删除时摘除并关句柄。
    writers: Mutex<HashMap<String, LogWriter>>,
    /// 已清除会话名单（close_and_purge 登记）：名单内会话的写入直接
    /// 丢弃——防删除竞态窗口的迟到写入重建文件（Q43 删除红线）。
    purged: Mutex<std::collections::HashSet<String>>,
    /// 应用日志写手（单文件，惰性打开）。
    app: Mutex<Option<LogWriter>>,
}

impl SessionLogger {
    /// 构造：给定日志根目录与配置缓存句柄。目录惰性创建（首次写入时）。
    pub fn new(root: PathBuf, config: Arc<ConfigStore<AppConfig>>) -> Self {
        Self {
            root,
            config,
            writers: Mutex::new(HashMap::new()),
            purged: Mutex::new(std::collections::HashSet::new()),
            app: Mutex::new(None),
        }
    }

    /// 日志根目录（诊断/命令面用：`open_logs_dir` 与重绑配置时取路径）。
    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// 当前生效级别（`config.log.level`，缺省 Info；缓存读、零 IO——
    /// save_config 热替换后此处立即读到新值）。
    fn level(&self) -> LogLevel {
        self.config
            .load()
            .map(|c| c.log.level)
            .unwrap_or(LogLevel::Info)
    }

    /// 级别序数（Debug=0 < Info=1 < Warn=2 < Error=3；高于等于阈值才写）。
    fn rank(level: LogLevel) -> u8 {
        match level {
            LogLevel::Debug => 0,
            LogLevel::Info => 1,
            LogLevel::Warn => 2,
            LogLevel::Error => 3,
        }
    }

    /// 会话日志是否处于 verbose（Debug 级）——llm 请求/响应全文只有
    /// verbose 才落（调用方据此跳过序列化，省大载荷成本）。
    pub fn verbose(&self) -> bool {
        self.level() == LogLevel::Debug
    }

    /// 追加一行会话日志（级别过滤 → 组行 → 轮转判定 → 写入）。
    ///
    /// `data` 为已脱敏载荷（调用方保证过 [`redact_json`](crate::util::redact
    /// ::redact_json) / [`summarize_images`](crate::util::redact
    /// ::summarize_images) 闸——本模块不做二次脱敏，入口单一在调用方）。
    #[allow(clippy::too_many_arguments)]
    pub fn session_line(
        &self,
        session_id: &str,
        level: LogLevel,
        kind: LogKind,
        msg: String,
        data: Option<Value>,
        run_id: Option<&str>,
        call_id: Option<&str>,
        sub_id: Option<&str>,
    ) {
        if Self::rank(level) < Self::rank(self.level()) {
            return; // 级别过滤（热生效：每次写入读最新配置）
        }
        let mut line = json!({
            "ts": now_rfc3339(),
            "level": level_name(level),
            "kind": kind.as_str(),
            "msg": msg,
        });
        let object = line.as_object_mut().expect("行结构恒为对象");
        if let Some(data) = data {
            object.insert("data".to_owned(), data);
        }
        if let Some(run_id) = run_id {
            object.insert("runId".to_owned(), json!(run_id));
        }
        if let Some(call_id) = call_id {
            object.insert("callId".to_owned(), json!(call_id));
        }
        if let Some(sub_id) = sub_id {
            object.insert("subId".to_owned(), json!(sub_id));
        }
        let Ok(mut body) = serde_json::to_string(&line) else {
            return; // 序列化失败（不可达防御面）：吞掉，日志可丢
        };
        body.push('\n');
        self.write_session(session_id, body.as_bytes(), SESSION_ROTATE_BYTES);
    }

    /// 追加一行应用日志（`app.log`：运行元数据）。红线：调用方不得把
    /// 会话正文或图片放进 `msg`/`data`（应用日志不随会话删除，不能成为
    /// 内容副本——Q43）。
    pub fn app_line(&self, level: LogLevel, msg: String, data: Option<Value>) {
        if Self::rank(level) < Self::rank(self.level()) {
            return;
        }
        let mut line = json!({
            "ts": now_rfc3339(),
            "level": level_name(level),
            "msg": msg,
        });
        if let Some(data) = data {
            line.as_object_mut()
                .expect("行结构恒为对象")
                .insert("data".to_owned(), data);
        }
        let Ok(mut body) = serde_json::to_string(&line) else {
            return;
        };
        body.push('\n');
        self.write_app(body.as_bytes());
    }

    /// 会话删除联动（Q43）：关闭句柄并删除本会话日志及轮转副本
    /// （`<sid>.jsonl` + `<sid>.jsonl.1`）。**删除失败必须报告**——返回
    /// 失败清单（非空 = 有残留，调用方不得声称全部删除成功）。清除后该
    /// 会话进入已清除名单：迟到的写入（删除与在飞任务收尾的竞态窗口）
    /// 直接丢弃——删除后的会话不能在磁盘上重建日志副本（Q43 红线）。
    pub fn close_and_purge(&self, session_id: &str) -> Vec<String> {
        // 先摘写手（关句柄：后续写不再复活文件）
        let _ = self
            .writers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(session_id);
        self.purged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(session_id.to_owned());
        let mut failures = Vec::new();
        for suffix in [".jsonl", ".jsonl.1"] {
            let path = self.sessions_dir().join(format!("{session_id}{suffix}"));
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                // 不存在 = 本来就干净（verbose 从未开过等），不算失败
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => failures.push(format!("{}: {e}", path.display())),
            }
        }
        failures
    }

    /// 读会话日志尾部（§3.3 #54）：文件不存在 → 空清单不报错；`tail`
    /// 为返回的最大行数（调用方已按契约 clamp）。坏行（不完整 JSON 行）
    /// 原样输出（容错：日志是诊断面，不给读制造障碍）。
    pub fn read_tail(&self, session_id: &str, tail: usize) -> Vec<String> {
        let path = self.sessions_dir().join(format!("{session_id}.jsonl"));
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return Vec::new();
        };
        let lines: Vec<&str> = raw.lines().collect();
        let start = lines.len().saturating_sub(tail);
        lines[start..]
            .iter()
            .map(|l| (*l).to_owned())
            .collect::<Vec<_>>()
    }

    /// 列出 logs/ 下全部日志文件相对名（§3.3 #55：`app.log*` +
    /// `sessions/*`；spill 子目录不在此列——票面布局明确两项）。
    pub fn list_files(&self) -> Vec<String> {
        let mut out = Vec::new();
        let entries = match std::fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(_) => return out, // 目录未创建（从未写过）→ 空清单
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("app.log") {
                out.push(name);
            }
        }
        if let Ok(sessions) = std::fs::read_dir(self.sessions_dir()) {
            for entry in sessions.flatten() {
                out.push(format!("sessions/{}", entry.file_name().to_string_lossy()));
            }
        }
        out.sort();
        out
    }

    /// 会话日志目录（`<logs>/sessions/`）。
    fn sessions_dir(&self) -> PathBuf {
        self.root.join("sessions")
    }

    /// 会话日志写入（含轮转：超过阈值先把当前文件改名 `.1`（覆盖旧副本）
    /// 再重开新文件续写）。写失败静默（日志可丢，不反压业务）。
    /// 已清除会话（close_and_purge 登记过）的写入直接丢弃。
    fn write_session(&self, session_id: &str, bytes: &[u8], threshold: u64) {
        if self
            .purged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(session_id)
        {
            return; // 删除后的迟到写入：丢弃，不重建文件（Q43 红线）
        }
        let Ok(mut writers) = self.writers.lock() else {
            return;
        };
        if self.ensure_session_dir().is_err() {
            return; // 目录创建失败：写不进，静默丢弃（不反压）
        }
        let path = self.sessions_dir().join(format!("{session_id}.jsonl"));
        let needs_rotate = writers
            .get(session_id)
            .is_some_and(|w| w.bytes + bytes.len() as u64 > threshold);
        if needs_rotate {
            writers.remove(session_id); // 关句柄再轮转
        }
        if !writers.contains_key(session_id) {
            // 新写手：打开前先探现有文件大小做轮转判定（重启续写 / 预置
            // 已超阈值文件的场景——首写就落进轮转后的新文件，而不是把
            // 超限旧文件带上第一行）
            let existing = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if existing + bytes.len() as u64 > threshold {
                rotate_file(&path);
            }
            match LogWriter::open(&path) {
                Ok(w) => {
                    writers.insert(session_id.to_owned(), w);
                }
                Err(_) => return, // 打开失败（磁盘满等）：静默丢弃
            }
        }
        if let Some(w) = writers.get_mut(session_id) {
            let _ = w.file.write_all(bytes);
            w.bytes += bytes.len() as u64;
        }
    }

    /// 应用日志写入（单文件惰性打开 + 5MB 轮转）。
    fn write_app(&self, bytes: &[u8]) {
        let Ok(mut guard) = self.app.lock() else {
            return;
        };
        let path = self.root.join("app.log");
        let needs_rotate = guard
            .as_ref()
            .is_some_and(|w| w.bytes + bytes.len() as u64 > APP_ROTATE_BYTES);
        if needs_rotate {
            *guard = None; // 关句柄再轮转
        }
        if guard.is_none() {
            if std::fs::create_dir_all(&self.root).is_err() {
                return;
            }
            if needs_rotate {
                rotate_file(&path);
            }
            match LogWriter::open(&path) {
                Ok(w) => *guard = Some(w),
                Err(_) => return,
            }
        }
        if let Some(w) = guard.as_mut() {
            let _ = w.file.write_all(bytes);
            w.bytes += bytes.len() as u64;
        }
    }

    /// 确保会话日志目录存在。
    fn ensure_session_dir(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(self.sessions_dir())
    }
}

/// 轮转原语：当前文件改名 `<原名>.1`（先删旧 `.1`——保留 1 份、再满覆盖）。
fn rotate_file(path: &std::path::Path) {
    let mut rotated = path.as_os_str().to_owned();
    rotated.push(".1");
    let rotated = PathBuf::from(rotated);
    let _ = std::fs::remove_file(&rotated);
    let _ = std::fs::rename(path, &rotated);
}

/// 当前行时间戳（RFC3339 UTC，秒精度——诊断面够用、行短）。
fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// 级别的小写行内名（与 `config.log.level` 字面量一致）。
fn level_name(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Debug => "debug",
        LogLevel::Info => "info",
        LogLevel::Warn => "warn",
        LogLevel::Error => "error",
    }
}

#[cfg(test)]
#[path = "session_log_test.rs"]
mod session_log_test;
