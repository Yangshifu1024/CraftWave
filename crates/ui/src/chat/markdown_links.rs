//! 会话正文的链接分流（gpui-rewrite 阶段 2.C-3，rendering.md「Markdown 正文」
//! 细节二：Wake `markdown_links.rs` 判序照搬）。
//!
//! Markdown 正文里的链接有五类归宿，判定是一条**严格顺序**（顺序错就会把
//! 文件路径当 URL 交给浏览器，或反过来把 URL 拿去探测本地文件系统）：
//!
//! 1. 空串 / `#` 锚点 → 忽略（页内跳转，无动作）；
//! 2. **Windows 盘符先于 scheme 识别**：`C:/...` 长得像 URL scheme
//!    （`C` 是合法 scheme 名），但它是路径——不判盘符就会交给 URL 打开器；
//! 3. **裸文件名 `main.rs:11` 不当 scheme**：`main.rs` 含点、冒号后是行号，
//!    满足 URL scheme 语法，但要按文件语义走（在 scheme 识别阶段就排除，
//!    不探测文件系统、不受远程守卫影响）；
//! 4. 其余带 scheme 且非 `file:` → 外部浏览器（https / mailto / vscode://
//!    / 自定义 app scheme 一律 `cx.open_url`，gpui 走系统 opener）；
//! 5. **远程会话一律 Remote**：rsync 只镜像会话数据，远程正文的路径不能
//!    展开本机 HOME，也不能探测本机文件系统（连 HOME 都不许碰）；
//! 6. `file:` URI 严格解析：必须绝对路径、不许反斜杠、不许远程 host、
//!    不许 query/fragment——宽松 URL 解析会把 `file:relative` 猜成本机路径；
//! 7. 裸路径（无 scheme）先**按字面查**（保留 `% # ?`——真实文件名可能含
//!    这些字符），失败且含 `%` 才**单次**百分号解码重试（只解一次，不把
//!    失败路径回退成 open_url，也不猜缺失的 file URI authority）；
//! 8. `:N` 行号后缀：完整路径不存在时去掉行号再找（先于百分号解码判定，
//!    避免把文件名里的 `%3A` 误当定位符）。
//!
//! 打开动作：外部浏览器 / 文件管理器走系统 opener（[`open_in_file_manager`]
//! 窄接缝 + gpui `open_url`）；**应用内编辑器跳转**是阶段 3 面板族的接线
//! （engine `api::misc::open_in_editor` 的接口在那批接），本批 `File` 目标
//! 先用文件管理器承载。`resolve_link` 本身是纯判定函数（可注入 home 探测，
//! 测试不依赖真实环境）。

use std::path::{Path, PathBuf};
use std::process::Command;

use gpui_kit::component::notification::Notification;
use gpui_kit::component::text::TextView;
use gpui_kit::component::WindowExt as _;
use gpui_kit::{ClickEvent, MouseButton};
use percent_encoding::percent_decode_str;
use url::Url;

use crate::i18n::t;

/// 链接点击接线：给正文 `TextView` 挂上分流后的打开动作。
///
/// `project_path` 是会话归属的项目根（相对路径按它展开）。当前壳只有本机
/// 会话（`remote` 固定 false）；远程会话形态随阶段 3.3 会话树接入，届时
/// 按会话主机传参。
pub(crate) fn with_session_links(view: TextView, project_path: &str) -> TextView {
    let project_path = project_path.to_owned();
    view.on_link_click(move |href, event, window, cx| {
        // 组件设置 handler 后会连右键一起转发，须保留默认激活规则
        // （左/中键激活、键盘激活、触摸非长按激活）。
        let activate = match event {
            ClickEvent::Mouse(click) => {
                matches!(click.up.button, MouseButton::Left | MouseButton::Middle)
            }
            ClickEvent::Keyboard(_) => true,
            ClickEvent::Touch(click) => !click.long_press,
        };
        if !activate {
            return;
        }
        // HOME 探测经回调注入：resolve_link 的判定不直接读环境
        match resolve_link(href, false, &project_path, home_dir) {
            LinkTarget::External => cx.open_url(href),
            LinkTarget::File(path) => {
                // 阶段 3 面板族换成应用内编辑器打开（engine
                // open_in_editor 接口）；本批文件管理器承载
                if let Some(path) = path.to_str() {
                    open_in_file_manager(path);
                } else {
                    window.push_notification(
                        Notification::info(t(
                            "The linked path is unavailable on this machine.",
                        )),
                        cx,
                    );
                }
            }
            LinkTarget::Remote => window.push_notification(
                Notification::info(t(
                    "Remote sessions are read-only — files stay on the remote host",
                )),
                cx,
            ),
            LinkTarget::Unavailable => window.push_notification(
                Notification::info(t("The linked path is unavailable on this machine.")),
                cx,
            ),
            LinkTarget::Ignore => {}
        }
    })
}

