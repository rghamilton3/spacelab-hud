use axum::{
    Router,
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Redirect},
    routing::{get, post},
    Form,
};
use serde::Deserialize;

use crate::config::{AppConfig, ConfigRef};

#[derive(Deserialize)]
struct GhUserResp {
    login: String,
}

async fn github_username_for_pat(pat: &str) -> Option<String> {
    let resp = reqwest::Client::new()
        .get("https://api.github.com/user")
        .header("Authorization", format!("Bearer {pat}"))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", "spacelab-hud/1.0")
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() { return None; }
    resp.json::<GhUserResp>().await.ok().map(|u| u.login)
}

pub async fn serve(config: ConfigRef, port: u16) {
    let app = Router::new()
        .route("/", get(ui_handler))
        .route("/config", post(save_handler))
        .with_state(config);

    let addr = format!("0.0.0.0:{port}");
    eprintln!("web_config: listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await
        .expect("web_config: failed to bind");
    axum::serve(listener, app).await
        .expect("web_config: server error");
}

async fn ui_handler(State(config): State<ConfigRef>) -> impl IntoResponse {
    let cfg    = config.read().await.clone();
    let repos  = cfg.github_repos.join("\n");
    let pat    = &cfg.github_pat;
    let user   = &cfg.github_username;
    let poll   = cfg.github_poll_secs;

    Html(format!(r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>SpaceLab HUD — Config</title>
<style>
  * {{ box-sizing: border-box; margin: 0; padding: 0; }}
  body {{ background: #0a0e14; color: #d0e8ff; font-family: monospace; padding: 32px; }}
  h1 {{ color: #00b4d8; letter-spacing: 3px; font-size: 18px; margin-bottom: 24px; }}
  label {{ display: block; margin-bottom: 16px; }}
  span {{ display: block; color: #3e6ea0; font-size: 12px; letter-spacing: 2px; margin-bottom: 6px; }}
  input, textarea {{ width: 100%; background: #0f1828; border: 1px solid #1e2d4a;
    color: #d0e8ff; font-family: monospace; font-size: 14px; padding: 10px;
    border-radius: 4px; outline: none; }}
  input:focus, textarea:focus {{ border-color: #00b4d8; }}
  textarea {{ height: 120px; resize: vertical; }}
  button {{ margin-top: 24px; background: #00b4d8; color: #0a0e14; border: none;
    padding: 12px 28px; font-family: monospace; font-size: 14px; letter-spacing: 2px;
    cursor: pointer; border-radius: 4px; }}
  button:hover {{ background: #48cae4; }}
  .hint {{ color: #2a4060; font-size: 11px; margin-top: 6px; }}
  .pat-help {{ background: #0f1828; border: 1px solid #1e2d4a; border-radius: 4px;
    padding: 12px 14px; margin-top: 8px; font-size: 12px; line-height: 1.7; }}
  .pat-help b {{ color: #00b4d8; }}
  .pat-help li {{ margin-left: 16px; color: #3e6ea0; }}
</style>
</head>
<body>
<h1>SPACELAB-1 // CONFIG</h1>
<form method="post" action="/config">
  <label>
    <span>GITHUB PAT</span>
    <input type="password" name="github_pat" value="{pat}" placeholder="ghp_…" autocomplete="off">
    <div class="pat-help">
      <b>Fine-grained PAT</b> (recommended) — Settings → Developer settings → Fine-grained tokens<br>
      Set <b>Repository access</b> to your watched repos, then enable:<br>
      <ul>
        <li><b>Actions</b> → Read-only &nbsp;(CI run status)</li>
        <li><b>Issues</b> → Read-only &nbsp;(issues + comments)</li>
        <li><b>Pull requests</b> → Read-only &nbsp;(PRs + comments)</li>
        <li><b>Metadata</b> → Read-only &nbsp;(auto-selected, mandatory)</li>
      </ul>
      <b>Classic PAT</b> — scope <b>public_repo</b> (public repos) or <b>repo</b> (private repos)
    </div>
  </label>
  <label>
    <span>GITHUB USERNAME</span>
    <input type="text" name="github_username" value="{user}" placeholder="your-username">
    <p class="hint">Used to distinguish your activity from others</p>
  </label>
  <label>
    <span>REPOS TO WATCH (one per line, owner/repo)</span>
    <textarea name="github_repos">{repos}</textarea>
  </label>
  <label>
    <span>POLL INTERVAL (seconds, min 30)</span>
    <input type="number" name="github_poll_secs" value="{poll}" min="30" max="3600">
  </label>
  <button type="submit">SAVE CONFIG</button>
</form>
</body>
</html>"#))
}

#[derive(Deserialize)]
struct ConfigForm {
    github_pat:       String,
    github_username:  String,
    github_repos:     String,
    github_poll_secs: u64,
}

async fn save_handler(
    State(config): State<ConfigRef>,
    Form(form):    Form<ConfigForm>,
) -> impl IntoResponse {
    let repos: Vec<String> = form.github_repos
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.contains('/'))
        .map(str::to_string)
        .collect();

    let pat = form.github_pat.trim().to_string();

    // Auto-discover username from PAT when the field is left blank
    let username = if form.github_username.trim().is_empty() && !pat.is_empty() {
        github_username_for_pat(&pat).await.unwrap_or_default()
    } else {
        form.github_username.trim().to_string()
    };

    let new_cfg = AppConfig {
        github_pat:       pat,
        github_username:  username,
        github_repos:     repos,
        github_poll_secs: form.github_poll_secs.max(30),
        web_config_port:  config.read().await.web_config_port,
    };

    if let Err(e) = new_cfg.save() {
        eprintln!("web_config: failed to save config: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    *config.write().await = new_cfg;
    Redirect::to("/").into_response()
}
