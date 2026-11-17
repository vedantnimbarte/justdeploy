//! Every name justdeploy gives to something in Docker or DNS, in one place.

use crate::error::{bad_request, Result};
use uuid::Uuid;

const MAX_NAME: usize = 24;

/// Project and service names become DNS labels and container names, so they are
/// held to lowercase letters, digits and hyphens.
pub fn validate(name: &str, what: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= MAX_NAME
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && !name.ends_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if ok {
        Ok(())
    } else {
        Err(bad_request(format!(
            "{what} name must be 1-{MAX_NAME} lowercase letters, digits or hyphens, starting with a letter"
        )))
    }
}

pub fn validate_hostname(host: &str) -> Result<()> {
    let ok = host.len() <= 253
        && host.contains('.')
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        });
    if ok {
        Ok(())
    } else {
        Err(bad_request(format!("{host} is not a valid lowercase hostname")))
    }
}

fn short(id: Uuid) -> String {
    id.simple().to_string()[..8].to_string()
}

pub fn generated_host(service: &str, project: &str, base: &str) -> String {
    format!("{service}-{project}.{base}")
}

pub fn preview_host(pr: i32, service: &str, project: &str, base: &str) -> String {
    format!("pr-{pr}-{service}-{project}.{base}")
}

pub fn container(project: &str, service: &str, deployment: Uuid) -> String {
    format!("jd-{project}-{service}-{}", short(deployment))
}

pub fn project_network(project: Uuid) -> String {
    format!("jd-proj-{}", short(project))
}

pub fn volume(service: Uuid, mount_path: &str) -> String {
    let slug: String = mount_path
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect();
    format!("jd-{}{}", short(service), slug.trim_end_matches('-'))
}

/// Built images live only in the local Docker image store.
pub fn image(service: Uuid, deployment: Uuid) -> String {
    format!("jd-{}:{}", service.simple(), short(deployment))
}

pub fn is_built_image(image: &str) -> bool {
    image.starts_with("jd-")
}

/// Traefik router and service name. Previews get their own so they never share
/// a load balancer with production.
pub fn router(service: Uuid, pr: Option<i32>) -> String {
    match pr {
        Some(pr) => format!("jd-{}-pr{pr}", service.simple()),
        None => format!("jd-{}", service.simple()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_dns_rules() {
        assert!(validate("api", "service").is_ok());
        assert!(validate("my-app2", "service").is_ok());
        for bad in ["", "Api", "2app", "app-", "a_b", "a.b", "abcdefghijklmnopqrstuvwxy"] {
            assert!(validate(bad, "service").is_err(), "{bad} should be refused");
        }
    }

    #[test]
    fn hostnames_are_validated_per_label() {
        assert!(validate_hostname("app.example.com").is_ok());
        assert!(validate_hostname("localhost").is_err());
        assert!(validate_hostname("-a.example.com").is_err());
        assert!(validate_hostname("APP.example.com").is_err());
    }

    #[test]
    fn generated_names_are_stable_and_distinct() {
        let s = Uuid::nil();
        assert_eq!(generated_host("api", "shop", "example.com"), "api-shop.example.com");
        assert_eq!(preview_host(7, "api", "shop", "example.com"), "pr-7-api-shop.example.com");
        assert_ne!(router(s, None), router(s, Some(1)));
        assert_eq!(volume(s, "/var/lib/postgresql/data"), "jd-00000000-var-lib-postgresql-data");
        assert!(is_built_image(&image(s, s)));
        assert!(!is_built_image("postgres:17-alpine"));
    }
}
