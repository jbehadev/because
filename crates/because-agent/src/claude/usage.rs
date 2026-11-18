//! How much of the Claude subscription's allowance is left.
//!
//! The CLI has no flag for this — `/usage` is an interactive command — so this
//! asks the same place it does: the OAuth usage endpoint, with the login token
//! Claude Code already stored. Nothing here is a published contract, so every
//! step degrades to an error the UI can show rather than to a wrong number.
//!
//! Shelling out to `curl` keeps this crate's dependencies as they are, in
//! keeping with `because-git` shelling out to `git` and the agent to `claude`.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use serde_json::Value;
use because_core::{Usage, UsageWindow};

const ENDPOINT: &str = "https://api.anthropic.com/api/oauth/usage";
/// The beta the OAuth-token endpoints are behind.
const OAUTH_BETA: &str = "oauth-2025-04-20";

/// Ask the endpoint what is left. Blocking; call it off the UI thread.
pub fn fetch() -> Result<Usage> {
    let token = access_token()?;
    let body = get(ENDPOINT, &token)?;
    parse(&body)
}

/// The same request, with the raw response returned alongside, for `--check`
/// and the log: when the shape changes, the body is what says how.
pub fn fetch_raw() -> Result<(Usage, String)> {
    let token = access_token()?;
    let body = get(ENDPOINT, &token)?;
    Ok((parse(&body)?, body))
}

/// Turn the endpoint's answer into windows, keeping any object that carries a
/// utilization and ignoring everything else — new windows should appear on
/// their own, and fields we do not know about should not break the ones we do.
pub fn parse(body: &str) -> Result<Usage> {
    let v: Value = serde_json::from_str(body)
        .with_context(|| format!("usage endpoint did not return JSON: {}", first_chars(body, 160)))?;
    if let Some(e) = v["error"]["message"].as_str() {
        bail!("usage endpoint: {e}");
    }
    let Some(obj) = v.as_object() else { bail!("usage endpoint returned {}, not an object", kind(&v)) };
    let mut windows = Vec::new();
    for (key, val) in obj {
        let Some(pct) = val["utilization"].as_f64() else { continue };
        windows.push(UsageWindow {
            key: key.clone(),
            label: label_for(key),
            used_pct: pct.clamp(0.0, 100.0),
            resets_at: val["resets_at"].as_str().unwrap_or_default().to_string(),
        });
    }
    if windows.is_empty() {
        bail!("no usage windows in the reply: {}", first_chars(body, 160));
    }
    // Shortest window first, which is both the order `/usage` shows and the
    // order they bite in.
    windows.sort_by_key(|w| order_of(&w.key));
    Ok(Usage { windows })
}

/// A label that fits the Sessions column. Unknown keys keep their own name,
/// shortened, so a window added later still shows up with something readable.
fn label_for(key: &str) -> String {
    match key {
        "five_hour" => "5h".into(),
        "seven_day" => "week".into(),
        "seven_day_opus" => "opus".into(),
        other => other.replace('_', " ").chars().take(6).collect(),
    }
}

fn order_of(key: &str) -> u8 {
    match key {
        "five_hour" => 0,
        "seven_day" => 1,
        "seven_day_opus" => 2,
        _ => 3,
    }
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

fn first_chars(s: &str, n: usize) -> String {
    s.trim().chars().take(n).collect()
}

/// GET with the token in a curl config read from stdin, so the credential never
/// appears in this process's arguments and so never in `ps`.
fn get(url: &str, token: &str) -> Result<String> {
    let config = format!(
        "url = \"{url}\"\nheader = \"Authorization: Bearer {token}\"\nheader = \"anthropic-beta: {OAUTH_BETA}\"\n\
         header = \"Accept: application/json\"\nsilent\nshow-error\nmax-time = 10\n"
    );
    let mut child = Command::new("curl")
        .arg("--config")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("could not run `curl` to ask for the usage allowance")?;
    child.stdin.take().expect("piped stdin").write_all(config.as_bytes())?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!("curl failed: {}", first_chars(&String::from_utf8_lossy(&out.stderr), 200));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The OAuth token Claude Code logged in with: the macOS Keychain first, since
/// that is where it puts it there, then the credentials file it uses elsewhere.
fn access_token() -> Result<String> {
    let mut why = Vec::new();
    for source in [keychain_credentials as fn() -> Result<String>, file_credentials] {
        match source().and_then(|raw| token_from(&raw)) {
            Ok(t) => return Ok(t),
            Err(e) => why.push(e.to_string()),
        }
    }
    bail!(
        "no Claude login to read the allowance with ({}). `claude auth` logs in; an API-key login has no \
         subscription allowance to report.",
        why.join("; ")
    )
}

fn token_from(raw: &str) -> Result<String> {
    let v: Value = serde_json::from_str(raw).context("stored credentials are not JSON")?;
    v["claudeAiOauth"]["accessToken"]
        .as_str()
        .map(str::to_string)
        .context("stored credentials hold no OAuth access token")
}

fn keychain_credentials() -> Result<String> {
    if !cfg!(target_os = "macos") {
        bail!("no keychain on this platform");
    }
    let out = Command::new("security")
        .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
        .output()
        .context("could not run `security`")?;
    if !out.status.success() {
        bail!("keychain: {}", first_chars(&String::from_utf8_lossy(&out.stderr), 120));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn file_credentials() -> Result<String> {
    let dir = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|h| h.join(".claude")))
        .context("neither CLAUDE_CONFIG_DIR nor the home directory is set")?;
    let path = dir.join(".credentials.json");
    std::fs::read_to_string(&path).with_context(|| format!("{}", path.display()))
}
