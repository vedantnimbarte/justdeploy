//! Messages to Slack, Discord or any webhook when a deploy, run or backup fails,
//! or a service crashes. Delivery never blocks or fails the work it reports on.

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

const KINDS: [&str; 3] = ["slack", "discord", "webhook"];

#[derive(Serialize)]
pub struct Channel {
    id: Uuid,
    name: String,
    kind: String,
    notify_success: bool,
    created_at: DateTime<Utc>,
    /// Where messages go, as a host only: the URL itself is a secret.
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
    /// deploy.failed | deploy.succeeded | service.crashed | run.failed | backup.failed
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
            Ok(Channel {
                id: r.id,
                name: r.name,
                kind: r.kind,
                notify_success: r.notify_success,
                created_at: r.created_at,
                target: url
                    .split("://")
                    .nth(1)
                    .and_then(|rest| rest.split('/').next())
                    .unwrap_or_default()
                    .to_string(),
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
        return Err(bad_request("kind must be slack, discord or webhook"));
    }
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(bad_request("the URL must start with https://"));
    }
    let test = Event {
        kind: "test",
        text: "justdeploy is connected. Failed deploys, runs and backups, and crashed services, will be posted here.".into(),
        path: "/settings".into(),
    };
    post(&new.kind, url, &test, &state.config.public_url)
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
                Ok(url) => post(&row.kind, &url, &event, &state.config.public_url).await,
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
    fn repeats_within_the_hour_are_dropped() {
        let key = Uuid::new_v4();
        assert!(first_in_an_hour(key));
        assert!(!first_in_an_hour(key));
        assert!(first_in_an_hour(Uuid::new_v4()));
    }
}
