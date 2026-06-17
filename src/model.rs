//! Agent 面向的输出契约。所有命令吐稳定 schema 的 JSON。
//! 设计原则：token-lean —— `MessageMeta`（列表）不含正文，正文按需用 `read` 拉取。

use serde::Serialize;

/// 列表/搜索结果的单条元数据，刻意不含正文以省 token。
#[derive(Debug, Clone, Serialize)]
pub struct MessageMeta {
    pub uid: u32,
    pub from: String,
    pub subject: String,
    pub date: Option<String>,
    pub unread: bool,
    pub size: Option<u32>,
    /// 是否含 `List-Unsubscribe` 头——群发/营销邮件的可靠客观信号。
    pub is_bulk: bool,
}

/// `search` 输出：带 UIDVALIDITY，供 Agent 后续操作传 `--expect-uidvalidity` 做一致性校验。
#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub folder: String,
    pub uidvalidity: u32,
    pub messages: Vec<MessageMeta>,
}

/// `read` 的完整正文输出。
#[derive(Debug, Clone, Serialize)]
pub struct MessageBody {
    pub uid: u32,
    pub from: String,
    pub to: Vec<String>,
    pub subject: String,
    pub date: Option<String>,
    pub text: Option<String>,
    /// 是否含 HTML 正文（不直接吐 HTML，避免 token 爆炸）。
    pub has_html: bool,
    pub attachments: Vec<String>,
}

/// 文件夹/标签信息（`folders` 命令输出）。
#[derive(Debug, Clone, Serialize)]
pub struct FolderInfo {
    pub name: String,
    /// 是否可 SELECT（`\Noselect` 的层级节点为 false）。
    pub selectable: bool,
}

/// 写操作的统一确认输出，便于 Agent 解析执行结果。
#[derive(Debug, Clone, Serialize)]
pub struct ActionResult {
    pub ok: bool,
    pub action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uid: Option<u32>,
    pub detail: String,
}

pub fn print_json<T: Serialize>(value: &T) -> crate::error::Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
