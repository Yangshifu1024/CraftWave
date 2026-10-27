//! append-only 分段 JSONL 读写器（技术设计文档 §7.2 分段与封口、§7.3 落盘纪律、§7.4 原子写）。
//!
//! 术语解释：「append-only（只追加）」指日志只允许在末尾追加新行，永不改写或
//! 删除已有内容；「分段」指把一个会话的全部日志切成多个编号递增的小文件
//! （0001.jsonl、0002.jsonl …），避免单文件无限膨胀；「封口（seal）」指某一段
//! 写满后永久关闭、不再续写，之后的新内容写到下一段；「指纹」是把全部条目行
//! 逐行哈希串联得到的校验值（类似文件的 checksum），改动任何一行都会让指纹
//! 对不上；「segmeta」指段清单文件 `.segmeta.json`，记录每段的统计信息和整条
//! 追加链的指纹。数据落盘位置：每个会话一个目录，段文件与 segmeta 都在该目录内。
//!
//! 纪律（复刻 rig-session/4，D12）：
//! - 条目按追加顺序线性排列，永不改写/截断既有段；封口仅在写完一批后的安全点判定。
//! - 封口条件：条目数 ≥200 或字节数 ≥512KB；封口即原子写 `.segmeta.json`
//!   （段清单 + 每段 head 条目哈希 + 全会话追加链指纹）。
//! - 指纹 = 追加链 h₀=SHA256("")、hₙ=SHA256(hex(hₙ₋₁)||行字节)；恢复时校验
//!   segmeta 覆盖前缀的指纹，不匹配只新增新段、绝不截断既有段。
//! - 损坏尾段容错（§7.3「无半行」）：仅最后一段容忍——无换行收尾的半行与
//!   首个非法 JSON 行起的内容全部跳过；非最后一段损坏属真实数据损伤，直接报错。
//!   为什么这么设计：进程随时可能崩溃，崩溃时最后写入的那一行可能只写了一半
//!   （没有换行符收尾），丢弃它是安全的；但中间某段损坏说明磁盘数据被外部破坏，
//!   必须立刻报错而不是悄悄吞掉。

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::entries::StoredEntry;
use crate::util::atomic::write_atomic;

/// 封口条件：条目数 ≥200（§7.2）。
pub const SEGMENT_MAX_ENTRIES: usize = 200;
/// 封口条件：字节数 ≥512KB（§7.2）。
pub const SEGMENT_MAX_BYTES: usize = 512 * 1024;
/// 段文件扩展名（§7.1：0001.jsonl …）。
pub const SEGMENT_EXT: &str = "jsonl";
/// 段清单文件（§7.1：.segmeta.json）。
pub const SEGMETA_FILE: &str = ".segmeta.json";

/// 段信息（segmeta 的清单元素；§7.1 `{file,count,bytes,headHash}`）：
/// 一段的文件名、条目数、字节数与末行哈希。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentInfo {
    pub file: String,
    pub count: usize,
    pub bytes: usize,
    /// 该段最后一个条目行的 SHA-256（小写 hex）。
    pub head_hash: String,
}

/// 段清单与全会话指纹（§7.1：`{segments:[…], headFingerprint}`）。
/// 「全会话指纹」指把所有条目行串起来算出的链式哈希，用于检测历史行是否被改动。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Segmeta {
    #[serde(default)]
    pub segments: Vec<SegmentInfo>,
    #[serde(default)]
    pub head_fingerprint: String,
}

/// 段状态（内存中跟踪的一段；含仍在写入的最后一段「开放段」）。
#[derive(Debug)]
struct SegmentState {
    name: String,
    count: usize,
    bytes: usize,
    head_hash: String,
}

/// 已打开的会话日志：打开时把磁盘上全部段读成只读快照，之后提供追加原语。
/// 同一时刻一个会话目录只有一个 `SessionLog` 实例持有写权限（由上层锁保证）。
#[derive(Debug)]
pub struct SessionLog {
    dir: PathBuf,
    entries: Vec<StoredEntry>,
    segments: Vec<SegmentState>,
    /// 当前段已封口（或不可续写），下次追加必须开新段。
    sealed: bool,
    /// 读取时跳过了最后一段的损坏尾行（§7.3 崩溃半行）。
    corrupt_tail_skipped: bool,
    /// segmeta 指纹校验结果（无 segmeta 视为通过）。
    fingerprint_matched: bool,
    /// 全会话追加链指纹（读到当前为止，小写 hex）。
    fingerprint: String,
    /// 最新 meta 条目的 revision（追加式「最新赢」；无 meta 行 → None）。
    revision: Option<u64>,
}