/// 本机用户目录（Unix `HOME` / Windows `USERPROFILE`；缺一即 None）。
/// 与 text_util 的 home_dir 同口径——两处都要「env 缺失时安静回退」。
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|h| !h.is_empty())
}

/// 链接分流判定结果（`Debug`/`PartialEq` 供测试断言）。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LinkTarget {
    /// 外部浏览器打开（原 href 交给 `cx.open_url`）。
    External,
    /// 本机文件/目录（应用内打开的接线在阶段 3；本批文件管理器承载）。
    File(PathBuf),
    /// 远程会话的路径：只提示，不探测本机。
    Remote,
    /// 路径无效或本机不存在：提示。
    Unavailable,
    /// 无动作（空链接 / 页内锚点）。
    Ignore,
}

/// 链接分流判定（判序见模块注释；`home_dir` 参数可注入——测试传假 HOME，
/// 生产传 [`home_dir`]，判定本体不读环境变量）。
pub(crate) fn resolve_link(
    href: &str,
    remote: bool,
    project_path: &str,
    home_dir: impl FnOnce() -> Option<PathBuf>,
) -> LinkTarget {
    // 1. 空链接与页内锚点：忽略
    if href.is_empty() || href.starts_with('#') {
        return LinkTarget::Ignore;
    }
    // 2. 盘符须先于 scheme 识别，否则 C:/... 会被交给 URL 打开器
    let windows = windows_path(href);
    let scheme = (!windows)
        .then(|| url_scheme(href))
        .flatten()
        .filter(|scheme| {
            // 3. 裸文件名 main.rs:11 也满足 URL scheme 语法：含点的 scheme
            //    且去掉行号后恰好等于 scheme 名的，按文件语义走——先于远程
            //    守卫、不探测本地文件系统
            !(scheme.contains('.') && without_line_number(href) == Some(*scheme))
        });
    // 4. 其余带 scheme 且非 file: 的：外部浏览器
    if scheme.is_some_and(|scheme| !scheme.eq_ignore_ascii_case("file")) {
        return LinkTarget::External;
    }
    // 5. 远程会话：正文路径不展开本机 HOME、不探测本机（连 HOME 回调都不调）
    if remote {
        return LinkTarget::Remote;
    }
    if windows || href.chars().any(char::is_control) {
        return LinkTarget::Unavailable;
    }
    // 6. file: URI 严格解析（file:relative 与反斜杠不在范围）
    if scheme.is_some() {
        let Some(path) = file_uri_path(href) else {
            return LinkTarget::Unavailable;
        };
        if path.exists() {
            return LinkTarget::File(path);
        }
        // 完整 URI 不存在时去掉 :N 行号再找（找到的必须是文件）
        return without_line_number(href)
            .and_then(file_uri_path)
            .filter(|path| path.is_file())
            .map_or(LinkTarget::Unavailable, LinkTarget::File);
    }
    // 7. 裸路径：先按字面查（保留 % # ?），失败且含 % 才单次解码重试
    let home = home_dir();
    if let Some(path) = existing_local_path(href, project_path, home.as_deref()) {
        return LinkTarget::File(path);
    }
    // 8. :N 行号后缀：文件不存在时去掉再找（找到的必须是文件）
    without_line_number(href)
        .and_then(|href| existing_local_path(href, project_path, home.as_deref()))
        .filter(|path| path.is_file())
        .map_or(LinkTarget::Unavailable, LinkTarget::File)
}

/// 字面查找 + 单次百分号解码重试：Markdown href 也可能编码了空格/中文。
/// 只解码一次，不把失败的文件路径回退为 open_url，也不猜缺失的 file URI
/// authority。
fn existing_local_path(href: &str, project_path: &str, home: Option<&Path>) -> Option<PathBuf> {
    // 先按原字面查找，保留真实文件名里的 %, #, ? 等字符
    if let Some(path) = local_path(href, project_path, home).filter(|p| p.exists()) {
        return Some(path);
    }
    if href.contains('%')
        && let Ok(decoded) = percent_decode_str(href).decode_utf8()
    {
        return local_path(&decoded, project_path, home).filter(|p| p.exists());
    }
    None
}

/// `:N` 行号后缀剥离：冒号后全是数字且 > 0 才算行号。先于百分号解码的
/// 判定次序由调用方保证（避免把文件名里的 %3A 误当定位符）；Finder 只
/// 选中文件，不负责跳转到该行。
fn without_line_number(href: &str) -> Option<&str> {
    let (path, line) = href.rsplit_once(':')?;
    (!path.is_empty()
        && line.bytes().all(|byte| byte.is_ascii_digit())
        && line.parse::<usize>().is_ok_and(|line| line > 0))
    .then_some(path)
}

