//! `jd`: justdeploy from a terminal or CI, through the HTTP API with an API token.

use reqwest::blocking::{Client, RequestBuilder, Response};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
    process::ExitCode,
    thread::sleep,
    time::Duration,
};

const USAGE: &str = "\
jd: justdeploy from your terminal

Usage:
  jd login <server-url> <api-token>   Save where justdeploy is and a token (Account, API tokens)
  jd whoami                           Who the token belongs to
  jd projects                         Projects, their environments and services
  jd status <project>/<service>       A service's status and latest deploy
  jd deploy <project>/<service>       Deploy the latest commit or image and follow the log
       [--no-wait]                    ...or return as soon as it's queued
  jd restart <project>/<service>      Run the current image again
  jd logs <project>/<service>         Follow the running service's output (Ctrl-C stops)
  jd env <project>/<service>          Print variables as KEY=value
  jd env set <project>/<service> KEY=value...
  jd env unset <project>/<service> KEY...
  jd run <project>/<service> -- <command>
                                      Run a command once from the live image; exits with its code

An environment is its own project: shop-staging/web is web in shop's staging environment.
JD_URL and JD_TOKEN override the saved login, for CI.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("jd: {message}");
            ExitCode::FAILURE
        }
    }
}

type Result<T> = std::result::Result<T, String>;

fn run(args: &[String]) -> Result<ExitCode> {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["login", url, token] => login(url, token),
        ["whoami"] => {
            let me = Api::new()?.get("/auth/me")?;
            println!("{} ({})", text(&me["email"]), text(&me["role"]));
            Ok(ExitCode::SUCCESS)
        }
        ["projects"] => projects(&Api::new()?),
        ["status", target] => status(&Api::new()?, target),
        ["deploy", target, rest @ ..] => deploy(&Api::new()?, target, !rest.contains(&"--no-wait")),
        ["restart", target] => {
            let api = Api::new()?;
            let service = api.find(target)?;
            api.post(
                &format!("/services/{}/actions", service.id),
                json!({ "action": "restart" }),
            )?;
            println!("Restarting {target}");
            Ok(ExitCode::SUCCESS)
        }
        ["logs", target] => logs(&Api::new()?, target),
        ["env", target] => env_print(&Api::new()?, target),
        ["env", "set", target, pairs @ ..] if !pairs.is_empty() => {
            env_set(&Api::new()?, target, pairs)
        }
        ["env", "unset", target, keys @ ..] if !keys.is_empty() => {
            env_unset(&Api::new()?, target, keys)
        }
        ["run", target, "--", command @ ..] if !command.is_empty() => {
            run_command(&Api::new()?, target, &command.join(" "))
        }
        ["--version" | "-V"] => {
            println!("jd {}", env!("CARGO_PKG_VERSION"));
            Ok(ExitCode::SUCCESS)
        }
        _ => {
            println!("{USAGE}");
            Ok(
                if words.is_empty() || words == ["help"] || words == ["--help"] {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::FAILURE
                },
            )
        }
    }
}

// ---------------------------------------------------------------- config

#[derive(Serialize, Deserialize)]
struct Login {
    url: String,
    token: String,
}

fn config_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .ok_or("can't tell where your home folder is; set HOME")?;
    Ok(PathBuf::from(home).join(".jd.json"))
}

