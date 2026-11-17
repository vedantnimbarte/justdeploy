//! Git provider webhooks: push deploys (GitHub and GitLab) and pull-request
//! previews (GitHub). Each service has its own URL and secret.

use crate::{
    error::{Error, Result},
    models::Service,
    queue::{self, Job},
    services::{self, NewDeployment},
    state::SharedState,
};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;

/// Verifies `X-Hub-Signature-256`. The MAC comparison is constant-time.
pub fn verify_github(secret: &str, signature_header: &str, body: &[u8]) -> bool {
    let Some(hex) = signature_header.strip_prefix("sha256=") else {
        return false;
    };
    let Ok(expected) = hex::decode(hex) else {
        return false;
    };
    let Ok(mut mac) = <Hmac<Sha256>>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&expected).is_ok()
}

/// GitLab sends the secret itself; compare without short-circuiting.
pub fn verify_gitlab(secret: &str, token: &str) -> bool {
    secret.len() == token.len()
        && secret
            .bytes()
            .zip(token.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

#[derive(Debug, Deserialize)]
pub struct Push {
    #[serde(rename = "ref")]
    git_ref: Option<String>,
    /// GitHub
    after: Option<String>,
    /// GitLab
    checkout_sha: Option<String>,
    head_commit: Option<Commit>,
    #[serde(default)]
    commits: Vec<Commit>,
}

#[derive(Debug, Deserialize)]
struct Commit {
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PullRequestEvent {
    action: String,
    number: i32,
    pull_request: PullRequest,
}

#[derive(Debug, Deserialize)]
struct PullRequest {
    title: Option<String>,
    head: Head,
}

#[derive(Debug, Deserialize)]
struct Head {
    sha: String,
    #[serde(rename = "ref")]
    git_ref: String,
    repo: Option<Repo>,
}

#[derive(Debug, Deserialize)]
struct Repo {
    clone_url: String,
}

fn first_line(message: Option<&str>) -> Option<String> {
    message.and_then(|m| m.lines().next()).map(str::to_string)
}

/// Deploys when the push is to the branch the service tracks.
pub async fn on_push(state: &SharedState, service: &Service, push: Push) -> Result<String> {
    let branch = push.git_ref.as_deref().and_then(|r| r.strip_prefix("refs/heads/"));
    let tracked = service.branch.as_deref().unwrap_or("main");
    if branch != Some(tracked) {
        return Ok(format!("ignored: not a push to {tracked}"));
    }
    let sha = push
        .after
        .or(push.checkout_sha)
        .filter(|sha| !sha.chars().all(|c| c == '0'));
    let Some(sha) = sha else {
        return Ok("ignored: the branch was deleted".into());
    };
    if service.kind != "app" || service.desired_state != "running" {
        return Ok("ignored: the service is not a running app".into());
    }
    let message = first_line(
        push.head_commit
            .as_ref()
            .or(push.commits.last())
            .and_then(|c| c.message.as_deref()),
    );
    let deployment = services::create_deployment(
        state,
        service,
        NewDeployment {
            kind: "production",
            trigger: "webhook",
            pr_number: None,
            repo_url: None,
            branch: Some(tracked.to_string()),
            commit_sha: Some(sha),
            commit_message: message,
            image_ref: None,
        },
    )
    .await?;
    Ok(format!("deploying {}", deployment.id))
}

/// Builds a preview for an open pull request and removes it when it closes.
pub async fn on_pull_request(state: &SharedState, service: &Service, event: PullRequestEvent) -> Result<String> {
    if !service.preview_enabled {
        return Ok("ignored: previews are off for this service".into());
    }
    match event.action.as_str() {
        "opened" | "reopened" | "synchronize" => {
            let pr = event.pull_request;
            let deployment = services::create_deployment(
                state,
                service,
                NewDeployment {
                    kind: "preview",
                    trigger: "webhook",
                    pr_number: Some(event.number),
                    repo_url: pr.head.repo.map(|r| r.clone_url),
                    branch: Some(pr.head.git_ref),
                    commit_sha: Some(pr.head.sha),
                    commit_message: pr.title,
                    image_ref: None,
                },
            )
            .await?;
            Ok(format!("building preview {}", deployment.id))
        }
        "closed" => {
            queue::enqueue(
                &state.db,
                Job::RemovePreview {
                    service_id: service.id,
                    pr: event.number,
                },
            )
            .await?;
            Ok("removing preview".into())
        }
        _ => Ok("ignored".into()),
    }
}

pub fn forbidden() -> Error {
    Error::BadRequest("signature does not match the service's webhook secret".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_signatures_are_checked() {
        let body = br#"{"ref":"refs/heads/main"}"#;
        let mut mac = <Hmac<Sha256>>::new_from_slice(b"s3cret").unwrap();
        mac.update(body);
        let signature = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
        assert!(verify_github("s3cret", &signature, body));
        assert!(!verify_github("other", &signature, body));
        assert!(!verify_github("s3cret", &signature, b"tampered"));
        assert!(!verify_github("s3cret", "sha1=abc", body));
    }

    #[test]
    fn gitlab_tokens_must_match_exactly() {
        assert!(verify_gitlab("abc", "abc"));
        assert!(!verify_gitlab("abc", "abd"));
        assert!(!verify_gitlab("abc", "ab"));
    }

    #[test]
    fn push_payloads_parse_from_both_providers() {
        let github: Push = serde_json::from_str(
            r#"{"ref":"refs/heads/main","after":"abc","head_commit":{"message":"Fix\n\nbody"}}"#,
        )
        .unwrap();
        assert_eq!(github.after.as_deref(), Some("abc"));
        assert_eq!(first_line(github.head_commit.unwrap().message.as_deref()).as_deref(), Some("Fix"));

        let gitlab: Push = serde_json::from_str(
            r#"{"object_kind":"push","ref":"refs/heads/main","checkout_sha":"def","commits":[{"message":"Add"}]}"#,
        )
        .unwrap();
        assert_eq!(gitlab.checkout_sha.as_deref(), Some("def"));
    }
}