/// URL scheme 识别：冒号前、字母开头、只含字母数字与 `+ - .`（RFC 3986）。
fn url_scheme(href: &str) -> Option<&str> {
    let (scheme, _) = href.split_once(':')?;
    let mut bytes = scheme.bytes();
    (bytes.next()?.is_ascii_alphabetic()
        && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.')))
    .then_some(scheme)
}

/// Windows 路径形态：UNC（`\\server\share`）或盘符（`C:`、`C:/x`、`C:\x`）。
/// `C://x` 不算（双斜杠是 URL 形态），交给 scheme 识别。
fn windows_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    path.starts_with("\\\\")
        || (bytes.len() >= 2
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && (bytes.len() == 2
                || bytes[2] == b'\\'
                || (bytes[2] == b'/' && bytes.get(3) != Some(&b'/'))))
}

/// `file:` URI 严格解析为本机绝对路径：必须 `/` 开头（`file:relative` 不
/// 猜）、不含反斜杠、host 只许 localhost 或无、不许 query/fragment；解析
/// 结果再验一次非 `//` 开头与 Windows 形态。
fn file_uri_path(href: &str) -> Option<PathBuf> {
    if !href.split_once(':')?.1.starts_with('/') || href.contains('\\') {
        return None;
    }
    let url = Url::parse(href).ok()?;
    if url.host_str().is_some_and(|host| host != "localhost")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let path = url.to_file_path().ok()?;
    let text = path.to_str()?;
    if !path.is_absolute() || text.starts_with("//") || windows_path(text.trim_start_matches('/')) {
        return None;
    }
    Some(path)
}

/// 裸路径 → 本机路径：`~` 展开（HOME 注入）、绝对路径直用、其余按项目根
/// 拼（项目根本身必须绝对且存在——**不用 cwd**，会话基准缺席时宁可不解析）。
fn local_path(href: &str, project_path: &str, home: Option<&Path>) -> Option<PathBuf> {
    if href.is_empty()
        || href.starts_with(['#', '?'])
        || href.starts_with("//")
        || windows_path(href)
        || href.chars().any(char::is_control)
    {
        return None;
    }
    let path = if href == "~" {
        home?.to_path_buf()
    } else if let Some(relative) = href.strip_prefix("~/") {
        home?.join(relative.trim_start_matches('/'))
    } else if href.starts_with('~') {
        // ~someone/... 不是本机 HOME 下的东西，不猜
        return None;
    } else if Path::new(href).is_absolute() {
        PathBuf::from(href)
    } else {
        let project = Path::new(project_path);
        if !project.is_absolute() || !project.is_dir() {
            return None;
        }
        project.join(href)
    };
    path.is_absolute().then_some(path)
}

// ======================================================================
// 系统 opener：文件管理器（外部浏览器走 gpui cx.open_url，系统原生）
// ======================================================================

/// 文件管理器打开：目录直接打开，文件 reveal 选中。平台差异窄接缝
/// （macOS `open [-R]` / Linux `xdg-open` / Windows `explorer /select,`）。
///
/// spawn 后交短命线程收尸（Wake 同款）：`status()` 会把调用线程阻塞到
/// 文件管理器退出（冷启上百毫秒），裸 `spawn()` 丢掉 Child 会在 Unix 上
/// 攒僵尸进程；Windows 无僵尸概念但同样及时归还句柄。
fn open_in_file_manager(path: &str) {
    let path = path.to_owned();
    std::thread::spawn(move || {
        let mut cmd = reveal_command(&path);
        if let Ok(child) = cmd.spawn() {
            let _ = std::thread::spawn(move || {
                let mut child = child;
                let _ = child.wait();
            });
        }
    });
}

/// 按平台构造文件管理器命令（三端的参数形制差异都收在这里）。
fn reveal_command(path: &str) -> Command {
    let is_dir = Path::new(path).is_dir();
    #[cfg(target_os = "macos")]
    {
        let mut cmd = Command::new("open");
        if !is_dir {
            // -R = 在 Finder 中 reveal（选中文件）
            cmd.arg("-R");
        }
        cmd.arg(path);
        cmd
    }
    #[cfg(target_os = "linux")]
    {
        // xdg-open 不分目录/文件：目录开管理器，文件交默认应用（够用的
        // 近似；精确 reveal 需要 D-Bus ShowItems，阶段 5 平台打磨再接）
        let _ = is_dir;
        let mut cmd = Command::new("xdg-open");
        cmd.arg(path);
        cmd
    }
    #[cfg(target_os = "windows")]
    {
        // explorer 自解析命令行、不吃 argv 转义：/select 与路径必须整体
        // 一个参数（raw_arg 原样直达）
        use std::os::windows::process::CommandExt as _;
        let mut cmd = Command::new("explorer");
        if is_dir {
            cmd.arg(path);
        } else {
            cmd.raw_arg(format!("/select,\"{path}\""));
        }
        cmd
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = is_dir;
        let mut cmd = Command::new("xdg-open");
        cmd.arg(path);
        cmd
    }
}
