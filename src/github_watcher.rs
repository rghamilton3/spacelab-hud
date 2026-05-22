use std::collections::HashSet;

use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;
use tokio::time::{sleep, Duration};

use crate::config::ConfigRef;
use crate::shared_state::{AlertData, SharedAlertsRef, push_alerts_to_ui};

// ── GitHub API response types ─────────────────────────────────────────

#[derive(Deserialize)]
struct GhUser {
    login: String,
}

#[derive(Deserialize)]
struct GhPr {
    id:         u64,
    number:     u32,
    title:      String,
    user:       GhUser,
    created_at: String,
}

#[derive(Deserialize)]
struct GhIssue {
    id:           u64,
    number:       u32,
    title:        String,
    user:         GhUser,
    created_at:   String,
    // GitHub returns PRs in the issues endpoint; absent on real issues.
    pull_request: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct GhWorkflowRun {
    id:         u64,
    name:       String,
    status:     String,
    conclusion: Option<String>,
    created_at: String,
}

#[derive(Deserialize)]
struct GhWorkflowRunsPage {
    workflow_runs: Vec<GhWorkflowRun>,
}

#[derive(Deserialize)]
struct GhComment {
    id:         u64,
    user:       GhUser,
    body:       String,
    created_at: String,
    issue_url:  String,
}

// ── Internal state ────────────────────────────────────────────────────

#[derive(Default)]
struct SeenIds {
    prs:      HashSet<u64>,
    issues:   HashSet<u64>,
    ci_runs:  HashSet<u64>,
    comments: HashSet<u64>,
}

struct RepoPoll {
    open_prs:     Vec<GhPr>,
    open_issues:  Vec<GhIssue>,
    latest_run:   Option<GhWorkflowRun>,
    new_comments: Vec<GhComment>,
}

// ── GitHub API helpers ────────────────────────────────────────────────

fn build_client(pat: &str) -> reqwest::Result<Client> {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        format!("Bearer {pat}").parse().unwrap(),
    );
    headers.insert(
        reqwest::header::ACCEPT,
        "application/vnd.github+json".parse().unwrap(),
    );
    headers.insert(
        reqwest::header::HeaderName::from_static("x-github-api-version"),
        "2022-11-28".parse().unwrap(),
    );
    Client::builder()
        .user_agent("spacelab-hud/1.0")
        .default_headers(headers)
        .timeout(Duration::from_secs(10))
        .build()
}

async fn fetch_open_prs(client: &Client, repo: &str) -> Option<Vec<GhPr>> {
    let url = format!("https://api.github.com/repos/{repo}/pulls?state=open&per_page=50");
    let resp = client.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        eprintln!("github: PR fetch failed for {repo}: {}", resp.status());
        return None;
    }
    resp.json().await.ok()
}

async fn fetch_open_issues(client: &Client, repo: &str) -> Option<Vec<GhIssue>> {
    let url = format!("https://api.github.com/repos/{repo}/issues?state=open&per_page=50");
    let resp = client.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        eprintln!("github: issue fetch failed for {repo}: {}", resp.status());
        return None;
    }
    let all: Vec<GhIssue> = resp.json().await.ok()?;
    Some(all.into_iter().filter(|i| i.pull_request.is_none()).collect())
}

async fn fetch_latest_run(client: &Client, repo: &str) -> Option<GhWorkflowRun> {
    let url = format!(
        "https://api.github.com/repos/{repo}/actions/runs?per_page=5"
    );
    let resp = client.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        eprintln!("github: CI fetch failed for {repo}: {}", resp.status());
        return None;
    }
    let page: GhWorkflowRunsPage = resp.json().await.ok()?;
    page.workflow_runs.into_iter().find(|r| r.status == "completed")
}

async fn fetch_recent_comments(client: &Client, repo: &str, since: &str) -> Vec<GhComment> {
    let url = format!(
        "https://api.github.com/repos/{repo}/issues/comments?since={since}&per_page=50&sort=created"
    );
    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(e) => { eprintln!("github: comment fetch failed for {repo}: {e}"); return vec![]; }
    };
    if !resp.status().is_success() { return vec![]; }
    resp.json().await.unwrap_or_default()
}

// ── Per-repo concurrent poll ──────────────────────────────────────────

async fn poll_repo(client: &Client, repo: &str, since: &str) -> Option<RepoPoll> {
    let (prs, issues, run, comments) = tokio::join!(
        fetch_open_prs(client, repo),
        fetch_open_issues(client, repo),
        fetch_latest_run(client, repo),
        fetch_recent_comments(client, repo, since),
    );
    Some(RepoPoll {
        open_prs:     prs?,
        open_issues:  issues?,
        latest_run:   run,
        new_comments: comments,
    })
}

// ── Helpers ───────────────────────────────────────────────────────────

fn format_age(created_at: &str) -> String {
    let Ok(dt) = DateTime::parse_from_rfc3339(created_at) else {
        return "?".to_string();
    };
    let secs = Utc::now().signed_duration_since(dt.with_timezone(&Utc)).num_seconds();
    if secs < 60         { format!("{secs}s") }
    else if secs < 3600  { format!("{}m", secs / 60) }
    else if secs < 86400 { format!("{}h", secs / 3600) }
    else                 { format!("{}d", secs / 86400) }
}

