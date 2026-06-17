//! MIME 解析，基于 mail-parser。只抽取 Agent 需要的字段，HTML 仅标记存在性。

use crate::error::{Error, Result};
use crate::model::MessageBody;
use mail_parser::{MessageParser, MimeHeaders};

pub fn parse(uid: u32, raw: &[u8]) -> Result<MessageBody> {
    let msg = MessageParser::default()
        .parse(raw)
        .ok_or(Error::MimeParse)?;

    let subject = msg.subject().unwrap_or_default().to_string();
    let from = msg
        .from()
        .and_then(|addrs| addrs.first())
        .and_then(|a| a.address())
        .unwrap_or_default()
        .to_string();
    let to = msg
        .to()
        .map(|addrs| {
            addrs
                .iter()
                .filter_map(|a| a.address().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let date = msg.date().map(|d| d.to_rfc3339());
    let text = msg.body_text(0).map(|c| c.into_owned());
    let has_html = msg.body_html(0).is_some();
    let attachments = msg
        .attachments()
        .filter_map(|a| a.attachment_name().map(str::to_string))
        .collect();

    Ok(MessageBody {
        uid,
        from,
        to,
        subject,
        date,
        text,
        has_html,
        attachments,
    })
}
