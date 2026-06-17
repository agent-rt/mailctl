//! SMTP 发信层。构造与发送分离：草稿优先策略下，先 `build` 出邮件，
//! 未确认则 APPEND 到草稿箱，确认 (`--confirm`) 才真正 `send`。

use crate::auth;
use crate::config::Account;
use crate::error::Result;
use crate::oauth;
use crate::provider::Provider;
use lettre::message::Message;
use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::{Credentials, Mechanism};
use lettre::{SmtpTransport, Transport};

/// 构造一封纯文本邮件（M4 起扩展 HTML / 附件）。
pub fn build(account: &Account, to: &[String], subject: &str, body: &str) -> Result<Message> {
    let mut builder = Message::builder()
        .from(account.email.parse()?)
        .subject(subject);
    for recipient in to {
        builder = builder.to(recipient.parse()?);
    }
    Ok(builder
        .header(ContentType::TEXT_PLAIN)
        .body(body.to_string())?)
}

/// 经 SMTP submission（587 STARTTLS）发送。Gmail 用 App Password (LOGIN/PLAIN)，
/// Hotmail 用 OAuth2 access_token (XOAUTH2)。
pub fn send(account: &Account, message: &Message) -> Result<()> {
    let builder = SmtpTransport::relay(account.provider.smtp_host())?;
    let mailer = match account.provider {
        Provider::Gmail => {
            let password = auth::load_password(account)?;
            builder
                .credentials(Credentials::new(account.email.clone(), password))
                .build()
        }
        Provider::Hotmail => {
            let access_token = oauth::access_token_for(account)?;
            builder
                .credentials(Credentials::new(account.email.clone(), access_token))
                .authentication(vec![Mechanism::Xoauth2])
                .build()
        }
    };
    mailer.send(message)?;
    Ok(())
}