/// 条目 → JSONL 行（不含换行符）。serde_json 保持结构体字段声明顺序输出，
/// 因此键序 = 字段声明序 = 落盘字节序（跨语言契约测试依赖这一点比对字节）。
pub fn entry_line(entry: &StoredEntry) -> serde_json::Result<String> {
    serde_json::to_string(entry)
}

/// 追加链指纹：h₀=SHA256("")；hₙ=SHA256(hex(hₙ₋₁) || 行字节)。返回小写 hex。
fn chain_hash(prev_hex: &str, line: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(prev_hex.as_bytes());
    hasher.update(line);
    hex(&hasher.finalize())
}

/// 字节数组 → 小写十六进制字符串（每字节两个字符），用于哈希值的字符串表示。
fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// 列出会话目录下的段文件（按段号升序）；目录不存在 → 空。
fn segment_files(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files: Vec<(u32, PathBuf)> = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(stem) = name.strip_suffix(&format!(".{SEGMENT_EXT}")) else {
            continue;
        };
        if let Ok(n) = stem.parse::<u32>() {
            files.push((n, path));
        }
    }
    files.sort_by_key(|(n, _)| *n);
    Ok(files.into_iter().map(|(_, p)| p).collect())
}

/// 读取段清单文件 `.segmeta.json`；文件不存在或内容不合法（解析失败）都返回 None，
/// 由调用方决定如何处理（open 中会把「存在但解析失败」视为指纹失配）。
fn read_segmeta(dir: &Path) -> Option<Segmeta> {
    let raw = fs::read_to_string(dir.join(SEGMETA_FILE)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// `SessionLog` 的实现：打开/重建日志、查询状态、追加与封口。
impl SessionLog {
    /// 打开（或空建）一个会话日志：扫全部段、重建条目序列并校验指纹。
    pub fn open(dir: PathBuf) -> io::Result<Self> {
        let mut log = Self {
            dir,
            entries: Vec::new(),
            segments: Vec::new(),
            sealed: true, // 无开放段；首次 append 时创建 0001
            corrupt_tail_skipped: false,
            fingerprint_matched: true,
            fingerprint: chain_hash("", b""),
            revision: None,
        };
        let files = if log.dir.exists() {
            segment_files(&log.dir)?
        } else {
            Vec::new() // 新会话：目录尚不存在，视为空日志（首次 append 时建目录）
        };
        if files.is_empty() {
            return Ok(log);
        }
        let segmeta = read_segmeta(&log.dir);
        let sealed_prefix: Option<usize> = segmeta
            .as_ref()
            .map(|m| m.segments.iter().map(|s| s.count).sum());
        let sealed_names: HashSet<String> = segmeta
            .as_ref()
            .map(|m| m.segments.iter().map(|s| s.file.clone()).collect())
            .unwrap_or_default();
        if segmeta.is_none() && log.dir.join(SEGMETA_FILE).exists() {
            // segmeta 存在但不可读/不可解析：视为指纹失配（保守处理，绝不续写既有段）
            log.fingerprint_matched = false;
        }

        let last_file_index = files.len() - 1;
        let mut chain_at_prefix = log.fingerprint.clone();
        for (index, path) in files.iter().enumerate() {
            let is_last_file = index == last_file_index;
            let bytes = fs::read(path)?;
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            let mut state = SegmentState {
                name,
                count: 0,
                bytes: bytes.len(),
                head_hash: String::new(),
            };
            let mut chunks = bytes.split(|b| *b == b'\n').peekable();
            while let Some(line) = chunks.next() {
                let is_file_tail = chunks.peek().is_none();
                if is_file_tail && line.is_empty() {
                    break; // 文件以换行收尾，干净 EOF
                }
                if is_file_tail {
                    // 无换行收尾 = 崩溃半行：内容不可信，跳过（§7.3）
                    log.corrupt_tail_skipped = true;
                    break;
                }
                if line.is_empty() {
                    continue; // 防御：正常写入不产生空行
                }
                match serde_json::from_slice::<StoredEntry>(line) {
                    Ok(entry) => {
                        if let StoredEntry::Meta(meta) = &entry {
                            log.revision = Some(meta.revision);
                        }
                        log.fingerprint = chain_hash(&log.fingerprint, line);
                        state.count += 1;
                        state.head_hash = chain_hash("", line);
                        log.entries.push(entry);
                        if Some(log.entries.len()) == sealed_prefix {
                            chain_at_prefix = log.fingerprint.clone();
                        }
                    }
                    Err(_) if is_last_file => {
                        // 最后一段内首个非法行：按损坏尾段处理，其后的内容一并跳过
                        log.corrupt_tail_skipped = true;
                        break;
                    }
                    Err(err) => {
                        // 非最后一段中部损坏：真实数据损伤，fail fast
                        return Err(io::Error::other(format!(
                            "段 {} 第 {} 条附近损坏（非尾段不容错）: {err}",
                            state.name,
                            state.count + 1
                        )));
                    }
                }
            }
            log.segments.push(state);
        }

        // 指纹校验：segmeta 覆盖前缀（Σ sealed count）的链值必须一致；条目数不足亦视为失配
        if let (Some(expected), Some(prefix)) = (
            segmeta.as_ref().map(|m| m.head_fingerprint.as_str()),
            sealed_prefix,
        ) {
            log.fingerprint_matched = log.fingerprint_matched
                && log.entries.len() >= prefix
                && chain_at_prefix == expected;
        }

        // 最后一段可否续写：未封口且无损坏尾行且指纹通过；否则下次追加开新段（§7.2）
        let last = log.segments.last().expect("files 非空则 segments 非空");
        log.sealed = log.corrupt_tail_skipped
            || !log.fingerprint_matched
            || sealed_names.contains(&last.name);
        Ok(log)
    }

    /// 已重建的全部条目（按追加序）。
    pub fn entries(&self) -> &[StoredEntry] {
        &self.entries
    }

    /// 当前 revision（最新 meta 行；无 → None）。
    pub fn revision(&self) -> Option<u64> {
        self.revision
    }

    /// 是否无任何有效条目（新会话）。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 读取时是否跳过损坏尾行。
    pub fn corrupt_tail_skipped(&self) -> bool {
        self.corrupt_tail_skipped
    }

    /// segmeta 指纹校验是否通过（无 segmeta 视为通过）。
    pub fn fingerprint_matched(&self) -> bool {
        self.fingerprint_matched
    }

    /// 全会话追加链指纹（小写 hex）。
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// 追加一批条目（一个落盘点，批末一次 fsync，§7.3）。
    /// 批内逐行检查封口条件（§7.2），满段即原子写 segmeta 并滚动新段；
    /// 当前段不可续写（封口/损坏尾/指纹失配）时自动开新段。
    pub fn append(&mut self, batch: &[StoredEntry]) -> io::Result<()> {
        if batch.is_empty() {
            return Ok(());
        }
        if self.sealed || self.segments.is_empty() {
            self.open_new_segment()?;
        }
        let mut seg_index = self.segments.len() - 1;
        let mut file = OpenOptions::new()
            .append(true)
            .open(self.segment_path(seg_index))?;
        for entry in batch {
            if self.segments[seg_index].count >= SEGMENT_MAX_ENTRIES
                || self.segments[seg_index].bytes >= SEGMENT_MAX_BYTES
            {
                file.sync_all()?;
                drop(file);
                self.seal()?;
                self.open_new_segment()?;
                seg_index = self.segments.len() - 1;
                file = OpenOptions::new()
                    .append(true)
                    .open(self.segment_path(seg_index))?;
            }
            let line = entry_line(entry)?;
            let seg = &mut self.segments[seg_index];
            seg.count += 1;
            seg.bytes += line.len() + 1;
            seg.head_hash = chain_hash("", line.as_bytes());
            self.fingerprint = chain_hash(&self.fingerprint, line.as_bytes());
            if let StoredEntry::Meta(meta) = entry {
                self.revision = Some(meta.revision);
            }
            self.entries.push(entry.clone());
            file.write_all(line.as_bytes())?;
            file.write_all(b"\n")?;
        }
        file.sync_all()?; // §7.3：落盘点 fsync
        Ok(())
    }

    /// 封口当前段：原子写 `.segmeta.json`（段清单 + head 哈希 + 全会话指纹，§7.2/§7.4）。
    fn seal(&mut self) -> io::Result<()> {
        let meta = Segmeta {
            segments: self
                .segments
                .iter()
                .map(|s| SegmentInfo {
                    file: s.name.clone(),
                    count: s.count,
                    bytes: s.bytes,
                    head_hash: s.head_hash.clone(),
                })
                .collect(),
            head_fingerprint: self.fingerprint.clone(),
        };
        write_atomic(
            &self.dir.join(SEGMETA_FILE),
            &serde_json::to_vec(&meta).map_err(io::Error::other)?,
        )?;
        self.sealed = true;
        Ok(())
    }

    /// 开新段（0001 起，自动建目录）。
    fn open_new_segment(&mut self) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let name = format!("{:04}.{SEGMENT_EXT}", self.segments.len() + 1);
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join(&name))?;
        self.segments.push(SegmentState {
            name,
            count: 0,
            bytes: 0,
            head_hash: String::new(),
        });
        self.sealed = false;
        Ok(())
    }

    /// 取第 index 段的完整磁盘路径。
    fn segment_path(&self, index: usize) -> PathBuf {
        self.dir.join(&self.segments[index].name)
    }
}
