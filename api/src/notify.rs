//! Messages to Slack, Discord, email or any webhook when a deploy, run or backup
//! fails, a service crashes or goes down, or a domain stops resolving. Delivery
//! never blocks or fails the work it reports on.

use crate::{
    error::{bad_request, Error, Result},
    state::SharedState,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
use uuid::Uuid;

const KINDS: [&str; 4] = ["slack", "discord", "webhook", "email"];

#[derive(Serialize)]
pub struct Channel {
    id: Uuid,
    name: String,
    kind: String,
    notify_success: bool,
    created_at: DateTime<Utc>,
    /// Where messages go: a host for webhooks (the URL itself is a secret), the
    /// addresses for email.
    target: String,
}

#[derive(Deserialize)]
pub struct NewChannel {
    name: String,
    kind: String,
    url: String,
    #[serde(default)]
    notify_success: bool,
}

/// Something worth telling people about. `path` is a dashboard page.
pub struct Event {
    /// deploy.failed | deploy.succeeded | service.crashed | service.down |
    /// service.recovered | run.failed | backup.failed | domain.unreachable |
    /// domain.reachable
    pub kind: &'static str,
    pub text: String,
    pub path: String,
}

impl Event {
    fn success(&self) -> bool {
        self.kind.ends_with(".succeeded")
    }
}

pub async fn list(state: &SharedState) -> Result<Vec<Channel>> {
    let rows = sqlx::query!(
        "SELECT id, name, kind, url_enc, notify_success, created_at
         FROM notification_channels ORDER BY name"
    )
    .fetch_all(&state.db)
    .await?;
    rows.into_iter()
        .map(|r| {
            let url = state.cipher.decrypt(&r.url_enc)?;
            let target = if r.kind == "email" {
                url
            } else {
                url.split("://")
                    .nth(1)
                    .and_then(|rest| rest.split('/').next())
                    .unwrap_or_default()
                    .to_string()
            };
            Ok(Channel {
                id: r.id,
                name: r.name,
                kind: r.kind,
                notify_success: r.notify_success,
                created_at: r.created_at,
                target,
            })
        })
        .collect()
}

/// Sends a test message first, so a channel that can't be reached is never saved.
pub async fn create(state: &SharedState, new: NewChannel) -> Result<Uuid> {
    let name = new.name.trim();
    let url = new.url.trim();
    if name.is_empty() {
        return Err(bad_request("give the channel a name"));
    }
    if !KINDS.contains(&new.kind.as_str()) {
        return Err(bad_request("kind must be slack, discord, webhook or email"));
    }
    let url = if new.kind == "email" {
        if smtp_settings(state).await?.is_none() {
            return Err(bad_request(
                "set up an SMTP server under Settings → Email first",
            ));
        }
        recipients(url)?.join(", ")
    } else if url.starts_with("https://") || url.starts_with("http://") {
        url.to_string()
    } else {
        return Err(bad_request("the URL must start with https://"));
    };
    let url = url.as_str();
    let test = Event {
        kind: "test",
        text: "justdeploy is connected. Failed deploys, runs and backups, and crashed services, will be posted here.".into(),
        path: "/settings".into(),
    };
    deliver(state, &new.kind, url, &test)
        .await
        .map_err(|e| bad_request(format!("couldn't deliver a test message: {e:#}")))?;
    let id = Uuid::new_v4();
    sqlx::query!(
        "INSERT INTO notification_channels (id, name, kind, url_enc, notify_success)
         VALUES ($1, $2, $3, $4, $5)",
        id,
        name,
        new.kind,
        state.cipher.encrypt(url),
        new.notify_success
    )
    .execute(&state.db)
    .await
    .map_err(|e| match Error::from(e) {
        Error::Conflict(_) => Error::Conflict(format!("a channel named {name} already exists")),
        other => other,
    })?;
    Ok(id)
}

pub async fn delete(state: &SharedState, id: Uuid) -> Result<()> {
    let removed = sqlx::query!("DELETE FROM notification_channels WHERE id = $1", id)
        .execute(&state.db)
        .await?
        .rows_affected();
    if removed == 0 {
        return Err(Error::NotFound);
    }
    Ok(())
}

/// Tells every channel, in the background. Success events only go to channels
/// that asked for them.
pub fn send(state: &SharedState, event: Event) {
    let state = state.clone();
    tokio::spawn(async move {
        let rows = match sqlx::query!(
            "SELECT name, kind, url_enc, notify_success FROM notification_channels"
        )
        .fetch_all(&state.db)
        .await
        {
            Ok(rows) => rows,
            Err(error) => return tracing::warn!(%error, "could not load notification channels"),
        };
        for row in rows
            .into_iter()
            .filter(|r| r.notify_success || !event.success())
        {
            let delivered = match state.cipher.decrypt(&row.url_enc) {
                Ok(url) => deliver(&state, &row.kind, &url, &event).await,
                Err(e) => Err(anyhow::anyhow!("{e}")),
            };
            if let Err(error) = delivered {
                tracing::warn!(
                    channel = row.name,
                    error = format!("{error:#}"),
                    "notification not delivered"
                );
            }
        }
    });
}

async fn deliver(state: &SharedState, kind: &str, to: &str, event: &Event) -> anyhow::Result<()> {
    if kind == "email" {
        let smtp = smtp_settings(state)
            .await?
            .ok_or_else(|| anyhow::anyhow!("no SMTP server is set up"))?;
        return email(state, &smtp, to, event).await;
    }
    post(kind, to, event, &state.config.public_url).await
}

async fn post(kind: &str, url: &str, event: &Event, public_url: &str) -> anyhow::Result<()> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    let client = CLIENT.get_or_init(reqwest::Client::new);
    let link = format!("{public_url}{}", event.path);
    // Discord rejects messages over 2000 characters.
    let text: String = event.text.chars().take(1800).collect();
    let body = match kind {
        "slack" => json!({ "text": format!("{text}\n<{link}|Open in justdeploy>") }),
        "discord" => json!({ "content": format!("{text}\n{link}") }),
        _ => json!({ "event": event.kind, "text": text, "url": link }),
    };
    client
        .post(url)
        .json(&body)
        .timeout(Duration::from_secs(10))
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

// ---------------------------------------------------------------- email

/// The SMTP server email goes through, as stored. The password is encrypted.
#[derive(Clone, Serialize, Deserialize)]
pub struct Smtp {
    pub host: String,
    pub port: u16,
    /// starttls (usually 587) | tls (usually 465) | none (a relay on a trusted network)
    pub security: String,
    #[serde(default)]
    pub username: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_enc: Option<String>,
    /// The sender, like `justdeploy <alerts@example.com>`.
    pub from: String,
}

/// What Settings shows: everything but the password.
#[derive(Serialize)]
pub struct SmtpView {
    host: String,
    port: u16,
    security: String,
    username: String,
    has_password: bool,
    from: String,
}

#[derive(Deserialize)]
pub struct SmtpUpdate {
    host: String,
    port: u16,
    security: String,
    #[serde(default)]
    username: String,
    /// Empty or absent keeps the stored one.
    password: Option<String>,
    from: String,
    /// Where to send a test message before saving.
    test_to: String,
}

pub async fn smtp_settings(state: &SharedState) -> Result<Option<Smtp>> {
    let stored = sqlx::query_scalar!("SELECT value FROM settings WHERE key = 'smtp'")
        .fetch_optional(&state.db)
        .await?;
    Ok(stored.and_then(|v| serde_json::from_str(&v).ok()))
}

pub async fn smtp_view(state: &SharedState) -> Result<Option<SmtpView>> {
    Ok(smtp_settings(state).await?.map(|s| SmtpView {
        has_password: s.password_enc.is_some(),
        host: s.host,
        port: s.port,
        security: s.security,
        username: s.username,
        from: s.from,
    }))
}

/// Saves the SMTP server once a test message through it is accepted.
pub async fn set_smtp(state: &SharedState, update: SmtpUpdate) -> Result<()> {
    let host = update.host.trim().to_string();
    if host.is_empty() {
        return Err(bad_request("enter the SMTP server's hostname"));
    }
    if !["starttls", "tls", "none"].contains(&update.security.as_str()) {
        return Err(bad_request("security must be starttls, tls or none"));
    }
    update
        .from
        .trim()
        .parse::<lettre::message::Mailbox>()
        .map_err(|_| bad_request("the sender must be an address, like alerts@example.com"))?;
    let password_enc = match update.password.as_deref().map(str::trim) {
        Some(p) if !p.is_empty() => Some(state.cipher.encrypt(p)),
        _ => smtp_settings(state)
            .await?
            .filter(|old| old.host == host && old.username == update.username.trim())
            .and_then(|old| old.password_enc),
    };
    let smtp = Smtp {
        host,
        port: update.port,
        security: update.security,
        username: update.username.trim().to_string(),
        password_enc,
        from: update.from.trim().to_string(),
    };
    let to = recipients(&update.test_to)?.join(", ");
    let test = Event {
        kind: "test",
        text: "justdeploy can send email. Email channels under Notifications will use this server."
            .into(),
        path: "/settings".into(),
    };
    email(state, &smtp, &to, &test)
        .await
        .map_err(|e| bad_request(format!("the test message wasn't accepted: {e:#}")))?;
    let value = serde_json::to_string(&smtp).map_err(|e| Error::Internal(e.into()))?;
    sqlx::query!(
        "INSERT INTO settings (key, value) VALUES ('smtp', $1)
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
        value
    )
    .execute(&state.db)
    .await?;
    Ok(())
}

pub async fn remove_smtp(state: &SharedState) -> Result<()> {
    let channels = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM notification_channels WHERE kind = 'email'"#
    )
    .fetch_one(&state.db)
    .await?;
    if channels > 0 {
        return Err(bad_request("remove the email channels first"));
    }
    sqlx::query!("DELETE FROM settings WHERE key = 'smtp'")
        .execute(&state.db)
        .await?;
    Ok(())
}

