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

    let last_fetch = run_git_opt(&["log", "-1", "--format=%cr", "FETCH_HEAD"]);

    Ok(RepoSummary {
        root,
        branch,
        remote_url,
        upstream,
        ahead,
        behind,
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
        "--archived=false",
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
        "--archived=false",
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
        "name,nameWithOwner,description,pushedAt,defaultBranchRef,primaryLanguage,stargazerCount,isPrivate",
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

#[derive(Debug, Deserialize)]
struct RawApiCommitFull {
    sha: String,
    commit: RawApiCommitFullInner,
    #[serde(default)]
    files: Vec<RawApiCommitFile>,
}

#[derive(Debug, Deserialize)]
struct RawApiCommitFullInner {
    author: RawCommitAuthor,
    message: String,
}

#[derive(Debug, Deserialize)]
struct RawApiCommitFile {
    filename: String,
    additions: u64,
    deletions: u64,
    status: String,
}

pub fn org_commit_detail(owner: &str, repo: &str, sha: &str) -> Result<CommitDetail> {
    let stdout = run_gh(&[
        "api",
        &format!("/repos/{}/{}/commits/{}", owner, repo, sha),
    ])
    .context("gh api commit detail failed")?;
    let raw: RawApiCommitFull =
        serde_json::from_slice(&stdout).context("failed to parse commit detail JSON")?;

    let mut lines = raw.commit.message.lines();
    let subject = lines.next().unwrap_or("").to_string();
    let body: String = lines
        .skip_while(|l| l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string();

    let mut total_add = 0u64;
    let mut total_del = 0u64;
    let mut stat_lines: Vec<String> = raw
        .files
        .iter()
        .map(|f| {
            total_add += f.additions;
            total_del += f.deletions;
            let status = match f.status.as_str() {
                "added" => "+",
                "removed" => "-",
                "renamed" => "R",
                "modified" => "M",
                other => other,
            };
            format!(
                " {} {}  +{} -{}",
                status, f.filename, f.additions, f.deletions
            )
        })
        .collect();
    if !raw.files.is_empty() {
        stat_lines.push(format!(
            " {} files changed, {} insertions(+), {} deletions(-)",
            raw.files.len(),
            total_add,
            total_del
        ));
    }

    Ok(CommitDetail {
        sha: raw.sha,
        author: raw.commit.author.name,
        email: String::new(),
        date: humanize_iso(&raw.commit.author.date),
        subject,
        body,
        stat_lines,
    })
}

#[derive(Debug, Clone)]
pub struct DirtyFile {
    pub status: String,
    pub path: String,
}

pub fn dirty_file_list() -> Result<Vec<DirtyFile>> {
    let out = run_git(&["status", "--porcelain"])?;
    let files = out
        .lines()
        .filter(|l| l.len() >= 3)
        .map(|l| {
            let status = l[..2].to_string();
            let path = l[3..].to_string();
            DirtyFile { status, path }
        })
        .collect();
    Ok(files)
}

pub fn file_diff(path: &str, status: &str) -> Result<Vec<String>> {
    let trimmed = status.trim();
    let output = if trimmed == "??" {
        // Untracked: show full file as a new-add diff.
        Command::new("git")
            .args(["diff", "--no-index", "--no-color", "/dev/null", path])
            .output()
            .with_context(|| format!("git diff --no-index {} failed", path))?
    } else {
        // Tracked: show staged + unstaged changes against HEAD.
        Command::new("git")
            .args(["diff", "HEAD", "--no-color", "--", path])
            .output()
            .with_context(|| format!("git diff HEAD -- {} failed", path))?
    };

    // `git diff --no-index` returns exit code 1 when files differ. That's success
    // for our purposes; only treat non-zero with non-empty stderr as failure.
    if !output.status.success() && output.stdout.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.trim().is_empty() {
            bail!("git diff failed: {}", stderr.trim());
        }
    }

    let body = String::from_utf8_lossy(&output.stdout);
    Ok(body.lines().map(|l| l.to_string()).collect())
}

#[derive(Debug, Clone)]
pub struct ReviewRequestedPr {
    pub number: u64,
    pub title: String,
    pub repo: String,
    pub author: String,
    pub updated_at: String,
    pub is_draft: bool,
}

pub fn review_requested_prs(org: &str, limit: usize) -> Result<Vec<ReviewRequestedPr>> {
    let stdout = run_gh(&[
        "search",
        "prs",
        "--archived=false",
        "--owner",
        org,
        "--review-requested",
        "@me",
        "--state",
        "open",
        "--sort",
        "updated",
        "--limit",
        &limit.to_string(),
        "--json",
        "number,title,author,repository,state,isDraft,updatedAt",
    ])
    .context("gh search review-requested PRs failed")?;
    let raw: Vec<SearchPr> =
        serde_json::from_slice(&stdout).context("failed to parse review-requested PR JSON")?;
    Ok(raw
        .into_iter()
        .map(|p| ReviewRequestedPr {
            number: p.number,
            title: p.title,
            repo: p.repository.name_with_owner,
            author: p.author.map(|a| a.login).unwrap_or_default(),
            is_draft: p.is_draft,
            updated_at: humanize_iso(&p.updated_at),
        })
        .collect())
}

pub fn authed_user_login() -> Result<String> {
    let stdout = run_gh(&["api", "user", "--jq", ".login"]).context("gh api user failed")?;
    let login = String::from_utf8_lossy(&stdout).trim().to_string();
    if login.is_empty() {
        bail!("gh api user returned empty login");
    }
    Ok(login)
}

#[derive(Debug, Deserialize)]
struct RawUserOrg {
    login: String,
}

pub fn resolve_dashboard_org(cwd_org: Option<&str>) -> Result<String> {
    if let Some(o) = cwd_org {
        if !o.is_empty() {
            return Ok(o.to_string());
        }
    }
    let stdout = run_gh(&["api", "/user/orgs"]).context("gh api /user/orgs failed")?;
    let raw: Vec<RawUserOrg> =
        serde_json::from_slice(&stdout).context("failed to parse /user/orgs JSON")?;
    raw.into_iter()
        .next()
        .map(|o| o.login)
        .ok_or_else(|| anyhow::anyhow!("user belongs to no orgs"))
}

pub fn is_git_repo() -> bool {
    run_git(&["rev-parse", "--is-inside-work-tree"]).is_ok()
}

pub fn dashboard_open_prs(limit: usize) -> Result<Vec<UserPr>> {
    let stdout = run_gh(&[
        "search",
        "prs",
        "--archived=false",
        "--author",
        "@me",
        "--state",
        "open",
        "--sort",
        "updated",
        "--limit",
        &limit.to_string(),
        "--json",
        "number,title,author,repository,state,isDraft,updatedAt",
    ])
    .context("gh search my open PRs failed")?;
    let raw: Vec<SearchPr> =
        serde_json::from_slice(&stdout).context("failed to parse my open PR JSON")?;
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

#[derive(Debug, Clone)]
pub struct Notification {
    pub reason: String,
    pub title: String,
    pub repo: String,
    pub kind: String,
    pub unread: bool,
    pub updated_at: String,
    pub web_url: String,
}

#[derive(Debug, Deserialize)]
struct RawNotification {
    reason: String,
    unread: bool,
    updated_at: String,
    subject: RawNotificationSubject,
    repository: RawNotificationRepo,
}

#[derive(Debug, Deserialize)]
struct RawNotificationSubject {
    title: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawNotificationRepo {
    full_name: String,
}

pub fn notifications(limit: usize) -> Result<Vec<Notification>> {
    let stdout = run_gh(&[
        "api",
        &format!("/notifications?per_page={}", limit.min(50)),
    ])
    .context("gh api /notifications failed")?;
    let raw: Vec<RawNotification> =
        serde_json::from_slice(&stdout).context("failed to parse notifications JSON")?;
    Ok(raw
        .into_iter()
        .map(|n| {
            let web_url = notification_web_url(
                n.subject.url.as_deref(),
                &n.subject.kind,
                &n.repository.full_name,
            );
            Notification {
                reason: n.reason,
                title: n.subject.title,
                repo: n.repository.full_name,
                kind: n.subject.kind,
                unread: n.unread,
                updated_at: humanize_iso(&n.updated_at),
                web_url,
            }
        })
        .collect())
}

fn notification_web_url(api_url: Option<&str>, kind: &str, repo: &str) -> String {
    // Subject URLs come back as `https://api.github.com/repos/X/Y/<resource>/<id>`
    // Convert to the user-facing github.com equivalent.
    if let Some(api) = api_url {
        if let Some(rest) = api.strip_prefix("https://api.github.com/repos/") {
            let converted = rest
                .replacen("/pulls/", "/pull/", 1)
                .replacen("/commits/", "/commit/", 1);
            return format!("https://github.com/{}", converted);
        }
    }
    // Fallback: just the repo page if we couldn't parse the subject URL.
    let _ = kind;
    format!("https://github.com/{}", repo)
}

pub fn my_recent_commits(login: &str, limit: usize) -> Result<Vec<UserCommit>> {
    let stdout = run_gh(&[
        "search",
        "commits",
        "--author",
        login,
        "--sort",
        "author-date",
        "--order",
        "desc",
        "--limit",
        &limit.to_string(),
        "--json",
        "sha,commit,repository",
    ])
    .context("gh search my commits failed")?;
    let raw: Vec<RawSearchCommit> =
        serde_json::from_slice(&stdout).context("failed to parse my commits JSON")?;
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
pub struct PrDetail {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub url: String,
    pub author: String,
    pub state: String,
    pub is_draft: bool,
    pub head_ref: String,
    pub base_ref: String,
    pub additions: u64,
    pub deletions: u64,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
struct RawPrDetail {
    number: u64,
    title: String,
    body: String,
    url: String,
    author: Option<SearchAuthor>,
    state: String,
    #[serde(rename = "isDraft", default)]
    is_draft: bool,
    #[serde(rename = "headRefName")]
    head_ref_name: String,
    #[serde(rename = "baseRefName")]
    base_ref_name: String,
    #[serde(default)]
    additions: u64,
    #[serde(default)]
    deletions: u64,
    #[serde(rename = "updatedAt")]
    updated_at: String,
}

pub fn pr_detail(owner: &str, repo: &str, number: u64) -> Result<PrDetail> {
    let repo_arg = format!("{}/{}", owner, repo);
    let num_arg = number.to_string();
    let stdout = run_gh(&[
        "pr",
        "view",
        &num_arg,
        "--repo",
        &repo_arg,
        "--json",
        "number,title,body,url,author,state,isDraft,headRefName,baseRefName,additions,deletions,updatedAt",
    ])
    .context("gh pr view failed")?;
    let raw: RawPrDetail =
        serde_json::from_slice(&stdout).context("failed to parse PR detail JSON")?;
    Ok(PrDetail {
        number: raw.number,
        title: raw.title,
        body: raw.body,
        url: raw.url,
        author: raw.author.map(|a| a.login).unwrap_or_default(),
        state: raw.state,
        is_draft: raw.is_draft,
        head_ref: raw.head_ref_name,
        base_ref: raw.base_ref_name,
        additions: raw.additions,
        deletions: raw.deletions,
        updated_at: humanize_iso(&raw.updated_at),
    })
}

pub fn open_in_browser(url: &str) -> Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "cmd"
    } else {
        "xdg-open"
    };
    let mut cmd = Command::new(opener);
    if cfg!(target_os = "windows") {
        cmd.args(["/C", "start", "", url]);
    } else {
        cmd.arg(url);
    }
    let status = cmd
        .status()
        .with_context(|| format!("failed to launch {} for {}", opener, url))?;
    if !status.success() {
        bail!("{} exited with status {}", opener, status);
    }
    Ok(())
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
        "--archived=false",
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
        "--archived=false",
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