fn login(url: &str, token: &str) -> Result<ExitCode> {
    let login = Login {
        url: url.trim_end_matches('/').to_string(),
        token: token.to_string(),
    };
    let api = Api::with(&login)?;
    let me = api.get("/auth/me")?;
    let path = config_path()?;
    let body = serde_json::to_string_pretty(&login).map_err(|e| e.to_string())?;
    std::fs::write(&path, body).map_err(|e| format!("couldn't save {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // The token can do whatever its owner can: only you may read it.
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    println!(
        "Signed in to {} as {} ({}). Saved to {}",
        login.url,
        text(&me["email"]),
        text(&me["role"]),
        path.display()
    );
    Ok(ExitCode::SUCCESS)
}

// ---------------------------------------------------------------- API

struct Api {
    client: Client,
    url: String,
    token: String,
}

struct Target {
    id: String,
}

impl Api {
    fn new() -> Result<Api> {
        let login = match (std::env::var("JD_URL"), std::env::var("JD_TOKEN")) {
            (Ok(url), Ok(token)) => Login { url, token },
            _ => {
                let path = config_path()?;
                let saved = std::fs::read_to_string(&path).map_err(|_| {
                    "not signed in: run `jd login <server-url> <api-token>` first".to_string()
                })?;
                serde_json::from_str(&saved)
                    .map_err(|e| format!("{} is unreadable: {e}", path.display()))?
            }
        };
        Api::with(&login)
    }

    fn with(login: &Login) -> Result<Api> {
        Ok(Api {
            // No overall timeout: logs stream for as long as they run.
            client: Client::builder()
                .timeout(None)
                .connect_timeout(Duration::from_secs(15))
                .user_agent(concat!("jd/", env!("CARGO_PKG_VERSION")))
                .build()
                .map_err(|e| e.to_string())?,
            url: login.url.trim_end_matches('/').to_string(),
            token: login.token.clone(),
        })
    }

    fn request(&self, method: reqwest::Method, path: &str) -> RequestBuilder {
        self.client
            .request(method, format!("{}/api/v1{path}", self.url))
            .bearer_auth(&self.token)
    }

    fn send(&self, request: RequestBuilder) -> Result<Response> {
        let response = request
            .send()
            .map_err(|e| format!("can't reach {}: {e}", self.url))?;
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status();
        let body: Value = response.json().unwrap_or_default();
        Err(match (status.as_u16(), body["error"].as_str()) {
            (401, _) => {
                "the token was refused; create a new one under Account and run `jd login` again"
                    .into()
            }
            (_, Some(message)) => message.to_string(),
            _ => format!("the server answered {status}"),
        })
    }

    fn get(&self, path: &str) -> Result<Value> {
        let response = self.send(
            self.request(reqwest::Method::GET, path)
                .timeout(Duration::from_secs(60)),
        )?;
        response.json().map_err(|e| e.to_string())
    }

    fn post(&self, path: &str, body: Value) -> Result<Value> {
        self.write(reqwest::Method::POST, path, body)
    }

    fn write(&self, method: reqwest::Method, path: &str, body: Value) -> Result<Value> {
        let response = self.send(
            self.request(method, path)
                .json(&body)
                .timeout(Duration::from_secs(60)),
        )?;
        let text = response.text().map_err(|e| e.to_string())?;
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    /// `project/service` to ids. An environment is addressed by its own
    /// project name, like `shop-staging/web`.
    fn find(&self, target: &str) -> Result<Target> {
        let (project, service) = target
            .split_once('/')
            .ok_or_else(|| format!("{target}: write it as <project>/<service>"))?;
        let projects = self.get("/projects")?;
        let project_id = project_id_for(&projects, project)
            .ok_or_else(|| format!("there is no project called {project}"))?;
        let detail = self.get(&format!("/projects/{project_id}"))?;
        let id = detail["services"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|s| text(&s["name"]) == service)
            .map(|s| text(&s["id"]).to_string())
            .ok_or_else(|| format!("{project} has no service called {service}"))?;
        Ok(Target { id })
    }

    /// Prints a server-sent event stream until it ends.
    fn stream(&self, path: &str) -> Result<()> {
        let response = self.send(
            self.request(reqwest::Method::GET, path)
                .header("Accept", "text/event-stream"),
        )?;
        let mut event = String::new();
        for line in BufReader::new(response).lines() {
            let line = line.map_err(|e| format!("the stream broke: {e}"))?;
            if let Some(name) = line.strip_prefix("event:") {
                event = name.trim().to_string();
            } else if let Some(data) = line.strip_prefix("data:") {
                if event == "end" {
                    return Ok(());
                }
                println!("{}", data.strip_prefix(' ').unwrap_or(data));
            } else if line.is_empty() {
                if event == "end" {
                    return Ok(());
                }
                event.clear();
            }
        }
        Ok(())
    }
}

/// A project's id by the name `jd` uses: the project's own for production,
/// `<project>-<environment>` for its other environments.
fn project_id_for(projects: &Value, name: &str) -> Option<String> {
    projects.as_array()?.iter().find_map(|p| {
        p["environments"].as_array()?.iter().find_map(|env| {
            let full = match text(&env["environment"]) {
                "production" => text(&p["name"]).to_string(),
                other => format!("{}-{other}", text(&p["name"])),
            };
            (full == name).then(|| text(&env["id"]).to_string())
        })
    })
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

// ---------------------------------------------------------------- commands

fn projects(api: &Api) -> Result<ExitCode> {
    for p in api.get("/projects")?.as_array().into_iter().flatten() {
        let envs: Vec<&str> = p["environments"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|e| text(&e["environment"]))
            .collect();
        println!("{}  ({})", text(&p["name"]), envs.join(", "));
        for s in p["services"].as_array().into_iter().flatten() {
            println!(
                "  {:<20} {:<10} {}",
                text(&s["name"]),
                text(&s["status"]),
                text(&s["url"])
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn status(api: &Api, target: &str) -> Result<ExitCode> {
    let service = api.get(&format!("/services/{}", api.find(target)?.id))?;
    println!("{target}: {}", text(&service["status"]));
    if let Some(url) = service["url"].as_str() {
        println!("  {url}");
    }
    let latest = &service["latest_deployment"];
    if latest.is_object() {
        let commit = latest["commit_sha"]
            .as_str()
            .map(|s| &s[..s.len().min(7)])
            .unwrap_or("");
        println!(
            "  latest deploy: {} {} {}",
            text(&latest["status"]),
            commit,
            text(&latest["commit_message"])
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn deploy(api: &Api, target: &str, wait: bool) -> Result<ExitCode> {
    let service = api.find(target)?;
    api.post(
        &format!("/services/{}/actions", service.id),
        json!({ "action": "deploy" }),
    )?;
    let deployments = api.get(&format!("/services/{}/deployments", service.id))?;
    let id = text(&deployments[0]["id"]).to_string();
    println!("Deploying {target} ({})", &id[..id.len().min(8)]);
    if !wait {
        return Ok(ExitCode::SUCCESS);
    }
    api.stream(&format!("/deployments/{id}/logs"))?;
    // The log ends when the deploy does; read how it ended.
    loop {
        let deployment = api.get(&format!("/deployments/{id}"))?;
        match text(&deployment["status"]) {
            "running" | "superseded" => return Ok(ExitCode::SUCCESS),
            "failed" | "removed" => {
                eprintln!("jd: the deploy failed: {}", text(&deployment["error"]));
                return Ok(ExitCode::FAILURE);
            }
            _ => sleep(Duration::from_secs(2)),
        }
    }
}

fn logs(api: &Api, target: &str) -> Result<ExitCode> {
    let service = api.find(target)?;
    api.stream(&format!("/services/{}/logs", service.id))?;
    Ok(ExitCode::SUCCESS)
}

fn env_vars(api: &Api, service: &Target) -> Result<Vec<(String, String)>> {
    let vars = api.get(&format!("/services/{}/env", service.id))?;
    Ok(vars
        .as_array()
        .into_iter()
        .flatten()
        .map(|v| (text(&v["key"]).to_string(), text(&v["value"]).to_string()))
        .collect())
}

fn save_env(api: &Api, service: &Target, vars: &[(String, String)]) -> Result<()> {
    let body: Vec<Value> = vars
        .iter()
        .map(|(k, v)| json!({ "key": k, "value": v }))
        .collect();
    let saved = api.write(
        reqwest::Method::PUT,
        &format!("/services/{}/env", service.id),
        Value::Array(body),
    )?;
    println!(
        "{}",
        if saved["redeploying"].as_bool() == Some(true) {
            "Saved. Redeploying with the new values."
        } else {
            "Saved."
        }
    );
    Ok(())
}

fn env_print(api: &Api, target: &str) -> Result<ExitCode> {
    let service = api.find(target)?;
    for (key, value) in env_vars(api, &service)? {
        println!("{key}={value}");
    }
    Ok(ExitCode::SUCCESS)
}

fn env_set(api: &Api, target: &str, pairs: &[&str]) -> Result<ExitCode> {
    let service = api.find(target)?;
    let mut vars = env_vars(api, &service)?;
    for pair in pairs {
        let (key, value) = pair
            .split_once('=')
            .ok_or_else(|| format!("{pair}: write it as KEY=value"))?;
        match vars.iter_mut().find(|(k, _)| k == key) {
            Some(existing) => existing.1 = value.to_string(),
            None => vars.push((key.to_string(), value.to_string())),
        }
    }
    save_env(api, &service, &vars)?;
    Ok(ExitCode::SUCCESS)
}

fn env_unset(api: &Api, target: &str, keys: &[&str]) -> Result<ExitCode> {
    let service = api.find(target)?;
    let mut vars = env_vars(api, &service)?;
    let before = vars.len();
    vars.retain(|(k, _)| !keys.contains(&k.as_str()));
    if vars.len() == before {
        println!("Nothing to remove.");
        return Ok(ExitCode::SUCCESS);
    }
    save_env(api, &service, &vars)?;
    Ok(ExitCode::SUCCESS)
}

fn run_command(api: &Api, target: &str, command: &str) -> Result<ExitCode> {
    let service = api.find(target)?;
    let run = api.post(
        &format!("/services/{}/runs", service.id),
        json!({ "command": command }),
    )?;
    let id = text(&run["id"]).to_string();
    eprintln!("Running `{command}` on {target}…");
    // Output is saved when the run ends; wait for that.
    let finished = loop {
        sleep(Duration::from_secs(2));
        let list = api.get(&format!("/services/{}/runs", service.id))?;
        let found = list["runs"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|r| text(&r["id"]) == id)
            .cloned();
        if let Some(run) = found.filter(|r| text(&r["status"]) != "running") {
            break run;
        }
    };
    print!("{}", text(&finished["output"]));
    if !text(&finished["output"]).ends_with('\n') {
        println!();
    }
    match finished["exit_code"].as_i64() {
        Some(0) => Ok(ExitCode::SUCCESS),
        Some(code) => Ok(ExitCode::from(u8::try_from(code).unwrap_or(1))),
        None => Err(text(&finished["error"]).to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environments_are_addressed_by_project_dash_environment() {
        let projects = json!([{
            "name": "shop",
            "environments": [
                { "id": "p1", "environment": "production" },
                { "id": "p2", "environment": "staging" }
            ]
        }]);
        assert_eq!(project_id_for(&projects, "shop").as_deref(), Some("p1"));
        assert_eq!(
            project_id_for(&projects, "shop-staging").as_deref(),
            Some("p2")
        );
        assert_eq!(project_id_for(&projects, "shop-dev"), None);
    }
}
