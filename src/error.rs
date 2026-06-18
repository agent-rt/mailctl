//! 结构化错误。整个 crate 不允许 `unwrap`/`expect`/`panic`，
//! 所有失败路径收敛到此枚举，并以稳定 JSON 形式吐给上层 Agent。

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("配置错误: {0}")]
    Config(String),

    #[error("找不到账户 `{0}`，请先运行 `mailctl auth login`")]
    AccountNotFound(String),

    #[error("未配置默认账户，请用 --account 指定，或 `mailctl auth login`")]
    NoDefaultAccount,

    #[error("不支持的 provider: {0}（支持 gmail / hotmail）")]
    UnknownProvider(String),

    #[error("发送被拦截：草稿已保存，确认无误后请加 --confirm 真正发送")]
    SendNotConfirmed,

    #[error("OAuth 错误: {0}")]
    OAuth(String),

    #[error("HTTP 错误: {0}")]
    Http(String),

    #[error("找不到邮件 uid={0}")]
    MessageNotFound(u32),

    #[error("凭据存取失败: {0}")]
    Keyring(#[from] keyring::Error),

    #[error("IMAP 错误: {0}")]
    Imap(#[from] imap::Error),

    #[error("SMTP 发送错误: {0}")]
    Smtp(#[from] lettre::transport::smtp::Error),

    #[error("邮件构造错误: {0}")]
    MailBuild(#[from] lettre::error::Error),

    #[error("地址解析错误: {0}")]
    Address(#[from] lettre::address::AddressError),

    #[error("TLS 错误: {0}")]
    Tls(#[from] native_tls::Error),

    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),

    #[error("配置序列化错误: {0}")]
    TomlSer(#[from] toml::ser::Error),

    #[error("配置解析错误: {0}")]
    TomlDe(#[from] toml::de::Error),

    #[error("JSON 错误: {0}")]
    Json(#[from] serde_json::Error),

    #[error("邮件解析失败")]
    MimeParse,

    #[error("缓存数据库错误: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error(
        "UIDVALIDITY 不一致：期望 {expected}，实际 {actual}。邮箱可能已重建，UID 已失效，请重新 search 后再操作"
    )]
    UidValidityMismatch { expected: u32, actual: u32 },

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;
