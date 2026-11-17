//! One-click databases: image, port, data path, credentials, and how to dump and
//! restore each one. Commands run inside the database's own container with `sh -c`,
//! reading credentials from its environment, so they never pass through justdeploy.

use crate::crypto::random_string;
use std::collections::HashMap;

pub struct Engine {
    pub id: &'static str,
    pub label: &'static str,
    pub image: &'static str,
    pub port: i32,
    pub data_path: &'static str,
    /// File extension of a backup, before `.gz`.
    pub dump_ext: &'static str,
}

pub const ENGINES: &[Engine] = &[
    Engine {
        id: "postgres",
        label: "PostgreSQL",
        image: "postgres:17-alpine",
        port: 5432,
        data_path: "/var/lib/postgresql/data",
        dump_ext: "sql",
    },
    Engine {
        id: "mysql",
        label: "MySQL",
        image: "mysql:8.4",
        port: 3306,
        data_path: "/var/lib/mysql",
        dump_ext: "sql",
    },
    Engine {
        id: "mariadb",
        label: "MariaDB",
        image: "mariadb:11.8",
        port: 3306,
        data_path: "/var/lib/mysql",
        dump_ext: "sql",
    },
    Engine {
        id: "redis",
        label: "Redis",
        image: "redis:8-alpine",
        port: 6379,
        data_path: "/data",
        dump_ext: "rdb",
    },
    Engine {
        id: "mongodb",
        label: "MongoDB",
        image: "mongo:8.0",
        port: 27017,
        data_path: "/data/db",
        dump_ext: "archive",
    },
];

pub fn get(id: &str) -> Option<&'static Engine> {
    ENGINES.iter().find(|e| e.id == id)
}

/// Credentials generated when the database is created. They are ordinary env vars
/// on the service afterwards, encrypted like any other.
pub fn initial_env(engine: &str) -> Vec<(String, String)> {
    let pairs: Vec<(&str, String)> = match engine {
        "postgres" => vec![
            ("POSTGRES_USER", "app".into()),
            ("POSTGRES_PASSWORD", random_string(24)),
            ("POSTGRES_DB", "app".into()),
        ],
        "mysql" => vec![
            ("MYSQL_ROOT_PASSWORD", random_string(24)),
            ("MYSQL_DATABASE", "app".into()),
            ("MYSQL_USER", "app".into()),
            ("MYSQL_PASSWORD", random_string(24)),
        ],
        "mariadb" => vec![
            ("MARIADB_ROOT_PASSWORD", random_string(24)),
            ("MARIADB_DATABASE", "app".into()),
            ("MARIADB_USER", "app".into()),
            ("MARIADB_PASSWORD", random_string(24)),
        ],
        "redis" => vec![("REDIS_PASSWORD", random_string(24))],
        "mongodb" => vec![
            ("MONGO_INITDB_ROOT_USERNAME", "app".into()),
            ("MONGO_INITDB_ROOT_PASSWORD", random_string(24)),
        ],
        _ => vec![],
    };
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

/// Redis needs its password on the command line; the shell reads it from the env.
/// Persistence is RDB snapshots (at most a minute of writes at risk), which keeps
/// backup and restore a matter of one file.
pub fn command(engine: &str) -> Option<Vec<String>> {
    (engine == "redis").then(|| {
        vec![
            "sh".into(),
            "-c".into(),
            r#"exec redis-server --requirepass "$REDIS_PASSWORD" --save 60 1 --appendonly no"#
                .into(),
        ]
    })
}

/// How another service in the same project connects, using the service name as host.
pub fn connection_url(engine: &str, host: &str, env: &HashMap<String, String>) -> Option<String> {
    let get = |k: &str| env.get(k).cloned().unwrap_or_default();
    Some(match engine {
        "postgres" => format!(
            "postgresql://{}:{}@{host}:5432/{}",
            get("POSTGRES_USER"),
            get("POSTGRES_PASSWORD"),
            get("POSTGRES_DB")
        ),
        "mysql" => format!(
            "mysql://{}:{}@{host}:3306/{}",
            get("MYSQL_USER"),
            get("MYSQL_PASSWORD"),
            get("MYSQL_DATABASE")
        ),
        "mariadb" => format!(
            "mysql://{}:{}@{host}:3306/{}",
            get("MARIADB_USER"),
            get("MARIADB_PASSWORD"),
            get("MARIADB_DATABASE")
        ),
        "redis" => format!("redis://default:{}@{host}:6379", get("REDIS_PASSWORD")),
        "mongodb" => format!(
            "mongodb://{}:{}@{host}:27017/?authSource=admin",
            get("MONGO_INITDB_ROOT_USERNAME"),
            get("MONGO_INITDB_ROOT_PASSWORD")
        ),
        _ => return None,
    })
}

/// Writes a full dump to stdout.
pub fn dump_command(engine: &str) -> Option<&'static str> {
    Some(match engine {
        "postgres" => r#"pg_dump -U "$POSTGRES_USER" --clean --if-exists "$POSTGRES_DB""#,
        "mysql" => {
            r#"MYSQL_PWD="$MYSQL_ROOT_PASSWORD" mysqldump -uroot --single-transaction --routines --triggers --databases "$MYSQL_DATABASE""#
        }
        "mariadb" => {
            r#"MYSQL_PWD="$MARIADB_ROOT_PASSWORD" mariadb-dump -uroot --single-transaction --routines --triggers --databases "$MARIADB_DATABASE""#
        }
        "redis" => {
            r#"redis-cli -a "$REDIS_PASSWORD" --no-auth-warning --rdb /tmp/jd-backup.rdb >&2 && cat /tmp/jd-backup.rdb && rm -f /tmp/jd-backup.rdb"#
        }
        "mongodb" => {
            r#"mongodump --archive --quiet --username "$MONGO_INITDB_ROOT_USERNAME" --password "$MONGO_INITDB_ROOT_PASSWORD" --authenticationDatabase admin"#
        }
        _ => return None,
    })
}

/// Reads a dump from stdin. Redis has none: its restore swaps the RDB file while
/// the server is stopped (see backups.rs).
pub fn restore_command(engine: &str) -> Option<&'static str> {
    Some(match engine {
        "postgres" => {
            r#"psql -q -v ON_ERROR_STOP=1 -U "$POSTGRES_USER" -d "$POSTGRES_DB" >/dev/null"#
        }
        "mysql" => r#"MYSQL_PWD="$MYSQL_ROOT_PASSWORD" mysql -uroot"#,
        "mariadb" => r#"MYSQL_PWD="$MARIADB_ROOT_PASSWORD" mariadb -uroot"#,
        "mongodb" => {
            r#"mongorestore --archive --drop --quiet --username "$MONGO_INITDB_ROOT_USERNAME" --password "$MONGO_INITDB_ROOT_PASSWORD" --authenticationDatabase admin"#
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_engine_has_credentials_a_url_and_a_dump() {
        for engine in ENGINES {
            let env: HashMap<_, _> = initial_env(engine.id).into_iter().collect();
            assert!(!env.is_empty(), "{} has no credentials", engine.id);
            let url = connection_url(engine.id, "db", &env).unwrap();
            assert!(url.contains("@db:"), "{url}");
            assert!(dump_command(engine.id).is_some());
        }
    }

    #[test]
    fn urls_embed_the_generated_password() {
        let env: HashMap<_, _> = initial_env("postgres").into_iter().collect();
        let url = connection_url("postgres", "db", &env).unwrap();
        assert!(url.contains(&env["POSTGRES_PASSWORD"]));
        assert!(url.ends_with("/app"));
    }
}
