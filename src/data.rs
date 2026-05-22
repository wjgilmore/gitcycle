use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct RepoSummary {
    pub root: String,
    pub branch: String,
    pub remote_url: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub dirty_files: u32,
    pub last_fetch: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Commit {
    pub sha: String,
    pub author: String,
    pub date: String,
    pub subject: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub author: PrAuthor,
    #[serde(rename = "headRefName")]
    pub head: String,
    #[serde(rename = "isDraft")]
    pub is_draft: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PrAuthor {
    pub login: String,
}

fn run_git(args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .with_context(|| format!("failed to execute git {:?}", args))?;
    if !output.status.success() {
        bail!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn run_git_opt(args: &[&str]) -> Option<String> {
    run_git(args).ok().filter(|s| !s.is_empty())
}

pub fn repo_summary() -> Result<RepoSummary> {
    let root = run_git(&["rev-parse", "--show-toplevel"])?;
    let branch = run_git(&["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_else(|_| "HEAD".into());
    let remote_url = run_git_opt(&["config", "--get", "remote.origin.url"]);
    let upstream = run_git_opt(&["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"]);

    let (ahead, behind) = if upstream.is_some() {
        match run_git(&["rev-list", "--left-right", "--count", "HEAD...@{u}"]) {
            Ok(s) => {
                let mut parts = s.split_whitespace();
                let a = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                let b = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                (a, b)
            }
            Err(_) => (0, 0),
        }
    } else {
        (0, 0)
    };

    let dirty_files = run_git(&["status", "--porcelain"])
        .map(|s| s.lines().filter(|l| !l.is_empty()).count() as u32)
        .unwrap_or(0);

    let last_fetch = run_git_opt(&["log", "-1", "--format=%cr", "FETCH_HEAD"]);

    Ok(RepoSummary {
        root,
        branch,
        remote_url,
        upstream,
        ahead,
        behind,
        dirty_files,
        last_fetch,
    })
}

pub fn recent_commits(limit: usize) -> Result<Vec<Commit>> {
    let fmt = "%h%x1f%an%x1f%cr%x1f%s";
    let out = run_git(&[
        "log",
        &format!("-n{}", limit),
        &format!("--pretty=format:{}", fmt),
    ])?;
    let commits = out
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\u{1f}');
            Some(Commit {
                sha: parts.next()?.to_string(),
                author: parts.next()?.to_string(),
                date: parts.next()?.to_string(),
                subject: parts.next()?.to_string(),
            })
        })
        .collect();
    Ok(commits)
}

pub fn open_pull_requests(limit: usize) -> Result<Vec<PullRequest>> {
    let stdout = run_gh(&[
        "pr",
        "list",
        "--state",
        "open",
        "--limit",
        &limit.to_string(),
        "--json",
        "number,title,author,headRefName,isDraft",
    ])
    .context("gh pr list failed")?;
    let prs: Vec<PullRequest> =
        serde_json::from_slice(&stdout).context("failed to parse gh pr list JSON")?;
    Ok(prs)
}

fn run_gh(args: &[&str]) -> Result<Vec<u8>> {
    let max_attempts = 3;
    let mut delay_ms: u64 = 300;
    let mut last_err: Option<String> = None;

    for attempt in 1..=max_attempts {
        let output = Command::new("gh")
            .args(args)
            .output()
            .with_context(|| format!("failed to execute gh {:?}", args))?;
        if output.status.success() {
            return Ok(output.stdout);
        }
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        if attempt < max_attempts && is_transient(&stderr) {
            sleep(Duration::from_millis(delay_ms));
            delay_ms = delay_ms.saturating_mul(3);
            last_err = Some(stderr);
            continue;
        }
        bail!(stderr.trim().to_string());
    }
    bail!(last_err.unwrap_or_else(|| "gh failed".into()));
}

fn is_transient(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    s.contains("http 5")
        || s.contains("http 429")
        || s.contains("http 408")
        || s.contains("timeout")
        || s.contains("timed out")
        || s.contains("connection reset")
        || s.contains("connection refused")
        || s.contains("temporarily unavailable")
}

pub fn detect_org() -> Result<String> {
    let url = run_git(&["config", "--get", "remote.origin.url"])
        .context("no origin remote configured")?;
    parse_org_from_url(&url)
        .ok_or_else(|| anyhow::anyhow!("could not parse owner from remote URL: {}", url))
}

fn parse_org_from_url(url: &str) -> Option<String> {
    let s = url.trim().trim_end_matches(".git");
    if let Some(rest) = s.strip_prefix("git@github.com:") {
        return rest.split('/').next().map(|s| s.to_string());
    }
    if let Some(rest) = s.strip_prefix("ssh://git@github.com/") {
        return rest.split('/').next().map(|s| s.to_string());
    }
    for prefix in ["https://github.com/", "http://github.com/", "github.com/"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            return rest.split('/').next().map(|s| s.to_string());
        }
    }
    None
}

#[derive(Debug, Clone)]
pub struct UserActivity {
    pub login: String,
    pub events: Vec<ActivityEvent>,
}

#[derive(Debug, Clone)]
pub struct ActivityEvent {
    pub kind: String,
    pub repo: String,
    pub detail: String,
    pub when: String,
}

#[derive(Debug, Deserialize)]
struct SearchPr {
    number: u64,
    title: String,
    author: Option<SearchAuthor>,
    repository: SearchRepo,
    state: String,
    #[serde(rename = "isDraft", default)]
    is_draft: bool,
    #[serde(rename = "updatedAt")]
    updated_at: String,
}

#[derive(Debug, Deserialize)]
struct SearchIssue {
    number: u64,
    title: String,
    author: Option<SearchAuthor>,
    repository: SearchRepo,
    state: String,
    #[serde(rename = "updatedAt")]
    updated_at: String,
}

#[derive(Debug, Deserialize)]
struct SearchAuthor {
    #[serde(default)]
    login: String,
}

#[derive(Debug, Deserialize)]
struct SearchRepo {
    #[serde(rename = "nameWithOwner")]
    name_with_owner: String,
}

pub fn org_activity(org: &str, per_user_limit: usize) -> Result<Vec<UserActivity>> {
    let pr_stdout = run_gh(&[
        "search",
        "prs",
        "--owner",
        org,
        "--sort",
        "updated",
        "--limit",
        "100",
        "--json",
        "number,title,author,repository,state,isDraft,updatedAt",
    ])
    .context("gh search prs failed")?;
    let prs: Vec<SearchPr> =
        serde_json::from_slice(&pr_stdout).context("failed to parse PR search JSON")?;

    let issue_stdout = run_gh(&[
        "search",
        "issues",
        "--owner",
        org,
        "--sort",
        "updated",
        "--limit",
        "100",
        "--json",
        "number,title,author,repository,state,updatedAt",
    ])
    .context("gh search issues failed")?;
    let issues: Vec<SearchIssue> =
        serde_json::from_slice(&issue_stdout).context("failed to parse issue search JSON")?;

    let mut grouped: BTreeMap<String, Vec<ActivityEvent>> = BTreeMap::new();

    for pr in prs {
        let login = match &pr.author {
            Some(a) if !a.login.is_empty() => a.login.clone(),
            _ => continue,
        };
        let kind = if pr.is_draft { "PR-draft" } else { "PR" };
        let action = match pr.state.as_str() {
            "OPEN" | "open" => "active",
            "MERGED" | "merged" => "merged",
            "CLOSED" | "closed" => "closed",
            other => other,
        };
        let detail = format!("{} #{}: {}", action, pr.number, pr.title);
        grouped.entry(login).or_default().push(ActivityEvent {
            kind: kind.to_string(),
            repo: pr.repository.name_with_owner,
            detail,
            when: pr.updated_at,
        });
    }

    for issue in issues {
        let login = match &issue.author {
            Some(a) if !a.login.is_empty() => a.login.clone(),
            _ => continue,
        };
        let action = match issue.state.as_str() {
            "OPEN" | "open" => "open",
            "CLOSED" | "closed" => "closed",
            other => other,
        };
        let detail = format!("{} #{}: {}", action, issue.number, issue.title);
        grouped.entry(login).or_default().push(ActivityEvent {
            kind: "issue".to_string(),
            repo: issue.repository.name_with_owner,
            detail,
            when: issue.updated_at,
        });
    }

    // Sort each user's events newest first, then humanize timestamps and truncate.
    let mut users: Vec<UserActivity> = grouped
        .into_iter()
        .map(|(login, mut events)| {
            events.sort_by(|a, b| b.when.cmp(&a.when));
            events.truncate(per_user_limit);
            for ev in &mut events {
                ev.when = humanize_iso(&ev.when);
            }
            UserActivity { login, events }
        })
        .collect();

    users.sort_by(|a, b| b.events.len().cmp(&a.events.len()).then(a.login.cmp(&b.login)));
    Ok(users)
}

fn humanize_iso(ts: &str) -> String {
    if ts.len() >= 16 && ts.as_bytes().get(10) == Some(&b'T') {
        format!("{} {}", &ts[..10], &ts[11..16])
    } else {
        ts.to_string()
    }
}

#[derive(Debug, Clone)]
pub struct RepoInfo {
    pub name: String,
    pub full_name: String,
    pub description: Option<String>,
    pub pushed_at: Option<String>,
    pub default_branch: Option<String>,
    pub primary_language: Option<String>,
    pub stargazer_count: u64,
    pub is_archived: bool,
    pub is_private: bool,
}

#[derive(Debug, Deserialize)]
struct RawRepoListItem {
    name: String,
    #[serde(rename = "nameWithOwner")]
    name_with_owner: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(rename = "pushedAt", default)]
    pushed_at: Option<String>,
    #[serde(rename = "defaultBranchRef", default)]
    default_branch_ref: Option<RawDefaultBranch>,
    #[serde(rename = "primaryLanguage", default)]
    primary_language: Option<RawLanguage>,
    #[serde(rename = "stargazerCount", default)]
    stargazer_count: u64,
    #[serde(rename = "isArchived", default)]
    is_archived: bool,
    #[serde(rename = "isPrivate", default)]
    is_private: bool,
}

#[derive(Debug, Deserialize)]
struct RawDefaultBranch {
    name: String,
}

#[derive(Debug, Deserialize)]
struct RawLanguage {
    name: String,
}

pub fn org_repos(org: &str) -> Result<Vec<RepoInfo>> {
    let stdout = run_gh(&[
        "repo",
        "list",
        org,
        "--limit",
        "500",
        "--no-archived",
        "--json",
        "name,nameWithOwner,description,pushedAt,defaultBranchRef,primaryLanguage,stargazerCount,isArchived,isPrivate",
    ])
    .context("gh repo list failed")?;

    let raw: Vec<RawRepoListItem> =
        serde_json::from_slice(&stdout).context("failed to parse gh repo list JSON")?;

    let mut repos: Vec<RepoInfo> = raw
        .into_iter()
        .map(|r| RepoInfo {
            name: r.name,
            full_name: r.name_with_owner,
            description: r.description,
            pushed_at: r.pushed_at,
            default_branch: r.default_branch_ref.map(|b| b.name),
            primary_language: r.primary_language.map(|l| l.name),
            stargazer_count: r.stargazer_count,
            is_archived: r.is_archived,
            is_private: r.is_private,
        })
        .collect();

    // Most recently pushed first.
    repos.sort_by(|a, b| {
        b.pushed_at
            .as_deref()
            .unwrap_or("")
            .cmp(a.pushed_at.as_deref().unwrap_or(""))
    });
    Ok(repos)
}

#[derive(Debug, Clone)]
pub struct Contributor {
    pub login: String,
    pub contributions: u64,
}

#[derive(Debug, Deserialize)]
struct RawContributor {
    login: String,
    contributions: u64,
}

#[derive(Debug, Deserialize)]
struct RawApiCommit {
    sha: String,
    commit: RawCommitInner,
    author: Option<RawApiAuthor>,
}

#[derive(Debug, Deserialize)]
struct RawCommitInner {
    author: RawCommitAuthor,
    message: String,
}

#[derive(Debug, Deserialize)]
struct RawCommitAuthor {
    name: String,
    date: String,
}

#[derive(Debug, Deserialize)]
struct RawApiAuthor {
    login: String,
}

pub fn repo_recent_commits(owner: &str, repo: &str, limit: usize) -> Result<Vec<Commit>> {
    let stdout = run_gh(&[
        "api",
        &format!("/repos/{}/{}/commits?per_page={}", owner, repo, limit),
    ])
    .context("gh api commits failed")?;

    let raw: Vec<RawApiCommit> =
        serde_json::from_slice(&stdout).context("failed to parse commits JSON")?;
    Ok(raw
        .into_iter()
        .map(|c| {
            let subject = c
                .commit
                .message
                .lines()
                .next()
                .unwrap_or("")
                .to_string();
            let short_sha: String = c.sha.chars().take(7).collect();
            let author = c
                .author
                .map(|a| a.login)
                .unwrap_or(c.commit.author.name);
            Commit {
                sha: short_sha,
                author,
                date: humanize_iso(&c.commit.author.date),
                subject,
            }
        })
        .collect())
}

pub fn repo_recent_prs(owner: &str, repo: &str, limit: usize) -> Result<Vec<PullRequest>> {
    let stdout = run_gh(&[
        "pr",
        "list",
        "--repo",
        &format!("{}/{}", owner, repo),
        "--state",
        "all",
        "--limit",
        &limit.to_string(),
        "--json",
        "number,title,author,headRefName,isDraft",
    ])
    .context("gh pr list (repo) failed")?;
    let prs: Vec<PullRequest> =
        serde_json::from_slice(&stdout).context("failed to parse repo PR JSON")?;
    Ok(prs)
}

pub fn repo_top_contributors(
    owner: &str,
    repo: &str,
    limit: usize,
) -> Result<Vec<Contributor>> {
    let stdout = run_gh(&[
        "api",
        &format!("/repos/{}/{}/contributors?per_page={}", owner, repo, limit),
    ])
    .context("gh api contributors failed")?;
    // GitHub returns an object with a `message` field on errors-like-204 (empty repo).
    // Try array first; if it fails, return empty.
    let raw: Vec<RawContributor> = match serde_json::from_slice(&stdout) {
        Ok(v) => v,
        Err(_) => Vec::new(),
    };
    Ok(raw
        .into_iter()
        .map(|c| Contributor {
            login: c.login,
            contributions: c.contributions,
        })
        .collect())
}

#[derive(Debug, Clone)]
pub struct CommitDetail {
    pub sha: String,
    pub author: String,
    pub email: String,
    pub date: String,
    pub subject: String,
    pub body: String,
    pub stat_lines: Vec<String>,
}

pub fn commit_detail(sha: &str) -> Result<CommitDetail> {
    let fmt = "%H%x1f%an%x1f%ae%x1f%aI%x1f%s%x1f%b";
    let meta = run_git(&["show", "-s", &format!("--format={}", fmt), sha])
        .context("git show metadata failed")?;
    let mut parts = meta.split('\u{1f}');
    let full_sha = parts.next().unwrap_or("").to_string();
    let author = parts.next().unwrap_or("").to_string();
    let email = parts.next().unwrap_or("").to_string();
    let date_iso = parts.next().unwrap_or("").to_string();
    let subject = parts.next().unwrap_or("").to_string();
    let body = parts.next().unwrap_or("").trim_end().to_string();

    let stat = run_git(&["show", "--stat", "--format=", sha])
        .context("git show --stat failed")?;
    let stat_lines: Vec<String> = stat
        .lines()
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .collect();

    Ok(CommitDetail {
        sha: full_sha,
        author,
        email,
        date: humanize_iso(&date_iso),
        subject,
        body,
        stat_lines,
    })
}

pub fn split_full_name(full: &str) -> Option<(String, String)> {
    let mut parts = full.splitn(2, '/');
    let owner = parts.next()?.to_string();
    let repo = parts.next()?.to_string();
    if owner.is_empty() || repo.is_empty() {
        None
    } else {
        Some((owner, repo))
    }
}

#[derive(Debug, Clone)]
pub struct OrgMember {
    pub login: String,
}

#[derive(Debug, Deserialize)]
struct RawOrgMember {
    login: String,
}

pub fn org_members(org: &str) -> Result<Vec<OrgMember>> {
    let stdout = run_gh(&[
        "api",
        "--paginate",
        &format!("/orgs/{}/members?per_page=100", org),
    ])
    .map_err(|e| {
        let msg = format!("{:#}", e);
        if msg.contains("Not Found") || msg.contains("HTTP 404") {
            anyhow::anyhow!("'{}' is not a GitHub organization (or members are not visible)", org)
        } else {
            e
        }
    })?;

    // `--paginate` on a list endpoint concatenates page arrays. If multiple pages
    // ran, the stream looks like "][" between pages.
    let body = String::from_utf8_lossy(&stdout).replace("][", ",");
    let raw: Vec<RawOrgMember> =
        serde_json::from_str(&body).context("failed to parse org members JSON")?;
    let mut members: Vec<OrgMember> = raw
        .into_iter()
        .map(|m| OrgMember { login: m.login })
        .collect();
    members.sort_by(|a, b| a.login.to_ascii_lowercase().cmp(&b.login.to_ascii_lowercase()));
    Ok(members)
}

#[derive(Debug, Clone)]
pub struct UserCommit {
    pub sha: String,
    pub repo: String,
    pub subject: String,
    pub date: String,
}

#[derive(Debug, Deserialize)]
struct RawSearchCommit {
    sha: String,
    commit: RawSearchCommitInner,
    repository: CommitSearchRepo,
}

#[derive(Debug, Deserialize)]
struct CommitSearchRepo {
    #[serde(rename = "fullName")]
    full_name: String,
}

#[derive(Debug, Deserialize)]
struct RawSearchCommitInner {
    author: RawSearchCommitAuthor,
    message: String,
}

#[derive(Debug, Deserialize)]
struct RawSearchCommitAuthor {
    date: String,
}

pub fn user_recent_commits(org: &str, user: &str, limit: usize) -> Result<Vec<UserCommit>> {
    let stdout = run_gh(&[
        "search",
        "commits",
        "--owner",
        org,
        "--author",
        user,
        "--sort",
        "author-date",
        "--order",
        "desc",
        "--limit",
        &limit.to_string(),
        "--json",
        "sha,commit,repository",
    ])
    .context("gh search commits failed")?;
    let raw: Vec<RawSearchCommit> =
        serde_json::from_slice(&stdout).context("failed to parse user commits JSON")?;
    Ok(raw
        .into_iter()
        .map(|c| {
            let subject = c.commit.message.lines().next().unwrap_or("").to_string();
            let short_sha: String = c.sha.chars().take(7).collect();
            UserCommit {
                sha: short_sha,
                repo: c.repository.full_name,
                subject,
                date: humanize_iso(&c.commit.author.date),
            }
        })
        .collect())
}

#[derive(Debug, Clone)]
pub struct UserPr {
    pub number: u64,
    pub title: String,
    pub repo: String,
    pub state: String,
    pub is_draft: bool,
    pub author: String,
    pub updated_at: String,
}

fn fetch_user_prs(args: &[&str]) -> Result<Vec<UserPr>> {
    let stdout = run_gh(args).context("gh search prs failed")?;
    let raw: Vec<SearchPr> =
        serde_json::from_slice(&stdout).context("failed to parse user PR JSON")?;
    Ok(raw
        .into_iter()
        .map(|p| UserPr {
            number: p.number,
            title: p.title,
            repo: p.repository.name_with_owner,
            state: p.state,
            is_draft: p.is_draft,
            author: p.author.map(|a| a.login).unwrap_or_default(),
            updated_at: humanize_iso(&p.updated_at),
        })
        .collect())
}

pub fn user_submitted_prs(org: &str, user: &str, limit: usize) -> Result<Vec<UserPr>> {
    fetch_user_prs(&[
        "search",
        "prs",
        "--owner",
        org,
        "--author",
        user,
        "--sort",
        "updated",
        "--limit",
        &limit.to_string(),
        "--json",
        "number,title,author,repository,state,isDraft,updatedAt",
    ])
}

pub fn user_reviewed_prs(org: &str, user: &str, limit: usize) -> Result<Vec<UserPr>> {
    fetch_user_prs(&[
        "search",
        "prs",
        "--owner",
        org,
        "--reviewed-by",
        user,
        "--sort",
        "updated",
        "--limit",
        &limit.to_string(),
        "--json",
        "number,title,author,repository,state,isDraft,updatedAt",
    ])
}
