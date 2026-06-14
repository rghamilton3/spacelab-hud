use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect},
    routing::{get, post},
    Form,
};
use serde::{Deserialize, Serialize};

use crate::config::{AppConfig, ConfigRef};

// ── GitHub API helpers ────────────────────────────────────────────────

async fn gh_get<T: serde::de::DeserializeOwned>(pat: &str, url: &str) -> Result<T, String> {
    let resp = reqwest::Client::new()
        .get(url)
        .header("Authorization", format!("Bearer {pat}"))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", "spacelab-hud/1.0")
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status().as_u16() == 401 {
        return Err("PAT invalid or expired".to_string());
    }
    if !resp.status().is_success() {
        return Err(format!("GitHub returned {}", resp.status()));
    }
    resp.json::<T>().await.map_err(|e| e.to_string())
}

#[derive(Deserialize)]
struct GhUserResp {
    login: String,
}

async fn github_username_for_pat(pat: &str) -> Option<String> {
    gh_get::<GhUserResp>(pat, "https://api.github.com/user")
        .await
        .ok()
        .map(|u| u.login)
}

#[derive(Deserialize)]
struct GhRepoItem {
    full_name: String,
}

/// Fetches up to 500 repos accessible by the PAT (personal + org, sorted by updated).
async fn fetch_all_user_repos(pat: &str) -> Result<Vec<String>, String> {
    let mut repos = Vec::new();
    for page in 1u32..=5 {
        let url = format!(
            "https://api.github.com/user/repos?per_page=100&page={page}\
             &affiliation=owner,collaborator,organization_member&sort=updated"
        );
        match gh_get::<Vec<GhRepoItem>>(pat, &url).await {
            Err(e) if page == 1 => return Err(e),
            Err(_) => break,
            Ok(batch) => {
                let done = batch.len() < 100;
                repos.extend(batch.into_iter().map(|r| r.full_name));
                if done {
                    break;
                }
            }
        }
    }
    Ok(repos)
}

// ── Route: GET /repos ─────────────────────────────────────────────────