/// Comma- or space-separated addresses, each checked.
fn recipients(list: &str) -> Result<Vec<String>> {
    let out: Vec<String> = list
        .split([',', ' ', ';', '\n'])
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(|a| {
            a.parse::<lettre::Address>()
                .map(|_| a.to_string())
                .map_err(|_| bad_request(format!("{a} is not an email address")))
        })
        .collect::<Result<_>>()?;
    if out.is_empty() {
        return Err(bad_request("enter at least one email address"));
    }
    Ok(out)
}

async fn email(state: &SharedState, smtp: &Smtp, to: &str, event: &Event) -> anyhow::Result<()> {
    use lettre::{
        message::header::ContentType, transport::smtp::authentication::Credentials,
        AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    };
    let builder = match smtp.security.as_str() {
        "tls" => AsyncSmtpTransport::<Tokio1Executor>::relay(&smtp.host)?,
        "starttls" => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&smtp.host)?,
        _ => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&smtp.host),
    };
    let mut builder = builder
        .port(smtp.port)
        .timeout(Some(Duration::from_secs(15)));
    if !smtp.username.is_empty() {
        let password = match &smtp.password_enc {
            Some(enc) => state.cipher.decrypt(enc)?,
            None => String::new(),
        };
        builder = builder.credentials(Credentials::new(smtp.username.clone(), password));
    }
    let transport = builder.build();
    let first_line = event.text.lines().next().unwrap_or_default();
    let subject: String = first_line.chars().take(120).collect();
    let link = format!("{}{}", state.config.public_url, event.path);
    let mut message = Message::builder()
        .from(smtp.from.parse()?)
        .subject(format!("justdeploy: {subject}"))
        .header(ContentType::TEXT_PLAIN);
    for address in to.split(',').map(str::trim).filter(|a| !a.is_empty()) {
        message = message.to(address.parse()?);
    }
    let message = message.body(format!("{}\n\nOpen in justdeploy: {link}\n", event.text))?;
    transport.send(message).await?;
    Ok(())
}

/// True at most once an hour per key, so a crash loop sends one message rather
/// than one a minute.
pub fn first_in_an_hour(key: Uuid) -> bool {
    static SEEN: OnceLock<Mutex<HashMap<Uuid, Instant>>> = OnceLock::new();
    let mut seen = SEEN
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = Instant::now();
    match seen.get(&key) {
        Some(at) if now.duration_since(*at) < Duration::from_secs(3600) => false,
        _ => {
            seen.insert(key, now);
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipients_are_checked_and_split() {
        assert_eq!(
            recipients("a@example.com, b@example.com;c@example.com").unwrap(),
            vec!["a@example.com", "b@example.com", "c@example.com"]
        );
        assert!(recipients("not an address").is_err());
        assert!(recipients(" , ").is_err());
    }

    #[test]
    fn repeats_within_the_hour_are_dropped() {
        let key = Uuid::new_v4();
        assert!(first_in_an_hour(key));
        assert!(!first_in_an_hour(key));
        assert!(first_in_an_hour(Uuid::new_v4()));
    }
}
