use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect},
    routing::{get, post},
    Form,
};
use serde::{Deserialize, Serialize};

use crate::config::{ConfigRef, GithubSource};

/// Minimal HTML-attribute escaper for values interpolated into the config
/// page. The form values are LAN-trusted, but escaping keeps a stray `"` or
/// `<` in a saved field from breaking out of its attribute (or injecting
/// script) on the next page render.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

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

/// Lists the repos reachable by the PAT passed in the query string. Each source
/// in the config UI has its own chooser, so the PAT comes from the (possibly
/// unsaved) form field rather than from the stored config.
async fn repos_handler(Query(q): Query<PatQuery>) -> impl IntoResponse {
    let pat = q.github_pat.trim();
    if pat.is_empty() {
        return Json(ReposResp {
            repos: vec![],
            error: Some("Enter a PAT first".to_string()),
        })
        .into_response();
    }
    match fetch_all_user_repos(pat).await {
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
    // Escape every user-controlled string before it lands in an HTML attribute.
    let poll = cfg.github_poll_secs;
    // Seed the per-source chooser. Embedded in a <script type="application/json">
    // block (not an attribute), so JSON encoding of the quotes is the escaping.
    // serde_json doesn't escape `/`, so defensively neutralize any `</script>`
    // breakout: GitHub PATs can't contain it today, but a hand-edited config
    // could. `<\/` is a valid JSON escape that JSON.parse restores to `</`.
    let sources_json = serde_json::to_string(&cfg.github_sources)
        .unwrap_or_else(|_| "[]".to_string())
        .replace("</", "<\\/");

    let beszel_url      = esc(&cfg.beszel_url);
    let beszel_email    = esc(&cfg.beszel_email);
    // Signals to the password field whether a secret is already stored, so the
    // placeholder can say "leave blank to keep current" instead of echoing it.
    let beszel_pw_set   = if cfg.beszel_password.is_empty() { "" } else { "saved — leave blank to keep" };
    let vps_name        = esc(&cfg.vps_name);
    let vps_hostname    = esc(&cfg.vps_hostname);
    let vps_ip          = esc(&cfg.vps_ip);
    let probe_host      = esc(&cfg.probe_host);
    let probe_port      = cfg.probe_port;
    let nas_name        = esc(&cfg.nas_name);
    let nas_hostname    = esc(&cfg.nas_hostname);
    let nas_ip          = esc(&cfg.nas_ip);
    let ha_name         = esc(&cfg.ha_name);
    let ha_hostname     = esc(&cfg.ha_hostname);
    let ha_ip           = esc(&cfg.ha_ip);
    let fan_serial_port = esc(&cfg.fan_serial_port);
    let fan_temp_warn_c = cfg.fan_temp_warn_c;
    let fan_temp_crit_c = cfg.fan_temp_crit_c;

    Html(format!(r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>SpaceLab HUD — Config</title>
<script defer src="https://cdn.jsdelivr.net/npm/alpinejs@3.14.1/dist/cdn.min.js"></script>
<script>
function blankSource() {{
  return {{ pat:'', username:'', repos:[], available:[], search:'', loading:false, error:'', status:'' }};
}}

function configApp() {{
  const initial = JSON.parse(document.getElementById('initial-sources').textContent);
  const sources = initial.length
    ? initial.map(s => ({{ ...blankSource(), pat: s.pat || '', username: s.username || '', repos: s.repos || [] }}))
    : [ blankSource() ];

  return {{
    sources,

    init() {{
      // Pre-load the repo list for any source that already carries a PAT.
      for (const src of this.sources) if (src.pat) this.loadRepos(src);
    }},

    addSource()      {{ this.sources.push(blankSource()); }},
    removeSource(i)  {{ this.sources.splice(i, 1); if (!this.sources.length) this.addSource(); }},

    async validatePat(src) {{
      src.status = '';
      if (!src.pat) {{ src.available = []; return; }}
      try {{
        const resp = await fetch('/validate-pat?github_pat=' + encodeURIComponent(src.pat));
        src.status = await resp.text();
      }} catch (_) {{ src.status = ''; }}
      this.loadRepos(src);
    }},

    async loadRepos(src) {{
      if (!src.pat) {{ src.error = 'Enter a PAT first'; src.available = []; return; }}
      src.loading = true; src.error = '';
      try {{
        const resp = await fetch('/repos?github_pat=' + encodeURIComponent(src.pat));
        const data = await resp.json();
        src.available = data.repos || [];
        for (const r of src.repos) if (!src.available.includes(r)) src.available.push(r);
        src.error = data.error || '';
      }} catch (_) {{
        src.error = 'Failed to load repos';
        src.available = [...src.repos];
      }}
      src.loading = false;
    }},

    filtered(src) {{
      const q = src.search.toLowerCase();
      return q ? src.available.filter(r => r.toLowerCase().includes(q)) : src.available;
    }},

    addManual(src, val) {{
      val = (val || '').trim();
      if (!val || !val.includes('/')) return;
      if (!src.available.includes(val)) src.available.unshift(val);
      if (!src.repos.includes(val))     src.repos.push(val);
    }},

    get payload() {{
      return JSON.stringify(this.sources.map(s => ({{
        pat: s.pat, username: s.username, repos: s.repos
      }})));
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
  /* GitHub source blocks */
  .gh-source {{ border: 1px solid #1e2d4a; border-radius: 6px; padding: 16px;
    margin-bottom: 16px; background: #0c1420; }}
  .gh-source-head {{ display: flex; justify-content: space-between;
    align-items: center; margin-bottom: 12px; }}
  .rm-btn {{ width: auto; margin: 0; padding: 6px 12px; background: #3a1e1e;
    color: #e0a0a0; border: 1px solid #4a1e1e; font-family: monospace;
    font-size: 11px; letter-spacing: 1px; cursor: pointer; border-radius: 4px; }}
  .rm-btn:hover {{ background: #5a2a2a; }}
  /* Repo chooser */
  .repo-chooser {{ margin-bottom: 0; }}
  .repo-list {{ background: #0f1828; border: 1px solid #1e2d4a; border-radius: 4px;
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
<form method="post" action="/config" x-data="configApp()" x-init="init()">

  <h2>GITHUB</h2>
  <p class="hint" style="margin-bottom:14px">
    One source per resource owner. A fine-grained PAT only reaches a single org or
    account, so add a separate source for each org you want to watch.
  </p>

  <div class="pat-help">
    <b>Fine-grained PAT</b> (recommended) &#8212; Settings &#8594; Developer settings &#8594; Fine-grained tokens<br>
    Set <b>Repository access</b> to your watched repos, then enable:<br>
    <ul>
      <li><b>Actions</b> &#8594; Read-only &nbsp;(CI run status)</li>
      <li><b>Issues</b> &#8594; Read-only &nbsp;(issues + comments)</li>
      <li><b>Pull requests</b> &#8594; Read-only &nbsp;(PRs + comments)</li>
      <li><b>Metadata</b> &#8594; Read-only &nbsp;(auto-selected, mandatory)</li>
    </ul>
    <b>Classic PAT</b> &#8212; scope <b>public_repo</b> (public) or <b>repo</b> (private).
    Spans multiple orgs only if each org has SSO-authorized the token.
  </div>

  <template x-for="(src, i) in sources" :key="i">
    <div class="gh-source">
      <div class="gh-source-head">
        <span class="field-label" x-text="'SOURCE ' + (i + 1)"></span>
        <button type="button" class="rm-btn" @click="removeSource(i)" x-show="sources.length > 1">REMOVE</button>
      </div>

      <label>
        <span class="field-label">GITHUB PAT</span>
        <input type="password" x-model="src.pat" placeholder="ghp_&#8230;" autocomplete="off"
          @change="validatePat(src)">
        <div x-html="src.status"></div>
      </label>

      <label>
        <span class="field-label">GITHUB USERNAME</span>
        <input type="text" x-model="src.username" placeholder="auto-detected from PAT if blank">
        <p class="hint">Used to distinguish your activity from others on this account</p>
      </label>

      <div class="repo-chooser">
        <span class="field-label">REPOS TO WATCH</span>
        <input class="repo-search" type="text" x-model="src.search" placeholder="Filter repos&#8230;">
        <div class="repo-list">
          <div class="repo-status" x-show="src.loading">Loading repos&#8230;</div>
          <div class="repo-status" x-show="!src.loading && src.error && src.available.length === 0" x-text="src.error"></div>
          <div class="repo-warning" x-show="!src.loading && src.error && src.available.length > 0" x-text="'&#9888; ' + src.error"></div>
          <template x-for="repo in filtered(src)" :key="repo">
            <label class="repo-item">
              <input type="checkbox" :value="repo" x-model="src.repos">
              <span x-text="repo"></span>
            </label>
          </template>
          <div class="repo-status"
            x-show="!src.loading && filtered(src).length === 0 && src.available.length > 0">
            No repos match filter
          </div>
        </div>
        <div class="repo-footer">
          <span class="hint" x-text="src.repos.length + ' selected'"></span>
          <button type="button" class="add-btn" @click="loadRepos(src)">RELOAD</button>
        </div>
        <div class="add-repo-row">
          <input type="text" placeholder="owner/repo (manual add)"
            @keydown.enter.prevent="addManual(src, $event.target.value); $event.target.value=''">
          <button type="button" class="add-btn"
            @click="addManual(src, $event.target.previousElementSibling.value); $event.target.previousElementSibling.value=''">ADD</button>
        </div>
      </div>
    </div>
  </template>

  <button type="button" class="add-btn" @click="addSource()">+ ADD SOURCE</button>
  <textarea name="github_sources" style="display:none" x-effect="$el.value = payload"></textarea>

  <label>
    <span class="field-label">POLL INTERVAL (seconds, min 30)</span>
    <input type="number" name="github_poll_secs" value="{poll}" min="30" max="3600">
  </label>

  <h2>VPS / BESZEL</h2>

  <label>
    <span class="field-label">BESZEL URL</span>
    <input type="text" name="beszel_url" value="{beszel_url}" placeholder="http://host:8090">
    <p class="hint">Base URL of the Beszel monitoring instance. All monitored systems (VPS, NAS, HA) share this one instance.</p>
  </label>

  <div class="field-row">
    <label>
      <span class="field-label">BESZEL ADMIN EMAIL</span>
      <input type="text" name="beszel_email" value="{beszel_email}" placeholder="admin@example.com" autocomplete="off">
    </label>
    <label>
      <span class="field-label">BESZEL ADMIN PASSWORD</span>
      <input type="password" name="beszel_password" value="" placeholder="{beszel_pw_set}" autocomplete="off">
      <p class="hint">Superuser credentials used to obtain an API token. Leave password blank to keep the stored one.</p>
    </label>
  </div>

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

  <label>
    <span class="field-label">BESZEL SYSTEM NAME</span>
    <input type="text" name="nas_name" value="{nas_name}" placeholder="nas-01">
    <p class="hint">System name as registered in the same Beszel instance. Leave blank to keep the panel offline.</p>
  </label>

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

  <label>
    <span class="field-label">BESZEL SYSTEM NAME</span>
    <input type="text" name="ha_name" value="{ha_name}" placeholder="homeassistant">
    <p class="hint">System name as registered in the same Beszel instance. Leave blank to keep the panel offline.</p>
  </label>

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
<script type="application/json" id="initial-sources">{sources_json}</script>
</body>
</html>"##))
}

// ── Route: POST /config ───────────────────────────────────────────────

#[derive(Deserialize)]
struct ConfigForm {
    /// JSON array of `{pat, username, repos}` objects, emitted by the config UI.
    github_sources:   String,
    github_poll_secs: u64,

    beszel_url:      String,
    beszel_email:    String,
    beszel_password: String,
    vps_name:      String,
    vps_hostname:  String,
    vps_ip:        String,

    probe_host:    String,
    probe_port:    u16,

    nas_name:      String,
    nas_hostname:  String,
    nas_ip:        String,

    ha_name:       String,
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
    // Parse the source list emitted by the UI, then sanitize each entry:
    // drop sources with no PAT, keep only well-formed `owner/repo` names, and
    // auto-fill a blank username from the PAT's authenticated login.
    let parsed: Vec<GithubSource> =
        serde_json::from_str(&form.github_sources).unwrap_or_default();
    let mut github_sources = Vec::with_capacity(parsed.len());
    for src in parsed {
        let pat = src.pat.trim().to_string();
        if pat.is_empty() {
            continue;
        }
        let repos: Vec<String> = src.repos
            .into_iter()
            .map(|r| r.trim().to_string())
            .filter(|r| !r.is_empty() && r.contains('/'))
            .collect();
        let username = if src.username.trim().is_empty() {
            github_username_for_pat(&pat).await.unwrap_or_default()
        } else {
            src.username.trim().to_string()
        };
        github_sources.push(GithubSource { pat, username, repos });
    }

    // Start from the running config so web_config_port and any future
    // non-form fields are preserved, then overlay the submitted values.
    let mut new_cfg = config.read().unwrap().clone();
    new_cfg.github_sources   = github_sources;
    new_cfg.github_poll_secs = form.github_poll_secs.max(30);

    new_cfg.beszel_url   = form.beszel_url.trim().trim_end_matches('/').to_string();
    new_cfg.beszel_email = form.beszel_email.trim().to_string();
    // A blank password field means "keep the stored secret" — so we only
    // overwrite when the user actually typed a new one. (The page never echoes
    // the saved password back into the form.)
    let beszel_password = form.beszel_password.trim();
    if !beszel_password.is_empty() {
        new_cfg.beszel_password = beszel_password.to_string();
    }
    new_cfg.vps_name     = form.vps_name.trim().to_string();
    new_cfg.vps_hostname = form.vps_hostname.trim().to_string();
    new_cfg.vps_ip       = form.vps_ip.trim().to_string();

    new_cfg.probe_host = form.probe_host.trim().to_string();
    new_cfg.probe_port = form.probe_port;

    new_cfg.nas_name     = form.nas_name.trim().to_string();
    new_cfg.nas_hostname = form.nas_hostname.trim().to_string();
    new_cfg.nas_ip       = form.nas_ip.trim().to_string();

    new_cfg.ha_name     = form.ha_name.trim().to_string();
    new_cfg.ha_hostname = form.ha_hostname.trim().to_string();
    new_cfg.ha_ip       = form.ha_ip.trim().to_string();

    new_cfg.fan_serial_port = form.fan_serial_port.trim().to_string();
    // Keep warn <= crit so the threshold branches in fan_telem.rs can't
    // straddle an inverted range (which would silently fire neither).
    new_cfg.fan_temp_warn_c = form.fan_temp_warn_c.min(form.fan_temp_crit_c);
    new_cfg.fan_temp_crit_c = form.fan_temp_crit_c.max(form.fan_temp_warn_c);

    if let Err(e) = new_cfg.save() {
        eprintln!("web_config: failed to save config: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    *config.write().unwrap() = new_cfg;
    Redirect::to("/").into_response()
}
