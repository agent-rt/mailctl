//! clap 命令树。所有命令默认输出 JSON，面向 Agent 编排。

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "mailctl",
    version,
    about = "Agent 友好的邮件 CLI（Gmail / Hotmail）"
)]
pub struct Cli {
    /// 目标账户邮箱；省略则用默认账户。
    #[arg(long, global = true)]
    pub account: Option<String>,

    /// 操作的文件夹。
    #[arg(long, global = true, default_value = "INBOX")]
    pub folder: String,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// 账户认证与管理。
    Auth {
        #[command(subcommand)]
        action: AuthAction,
    },

    /// 搜索邮件，仅返回元数据（省 token）。
    ///
    /// 支持简易查询语法：`is:unread` `from:x` `to:x` `subject:x`，其余词作全文匹配。
    Search {
        /// 查询串；省略则列全部。
        query: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// 校验文件夹 UIDVALIDITY；不符则中止（防 UID 失效）。
        #[arg(long)]
        expect_uidvalidity: Option<u32>,
        /// 读本地缓存（零网络、需先 sync；flag 可能陈旧）。默认走 IMAP 实时。
        #[arg(long)]
        cached: bool,
    },

    /// 读取单封邮件正文。
    Read { uid: u32 },

    /// 修改邮件标志。
    Flag {
        uid: u32,
        /// 标记为已读。
        #[arg(long)]
        read: bool,
        /// 加星标。
        #[arg(long)]
        star: bool,
    },

    /// 移到回收站（可恢复）。支持一次多封；不带 --confirm 时仅预览将删清单。
    Trash {
        #[arg(required = true)]
        uids: Vec<u32>,
        /// 显式确认才真正移动；否则只返回预览（含真实主题）。
        #[arg(long)]
        confirm: bool,
        /// 校验文件夹 UIDVALIDITY；不符则中止。建议传 search/预览返回的值。
        #[arg(long)]
        expect_uidvalidity: Option<u32>,
    },

    /// 从回收站恢复邮件到收件箱（误删找回）。注意：uid 是回收站里的 uid，
    /// 需先 `search --folder <回收站>` 查出。
    Restore {
        #[arg(required = true)]
        uids: Vec<u32>,
        /// 恢复目标文件夹。
        #[arg(long, default_value = "INBOX")]
        to: String,
        /// 校验回收站 UIDVALIDITY；不符则中止。
        #[arg(long)]
        expect_uidvalidity: Option<u32>,
    },

    /// 列出文件夹/标签。
    Folders,

    /// 本地正文缓存管理。
    Cache {
        #[command(subcommand)]
        action: CacheAction,
    },

    /// 把当前账户指定文件夹的元数据增量同步进本地缓存（供 search --cached）。
    Sync,

    /// 移动邮件到其他文件夹（重组，可逆——移回即恢复）。
    Move {
        #[arg(required = true)]
        uids: Vec<u32>,
        /// 目标文件夹。
        #[arg(long)]
        to: String,
        /// 目标不存在则先创建。
        #[arg(long)]
        create: bool,
        /// 校验源文件夹 UIDVALIDITY；不符则中止。
        #[arg(long)]
        expect_uidvalidity: Option<u32>,
    },

    /// 增删 Gmail 标签（仅 Gmail；与 move 不同，不会移出当前位置）。
    /// Hotmail 无标签概念，请改用 move。
    Label {
        #[arg(required = true)]
        uids: Vec<u32>,
        /// 要添加的标签（可多次）。
        #[arg(long = "add")]
        add: Vec<String>,
        /// 要移除的标签（可多次）。
        #[arg(long = "remove")]
        remove: Vec<String>,
        /// 校验文件夹 UIDVALIDITY；不符则中止。
        #[arg(long)]
        expect_uidvalidity: Option<u32>,
    },

    /// 发信。默认仅存草稿；需 --confirm 才真正发送。
    Send {
        #[arg(long, required = true)]
        to: Vec<String>,
        #[arg(long)]
        subject: String,
        /// 正文文件路径（与 --body 二选一）。
        #[arg(long)]
        body_file: Option<PathBuf>,
        /// 正文内容（与 --body-file 二选一）。
        #[arg(long)]
        body: Option<String>,
        /// 显式确认真正发送；否则只存草稿。
        #[arg(long)]
        confirm: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum CacheAction {
    /// 显示缓存统计（条数 / 字节 / 路径）。
    Info,
    /// 清空正文缓存。
    Clear,
}

#[derive(Subcommand, Debug)]
pub enum AuthAction {
    /// 登录并把凭据写入钥匙串。
    /// Gmail：App Password 经 --password 或交互输入；Hotmail：浏览器 OAuth，需 --client-id。
    Login {
        #[arg(long)]
        provider: String,
        #[arg(long)]
        email: String,
        /// Gmail App Password（非交互；省略则提示输入）。
        #[arg(long)]
        password: Option<String>,
        /// OAuth client id。Hotmail：Azure public client；Gmail：Google Cloud Desktop 客户端。
        /// 提供它即走 OAuth（Gmail 不提供则走 App Password）。
        #[arg(long)]
        client_id: Option<String>,
        /// OAuth client secret。仅 Gmail OAuth 需要（Google Desktop 客户端）。
        #[arg(long)]
        client_secret: Option<String>,
        /// secretctl 密钥名。设置后主密钥走 secretctl/env，不写 keychain。
        /// Gmail：免 --password；运行时用 `secretctl exec --only <ref> -- mailctl ...`。
        #[arg(long)]
        secret_ref: Option<String>,
    },
    /// 列出已配置账户。
    List,
    /// 注销并清除钥匙串凭据。
    Logout { email: String },
}