fn ci_level(run: &GhWorkflowRun) -> i32 {
    match run.conclusion.as_deref() {
        Some("success")                   => 0,
        Some("failure") | Some("timed_out") => 2,
        _                                 => 1,
    }
}

fn ci_status_str(run: &GhWorkflowRun) -> String {
    match run.conclusion.as_deref() {
        Some("success")   => "PASSING".to_string(),
        Some("failure")   => "FAILING".to_string(),
        Some("timed_out") => "TIMEOUT".to_string(),
        Some("cancelled") => "CANCELLED".to_string(),
        Some(s)           => s.to_uppercase(),
        None if run.status == "in_progress" => "RUNNING".to_string(),
        None              => "PENDING".to_string(),
    }
}

/// Build the GitHub issues API URL for a PR/issue by number.
/// GitHub's comments endpoint uses the issues URL even for PR comments.
fn issue_api_url(repo: &str, number: u32) -> String {
    format!("https://api.github.com/repos/{repo}/issues/{number}")
}

// ── Main watcher loop ─────────────────────────────────────────────────

pub async fn run(
    ui_weak:       slint::Weak<crate::AppWindow>,
    config_ref:    ConfigRef,
    shared_alerts: SharedAlertsRef,
) {
    let mut seen        = SeenIds::default();
    let mut events: std::collections::VecDeque<crate::GithubEvent> = Default::default();
    let mut last_pat    = String::new();
    let mut client: Option<Client> = None;
    let mut first_cycle = true;
    let mut since       = Utc::now().to_rfc3339();

    loop {
        let cfg           = config_ref.read().await.clone();
        let username_lower = cfg.github_username.to_lowercase();

        if !cfg.is_configured() {
            push_github_state(&ui_weak, vec![], vec![], 0, 0, false, false);
            sleep(Duration::from_secs(cfg.github_poll_secs)).await;
            continue;
        }

        if cfg.github_pat != last_pat {
            client = build_client(&cfg.github_pat).ok();
            last_pat = cfg.github_pat.clone();
            seen = SeenIds::default();
            first_cycle = true;
        }

        let Some(ref cl) = client else {
            sleep(Duration::from_secs(cfg.github_poll_secs)).await;
            continue;
        };

        let cycle_since = since.clone();
        since = Utc::now().to_rfc3339();

        let polls: Vec<_> = cfg.github_repos.iter()
            .map(|repo| {
                let cl    = cl.clone();
                let repo  = repo.clone();
                let since = cycle_since.clone();
                tokio::spawn(async move {
                    let result = poll_repo(&cl, &repo, &since).await;
                    (repo, result)
                })
            })
            .collect();

        let results: Vec<(String, Option<RepoPoll>)> = futures::future::join_all(polls)
            .await
            .into_iter()
            .filter_map(|r| r.ok())
            .collect();

        let reachable        = results.iter().any(|(_, r)| r.is_some());
        let mut repo_states  = Vec::<crate::GithubRepo>::new();
        let mut new_gh_alerts = Vec::<AlertData>::new();
        let mut total_prs    = 0i32;
        let mut total_issues = 0i32;

        for (repo_name, poll) in &results {
            let Some(poll) = poll else {
                repo_states.push(crate::GithubRepo {
                    name:        repo_name.clone().into(),
                    pr_count:    0,
                    issue_count: 0,
                    ci_status:   "UNKNOWN".into(),
                    ci_level:    1,
                });
                continue;
            };

            // ── Build set of MY open issue/PR URLs for comment filtering ─
            let mut my_issue_urls: HashSet<String> = HashSet::new();
            for pr in &poll.open_prs {
                if pr.user.login.to_lowercase() == username_lower {
                    my_issue_urls.insert(issue_api_url(repo_name, pr.number));
                }
            }
            for issue in &poll.open_issues {
                if issue.user.login.to_lowercase() == username_lower {
                    my_issue_urls.insert(issue_api_url(repo_name, issue.number));
                }
            }

            // ── New PRs by others ────────────────────────────────────────
            for pr in &poll.open_prs {
                let is_new = seen.prs.insert(pr.id);
                if !is_new || first_cycle { continue; }
                if pr.user.login.to_lowercase() == username_lower { continue; }

                push_event(&mut events, crate::GithubEvent {
                    repo:   repo_name.clone().into(),
                    kind:   "PR".into(),
                    title:  pr.title.clone().into(),
                    author: pr.user.login.clone().into(),
                    age:    format_age(&pr.created_at).into(),
                    level:  1,
                });
                new_gh_alerts.push(AlertData {
                    name:     format!("GH PR #{}", pr.number),
                    severity: "warning".into(),
                    age:      format_age(&pr.created_at),
                    summary:  format!("{}: {}", pr.user.login, pr.title),
                });
            }

            // ── New issues by others ─────────────────────────────────────
            for issue in &poll.open_issues {
                let is_new = seen.issues.insert(issue.id);
                if !is_new || first_cycle { continue; }
                if issue.user.login.to_lowercase() == username_lower { continue; }

                push_event(&mut events, crate::GithubEvent {
                    repo:   repo_name.clone().into(),
                    kind:   "ISSUE".into(),
                    title:  issue.title.clone().into(),
                    author: issue.user.login.clone().into(),
                    age:    format_age(&issue.created_at).into(),
                    level:  1,
                });
                new_gh_alerts.push(AlertData {
                    name:     format!("GH ISSUE #{}", issue.number),
                    severity: "warning".into(),
                    age:      format_age(&issue.created_at),
                    summary:  format!("{}: {}", issue.user.login, issue.title),
                });
            }

            // ── CI completion transitions ─────────────────────────────────
            if let Some(run) = &poll.latest_run {
                let is_new = seen.ci_runs.insert(run.id);
                if is_new && !first_cycle {
                    let level = ci_level(run);
                    if level == 0 || level == 2 {
                        push_event(&mut events, crate::GithubEvent {
                            repo:   repo_name.clone().into(),
                            kind:   "CI".into(),
                            title:  format!("{}: {}", run.name, ci_status_str(run)).into(),
                            author: String::new().into(),
                            age:    format_age(&run.created_at).into(),
                            level,
                        });
                        new_gh_alerts.push(AlertData {
                            name:     format!("GH CI {}", repo_name),
                            severity: if level == 2 { "critical".into() } else { "warning".into() },
                            age:      format_age(&run.created_at),
                            summary:  format!("{}: {}", run.name, ci_status_str(run)),
                        });
                    }
                }
            }

            // ── Comments on MY issues/PRs by others ──────────────────────
            for comment in &poll.new_comments {
                let is_new = seen.comments.insert(comment.id);
                if !is_new || first_cycle { continue; }
                if comment.user.login.to_lowercase() == username_lower { continue; }
                // Only alert if the comment is on one of my open issues/PRs
                if !my_issue_urls.contains(&comment.issue_url) { continue; }

                let snippet = comment.body.chars().take(80).collect::<String>();
                push_event(&mut events, crate::GithubEvent {
                    repo:   repo_name.clone().into(),
                    kind:   "COMMENT".into(),
                    title:  snippet.clone().into(),
                    author: comment.user.login.clone().into(),
                    age:    format_age(&comment.created_at).into(),
                    level:  0,
                });
                new_gh_alerts.push(AlertData {
                    name:     format!("GH COMMENT {}", repo_name),
                    severity: "warning".into(),
                    age:      format_age(&comment.created_at),
                    summary:  format!("{}: {}", comment.user.login, snippet),
                });
            }

            // ── Repo card state ───────────────────────────────────────────
            let ci_lvl  = poll.latest_run.as_ref().map(ci_level).unwrap_or(0);
            let ci_str  = poll.latest_run.as_ref().map(ci_status_str)
                              .unwrap_or_else(|| "UNKNOWN".to_string());
            let pr_count  = poll.open_prs.len() as i32;
            let iss_count = poll.open_issues.len() as i32;
            total_prs    += pr_count;
            total_issues += iss_count;

            repo_states.push(crate::GithubRepo {
                name:        repo_name.clone().into(),
                pr_count,
                issue_count: iss_count,
                ci_status:   ci_str.into(),
                ci_level:    ci_lvl,
            });
        }

        if !first_cycle {
            let mut guard = shared_alerts.lock().unwrap();
            guard.github = new_gh_alerts;
            drop(guard);
            push_alerts_to_ui(&ui_weak, &shared_alerts, true);
        }

        push_github_state(
            &ui_weak,
            repo_states,
            events.iter().cloned().collect(),
            total_prs,
            total_issues,
            reachable,
            cfg.is_configured(),
        );

        first_cycle = false;
        sleep(Duration::from_secs(cfg.github_poll_secs)).await;
    }
}

