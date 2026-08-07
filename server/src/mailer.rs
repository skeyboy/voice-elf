use anyhow::{Context, Result};
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Mailbox, MultiPart},
    transport::smtp::{
        authentication::Credentials,
        client::{Tls, TlsParameters},
    },
};
use serde::Serialize;

use crate::config::{MailConfig, SmtpSecurity};

#[derive(Clone)]
pub struct MailService {
    config: MailConfig,
    transport: Option<AsyncSmtpTransport<Tokio1Executor>>,
    from: Mailbox,
}

#[derive(Serialize)]
pub struct MailStatus {
    pub configured: bool,
    pub host: String,
    pub port: u16,
    pub security: &'static str,
    pub username: String,
    pub from_address: String,
    pub reset_expiry_minutes: u64,
}

impl MailService {
    pub fn new(config: MailConfig) -> Result<Self> {
        let from_address = config
            .from_address
            .parse()
            .context("VOICE_ELF_SMTP_FROM_ADDRESS is invalid")?;
        let from = Mailbox::new(Some(config.from_name.clone()), from_address);
        let transport = if config.configured() {
            let tls_parameters =
                TlsParameters::new(config.host.clone()).context("failed to configure SMTP TLS")?;
            let mut builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&config.host)
                .port(config.port)
                .credentials(Credentials::new(
                    config.username.clone(),
                    config.password.clone().unwrap_or_default(),
                ));
            builder = match config.security {
                SmtpSecurity::Wrapper => builder.tls(Tls::Wrapper(tls_parameters)),
                SmtpSecurity::StartTls => builder.tls(Tls::Required(tls_parameters)),
                SmtpSecurity::None => builder.tls(Tls::None),
            };
            Some(builder.build())
        } else {
            None
        };
        Ok(Self {
            config,
            transport,
            from,
        })
    }

    pub fn configured(&self) -> bool {
        self.transport.is_some()
    }

    pub fn reset_expiry(&self) -> std::time::Duration {
        self.config.reset_expiry
    }

    pub fn public_url(&self) -> Option<&str> {
        self.config.public_url.as_deref()
    }

    pub fn status(&self) -> MailStatus {
        MailStatus {
            configured: self.configured(),
            host: self.config.host.clone(),
            port: self.config.port,
            security: match self.config.security {
                SmtpSecurity::Wrapper => "wrapper",
                SmtpSecurity::StartTls => "starttls",
                SmtpSecurity::None => "none",
            },
            username: self.config.username.clone(),
            from_address: self.config.from_address.clone(),
            reset_expiry_minutes: self.config.reset_expiry.as_secs() / 60,
        }
    }

    pub async fn send_password_reset(
        &self,
        recipient: &str,
        username: &str,
        system_name: &str,
        reset_url: &str,
    ) -> Result<()> {
        let transport = self
            .transport
            .as_ref()
            .context("SMTP password is not configured")?;
        let recipient = recipient.parse().context("user email address is invalid")?;
        let expiry_minutes = self.config.reset_expiry.as_secs() / 60;
        let plain = format!(
            "{username}，你好：\n\n有人申请重置你在 {system_name} 的登录密码。请在 {expiry_minutes} 分钟内打开以下链接：\n\n{reset_url}\n\n如果不是你本人操作，请忽略此邮件。该链接只能使用一次。"
        );
        let html = format!(
            "<div style=\"font-family:system-ui,sans-serif;color:#17202a;line-height:1.7;max-width:560px\"><h2 style=\"margin:0 0 16px\">重置登录密码</h2><p>{username}，你好：</p><p>有人申请重置你在 <strong>{system_name}</strong> 的登录密码。该链接将在 {expiry_minutes} 分钟后失效，且只能使用一次。</p><p style=\"margin:24px 0\"><a href=\"{reset_url}\" style=\"display:inline-block;background:#176b5b;color:#fff;text-decoration:none;padding:10px 18px;border-radius:6px\">设置新密码</a></p><p style=\"font-size:13px;color:#667085;word-break:break-all\">无法点击按钮时，请打开：{reset_url}</p><p style=\"font-size:13px;color:#667085\">如果不是你本人操作，请忽略此邮件。</p></div>",
            username = escape_html(username),
            system_name = escape_html(system_name),
            reset_url = escape_html(reset_url),
        );
        let message = Message::builder()
            .from(self.from.clone())
            .to(Mailbox::new(None, recipient))
            .subject(format!("重置 {system_name} 登录密码"))
            .multipart(MultiPart::alternative_plain_html(plain, html))
            .context("failed to build password reset email")?;
        transport
            .send(message)
            .await
            .context("failed to send password reset email")?;
        Ok(())
    }
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::escape_html;

    #[test]
    fn escapes_untrusted_email_content() {
        assert_eq!(escape_html("<a & 'b'>"), "&lt;a &amp; &#39;b&#39;&gt;");
    }
}
