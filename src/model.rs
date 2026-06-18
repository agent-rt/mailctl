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

/// `search --all-accounts` 的单账户分组结果。一个账户失败（error）不影响其他账户。
#[derive(Debug, Clone, Serialize)]
pub struct AccountSearch {
    pub account: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uidvalidity: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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

// ---- TSV 输出（search 默认，省 token）----
// 约定：`#` 开头行为元数据（key=value，tab 分隔）；随后一行列头；其余为数据行。
// 字段内的 tab/换行被替换为空格，保证每封一行。

fn tsv_clean(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

fn meta_row(m: &MessageMeta) -> String {
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}",
        m.uid,
        tsv_clean(&m.from),
        tsv_clean(&m.subject),
        tsv_clean(m.date.as_deref().unwrap_or("")),
        m.unread,
        m.size.map(|s| s.to_string()).unwrap_or_default(),
        m.is_bulk,
    )
}

const META_HEADER: &str = "uid\tfrom\tsubject\tdate\tunread\tsize\tis_bulk";

/// 单文件夹 search 的 TSV。`meta` 为 `#` 注释行的 key=value 对（如 folder/uidvalidity）。
pub fn print_tsv(meta: &[(&str, String)], messages: &[MessageMeta]) -> crate::error::Result<()> {
    if !meta.is_empty() {
        let kv: Vec<String> = meta
            .iter()
            .map(|(k, v)| format!("{k}={}", tsv_clean(v)))
            .collect();
        println!("# {}", kv.join("\t"));
    }
    println!("{META_HEADER}");
    for m in messages {
        println!("{}", meta_row(m));
    }
    Ok(())
}

/// `--all-accounts` 的 TSV：扁平化，首列 `account`；每账户的 uidvalidity/error 走 `#` 注释行。
pub fn print_tsv_accounts(folder: &str, accounts: &[AccountSearch]) -> crate::error::Result<()> {
    println!("# folder={}", tsv_clean(folder));
    for a in accounts {
        if let Some(e) = &a.error {
            println!(
                "# account={}\terror={}",
                tsv_clean(&a.account),
                tsv_clean(e)
            );
        } else if let Some(uv) = a.uidvalidity {
            println!("# account={}\tuidvalidity={uv}", tsv_clean(&a.account));
        }
    }
    println!("account\t{META_HEADER}");
    for a in accounts {
        for m in &a.messages {
            println!("{}\t{}", tsv_clean(&a.account), meta_row(m));
        }
    }
    Ok(())
}