#[derive(Serialize)]
struct ReposResp {
    repos: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

async fn repos_handler(State(config): State<ConfigRef>) -> impl IntoResponse {
    let pat = config.read().unwrap().github_pat.clone();
    if pat.is_empty() {
        return Json(ReposResp {
            repos: vec![],
            error: Some("PAT not configured — save a PAT first".to_string()),
        })
        .into_response();
    }
    match fetch_all_user_repos(&pat).await {
        Ok(repos) => Json(ReposResp { repos, error: None }).into_response(),
        Err(e) => Json(ReposResp { repos: vec![], error: Some(e) }).into_response(),
    }
}

// ── Route: GET /validate-pat ──────────────────────────────────────────

#[derive(Deserialize)]
struct PatQuery {
    github_pat: String,
}

async fn validate_pat_handler(Query(q): Query<PatQuery>) -> impl IntoResponse {
    let pat = q.github_pat.trim();
    if pat.is_empty() {
        return Html(String::new());
    }
    match github_username_for_pat(pat).await {
        Some(user) => Html(format!(
            r#"<span class="pat-ok">&#10003; {user}</span>"#
        )),
        None => Html(
            r#"<span class="pat-err">&#10007; Invalid or insufficient permissions</span>"#
                .to_string(),
        ),
    }
}

// ── Route: GET / ──────────────────────────────────────────────────────

pub async fn serve(config: ConfigRef, port: u16) {
    let app = Router::new()
        .route("/", get(ui_handler))
        .route("/config", post(save_handler))
        .route("/repos", get(repos_handler))
        .route("/validate-pat", get(validate_pat_handler))
        .with_state(config);

    let addr = format!("0.0.0.0:{port}");
    eprintln!("web_config: listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await
        .expect("web_config: failed to bind");
    axum::serve(listener, app).await
        .expect("web_config: server error");
}

async fn ui_handler(State(config): State<ConfigRef>) -> impl IntoResponse {
    let cfg  = config.read().unwrap().clone();
    let pat  = &cfg.github_pat;
    let user = &cfg.github_username;
    let poll = cfg.github_poll_secs;
    let current_repos_json = serde_json::to_string(&cfg.github_repos)
        .unwrap_or_else(|_| "[]".to_string());

    let beszel_url      = &cfg.beszel_url;
    let vps_name        = &cfg.vps_name;
    let vps_hostname    = &cfg.vps_hostname;
    let vps_ip          = &cfg.vps_ip;
    let probe_host      = &cfg.probe_host;
    let probe_port      = cfg.probe_port;
    let nas_hostname    = &cfg.nas_hostname;
    let nas_ip          = &cfg.nas_ip;
    let ha_hostname     = &cfg.ha_hostname;
    let ha_ip           = &cfg.ha_ip;
    let fan_serial_port = &cfg.fan_serial_port;
    let fan_temp_warn_c = cfg.fan_temp_warn_c;
    let fan_temp_crit_c = cfg.fan_temp_crit_c;

    Html(format!(r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>SpaceLab HUD — Config</title>
<script src="https://cdn.jsdelivr.net/npm/htmx.org@1.9.12/dist/htmx.min.js"></script>
<script defer src="https://cdn.jsdelivr.net/npm/alpinejs@3.14.1/dist/cdn.min.js"></script>
<script>
function repoChooser() {{
  const currentRepos = JSON.parse(document.getElementById('current-repos').textContent);
  return {{
    repos:    [],
    selected: [...currentRepos],
    search:   '',
    loading:  true,
    error:    '',

    get filteredRepos() {{
      const q = this.search.toLowerCase();
      return q ? this.repos.filter(r => r.toLowerCase().includes(q)) : this.repos;
    }},

    async init() {{
      try {{
        const resp = await fetch('/repos');
        const data = await resp.json();
        this.repos = data.repos || [];
        for (const r of currentRepos) {{
          if (!this.repos.includes(r)) this.repos.push(r);
        }}
        this.error = data.error || '';
      }} catch (_) {{
        this.error = 'Failed to load repos';
        this.repos = [...currentRepos];
      }}
      this.loading = false;
    }},

    addManual() {{
      const input = this.$refs.manualInput;
      const val   = input.value.trim();
      if (!val || !val.includes('/')) return;
      if (!this.repos.includes(val))     this.repos.unshift(val);
      if (!this.selected.includes(val))  this.selected.push(val);
      input.value = '';
    }},
  }};
}}
</script>
<style>
  * {{ box-sizing: border-box; margin: 0; padding: 0; }}
  body {{ background: #0a0e14; color: #d0e8ff; font-family: monospace; padding: 32px; }}
  h1 {{ color: #00b4d8; letter-spacing: 3px; font-size: 18px; margin-bottom: 24px; }}
  h2 {{ color: #00b4d8; letter-spacing: 2px; font-size: 13px; margin: 28px 0 14px;
    padding-bottom: 6px; border-bottom: 1px solid #1e2d4a; }}
  .field-row {{ display: flex; gap: 12px; }}
  .field-row > label {{ flex: 1; }}
  form > label {{ display: block; margin-bottom: 16px; }}
  .field-label {{ display: block; color: #3e6ea0; font-size: 12px; letter-spacing: 2px; margin-bottom: 6px; }}
  input, textarea {{ width: 100%; background: #0f1828; border: 1px solid #1e2d4a;
    color: #d0e8ff; font-family: monospace; font-size: 14px; padding: 10px;
    border-radius: 4px; outline: none; }}
  input:focus, textarea:focus {{ border-color: #00b4d8; }}
  button[type="submit"] {{ margin-top: 24px; background: #00b4d8; color: #0a0e14; border: none;
    padding: 12px 28px; font-family: monospace; font-size: 14px; letter-spacing: 2px;
    cursor: pointer; border-radius: 4px; }}
  button[type="submit"]:hover {{ background: #48cae4; }}
  .hint {{ color: #2a4060; font-size: 11px; margin-top: 6px; }}
  .pat-help {{ background: #0f1828; border: 1px solid #1e2d4a; border-radius: 4px;
    padding: 12px 14px; margin-top: 8px; font-size: 12px; line-height: 1.7; }}
  .pat-help b {{ color: #00b4d8; }}
  .pat-help li {{ margin-left: 16px; color: #3e6ea0; }}
  .pat-ok  {{ color: #00c875; font-size: 12px; }}
  .pat-err {{ color: #e05050; font-size: 12px; }}
  /* Repo chooser */
  .repo-chooser {{ margin-bottom: 16px; }}
  #repo-list {{ background: #0f1828; border: 1px solid #1e2d4a; border-radius: 4px;
    height: 220px; overflow-y: auto; margin-top: 8px; }}
  .repo-item {{ display: flex; align-items: center; gap: 8px;
    padding: 6px 12px; cursor: pointer; font-size: 13px; margin-bottom: 0; }}
  .repo-item:hover {{ background: #0d1b2e; }}
  .repo-item input[type="checkbox"] {{ width: auto; flex-shrink: 0; cursor: pointer; }}
  .repo-status {{ font-size: 12px; color: #3e6ea0; padding: 10px 12px; }}
  .repo-warning {{ font-size: 11px; color: #c8a800; padding: 4px 12px; }}
  .repo-footer {{ display: flex; justify-content: space-between;
    align-items: center; margin-top: 6px; }}
  .repo-search {{ margin-top: 8px; }}
  .add-repo-row {{ display: flex; gap: 8px; margin-top: 8px; }}
  .add-repo-row input {{ flex: 1; }}
  .add-btn {{ width: auto; margin-top: 0; padding: 10px 16px; background: #1e3a5f;
    color: #d0e8ff; border: 1px solid #1e2d4a; font-family: monospace;
    font-size: 12px; letter-spacing: 1px; cursor: pointer; border-radius: 4px; }}
  .add-btn:hover {{ background: #2a4e7a; }}
</style>
</head>
<body>
<h1>SPACELAB-1 // CONFIG</h1>
<form method="post" action="/config">

  <h2>GITHUB</h2>

  <label>
    <span class="field-label">GITHUB PAT</span>
    <input type="password" name="github_pat" id="github_pat" value="{pat}"
      placeholder="ghp_&#8230;" autocomplete="off"
      hx-get="/validate-pat"
      hx-trigger="change"
      hx-target="#pat-status"
      hx-include="#github_pat">
    <div id="pat-status"></div>
    <div class="pat-help">
      <b>Fine-grained PAT</b> (recommended) &#8212; Settings &#8594; Developer settings &#8594; Fine-grained tokens<br>
      Set <b>Repository access</b> to your watched repos, then enable:<br>
      <ul>
        <li><b>Actions</b> &#8594; Read-only &nbsp;(CI run status)</li>
        <li><b>Issues</b> &#8594; Read-only &nbsp;(issues + comments)</li>
        <li><b>Pull requests</b> &#8594; Read-only &nbsp;(PRs + comments)</li>
        <li><b>Metadata</b> &#8594; Read-only &nbsp;(auto-selected, mandatory)</li>
      </ul>
      <b>Classic PAT</b> &#8212; scope <b>public_repo</b> (public) or <b>repo</b> (private)
    </div>
  </label>

  <label>
    <span class="field-label">GITHUB USERNAME</span>
    <input type="text" name="github_username" value="{user}" placeholder="your-username">
    <p class="hint">Used to distinguish your activity from others</p>
  </label>

  <div class="repo-chooser" x-data="repoChooser()" x-init="init()">
    <span class="field-label">REPOS TO WATCH</span>
    <input class="repo-search" type="text" x-model="search" placeholder="Filter repos&#8230;">
    <div id="repo-list">
      <div class="repo-status" x-show="loading">Loading repos&#8230;</div>
      <div class="repo-status" x-show="!loading && error && repos.length === 0" x-text="error"></div>
      <div class="repo-warning" x-show="!loading && error && repos.length > 0" x-text="'&#9888; ' + error"></div>
      <template x-for="repo in filteredRepos" :key="repo">
        <label class="repo-item">
          <input type="checkbox" :value="repo" x-model="selected">
          <span x-text="repo"></span>
        </label>
      </template>
      <div class="repo-status"
        x-show="!loading && filteredRepos.length === 0 && repos.length > 0">
        No repos match filter
      </div>
    </div>
    <div class="repo-footer">
      <span class="hint" x-text="selected.length + ' selected'"></span>
      <span class="hint">Not seeing a repo? Grant the PAT access, save, and reload.</span>
    </div>
    <div class="add-repo-row">
      <input type="text" x-ref="manualInput" placeholder="owner/repo (manual add)"
        @keydown.enter.prevent="addManual()">
      <button type="button" class="add-btn" @click="addManual()">ADD</button>
    </div>
    <textarea name="github_repos" style="display:none"
      x-effect="$el.value = selected.join('\n')"></textarea>
  </div>

  <label>
    <span class="field-label">POLL INTERVAL (seconds, min 30)</span>
    <input type="number" name="github_poll_secs" value="{poll}" min="30" max="3600">
  </label>

  <h2>VPS / BESZEL</h2>

  <label>
    <span class="field-label">BESZEL URL</span>
    <input type="text" name="beszel_url" value="{beszel_url}" placeholder="http://host:8090">
    <p class="hint">Base URL of the Beszel monitoring instance. Credentials stay in the BESZEL_ADMIN_EMAIL / BESZEL_ADMIN_PASSWORD env vars.</p>
  </label>

  <label>
    <span class="field-label">BESZEL SYSTEM NAME</span>
    <input type="text" name="vps_name" value="{vps_name}" placeholder="spacevps">
    <p class="hint">System name as registered in Beszel (used to look up the record)</p>
  </label>

  <div class="field-row">
    <label>
      <span class="field-label">VPS HOSTNAME</span>
      <input type="text" name="vps_hostname" value="{vps_hostname}" placeholder="spacevps">
    </label>
    <label>
      <span class="field-label">VPS IP / ADDRESS</span>
      <input type="text" name="vps_ip" value="{vps_ip}" placeholder="host.example.net">
    </label>
  </div>

  <h2>NETWORK PROBE</h2>

  <div class="field-row">
    <label>
      <span class="field-label">PROBE HOST</span>
      <input type="text" name="probe_host" value="{probe_host}" placeholder="host.example.net">
    </label>
    <label>
      <span class="field-label">PROBE PORT</span>
      <input type="number" name="probe_port" value="{probe_port}" min="1" max="65535">
    </label>
  </div>
  <p class="hint">A successful TCP connect here lights the LOCAL NETWORK indicator.</p>

  <h2>NAS</h2>

  <div class="field-row">
    <label>
      <span class="field-label">NAS HOSTNAME</span>
      <input type="text" name="nas_hostname" value="{nas_hostname}" placeholder="nas-01">
    </label>
    <label>
      <span class="field-label">NAS IP / ADDRESS</span>
      <input type="text" name="nas_ip" value="{nas_ip}" placeholder="192.168.1.20">
    </label>
  </div>

  <h2>HOME ASSISTANT</h2>

  <div class="field-row">
    <label>
      <span class="field-label">HA HOSTNAME</span>
      <input type="text" name="ha_hostname" value="{ha_hostname}" placeholder="homeassistant.local">
    </label>
    <label>
      <span class="field-label">HA IP / ADDRESS</span>
      <input type="text" name="ha_ip" value="{ha_ip}" placeholder="192.168.1.30">
    </label>
  </div>

  <h2>FAN CONTROLLER</h2>

  <label>
    <span class="field-label">SERIAL PORT</span>
    <input type="text" name="fan_serial_port" value="{fan_serial_port}" placeholder="/dev/ttyACM0">
  </label>

  <div class="field-row">
    <label>
      <span class="field-label">TEMP WARN (°C)</span>
      <input type="number" name="fan_temp_warn_c" value="{fan_temp_warn_c}" min="0" max="120" step="0.5">
    </label>
    <label>
      <span class="field-label">TEMP CRIT (°C)</span>
      <input type="number" name="fan_temp_crit_c" value="{fan_temp_crit_c}" min="0" max="120" step="0.5">
    </label>
  </div>

  <button type="submit">SAVE CONFIG</button>
</form>
<script type="application/json" id="current-repos">{current_repos_json}</script>
</body>
</html>"##))
}

// ── Route: POST /config ───────────────────────────────────────────────

#[derive(Deserialize)]
struct ConfigForm {
    github_pat:       String,
    github_username:  String,
    github_repos:     String,
    github_poll_secs: u64,

    beszel_url:    String,
    vps_name:      String,
    vps_hostname:  String,
    vps_ip:        String,

    probe_host:    String,
    probe_port:    u16,

    nas_hostname:  String,
    nas_ip:        String,

    ha_hostname:   String,
    ha_ip:         String,

    fan_serial_port: String,
    fan_temp_warn_c: f32,
    fan_temp_crit_c: f32,
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

    let username = if form.github_username.trim().is_empty() && !pat.is_empty() {
        github_username_for_pat(&pat).await.unwrap_or_default()
    } else {
        form.github_username.trim().to_string()
    };

    // web_config_port has no form field — preserve the running value.
    let web_config_port = config.read().unwrap().web_config_port;

    let new_cfg = AppConfig {
        github_pat:       pat,
        github_username:  username,
        github_repos:     repos,
        github_poll_secs: form.github_poll_secs.max(30),
        web_config_port,

        beszel_url:    form.beszel_url.trim().trim_end_matches('/').to_string(),
        vps_name:      form.vps_name.trim().to_string(),
        vps_hostname:  form.vps_hostname.trim().to_string(),
        vps_ip:        form.vps_ip.trim().to_string(),

        probe_host:    form.probe_host.trim().to_string(),
        probe_port:    form.probe_port,

        nas_hostname:  form.nas_hostname.trim().to_string(),
        nas_ip:        form.nas_ip.trim().to_string(),

        ha_hostname:   form.ha_hostname.trim().to_string(),
        ha_ip:         form.ha_ip.trim().to_string(),

        fan_serial_port: form.fan_serial_port.trim().to_string(),
        fan_temp_warn_c: form.fan_temp_warn_c,
        fan_temp_crit_c: form.fan_temp_crit_c,
    };

    if let Err(e) = new_cfg.save() {
        eprintln!("web_config: failed to save config: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    *config.write().unwrap() = new_cfg;
    Redirect::to("/").into_response()
}