fn push_event(
    events: &mut std::collections::VecDeque<crate::GithubEvent>,
    ev:     crate::GithubEvent,
) {
    events.push_front(ev);
    if events.len() > 20 {
        events.pop_back();
    }
}

fn push_github_state(
    ui_weak:      &slint::Weak<crate::AppWindow>,
    repos:        Vec<crate::GithubRepo>,
    events:       Vec<crate::GithubEvent>,
    total_prs:    i32,
    total_issues: i32,
    reachable:    bool,
    configured:   bool,
) {
    let ui = ui_weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = ui.upgrade() else { return };

        let ci_worst = repos.iter().map(|r| r.ci_level).max().unwrap_or(0);
        let level    = if !configured || !reachable { 1 } else { ci_worst };
        let status   = match level {
            2 => "FAILING",
            1 => "WARN",
            _ => "OK",
        }.to_string();

        ui.set_github_repos(slint::ModelRc::new(slint::VecModel::from(repos)));
        ui.set_github_events(slint::ModelRc::new(slint::VecModel::from(events)));
        ui.set_github_pr_count(total_prs);
        ui.set_github_issue_count(total_issues);
        ui.set_github_level(level);
        ui.set_github_status(status.into());
        ui.set_github_configured(configured);
        ui.set_github_reachable(reachable);
    });
}
