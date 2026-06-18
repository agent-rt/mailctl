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

impl Error {
    /// 是否为瞬时错误（网络/连接层），可安全重试。
    /// 认证拒绝（No/Bad）、UIDVALIDITY 不符、本地错误等均为永久，不重试。
    pub fn is_transient(&self) -> bool {
        match self {
            Error::Io(_) | Error::Tls(_) | Error::Http(_) => true,
            Error::Imap(e) => matches!(
                e,
                imap::Error::Io(_)
                    | imap::Error::TlsHandshake(_)
                    | imap::Error::Tls(_)
                    | imap::Error::ConnectionLost
            ),
            _ => false,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::Error;

    #[test]
    fn transience_classification() {
        // 瞬时：网络/连接层
        let io = Error::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "reset",
        ));
        assert!(io.is_transient());
        // 永久：不可重试（重试会误伤）
        assert!(!Error::Config("x".into()).is_transient());
        assert!(!Error::OAuth("invalid_grant".into()).is_transient());
        assert!(
            !Error::UidValidityMismatch {
                expected: 1,
                actual: 2
            }
            .is_transient()
        );
        assert!(!Error::SendNotConfirmed.is_transient());
    }
}
