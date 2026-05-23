mod data;

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Tabs, Wrap},
};
use std::{
    io,
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

use data::{
    Commit, CommitDetail, Contributor, DirtyFile, Notification, OrgMember, PrDetail, PullRequest,
    RepoInfo, RepoSummary, ReviewRequestedPr, UserActivity, UserCommit, UserPr,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    Dashboard,
    Repo,
    Org,
    RepoDetail,
    CommitDetail,
    UserDetail,
    PrDetail,
    NotificationDetail,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DetailOrigin {
    Repo,
    Dashboard,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OrgSubview {
    Activity,
    Repos,
    Users,
}

struct RepoListState {
    repos: Vec<RepoInfo>,
    error: Option<String>,
    loaded: bool,
    filter: String,
    filtering: bool,
    list_state: ListState,
}

impl RepoListState {
    fn new() -> Self {
        Self {
            repos: Vec::new(),
            error: None,
            loaded: false,
            filter: String::new(),
            filtering: false,
            list_state: ListState::default(),
        }
    }
}

struct UserListState {
    members: Vec<OrgMember>,
    error: Option<String>,
    loaded: bool,
    filter: String,
    filtering: bool,
    list_state: ListState,
}

impl UserListState {
    fn new() -> Self {
        Self {
            members: Vec::new(),
            error: None,
            loaded: false,
            filter: String::new(),
            filtering: false,
            list_state: ListState::default(),
        }
    }
}

struct UserDetailState {
    login: String,
    commits: Vec<UserCommit>,
    commits_err: Option<String>,
    submitted_prs: Vec<UserPr>,
    submitted_err: Option<String>,
    reviewed_prs: Vec<UserPr>,
    reviewed_err: Option<String>,
}

struct OrgState {
    name: Option<String>,
    detect_err: Option<String>,
    activity: Vec<UserActivity>,
    activity_err: Option<String>,
    activity_loaded: bool,
    subview: OrgSubview,
    repos: RepoListState,
    users: UserListState,
}

struct RepoDetailState {
    full_name: String,
    info: Option<RepoInfo>,
    commits: Vec<Commit>,
    commits_err: Option<String>,
    prs: Vec<PullRequest>,
    prs_err: Option<String>,
    contributors: Vec<Contributor>,
    contributors_err: Option<String>,
    loaded: bool,
}

struct App {
    screen: Screen,
    in_git_repo: bool,
    dashboard_org: Option<String>,
    dashboard_org_err: Option<String>,
    authed_user: Option<String>,
    summary: Option<RepoSummary>,
    summary_err: Option<String>,
    commits: Vec<Commit>,
    commits_err: Option<String>,
    commits_focused: bool,
    commits_list_state: ListState,
    commit_detail: Option<CommitDetailView>,
    prs: Vec<PullRequest>,
    prs_err: Option<String>,
    dirty_files: Vec<DirtyFile>,
    dirty_files_err: Option<String>,
    dashboard: DashboardState,
    org: OrgState,
    org_scroll: u16,
    repo_detail: Option<RepoDetailState>,
    user_detail: Option<UserDetailState>,
    pr_detail: Option<PrDetailView>,
    notification_detail: Option<NotificationDetailView>,
    bg_rx: Option<Receiver<BgMessage>>,
}

struct DashboardState {
    review_prs: Vec<ReviewRequestedPr>,
    review_prs_err: Option<String>,
    review_prs_loaded: bool,
    review_focused: bool,
    review_list_state: ListState,
    my_prs: Vec<UserPr>,
    my_prs_err: Option<String>,
    my_prs_loaded: bool,
    my_prs_focused: bool,
    my_prs_list_state: ListState,
    notifications: Vec<Notification>,
    notifications_err: Option<String>,
    notifications_loaded: bool,
    notifications_focused: bool,
    notifications_list_state: ListState,
    my_commits: Vec<UserCommit>,
    my_commits_err: Option<String>,
    my_commits_loaded: bool,
    my_commits_focused: bool,
    my_commits_list_state: ListState,
}

struct PrDetailView {
    detail: Option<PrDetail>,
    error: Option<String>,
    scroll: u16,
}

impl DashboardState {
    fn new() -> Self {
        Self {
            review_prs: Vec::new(),
            review_prs_err: None,
            review_prs_loaded: false,
            review_focused: false,
            review_list_state: ListState::default(),
            my_prs: Vec::new(),
            my_prs_err: None,
            my_prs_loaded: false,
            my_prs_focused: false,
            my_prs_list_state: ListState::default(),
            notifications: Vec::new(),
            notifications_err: None,
            notifications_loaded: false,
            notifications_focused: false,
            notifications_list_state: ListState::default(),
            my_commits: Vec::new(),
            my_commits_err: None,
            my_commits_loaded: false,
            my_commits_focused: false,
            my_commits_list_state: ListState::default(),
        }
    }

    fn any_focused(&self) -> bool {
        self.review_focused
            || self.my_prs_focused
            || self.notifications_focused
            || self.my_commits_focused
    }

    fn unfocus_all(&mut self) {
        self.review_focused = false;
        self.my_prs_focused = false;
        self.notifications_focused = false;
        self.my_commits_focused = false;
    }
}

struct CommitDetailView {
    detail: Option<CommitDetail>,
    error: Option<String>,
    scroll: u16,
    origin: DetailOrigin,
}

struct NotificationDetailView {
    notification: Notification,
}

enum BgMessage {
    Activity(Result<Vec<UserActivity>, String>),
    Repos(Result<Vec<RepoInfo>, String>),
    Members(Result<Vec<OrgMember>, String>),
    Review(Result<Vec<ReviewRequestedPr>, String>),
    MyPrs(Result<Vec<UserPr>, String>),
    Notifications(Result<Vec<Notification>, String>),
    MyCommits(Result<Vec<UserCommit>, String>),
}

fn spawn_bg_prefetches(
    org: Option<String>,
    dashboard_org: Option<String>,
    authed_user: Option<String>,
) -> Receiver<BgMessage> {
    let (tx, rx) = mpsc::channel();

    // Org-scoped (only if cwd org is known)
    if let Some(o) = org.clone() {
        let tx_act = tx.clone();
        let oa = o.clone();
        thread::spawn(move || {
            let result = data::org_activity(&oa, 8).map_err(|e| format!("{:#}", e));
            let _ = tx_act.send(BgMessage::Activity(result));
        });

        let tx_repos = tx.clone();
        let or = o.clone();
        thread::spawn(move || {
            let result = data::org_repos(&or).map_err(|e| format!("{:#}", e));
            let _ = tx_repos.send(BgMessage::Repos(result));
        });

        let tx_mem = tx.clone();
        thread::spawn(move || {
            let result = data::org_members(&o).map_err(|e| format!("{:#}", e));
            let _ = tx_mem.send(BgMessage::Members(result));
        });
    }

    // Dashboard: awaiting-review is org-scoped (use dashboard_org if available)
    if let Some(d_org) = dashboard_org {
        let tx_rev = tx.clone();
        thread::spawn(move || {
            let result =
                data::review_requested_prs(&d_org, 15).map_err(|e| format!("{:#}", e));
            let _ = tx_rev.send(BgMessage::Review(result));
        });
    }

    // Dashboard: my open PRs (global)
    let tx_mp = tx.clone();
    thread::spawn(move || {
        let result = data::dashboard_open_prs(15).map_err(|e| format!("{:#}", e));
        let _ = tx_mp.send(BgMessage::MyPrs(result));
    });

    // Dashboard: notifications (global)
    let tx_no = tx.clone();
    thread::spawn(move || {
        let result = data::notifications(20).map_err(|e| format!("{:#}", e));
        let _ = tx_no.send(BgMessage::Notifications(result));
    });

    // Dashboard: my recent commits (needs authed user login)
    if let Some(login) = authed_user {
        thread::spawn(move || {
            let result = data::my_recent_commits(&login, 15).map_err(|e| format!("{:#}", e));
            let _ = tx.send(BgMessage::MyCommits(result));
        });
    }

    rx
}

impl App {
    fn load_repo() -> Self {
        let in_git_repo = data::is_git_repo();

        let (summary, summary_err, commits, commits_err, prs, prs_err, dirty_files, dirty_files_err) =
            if in_git_repo {
                let (summary, summary_err) = match data::repo_summary() {
                    Ok(s) => (Some(s), None),
                    Err(e) => (None, Some(format!("{:#}", e))),
                };
                let (commits, commits_err) = match data::recent_commits(15) {
                    Ok(c) => (c, None),
                    Err(e) => (vec![], Some(format!("{:#}", e))),
                };
                let (prs, prs_err) = match data::open_pull_requests(10) {
                    Ok(p) => (p, None),
                    Err(e) => (vec![], Some(format!("{:#}", e))),
                };
                let (dirty_files, dirty_files_err) = match data::dirty_file_list() {
                    Ok(d) => (d, None),
                    Err(e) => (vec![], Some(format!("{:#}", e))),
                };
                (
                    summary,
                    summary_err,
                    commits,
                    commits_err,
                    prs,
                    prs_err,
                    dirty_files,
                    dirty_files_err,
                )
            } else {
                (None, None, vec![], None, vec![], None, vec![], None)
            };

        let (name, detect_err) = if in_git_repo {
            match data::detect_org() {
                Ok(o) => (Some(o), None),
                Err(e) => (None, Some(format!("{:#}", e))),
            }
        } else {
            (None, None)
        };

        let (dashboard_org, dashboard_org_err) =
            match data::resolve_dashboard_org(name.as_deref()) {
                Ok(o) => (Some(o), None),
                Err(e) => (None, Some(format!("{:#}", e))),
            };

        let (authed_user, _authed_err) = match data::authed_user_login() {
            Ok(u) => (Some(u), None),
            Err(e) => (None, Some(format!("{:#}", e))),
        };

        let bg_rx = Some(spawn_bg_prefetches(
            name.clone(),
            dashboard_org.clone(),
            authed_user.clone(),
        ));

        App {
            screen: Screen::Dashboard,
            in_git_repo,
            dashboard_org,
            dashboard_org_err,
            authed_user,
            summary,
            summary_err,
            commits,
            commits_err,
            commits_focused: false,
            commits_list_state: ListState::default(),
            commit_detail: None,
            prs,
            prs_err,
            dirty_files,
            dirty_files_err,
            dashboard: DashboardState::new(),
            org: OrgState {
                name,
                detect_err,
                activity: vec![],
                activity_err: None,
                activity_loaded: false,
                subview: OrgSubview::Activity,
                repos: RepoListState::new(),
                users: UserListState::new(),
            },
            org_scroll: 0,
            repo_detail: None,
            user_detail: None,
            pr_detail: None,
            notification_detail: None,
            bg_rx,
        }
    }

    fn drain_bg(&mut self) {
        let Some(rx) = self.bg_rx.as_ref() else {
            return;
        };
        loop {
            match rx.try_recv() {
                Ok(BgMessage::Activity(Ok(a))) => {
                    self.org.activity = a;
                    self.org.activity_loaded = true;
                    self.org.activity_err = None;
                }
                Ok(BgMessage::Activity(Err(e))) => {
                    self.org.activity_loaded = true;
                    self.org.activity_err = Some(e);
                }
                Ok(BgMessage::Repos(Ok(r))) => {
                    self.org.repos.repos = r;
                    self.org.repos.loaded = true;
                    self.org.repos.error = None;
                    if !self.org.repos.repos.is_empty()
                        && self.org.repos.list_state.selected().is_none()
                    {
                        self.org.repos.list_state.select(Some(0));
                    }
                }
                Ok(BgMessage::Repos(Err(e))) => {
                    self.org.repos.loaded = true;
                    self.org.repos.error = Some(e);
                }
                Ok(BgMessage::Members(Ok(m))) => {
                    self.org.users.members = m;
                    self.org.users.loaded = true;
                    self.org.users.error = None;
                    if !self.org.users.members.is_empty()
                        && self.org.users.list_state.selected().is_none()
                    {
                        self.org.users.list_state.select(Some(0));
                    }
                }
                Ok(BgMessage::Members(Err(e))) => {
                    self.org.users.loaded = true;
                    self.org.users.error = Some(e);
                }
                Ok(BgMessage::Review(Ok(p))) => {
                    self.dashboard.review_prs = p;
                    self.dashboard.review_prs_loaded = true;
                    self.dashboard.review_prs_err = None;
                    if !self.dashboard.review_prs.is_empty()
                        && self.dashboard.review_list_state.selected().is_none()
                    {
                        self.dashboard.review_list_state.select(Some(0));
                    }
                }
                Ok(BgMessage::Review(Err(e))) => {
                    self.dashboard.review_prs_loaded = true;
                    self.dashboard.review_prs_err = Some(e);
                }
                Ok(BgMessage::MyPrs(Ok(p))) => {
                    self.dashboard.my_prs = p;
                    self.dashboard.my_prs_loaded = true;
                    self.dashboard.my_prs_err = None;
                }
                Ok(BgMessage::MyPrs(Err(e))) => {
                    self.dashboard.my_prs_loaded = true;
                    self.dashboard.my_prs_err = Some(e);
                }
                Ok(BgMessage::Notifications(Ok(n))) => {
                    self.dashboard.notifications = n;
                    self.dashboard.notifications_loaded = true;
                    self.dashboard.notifications_err = None;
                }
                Ok(BgMessage::Notifications(Err(e))) => {
                    self.dashboard.notifications_loaded = true;
                    self.dashboard.notifications_err = Some(e);
                }
                Ok(BgMessage::MyCommits(Ok(c))) => {
                    self.dashboard.my_commits = c;
                    self.dashboard.my_commits_loaded = true;
                    self.dashboard.my_commits_err = None;
                }
                Ok(BgMessage::MyCommits(Err(e))) => {
                    self.dashboard.my_commits_loaded = true;
                    self.dashboard.my_commits_err = Some(e);
                }
                Err(_) => break,
            }
        }
    }

    fn focus_commits(&mut self) {
        if self.commits.is_empty() {
            return;
        }
        self.commits_focused = true;
        if self.commits_list_state.selected().is_none() {
            self.commits_list_state.select(Some(0));
        }
    }

    fn unfocus_commits(&mut self) {
        self.commits_focused = false;
    }

    fn move_commit_selection(&mut self, delta: i32) {
        if self.commits.is_empty() {
            return;
        }
        let cur = self.commits_list_state.selected().unwrap_or(0) as i32;
        let len = self.commits.len() as i32;
        let mut next = cur + delta;
        if next < 0 {
            next = 0;
        }
        if next >= len {
            next = len - 1;
        }
        self.commits_list_state.select(Some(next as usize));
    }

    fn open_commit_detail(&mut self) {
        let Some(idx) = self.commits_list_state.selected() else {
            return;
        };
        let Some(commit) = self.commits.get(idx) else {
            return;
        };
        let sha = commit.sha.clone();
        let view = match data::commit_detail(&sha) {
            Ok(d) => CommitDetailView {
                detail: Some(d),
                error: None,
                scroll: 0,
                origin: DetailOrigin::Repo,
            },
            Err(e) => CommitDetailView {
                detail: None,
                error: Some(format!("{:#}", e)),
                scroll: 0,
                origin: DetailOrigin::Repo,
            },
        };
        self.commit_detail = Some(view);
        self.screen = Screen::CommitDetail;
    }

    fn close_commit_detail(&mut self) {
        let origin = self
            .commit_detail
            .as_ref()
            .map(|v| v.origin)
            .unwrap_or(DetailOrigin::Repo);
        self.commit_detail = None;
        self.screen = match origin {
            DetailOrigin::Repo => Screen::Repo,
            DetailOrigin::Dashboard => Screen::Dashboard,
        };
    }

    fn switch_subview(&mut self, next: OrgSubview) {
        self.org.subview = next;
    }

    fn reload(&mut self) {
        let screen = self.screen;
        let prev_subview = self.org.subview;
        let prev_repo_name = self.repo_detail.as_ref().map(|d| d.full_name.clone());
        let prev_user_login = self.user_detail.as_ref().map(|d| d.login.clone());
        *self = App::load_repo();
        self.screen = screen;
        self.org.subview = prev_subview;
        match screen {
            Screen::Dashboard | Screen::Repo | Screen::Org => {}
            Screen::RepoDetail => {
                if let Some(full) = prev_repo_name {
                    self.open_repo_detail(full);
                } else {
                    self.screen = Screen::Org;
                }
            }
            Screen::UserDetail => {
                if let Some(login) = prev_user_login {
                    self.open_user_detail(login);
                } else {
                    self.screen = Screen::Org;
                    self.org.subview = OrgSubview::Users;
                }
            }
            Screen::CommitDetail => {
                self.screen = Screen::Repo;
            }
            Screen::PrDetail | Screen::NotificationDetail => {
                self.screen = Screen::Dashboard;
            }
        }
    }

    fn open_repo_detail(&mut self, full_name: String) {
        let info = self
            .org
            .repos
            .repos
            .iter()
            .find(|r| r.full_name == full_name)
            .cloned();

        let mut detail = RepoDetailState {
            full_name: full_name.clone(),
            info,
            commits: Vec::new(),
            commits_err: None,
            prs: Vec::new(),
            prs_err: None,
            contributors: Vec::new(),
            contributors_err: None,
            loaded: false,
        };

        if let Some((owner, repo)) = data::split_full_name(&full_name) {
            match data::repo_recent_commits(&owner, &repo, 15) {
                Ok(c) => detail.commits = c,
                Err(e) => detail.commits_err = Some(format!("{:#}", e)),
            }
            match data::repo_recent_prs(&owner, &repo, 10) {
                Ok(p) => detail.prs = p,
                Err(e) => detail.prs_err = Some(format!("{:#}", e)),
            }
            match data::repo_top_contributors(&owner, &repo, 10) {
                Ok(c) => detail.contributors = c,
                Err(e) => detail.contributors_err = Some(format!("{:#}", e)),
            }
        } else {
            detail.commits_err = Some(format!("could not parse owner/repo from {}", full_name));
        }

        detail.loaded = true;
        self.repo_detail = Some(detail);
        self.screen = Screen::RepoDetail;
    }

    fn close_repo_detail(&mut self) {
        self.repo_detail = None;
        self.screen = Screen::Org;
        self.org.subview = OrgSubview::Repos;
    }

    fn org_total_lines(&self) -> u16 {
        let n: usize = self
            .org
            .activity
            .iter()
            .map(|u| 2 + u.events.len())
            .sum();
        n.try_into().unwrap_or(u16::MAX)
    }

    fn clamp_org_scroll(&mut self) {
        let max = self.org_total_lines().saturating_sub(1);
        if self.org_scroll > max {
            self.org_scroll = max;
        }
    }

    fn filtered_repo_indices(&self) -> Vec<usize> {
        let q = self.org.repos.filter.to_ascii_lowercase();
        self.org
            .repos
            .repos
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                if q.is_empty() {
                    return true;
                }
                r.name.to_ascii_lowercase().contains(&q)
                    || r.full_name.to_ascii_lowercase().contains(&q)
                    || r.description
                        .as_deref()
                        .map(|d| d.to_ascii_lowercase().contains(&q))
                        .unwrap_or(false)
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn move_repo_selection(&mut self, delta: i32) {
        let indices = self.filtered_repo_indices();
        if indices.is_empty() {
            self.org.repos.list_state.select(None);
            return;
        }
        let cur = self.org.repos.list_state.selected().unwrap_or(0);
        let len = indices.len() as i32;
        let mut next = cur as i32 + delta;
        if next < 0 {
            next = 0;
        }
        if next >= len {
            next = len - 1;
        }
        self.org.repos.list_state.select(Some(next as usize));
    }

    fn reset_repo_selection(&mut self) {
        let indices = self.filtered_repo_indices();
        if indices.is_empty() {
            self.org.repos.list_state.select(None);
        } else {
            self.org.repos.list_state.select(Some(0));
        }
    }

    fn filtered_user_indices(&self) -> Vec<usize> {
        let q = self.org.users.filter.to_ascii_lowercase();
        self.org
            .users
            .members
            .iter()
            .enumerate()
            .filter(|(_, m)| q.is_empty() || m.login.to_ascii_lowercase().contains(&q))
            .map(|(i, _)| i)
            .collect()
    }

    fn move_user_selection(&mut self, delta: i32) {
        let indices = self.filtered_user_indices();
        if indices.is_empty() {
            self.org.users.list_state.select(None);
            return;
        }
        let cur = self.org.users.list_state.selected().unwrap_or(0);
        let len = indices.len() as i32;
        let mut next = cur as i32 + delta;
        if next < 0 {
            next = 0;
        }
        if next >= len {
            next = len - 1;
        }
        self.org.users.list_state.select(Some(next as usize));
    }

    fn reset_user_selection(&mut self) {
        let indices = self.filtered_user_indices();
        if indices.is_empty() {
            self.org.users.list_state.select(None);
        } else {
            self.org.users.list_state.select(Some(0));
        }
    }

    fn open_user_detail(&mut self, login: String) {
        let org = self.org.name.clone().unwrap_or_default();
        let mut detail = UserDetailState {
            login: login.clone(),
            commits: Vec::new(),
            commits_err: None,
            submitted_prs: Vec::new(),
            submitted_err: None,
            reviewed_prs: Vec::new(),
            reviewed_err: None,
        };

        if org.is_empty() {
            detail.commits_err = Some("no org detected".into());
        } else {
            match data::user_recent_commits(&org, &login, 15) {
                Ok(c) => detail.commits = c,
                Err(e) => detail.commits_err = Some(format!("{:#}", e)),
            }
            match data::user_submitted_prs(&org, &login, 15) {
                Ok(p) => detail.submitted_prs = p,
                Err(e) => detail.submitted_err = Some(format!("{:#}", e)),
            }
            match data::user_reviewed_prs(&org, &login, 15) {
                Ok(p) => detail.reviewed_prs = p,
                Err(e) => detail.reviewed_err = Some(format!("{:#}", e)),
            }
        }

        self.user_detail = Some(detail);
        self.screen = Screen::UserDetail;
    }

    fn close_user_detail(&mut self) {
        self.user_detail = None;
        self.screen = Screen::Org;
        self.org.subview = OrgSubview::Users;
    }

    fn focus_review(&mut self) {
        if self.dashboard.review_prs.is_empty() {
            return;
        }
        self.dashboard.unfocus_all();
        self.dashboard.review_focused = true;
        if self.dashboard.review_list_state.selected().is_none() {
            self.dashboard.review_list_state.select(Some(0));
        }
    }

    fn move_review_selection(&mut self, delta: i32) {
        if self.dashboard.review_prs.is_empty() {
            return;
        }
        let cur = self.dashboard.review_list_state.selected().unwrap_or(0) as i32;
        let len = self.dashboard.review_prs.len() as i32;
        let mut next = cur + delta;
        if next < 0 {
            next = 0;
        }
        if next >= len {
            next = len - 1;
        }
        self.dashboard.review_list_state.select(Some(next as usize));
    }

    fn selected_review_pr(&self) -> Option<&ReviewRequestedPr> {
        let idx = self.dashboard.review_list_state.selected()?;
        self.dashboard.review_prs.get(idx)
    }

    fn open_pr_detail_for_selected_review(&mut self) {
        let Some(pr) = self.selected_review_pr() else {
            return;
        };
        let full = pr.repo.clone();
        let number = pr.number;
        let Some((owner, repo)) = data::split_full_name(&full) else {
            return;
        };
        let view = match data::pr_detail(&owner, &repo, number) {
            Ok(d) => PrDetailView {
                detail: Some(d),
                error: None,
                scroll: 0,
            },
            Err(e) => PrDetailView {
                detail: None,
                error: Some(format!("{:#}", e)),
                scroll: 0,
            },
        };
        self.pr_detail = Some(view);
        self.screen = Screen::PrDetail;
    }

    fn close_pr_detail(&mut self) {
        self.pr_detail = None;
        self.screen = Screen::Dashboard;
    }

    fn open_selected_review_in_browser(&mut self) -> Option<String> {
        let pr = self.selected_review_pr()?;
        let url = format!("https://github.com/{}/pull/{}", pr.repo, pr.number);
        match data::open_in_browser(&url) {
            Ok(()) => None,
            Err(e) => Some(format!("{:#}", e)),
        }
    }

    fn open_pr_detail_in_browser(&self) -> Option<String> {
        let view = self.pr_detail.as_ref()?;
        let url = view.detail.as_ref()?.url.clone();
        match data::open_in_browser(&url) {
            Ok(()) => None,
            Err(e) => Some(format!("{:#}", e)),
        }
    }

    fn focus_my_prs(&mut self) {
        if self.dashboard.my_prs.is_empty() {
            return;
        }
        self.dashboard.unfocus_all();
        self.dashboard.my_prs_focused = true;
        if self.dashboard.my_prs_list_state.selected().is_none() {
            self.dashboard.my_prs_list_state.select(Some(0));
        }
    }

    fn move_my_prs_selection(&mut self, delta: i32) {
        clamp_select(
            &mut self.dashboard.my_prs_list_state,
            self.dashboard.my_prs.len(),
            delta,
        );
    }

    fn selected_my_pr(&self) -> Option<&UserPr> {
        let idx = self.dashboard.my_prs_list_state.selected()?;
        self.dashboard.my_prs.get(idx)
    }

    fn open_pr_detail_for_selected_my_pr(&mut self) {
        let Some(pr) = self.selected_my_pr() else {
            return;
        };
        let full = pr.repo.clone();
        let number = pr.number;
        self.open_pr_detail_from(full, number);
    }

    fn open_selected_my_pr_in_browser(&self) -> Option<String> {
        let pr = self.selected_my_pr()?;
        let url = format!("https://github.com/{}/pull/{}", pr.repo, pr.number);
        data::open_in_browser(&url).err().map(|e| format!("{:#}", e))
    }

    fn open_pr_detail_from(&mut self, full: String, number: u64) {
        let Some((owner, repo)) = data::split_full_name(&full) else {
            return;
        };
        let view = match data::pr_detail(&owner, &repo, number) {
            Ok(d) => PrDetailView {
                detail: Some(d),
                error: None,
                scroll: 0,
            },
            Err(e) => PrDetailView {
                detail: None,
                error: Some(format!("{:#}", e)),
                scroll: 0,
            },
        };
        self.pr_detail = Some(view);
        self.screen = Screen::PrDetail;
    }

    fn focus_notifications(&mut self) {
        if self.dashboard.notifications.is_empty() {
            return;
        }
        self.dashboard.unfocus_all();
        self.dashboard.notifications_focused = true;
        if self.dashboard.notifications_list_state.selected().is_none() {
            self.dashboard.notifications_list_state.select(Some(0));
        }
    }

    fn move_notifications_selection(&mut self, delta: i32) {
        clamp_select(
            &mut self.dashboard.notifications_list_state,
            self.dashboard.notifications.len(),
            delta,
        );
    }

    fn selected_notification(&self) -> Option<&Notification> {
        let idx = self.dashboard.notifications_list_state.selected()?;
        self.dashboard.notifications.get(idx)
    }

    fn open_notification_detail(&mut self) {
        let Some(n) = self.selected_notification() else {
            return;
        };
        self.notification_detail = Some(NotificationDetailView {
            notification: n.clone(),
        });
        self.screen = Screen::NotificationDetail;
    }

    fn close_notification_detail(&mut self) {
        self.notification_detail = None;
        self.screen = Screen::Dashboard;
    }

    fn open_selected_notification_in_browser(&self) -> Option<String> {
        let n = self.selected_notification()?;
        data::open_in_browser(&n.web_url)
            .err()
            .map(|e| format!("{:#}", e))
    }

    fn open_notification_detail_in_browser(&self) -> Option<String> {
        let v = self.notification_detail.as_ref()?;
        data::open_in_browser(&v.notification.web_url)
            .err()
            .map(|e| format!("{:#}", e))
    }

    fn focus_my_commits(&mut self) {
        if self.dashboard.my_commits.is_empty() {
            return;
        }
        self.dashboard.unfocus_all();
        self.dashboard.my_commits_focused = true;
        if self.dashboard.my_commits_list_state.selected().is_none() {
            self.dashboard.my_commits_list_state.select(Some(0));
        }
    }

    fn move_my_commits_selection(&mut self, delta: i32) {
        clamp_select(
            &mut self.dashboard.my_commits_list_state,
            self.dashboard.my_commits.len(),
            delta,
        );
    }

    fn selected_my_commit(&self) -> Option<&UserCommit> {
        let idx = self.dashboard.my_commits_list_state.selected()?;
        self.dashboard.my_commits.get(idx)
    }

    fn open_my_commit_detail(&mut self) {
        let Some(c) = self.selected_my_commit() else {
            return;
        };
        let Some((owner, repo)) = data::split_full_name(&c.repo) else {
            return;
        };
        let sha = c.sha.clone();
        let view = match data::org_commit_detail(&owner, &repo, &sha) {
            Ok(d) => CommitDetailView {
                detail: Some(d),
                error: None,
                scroll: 0,
                origin: DetailOrigin::Dashboard,
            },
            Err(e) => CommitDetailView {
                detail: None,
                error: Some(format!("{:#}", e)),
                scroll: 0,
                origin: DetailOrigin::Dashboard,
            },
        };
        self.commit_detail = Some(view);
        self.screen = Screen::CommitDetail;
    }

    fn open_selected_my_commit_in_browser(&self) -> Option<String> {
        let c = self.selected_my_commit()?;
        let url = format!("https://github.com/{}/commit/{}", c.repo, c.sha);
        data::open_in_browser(&url).err().map(|e| format!("{:#}", e))
    }
}

fn clamp_select(state: &mut ListState, len: usize, delta: i32) {
    if len == 0 {
        state.select(None);
        return;
    }
    let cur = state.selected().unwrap_or(0) as i32;
    let mut next = cur + delta;
    let max = len as i32 - 1;
    if next < 0 {
        next = 0;
    }
    if next > max {
        next = max;
    }
    state.select(Some(next as usize));
}

fn main() -> Result<()> {
    let mut app = App::load_repo();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let res = run_app(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    res
}

fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> Result<()> {
    loop {
        app.drain_bg();
        terminal.draw(|f| ui(f, app))?;

        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                // Filter-input mode swallows most keys.
                if app.screen == Screen::Org
                    && app.org.subview == OrgSubview::Repos
                    && app.org.repos.filtering
                {
                    match key.code {
                        KeyCode::Esc => {
                            app.org.repos.filter.clear();
                            app.org.repos.filtering = false;
                            app.reset_repo_selection();
                        }
                        KeyCode::Enter => {
                            app.org.repos.filtering = false;
                        }
                        KeyCode::Backspace => {
                            app.org.repos.filter.pop();
                            app.reset_repo_selection();
                        }
                        KeyCode::Char(c) => {
                            app.org.repos.filter.push(c);
                            app.reset_repo_selection();
                        }
                        _ => {}
                    }
                    continue;
                }
                if app.screen == Screen::Org
                    && app.org.subview == OrgSubview::Users
                    && app.org.users.filtering
                {
                    match key.code {
                        KeyCode::Esc => {
                            app.org.users.filter.clear();
                            app.org.users.filtering = false;
                            app.reset_user_selection();
                        }
                        KeyCode::Enter => {
                            app.org.users.filtering = false;
                        }
                        KeyCode::Backspace => {
                            app.org.users.filter.pop();
                            app.reset_user_selection();
                        }
                        KeyCode::Char(c) => {
                            app.org.users.filter.push(c);
                            app.reset_user_selection();
                        }
                        _ => {}
                    }
                    continue;
                }

                match key.code {
                    KeyCode::Char('q') => return Ok(()),
                    KeyCode::Esc => match app.screen {
                        Screen::RepoDetail => app.close_repo_detail(),
                        Screen::CommitDetail => app.close_commit_detail(),
                        Screen::UserDetail => app.close_user_detail(),
                        Screen::PrDetail => app.close_pr_detail(),
                        Screen::NotificationDetail => app.close_notification_detail(),
                        Screen::Repo if app.commits_focused => app.unfocus_commits(),
                        Screen::Dashboard if app.dashboard.any_focused() => {
                            app.dashboard.unfocus_all()
                        }
                        _ => return Ok(()),
                    },
                    KeyCode::Char('r') => app.reload(),
                    KeyCode::Char('1') => {
                        app.repo_detail = None;
                        app.commit_detail = None;
                        app.user_detail = None;
                        app.pr_detail = None;
                        app.notification_detail = None;
                        app.commits_focused = false;
                        app.screen = Screen::Dashboard;
                    }
                    KeyCode::Char('2') => {
                        app.repo_detail = None;
                        app.commit_detail = None;
                        app.user_detail = None;
                        app.pr_detail = None;
                        app.notification_detail = None;
                        app.dashboard.unfocus_all();
                        app.screen = Screen::Repo;
                    }
                    KeyCode::Char('3') => {
                        app.repo_detail = None;
                        app.commit_detail = None;
                        app.user_detail = None;
                        app.pr_detail = None;
                        app.notification_detail = None;
                        app.commits_focused = false;
                        app.dashboard.unfocus_all();
                        app.screen = Screen::Org;
                    }
                    KeyCode::Tab | KeyCode::Right => {
                        app.repo_detail = None;
                        app.commit_detail = None;
                        app.user_detail = None;
                        app.pr_detail = None;
                        app.notification_detail = None;
                        app.commits_focused = false;
                        app.dashboard.unfocus_all();
                        app.screen = match app.screen {
                            Screen::Dashboard => Screen::Repo,
                            Screen::Repo => Screen::Org,
                            Screen::Org => Screen::Dashboard,
                            Screen::RepoDetail
                            | Screen::CommitDetail
                            | Screen::UserDetail
                            | Screen::PrDetail
                            | Screen::NotificationDetail => Screen::Dashboard,
                        };
                    }
                    KeyCode::BackTab | KeyCode::Left => {
                        app.repo_detail = None;
                        app.commit_detail = None;
                        app.user_detail = None;
                        app.pr_detail = None;
                        app.notification_detail = None;
                        app.commits_focused = false;
                        app.dashboard.unfocus_all();
                        app.screen = match app.screen {
                            Screen::Dashboard => Screen::Org,
                            Screen::Repo => Screen::Dashboard,
                            Screen::Org => Screen::Repo,
                            Screen::RepoDetail
                            | Screen::CommitDetail
                            | Screen::UserDetail
                            | Screen::PrDetail
                            | Screen::NotificationDetail => Screen::Dashboard,
                        };
                    }
                    KeyCode::Char(']') if app.screen == Screen::Org => {
                        let next = match app.org.subview {
                            OrgSubview::Activity => OrgSubview::Repos,
                            OrgSubview::Repos => OrgSubview::Users,
                            OrgSubview::Users => OrgSubview::Activity,
                        };
                        app.switch_subview(next);
                    }
                    KeyCode::Char('[') if app.screen == Screen::Org => {
                        let next = match app.org.subview {
                            OrgSubview::Activity => OrgSubview::Users,
                            OrgSubview::Repos => OrgSubview::Activity,
                            OrgSubview::Users => OrgSubview::Repos,
                        };
                        app.switch_subview(next);
                    }
                    KeyCode::Char('c') if app.screen == Screen::Repo => app.focus_commits(),
                    KeyCode::Char('v') if app.screen == Screen::Dashboard => app.focus_review(),
                    KeyCode::Char('p') if app.screen == Screen::Dashboard => app.focus_my_prs(),
                    KeyCode::Char('n') if app.screen == Screen::Dashboard => {
                        app.focus_notifications()
                    }
                    KeyCode::Char('c') if app.screen == Screen::Dashboard => {
                        app.focus_my_commits()
                    }
                    KeyCode::Char('o') if app.screen == Screen::Dashboard => {
                        if app.dashboard.review_focused {
                            let _ = app.open_selected_review_in_browser();
                        } else if app.dashboard.my_prs_focused {
                            let _ = app.open_selected_my_pr_in_browser();
                        } else if app.dashboard.notifications_focused {
                            let _ = app.open_selected_notification_in_browser();
                        } else if app.dashboard.my_commits_focused {
                            let _ = app.open_selected_my_commit_in_browser();
                        }
                    }
                    KeyCode::Char('o') if app.screen == Screen::PrDetail => {
                        let _ = app.open_pr_detail_in_browser();
                    }
                    KeyCode::Char('o') if app.screen == Screen::NotificationDetail => {
                        let _ = app.open_notification_detail_in_browser();
                    }
                    _ => {
                        handle_screen_key(app, key.code);
                    }
                }
            }
        }
    }
}

fn handle_screen_key(app: &mut App, code: KeyCode) {
    match app.screen {
        Screen::Repo if app.commits_focused => match code {
            KeyCode::Down | KeyCode::Char('j') => app.move_commit_selection(1),
            KeyCode::Up | KeyCode::Char('k') => app.move_commit_selection(-1),
            KeyCode::PageDown => app.move_commit_selection(10),
            KeyCode::PageUp => app.move_commit_selection(-10),
            KeyCode::Home | KeyCode::Char('g') => app.move_commit_selection(-9999),
            KeyCode::End | KeyCode::Char('G') => app.move_commit_selection(9999),
            KeyCode::Enter => app.open_commit_detail(),
            _ => {}
        },
        Screen::Dashboard if app.dashboard.review_focused => match code {
            KeyCode::Down | KeyCode::Char('j') => app.move_review_selection(1),
            KeyCode::Up | KeyCode::Char('k') => app.move_review_selection(-1),
            KeyCode::PageDown => app.move_review_selection(10),
            KeyCode::PageUp => app.move_review_selection(-10),
            KeyCode::Home | KeyCode::Char('g') => app.move_review_selection(-9999),
            KeyCode::End | KeyCode::Char('G') => app.move_review_selection(9999),
            KeyCode::Enter => app.open_pr_detail_for_selected_review(),
            _ => {}
        },
        Screen::Dashboard if app.dashboard.my_prs_focused => match code {
            KeyCode::Down | KeyCode::Char('j') => app.move_my_prs_selection(1),
            KeyCode::Up | KeyCode::Char('k') => app.move_my_prs_selection(-1),
            KeyCode::PageDown => app.move_my_prs_selection(10),
            KeyCode::PageUp => app.move_my_prs_selection(-10),
            KeyCode::Home | KeyCode::Char('g') => app.move_my_prs_selection(-9999),
            KeyCode::End | KeyCode::Char('G') => app.move_my_prs_selection(9999),
            KeyCode::Enter => app.open_pr_detail_for_selected_my_pr(),
            _ => {}
        },
        Screen::Dashboard if app.dashboard.notifications_focused => match code {
            KeyCode::Down | KeyCode::Char('j') => app.move_notifications_selection(1),
            KeyCode::Up | KeyCode::Char('k') => app.move_notifications_selection(-1),
            KeyCode::PageDown => app.move_notifications_selection(10),
            KeyCode::PageUp => app.move_notifications_selection(-10),
            KeyCode::Home | KeyCode::Char('g') => app.move_notifications_selection(-9999),
            KeyCode::End | KeyCode::Char('G') => app.move_notifications_selection(9999),
            KeyCode::Enter => app.open_notification_detail(),
            _ => {}
        },
        Screen::Dashboard if app.dashboard.my_commits_focused => match code {
            KeyCode::Down | KeyCode::Char('j') => app.move_my_commits_selection(1),
            KeyCode::Up | KeyCode::Char('k') => app.move_my_commits_selection(-1),
            KeyCode::PageDown => app.move_my_commits_selection(10),
            KeyCode::PageUp => app.move_my_commits_selection(-10),
            KeyCode::Home | KeyCode::Char('g') => app.move_my_commits_selection(-9999),
            KeyCode::End | KeyCode::Char('G') => app.move_my_commits_selection(9999),
            KeyCode::Enter => app.open_my_commit_detail(),
            _ => {}
        },
        Screen::CommitDetail => {
            if let Some(view) = app.commit_detail.as_mut() {
                match code {
                    KeyCode::Down | KeyCode::Char('j') => {
                        view.scroll = view.scroll.saturating_add(1)
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        view.scroll = view.scroll.saturating_sub(1)
                    }
                    KeyCode::PageDown => view.scroll = view.scroll.saturating_add(10),
                    KeyCode::PageUp => view.scroll = view.scroll.saturating_sub(10),
                    KeyCode::Home | KeyCode::Char('g') => view.scroll = 0,
                    _ => {}
                }
            }
        }
        Screen::PrDetail => {
            if let Some(view) = app.pr_detail.as_mut() {
                match code {
                    KeyCode::Down | KeyCode::Char('j') => {
                        view.scroll = view.scroll.saturating_add(1)
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        view.scroll = view.scroll.saturating_sub(1)
                    }
                    KeyCode::PageDown => view.scroll = view.scroll.saturating_add(10),
                    KeyCode::PageUp => view.scroll = view.scroll.saturating_sub(10),
                    KeyCode::Home | KeyCode::Char('g') => view.scroll = 0,
                    _ => {}
                }
            }
        }
        Screen::Org => match app.org.subview {
            OrgSubview::Activity => match code {
                KeyCode::Down | KeyCode::Char('j') => {
                    app.org_scroll = app.org_scroll.saturating_add(1);
                    app.clamp_org_scroll();
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    app.org_scroll = app.org_scroll.saturating_sub(1);
                }
                KeyCode::PageDown => {
                    app.org_scroll = app.org_scroll.saturating_add(10);
                    app.clamp_org_scroll();
                }
                KeyCode::PageUp => {
                    app.org_scroll = app.org_scroll.saturating_sub(10);
                }
                KeyCode::Home | KeyCode::Char('g') => app.org_scroll = 0,
                KeyCode::End | KeyCode::Char('G') => {
                    app.org_scroll = u16::MAX;
                    app.clamp_org_scroll();
                }
                _ => {}
            },
            OrgSubview::Repos => match code {
                KeyCode::Down | KeyCode::Char('j') => app.move_repo_selection(1),
                KeyCode::Up | KeyCode::Char('k') => app.move_repo_selection(-1),
                KeyCode::PageDown => app.move_repo_selection(10),
                KeyCode::PageUp => app.move_repo_selection(-10),
                KeyCode::Home | KeyCode::Char('g') => app.move_repo_selection(-9999),
                KeyCode::End | KeyCode::Char('G') => app.move_repo_selection(9999),
                KeyCode::Char('/') => {
                    app.org.repos.filtering = true;
                }
                KeyCode::Enter => {
                    let indices = app.filtered_repo_indices();
                    if let Some(sel) = app.org.repos.list_state.selected() {
                        if let Some(&actual) = indices.get(sel) {
                            let full = app.org.repos.repos[actual].full_name.clone();
                            app.open_repo_detail(full);
                        }
                    }
                }
                _ => {}
            },
            OrgSubview::Users => match code {
                KeyCode::Down | KeyCode::Char('j') => app.move_user_selection(1),
                KeyCode::Up | KeyCode::Char('k') => app.move_user_selection(-1),
                KeyCode::PageDown => app.move_user_selection(10),
                KeyCode::PageUp => app.move_user_selection(-10),
                KeyCode::Home | KeyCode::Char('g') => app.move_user_selection(-9999),
                KeyCode::End | KeyCode::Char('G') => app.move_user_selection(9999),
                KeyCode::Char('/') => {
                    app.org.users.filtering = true;
                }
                KeyCode::Enter => {
                    let indices = app.filtered_user_indices();
                    if let Some(sel) = app.org.users.list_state.selected() {
                        if let Some(&actual) = indices.get(sel) {
                            let login = app.org.users.members[actual].login.clone();
                            app.open_user_detail(login);
                        }
                    }
                }
                _ => {}
            },
        },
        _ => {}
    }
}

fn ui(f: &mut ratatui::Frame, app: &App) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(f.area());

    render_tabs(f, outer[0], app);
    match app.screen {
        Screen::Dashboard => render_dashboard_screen(f, outer[1], app),
        Screen::Repo => render_repo_screen(f, outer[1], app),
        Screen::Org => render_org_screen(f, outer[1], app),
        Screen::RepoDetail => render_repo_detail_screen(f, outer[1], app),
        Screen::CommitDetail => render_commit_detail_screen(f, outer[1], app),
        Screen::UserDetail => render_user_detail_screen(f, outer[1], app),
        Screen::PrDetail => render_pr_detail_screen(f, outer[1], app),
        Screen::NotificationDetail => render_notification_detail_screen(f, outer[1], app),
    }
    render_footer(f, outer[2], app);
}

fn render_tabs(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let dash_label = match &app.authed_user {
        Some(u) => format!("1 Dashboard (@{})", u),
        None => "1 Dashboard".to_string(),
    };
    let org_label = match &app.org.name {
        Some(n) => format!("3 Org ({})", n),
        None => "3 Org".to_string(),
    };
    let titles = vec![
        Line::from(dash_label),
        Line::from("2 Repo"),
        Line::from(org_label),
    ];
    let select = match app.screen {
        Screen::Dashboard | Screen::PrDetail | Screen::NotificationDetail => 0,
        Screen::Repo => 1,
        Screen::CommitDetail => match app.commit_detail.as_ref().map(|v| v.origin) {
            Some(DetailOrigin::Dashboard) => 0,
            _ => 1,
        },
        Screen::Org | Screen::RepoDetail | Screen::UserDetail => 2,
    };
    let tabs = Tabs::new(titles)
        .block(Block::default().borders(Borders::ALL).title(" gitcycle "))
        .select(select)
        .style(Style::default().fg(Color::Gray))
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );
    f.render_widget(tabs, area);
}

fn render_repo_screen(f: &mut ratatui::Frame, area: Rect, app: &App) {
    if !app.in_git_repo {
        let block = Block::default().borders(Borders::ALL).title(" repo ");
        f.render_widget(
            Paragraph::new("current directory is not a git repository\n\nrun gitcycle from inside a git repo to use this tab")
                .style(Style::default().fg(Color::DarkGray))
                .block(block)
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(8), Constraint::Min(0)])
        .split(cols[0]);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(cols[1]);

    render_summary(f, left[0], app);
    render_dirty_files(f, left[1], app);
    render_commits(f, right[0], app);
    render_prs(f, right[1], app);
}

fn render_dashboard_screen(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[0]);
    let bottom = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[1]);

    render_dashboard_review(f, top[0], app);
    render_dashboard_my_prs(f, top[1], app);
    render_dashboard_notifications(f, bottom[0], app);
    render_dashboard_my_commits(f, bottom[1], app);
}

fn render_dashboard_review(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let focused = app.dashboard.review_focused;
    let base_title = match &app.dashboard_org {
        Some(o) => format!(" awaiting your review — {} ", o),
        None => " awaiting your review ".to_string(),
    };
    let title = if focused {
        format!(" awaiting your review — ↑↓ Enter, o open, Esc unfocus ")
    } else if app.dashboard.review_prs.is_empty() {
        base_title
    } else {
        format!("{}(press v to focus)", base_title.trim_end())
    };
    let border_style = if focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);

    if let Some(err) = &app.dashboard_org_err {
        f.render_widget(
            Paragraph::new(format!("(no org to scope to: {})", err))
                .style(Style::default().fg(Color::DarkGray))
                .block(block)
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    if !app.dashboard.review_prs_loaded {
        f.render_widget(
            Paragraph::new("loading…")
                .style(Style::default().fg(Color::DarkGray))
                .block(block),
            area,
        );
        return;
    }
    if let Some(err) = &app.dashboard.review_prs_err {
        f.render_widget(
            Paragraph::new(format!("error: {}", err))
                .style(Style::default().fg(Color::Red))
                .block(block)
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    if app.dashboard.review_prs.is_empty() {
        f.render_widget(
            Paragraph::new("no PRs need your review")
                .style(Style::default().fg(Color::DarkGray))
                .block(block),
            area,
        );
        return;
    }
    let items: Vec<ListItem> = app
        .dashboard
        .review_prs
        .iter()
        .map(|p| {
            let draft = if p.is_draft {
                Span::styled(" [draft]", Style::default().fg(Color::DarkGray))
            } else {
                Span::raw("")
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("#{:<5}", p.number), Style::default().fg(Color::Green)),
                Span::styled(
                    format!("{:<24} ", truncate(&p.repo, 24)),
                    Style::default().fg(Color::Green),
                ),
                Span::raw(p.title.clone()),
                draft,
                Span::styled(
                    format!("  @{} ({})", p.author, p.updated_at),
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();

    let mut list = List::new(items).block(block);
    if focused {
        list = list
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▶ ");
    }
    let mut state = app.dashboard.review_list_state.clone();
    f.render_stateful_widget(list, area, &mut state);
}

fn render_dashboard_my_prs(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let focused = app.dashboard.my_prs_focused;
    let title = if focused {
        " your open PRs — ↑↓ Enter, o open, Esc unfocus ".to_string()
    } else if app.dashboard.my_prs.is_empty() {
        " your open PRs ".to_string()
    } else {
        " your open PRs (press p to focus) ".to_string()
    };
    let border_style = if focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);
    if !app.dashboard.my_prs_loaded {
        f.render_widget(
            Paragraph::new("loading…")
                .style(Style::default().fg(Color::DarkGray))
                .block(block),
            area,
        );
        return;
    }
    if let Some(err) = &app.dashboard.my_prs_err {
        f.render_widget(
            Paragraph::new(format!("error: {}", err))
                .style(Style::default().fg(Color::Red))
                .block(block)
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    if app.dashboard.my_prs.is_empty() {
        f.render_widget(
            Paragraph::new("no open PRs of yours")
                .style(Style::default().fg(Color::DarkGray))
                .block(block),
            area,
        );
        return;
    }
    let items: Vec<ListItem> = app
        .dashboard
        .my_prs
        .iter()
        .map(|p| {
            let (label, color) = pr_state_style(&p.state, p.is_draft);
            ListItem::new(Line::from(vec![
                Span::styled(format!("#{:<5}", p.number), Style::default().fg(Color::Green)),
                Span::styled(format!("{:<7}", label), Style::default().fg(color)),
                Span::styled(
                    format!("{:<24}", truncate(&p.repo, 24)),
                    Style::default().fg(Color::Green),
                ),
                Span::raw(" "),
                Span::raw(p.title.clone()),
                Span::styled(
                    format!("  ({})", p.updated_at),
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();
    let mut list = List::new(items).block(block);
    if focused {
        list = list
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▶ ");
    }
    let mut state = app.dashboard.my_prs_list_state.clone();
    f.render_stateful_widget(list, area, &mut state);
}

fn render_dashboard_notifications(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let focused = app.dashboard.notifications_focused;
    let title = if focused {
        " notifications — ↑↓ Enter, o open, Esc unfocus ".to_string()
    } else if app.dashboard.notifications.is_empty() {
        " notifications ".to_string()
    } else {
        " notifications (press n to focus) ".to_string()
    };
    let border_style = if focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);
    if !app.dashboard.notifications_loaded {
        f.render_widget(
            Paragraph::new("loading…")
                .style(Style::default().fg(Color::DarkGray))
                .block(block),
            area,
        );
        return;
    }
    if let Some(err) = &app.dashboard.notifications_err {
        f.render_widget(
            Paragraph::new(format!("error: {}", err))
                .style(Style::default().fg(Color::Red))
                .block(block)
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    if app.dashboard.notifications.is_empty() {
        f.render_widget(
            Paragraph::new("inbox zero")
                .style(Style::default().fg(Color::DarkGray))
                .block(block),
            area,
        );
        return;
    }
    let items: Vec<ListItem> = app
        .dashboard
        .notifications
        .iter()
        .map(|n| {
            let reason_color = match n.reason.as_str() {
                "review_requested" => Color::Yellow,
                "mention" | "team_mention" => Color::Magenta,
                "assign" => Color::Cyan,
                "ci_activity" => Color::Red,
                _ => Color::DarkGray,
            };
            let unread_marker = if n.unread {
                Span::styled("● ", Style::default().fg(Color::Yellow))
            } else {
                Span::raw("  ")
            };
            ListItem::new(Line::from(vec![
                unread_marker,
                Span::styled(
                    format!("{:<18}", truncate(&n.reason, 18)),
                    Style::default().fg(reason_color),
                ),
                Span::styled(
                    format!("{:<22} ", truncate(&n.repo, 22)),
                    Style::default().fg(Color::Green),
                ),
                Span::raw(n.title.clone()),
                Span::styled(
                    format!("  ({})", n.updated_at),
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();
    let mut list = List::new(items).block(block);
    if focused {
        list = list
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▶ ");
    }
    let mut state = app.dashboard.notifications_list_state.clone();
    f.render_stateful_widget(list, area, &mut state);
}

fn render_dashboard_my_commits(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let focused = app.dashboard.my_commits_focused;
    let title = if focused {
        " your recent commits — ↑↓ Enter, o open, Esc unfocus ".to_string()
    } else if app.dashboard.my_commits.is_empty() {
        " your recent commits ".to_string()
    } else {
        " your recent commits (press c to focus) ".to_string()
    };
    let border_style = if focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);
    if !app.dashboard.my_commits_loaded {
        f.render_widget(
            Paragraph::new("loading…")
                .style(Style::default().fg(Color::DarkGray))
                .block(block),
            area,
        );
        return;
    }
    if let Some(err) = &app.dashboard.my_commits_err {
        f.render_widget(
            Paragraph::new(format!("error: {}", err))
                .style(Style::default().fg(Color::Red))
                .block(block)
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    if app.dashboard.my_commits.is_empty() {
        f.render_widget(
            Paragraph::new("no recent commits")
                .style(Style::default().fg(Color::DarkGray))
                .block(block),
            area,
        );
        return;
    }
    let items: Vec<ListItem> = app
        .dashboard
        .my_commits
        .iter()
        .map(|c| {
            ListItem::new(Line::from(vec![
                Span::styled(format!("{} ", c.sha), Style::default().fg(Color::Yellow)),
                Span::styled(
                    format!("{:<24}", truncate(&c.repo, 24)),
                    Style::default().fg(Color::Green),
                ),
                Span::raw(" "),
                Span::raw(c.subject.clone()),
                Span::styled(
                    format!("  ({})", c.date),
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();
    let mut list = List::new(items).block(block);
    if focused {
        list = list
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▶ ");
    }
    let mut state = app.dashboard.my_commits_list_state.clone();
    f.render_stateful_widget(list, area, &mut state);
}

fn render_dirty_files(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = Block::default().borders(Borders::ALL).title(" dirty files ");
    if let Some(err) = &app.dirty_files_err {
        f.render_widget(
            Paragraph::new(format!("error: {}", err))
                .style(Style::default().fg(Color::Red))
                .block(block)
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    if app.dirty_files.is_empty() {
        f.render_widget(
            Paragraph::new("clean working tree")
                .style(Style::default().fg(Color::DarkGray))
                .block(block),
            area,
        );
        return;
    }
    let items: Vec<ListItem> = app
        .dirty_files
        .iter()
        .map(|d| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{} ", d.status),
                    Style::default().fg(status_color(&d.status)),
                ),
                Span::raw(d.path.clone()),
            ]))
        })
        .collect();
    f.render_widget(List::new(items).block(block), area);
}

fn status_color(s: &str) -> Color {
    let t = s.trim();
    if t == "??" {
        Color::Magenta
    } else if t.contains('A') {
        Color::Green
    } else if t.contains('D') {
        Color::Red
    } else if t.contains('M') {
        Color::Yellow
    } else if t.contains('R') {
        Color::Cyan
    } else {
        Color::Gray
    }
}

fn render_summary(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = Block::default().borders(Borders::ALL).title(" repo ");
    let body = match (&app.summary, &app.summary_err) {
        (Some(s), _) => {
            let upstream = s.upstream.as_deref().unwrap_or("(none)");
            let remote = s.remote_url.as_deref().unwrap_or("(none)");
            let fetch = s.last_fetch.as_deref().unwrap_or("(unknown)");
            let ahead_behind = format!("{} / {}", s.ahead, s.behind);
            vec![
                kv("root", &s.root),
                kv("branch", &s.branch),
                kv("upstream", upstream),
                kv("remote", remote),
                kv("ahead/behind", &ahead_behind),
                kv("last fetch", fetch),
            ]
        }
        (None, Some(e)) => vec![Line::from(Span::styled(
            format!("error: {}", e),
            Style::default().fg(Color::Red),
        ))],
        _ => vec![Line::from("loading...")],
    };
    f.render_widget(
        Paragraph::new(body).block(block).wrap(Wrap { trim: false }),
        area,
    );
}

fn render_commits(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let title = if app.commits_focused {
        " recent commits — ↑↓ Enter, Esc to unfocus "
    } else {
        " recent commits (press c to focus) "
    };
    let border_style = if app.commits_focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);

    if let Some(err) = &app.commits_err {
        f.render_widget(
            Paragraph::new(format!("error: {}", err))
                .style(Style::default().fg(Color::Red))
                .block(block),
            area,
        );
        return;
    }

    let items: Vec<ListItem> = app
        .commits
        .iter()
        .map(|c| {
            ListItem::new(Line::from(vec![
                Span::styled(format!("{} ", c.sha), Style::default().fg(Color::Yellow)),
                Span::styled(
                    format!("{:<14}", truncate(&c.author, 14)),
                    Style::default().fg(Color::Cyan),
                ),
                Span::raw(" "),
                Span::raw(c.subject.clone()),
                Span::styled(
                    format!("  ({})", c.date),
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();

    let mut list = List::new(items).block(block);
    if app.commits_focused {
        list = list
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▶ ");
    }
    let mut state = app.commits_list_state.clone();
    f.render_stateful_widget(list, area, &mut state);
}

fn render_commits_into(
    f: &mut ratatui::Frame,
    area: Rect,
    block: Block,
    commits: &[Commit],
    err: Option<&str>,
) {
    if let Some(err) = err {
        f.render_widget(
            Paragraph::new(format!("error: {}", err))
                .style(Style::default().fg(Color::Red))
                .block(block),
            area,
        );
        return;
    }
    let items: Vec<ListItem> = commits
        .iter()
        .map(|c| {
            ListItem::new(Line::from(vec![
                Span::styled(format!("{} ", c.sha), Style::default().fg(Color::Yellow)),
                Span::styled(
                    format!("{:<14}", truncate(&c.author, 14)),
                    Style::default().fg(Color::Cyan),
                ),
                Span::raw(" "),
                Span::raw(c.subject.clone()),
                Span::styled(
                    format!("  ({})", c.date),
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();
    f.render_widget(List::new(items).block(block), area);
}

fn render_prs(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = Block::default().borders(Borders::ALL).title(" open PRs ");
    render_prs_into(f, area, block, &app.prs, app.prs_err.as_deref(), "no open PRs");
}

fn render_prs_into(
    f: &mut ratatui::Frame,
    area: Rect,
    block: Block,
    prs: &[PullRequest],
    err: Option<&str>,
    empty_msg: &str,
) {
    if let Some(err) = err {
        f.render_widget(
            Paragraph::new(format!("error: {}", err))
                .style(Style::default().fg(Color::Red))
                .block(block)
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    if prs.is_empty() {
        f.render_widget(
            Paragraph::new(empty_msg.to_string())
                .style(Style::default().fg(Color::DarkGray))
                .block(block),
            area,
        );
        return;
    }
    let items: Vec<ListItem> = prs
        .iter()
        .map(|p| {
            let draft = if p.is_draft {
                Span::styled(" [draft]", Style::default().fg(Color::DarkGray))
            } else {
                Span::raw("")
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("#{:<5}", p.number),
                    Style::default().fg(Color::Green),
                ),
                Span::raw(p.title.clone()),
                draft,
                Span::styled(
                    format!("  @{} ({})", p.author.login, p.head),
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();
    f.render_widget(List::new(items).block(block), area);
}

fn render_org_screen(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);

    render_subtabs(f, rows[0], app);
    match app.org.subview {
        OrgSubview::Activity => render_org_activity(f, rows[1], app),
        OrgSubview::Repos => render_org_repos(f, rows[1], app),
        OrgSubview::Users => render_org_users(f, rows[1], app),
    }
}

fn render_subtabs(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let titles = vec![
        Line::from("Activity"),
        Line::from("Repos"),
        Line::from("Users"),
    ];
    let select = match app.org.subview {
        OrgSubview::Activity => 0,
        OrgSubview::Repos => 1,
        OrgSubview::Users => 2,
    };
    let tabs = Tabs::new(titles)
        .block(Block::default().borders(Borders::ALL).title(" view "))
        .select(select)
        .style(Style::default().fg(Color::Gray))
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );
    f.render_widget(tabs, area);
}

fn render_org_activity(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let title = match &app.org.name {
        Some(n) => format!(" activity by user — {} ", n),
        None => " activity by user ".to_string(),
    };
    let block = Block::default().borders(Borders::ALL).title(title);

    if let Some(err) = &app.org.detect_err {
        f.render_widget(
            Paragraph::new(format!("could not detect org: {}", err))
                .style(Style::default().fg(Color::Red))
                .block(block)
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }

    if !app.org.activity_loaded {
        f.render_widget(
            Paragraph::new("loading…")
                .style(Style::default().fg(Color::DarkGray))
                .block(block),
            area,
        );
        return;
    }

    if let Some(err) = &app.org.activity_err {
        f.render_widget(
            Paragraph::new(format!("error: {}", err))
                .style(Style::default().fg(Color::Red))
                .block(block)
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }

    if app.org.activity.is_empty() {
        f.render_widget(
            Paragraph::new("no recent activity")
                .style(Style::default().fg(Color::DarkGray))
                .block(block),
            area,
        );
        return;
    }

    let mut lines: Vec<Line> = Vec::new();
    for user in &app.org.activity {
        lines.push(Line::from(vec![
            Span::styled(
                format!("@{}", user.login),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  ({} events)", user.events.len()),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
        for ev in &user.events {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    format!("{:<10}", short_kind(&ev.kind)),
                    Style::default().fg(Color::Magenta),
                ),
                Span::raw(" "),
                Span::styled(
                    format!("{:<28}", truncate(&ev.repo, 28)),
                    Style::default().fg(Color::Green),
                ),
                Span::raw(" "),
                Span::raw(ev.detail.clone()),
                Span::styled(
                    format!("  ({})", ev.when),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
        lines.push(Line::from(""));
    }
    f.render_widget(
        Paragraph::new(lines)
            .block(block)
            .scroll((app.org_scroll, 0)),
        area,
    );
}

fn render_org_repos(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);

    render_filter_input(f, rows[0], app);
    render_repo_list(f, rows[1], app);
}

fn render_filter_input(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let mode = if app.org.repos.filtering {
        " filter (typing — Enter to confirm, Esc to clear) "
    } else if app.org.repos.filter.is_empty() {
        " filter (/ to start) "
    } else {
        " filter (/ to edit, Esc to clear) "
    };
    let cursor = if app.org.repos.filtering { "▏" } else { "" };
    let body = format!("{}{}", app.org.repos.filter, cursor);
    let style = if app.org.repos.filtering {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::Gray)
    };
    let block = Block::default().borders(Borders::ALL).title(mode);
    f.render_widget(Paragraph::new(body).style(style).block(block), area);
}

fn render_repo_list(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let title = match &app.org.name {
        Some(n) => format!(" repos — {} ", n),
        None => " repos ".to_string(),
    };
    let block = Block::default().borders(Borders::ALL).title(title);

    if let Some(err) = &app.org.detect_err {
        f.render_widget(
            Paragraph::new(format!("could not detect org: {}", err))
                .style(Style::default().fg(Color::Red))
                .block(block)
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }

    if !app.org.repos.loaded {
        f.render_widget(
            Paragraph::new("loading…")
                .style(Style::default().fg(Color::DarkGray))
                .block(block),
            area,
        );
        return;
    }

    if let Some(err) = &app.org.repos.error {
        f.render_widget(
            Paragraph::new(format!("error: {}", err))
                .style(Style::default().fg(Color::Red))
                .block(block)
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }

    let indices = app.filtered_repo_indices();
    if indices.is_empty() {
        let msg = if app.org.repos.filter.is_empty() {
            "no repos found"
        } else {
            "no matches"
        };
        f.render_widget(
            Paragraph::new(msg.to_string())
                .style(Style::default().fg(Color::DarkGray))
                .block(block),
            area,
        );
        return;
    }

    let items: Vec<ListItem> = indices
        .iter()
        .map(|&i| {
            let r = &app.org.repos.repos[i];
            let lang = r.primary_language.as_deref().unwrap_or("-");
            let pushed = r
                .pushed_at
                .as_deref()
                .map(humanize_short)
                .unwrap_or_else(|| "?".into());
            let priv_marker = if r.is_private { "🔒" } else { "  " };
            let stars = if r.stargazer_count > 0 {
                format!("★{:<4}", r.stargazer_count)
            } else {
                "     ".into()
            };
            ListItem::new(Line::from(vec![
                Span::raw(format!("{} ", priv_marker)),
                Span::styled(
                    format!("{:<38}", truncate(&r.name, 38)),
                    Style::default().fg(Color::Green),
                ),
                Span::styled(
                    format!("{:<14}", truncate(lang, 14)),
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(stars, Style::default().fg(Color::Yellow)),
                Span::raw(" "),
                Span::styled(
                    format!("pushed {}", pushed),
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    let mut state = app.org.repos.list_state.clone();
    f.render_stateful_widget(list, area, &mut state);
}

fn render_commit_detail_screen(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let Some(view) = &app.commit_detail else {
        return;
    };

    let block = Block::default().borders(Borders::ALL).title(" commit ");
    if let Some(err) = &view.error {
        f.render_widget(
            Paragraph::new(format!("error: {}", err))
                .style(Style::default().fg(Color::Red))
                .block(block)
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    let Some(detail) = &view.detail else {
        return;
    };

    let header_lines = 3
        + if detail.body.trim().is_empty() {
            0
        } else {
            (detail.body.lines().count() + 1) as u16
        };
    let header_height = header_lines.clamp(4, 12);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(header_height), Constraint::Min(0)])
        .split(area);

    render_commit_header(f, rows[0], detail);
    render_commit_files(f, rows[1], detail, view.scroll);
}

fn render_commit_header(f: &mut ratatui::Frame, area: Rect, detail: &CommitDetail) {
    let short_sha: String = detail.sha.chars().take(12).collect();
    let title = format!(" {} ", short_sha);
    let block = Block::default().borders(Borders::ALL).title(title);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("author  ", Style::default().fg(Color::DarkGray)),
        Span::styled(detail.author.clone(), Style::default().fg(Color::Cyan)),
        Span::raw(format!(" <{}>", detail.email)),
    ]));
    lines.push(Line::from(vec![
        Span::styled("date    ", Style::default().fg(Color::DarkGray)),
        Span::raw(detail.date.clone()),
    ]));
    lines.push(Line::from(vec![
        Span::styled("subject ", Style::default().fg(Color::DarkGray)),
        Span::styled(detail.subject.clone(), Style::default().add_modifier(Modifier::BOLD)),
    ]));
    if !detail.body.trim().is_empty() {
        lines.push(Line::from(""));
        for body_line in detail.body.lines() {
            lines.push(Line::from(body_line.to_string()));
        }
    }
    f.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: false }),
        area,
    );
}

fn render_commit_files(f: &mut ratatui::Frame, area: Rect, detail: &CommitDetail, scroll: u16) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" files changed ");
    if detail.stat_lines.is_empty() {
        f.render_widget(
            Paragraph::new("(no file changes)")
                .style(Style::default().fg(Color::DarkGray))
                .block(block),
            area,
        );
        return;
    }
    let lines: Vec<Line> = detail
        .stat_lines
        .iter()
        .map(|s| Line::from(colorize_stat_line(s)))
        .collect();
    f.render_widget(
        Paragraph::new(lines).block(block).scroll((scroll, 0)),
        area,
    );
}

fn colorize_stat_line(s: &str) -> Vec<Span<'static>> {
    // Summary line e.g. " 2 files changed, 12 insertions(+), 9 deletions(-)"
    if !s.contains('|') {
        return vec![Span::styled(
            s.to_string(),
            Style::default().fg(Color::DarkGray),
        )];
    }
    // File line e.g. " src/main.rs   | 15 ++++++-----"
    let mut out = Vec::new();
    if let Some((left, right)) = s.split_once('|') {
        out.push(Span::raw(left.to_string()));
        out.push(Span::styled("|".to_string(), Style::default().fg(Color::DarkGray)));
        for ch in right.chars() {
            match ch {
                '+' => out.push(Span::styled(
                    "+".to_string(),
                    Style::default().fg(Color::Green),
                )),
                '-' => out.push(Span::styled(
                    "-".to_string(),
                    Style::default().fg(Color::Red),
                )),
                c => out.push(Span::raw(c.to_string())),
            }
        }
    } else {
        out.push(Span::raw(s.to_string()));
    }
    out
}

fn render_org_users(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);

    render_user_filter_input(f, rows[0], app);
    render_user_list(f, rows[1], app);
}

fn render_user_filter_input(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let mode = if app.org.users.filtering {
        " filter (typing — Enter to confirm, Esc to clear) "
    } else if app.org.users.filter.is_empty() {
        " filter (/ to start) "
    } else {
        " filter (/ to edit, Esc to clear) "
    };
    let cursor = if app.org.users.filtering { "▏" } else { "" };
    let body = format!("{}{}", app.org.users.filter, cursor);
    let style = if app.org.users.filtering {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::Gray)
    };
    let block = Block::default().borders(Borders::ALL).title(mode);
    f.render_widget(Paragraph::new(body).style(style).block(block), area);
}

fn render_user_list(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let title = match &app.org.name {
        Some(n) => format!(" users — {} ", n),
        None => " users ".to_string(),
    };
    let block = Block::default().borders(Borders::ALL).title(title);

    if let Some(err) = &app.org.detect_err {
        f.render_widget(
            Paragraph::new(format!("could not detect org: {}", err))
                .style(Style::default().fg(Color::Red))
                .block(block)
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }

    if !app.org.users.loaded {
        f.render_widget(
            Paragraph::new("loading…")
                .style(Style::default().fg(Color::DarkGray))
                .block(block),
            area,
        );
        return;
    }

    if let Some(err) = &app.org.users.error {
        f.render_widget(
            Paragraph::new(format!("error: {}", err))
                .style(Style::default().fg(Color::Red))
                .block(block)
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }

    let indices = app.filtered_user_indices();
    if indices.is_empty() {
        let msg = if app.org.users.filter.is_empty() {
            "no visible members"
        } else {
            "no matches"
        };
        f.render_widget(
            Paragraph::new(msg.to_string())
                .style(Style::default().fg(Color::DarkGray))
                .block(block),
            area,
        );
        return;
    }

    let items: Vec<ListItem> = indices
        .iter()
        .map(|&i| {
            let m = &app.org.users.members[i];
            ListItem::new(Line::from(vec![Span::styled(
                format!("@{}", m.login),
                Style::default().fg(Color::Cyan),
            )]))
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    let mut state = app.org.users.list_state.clone();
    f.render_stateful_widget(list, area, &mut state);
}

fn render_pr_detail_screen(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let Some(view) = &app.pr_detail else {
        return;
    };

    let block = Block::default().borders(Borders::ALL).title(" pull request ");
    if let Some(err) = &view.error {
        f.render_widget(
            Paragraph::new(format!("error: {}", err))
                .style(Style::default().fg(Color::Red))
                .block(block)
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    let Some(detail) = &view.detail else {
        return;
    };

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Min(0)])
        .split(area);

    render_pr_detail_header(f, rows[0], detail);
    render_pr_detail_body(f, rows[1], detail, view.scroll);
}

fn render_pr_detail_header(f: &mut ratatui::Frame, area: Rect, detail: &PrDetail) {
    let title = format!(" #{} — {} ", detail.number, truncate(&detail.title, 80));
    let block = Block::default().borders(Borders::ALL).title(title);

    let (state_label, state_color) = pr_state_style(&detail.state, detail.is_draft);
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("state    ", Style::default().fg(Color::DarkGray)),
        Span::styled(state_label.to_string(), Style::default().fg(state_color)),
        Span::raw("   "),
        Span::styled("author ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("@{}", detail.author),
            Style::default().fg(Color::Cyan),
        ),
        Span::raw("   "),
        Span::styled("updated ", Style::default().fg(Color::DarkGray)),
        Span::raw(detail.updated_at.clone()),
    ]));
    lines.push(Line::from(vec![
        Span::styled("branch   ", Style::default().fg(Color::DarkGray)),
        Span::styled(detail.head_ref.clone(), Style::default().fg(Color::Yellow)),
        Span::styled(" → ", Style::default().fg(Color::DarkGray)),
        Span::styled(detail.base_ref.clone(), Style::default().fg(Color::Yellow)),
        Span::raw("   "),
        Span::styled(
            format!("+{}", detail.additions),
            Style::default().fg(Color::Green),
        ),
        Span::raw(" "),
        Span::styled(
            format!("-{}", detail.deletions),
            Style::default().fg(Color::Red),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("url      ", Style::default().fg(Color::DarkGray)),
        Span::styled(detail.url.clone(), Style::default().fg(Color::Blue)),
        Span::styled("   (press 'o' to open)", Style::default().fg(Color::DarkGray)),
    ]));
    f.render_widget(Paragraph::new(lines).block(block).wrap(Wrap { trim: false }), area);
}

fn render_pr_detail_body(f: &mut ratatui::Frame, area: Rect, detail: &PrDetail, scroll: u16) {
    let block = Block::default().borders(Borders::ALL).title(" description ");
    let body = if detail.body.trim().is_empty() {
        "(no description)".to_string()
    } else {
        detail.body.replace("\r\n", "\n")
    };
    let style = if detail.body.trim().is_empty() {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default()
    };
    f.render_widget(
        Paragraph::new(body)
            .style(style)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        area,
    );
}

fn render_notification_detail_screen(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let Some(view) = &app.notification_detail else {
        return;
    };
    let n = &view.notification;
    let title = format!(" notification — {} ", n.kind);
    let block = Block::default().borders(Borders::ALL).title(title);

    let reason_color = match n.reason.as_str() {
        "review_requested" => Color::Yellow,
        "mention" | "team_mention" => Color::Magenta,
        "assign" => Color::Cyan,
        "ci_activity" => Color::Red,
        _ => Color::DarkGray,
    };

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("reason  ", Style::default().fg(Color::DarkGray)),
        Span::styled(n.reason.clone(), Style::default().fg(reason_color)),
    ]));
    lines.push(Line::from(vec![
        Span::styled("type    ", Style::default().fg(Color::DarkGray)),
        Span::raw(n.kind.clone()),
    ]));
    lines.push(Line::from(vec![
        Span::styled("repo    ", Style::default().fg(Color::DarkGray)),
        Span::styled(n.repo.clone(), Style::default().fg(Color::Green)),
    ]));
    lines.push(Line::from(vec![
        Span::styled("title   ", Style::default().fg(Color::DarkGray)),
        Span::styled(n.title.clone(), Style::default().add_modifier(Modifier::BOLD)),
    ]));
    lines.push(Line::from(vec![
        Span::styled("status  ", Style::default().fg(Color::DarkGray)),
        if n.unread {
            Span::styled("● unread", Style::default().fg(Color::Yellow))
        } else {
            Span::styled("read", Style::default().fg(Color::DarkGray))
        },
    ]));
    lines.push(Line::from(vec![
        Span::styled("updated ", Style::default().fg(Color::DarkGray)),
        Span::raw(n.updated_at.clone()),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("url     ", Style::default().fg(Color::DarkGray)),
        Span::styled(n.web_url.clone(), Style::default().fg(Color::Blue)),
    ]));
    lines.push(Line::from(Span::styled(
        "(press 'o' to open in browser)",
        Style::default().fg(Color::DarkGray),
    )));

    f.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: false }),
        area,
    );
}

fn render_user_detail_screen(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let Some(detail) = &app.user_detail else {
        return;
    };

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Percentage(34),
            Constraint::Percentage(33),
            Constraint::Percentage(33),
        ])
        .split(area);

    render_user_detail_header(f, rows[0], detail, app.org.name.as_deref());
    render_user_commits_panel(f, rows[1], detail);
    render_user_prs_panel(
        f,
        rows[2],
        " PRs submitted ",
        &detail.submitted_prs,
        detail.submitted_err.as_deref(),
        "no submitted PRs",
        false,
    );
    render_user_prs_panel(
        f,
        rows[3],
        " PRs reviewed ",
        &detail.reviewed_prs,
        detail.reviewed_err.as_deref(),
        "no reviewed PRs",
        true,
    );
}

fn render_user_detail_header(
    f: &mut ratatui::Frame,
    area: Rect,
    detail: &UserDetailState,
    org: Option<&str>,
) {
    let title = format!(" @{} ", detail.login);
    let block = Block::default().borders(Borders::ALL).title(title);
    let org_label = org.unwrap_or("(unknown)");
    let line = Line::from(vec![
        Span::styled("org: ", Style::default().fg(Color::DarkGray)),
        Span::raw(org_label.to_string()),
        Span::raw("   "),
        Span::styled("commits: ", Style::default().fg(Color::DarkGray)),
        Span::raw(detail.commits.len().to_string()),
        Span::raw("   "),
        Span::styled("submitted: ", Style::default().fg(Color::DarkGray)),
        Span::raw(detail.submitted_prs.len().to_string()),
        Span::raw("   "),
        Span::styled("reviewed: ", Style::default().fg(Color::DarkGray)),
        Span::raw(detail.reviewed_prs.len().to_string()),
    ]);
    f.render_widget(Paragraph::new(vec![line]).block(block), area);
}

fn render_user_commits_panel(f: &mut ratatui::Frame, area: Rect, detail: &UserDetailState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" recent commits ");
    if let Some(err) = &detail.commits_err {
        f.render_widget(
            Paragraph::new(format!("error: {}", err))
                .style(Style::default().fg(Color::Red))
                .block(block)
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    if detail.commits.is_empty() {
        f.render_widget(
            Paragraph::new("no recent commits")
                .style(Style::default().fg(Color::DarkGray))
                .block(block),
            area,
        );
        return;
    }
    let items: Vec<ListItem> = detail
        .commits
        .iter()
        .map(|c| {
            ListItem::new(Line::from(vec![
                Span::styled(format!("{} ", c.sha), Style::default().fg(Color::Yellow)),
                Span::styled(
                    format!("{:<28}", truncate(&c.repo, 28)),
                    Style::default().fg(Color::Green),
                ),
                Span::raw(" "),
                Span::raw(c.subject.clone()),
                Span::styled(
                    format!("  ({})", c.date),
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();
    f.render_widget(List::new(items).block(block), area);
}

fn render_user_prs_panel(
    f: &mut ratatui::Frame,
    area: Rect,
    title: &str,
    prs: &[UserPr],
    err: Option<&str>,
    empty_msg: &str,
    show_author: bool,
) {
    let block = Block::default().borders(Borders::ALL).title(title.to_string());
    if let Some(err) = err {
        f.render_widget(
            Paragraph::new(format!("error: {}", err))
                .style(Style::default().fg(Color::Red))
                .block(block)
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    if prs.is_empty() {
        f.render_widget(
            Paragraph::new(empty_msg.to_string())
                .style(Style::default().fg(Color::DarkGray))
                .block(block),
            area,
        );
        return;
    }
    let items: Vec<ListItem> = prs
        .iter()
        .map(|p| {
            let (label, color) = pr_state_style(&p.state, p.is_draft);
            let mut spans = vec![
                Span::styled(format!("#{:<5}", p.number), Style::default().fg(Color::Green)),
                Span::styled(format!("{:<7}", label), Style::default().fg(color)),
                Span::styled(
                    format!("{:<28}", truncate(&p.repo, 28)),
                    Style::default().fg(Color::Green),
                ),
                Span::raw(" "),
                Span::raw(p.title.clone()),
            ];
            if show_author && !p.author.is_empty() {
                spans.push(Span::styled(
                    format!("  @{}", p.author),
                    Style::default().fg(Color::Cyan),
                ));
            }
            spans.push(Span::styled(
                format!("  ({})", p.updated_at),
                Style::default().fg(Color::DarkGray),
            ));
            ListItem::new(Line::from(spans))
        })
        .collect();
    f.render_widget(List::new(items).block(block), area);
}

fn pr_state_style(state: &str, is_draft: bool) -> (&'static str, Color) {
    if is_draft {
        return ("draft", Color::DarkGray);
    }
    match state.to_ascii_lowercase().as_str() {
        "open" => ("open", Color::Green),
        "merged" => ("merged", Color::Magenta),
        "closed" => ("closed", Color::Red),
        _ => ("?", Color::Gray),
    }
}

fn render_repo_detail_screen(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let Some(detail) = &app.repo_detail else {
        return;
    };

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(8), Constraint::Min(0)])
        .split(area);

    render_repo_detail_header(f, rows[0], detail);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(rows[1]);

    render_contributors(f, cols[0], detail);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(cols[1]);

    let commits_block = Block::default()
        .borders(Borders::ALL)
        .title(" recent commits ");
    render_commits_into(
        f,
        right[0],
        commits_block,
        &detail.commits,
        detail.commits_err.as_deref(),
    );

    let prs_block = Block::default().borders(Borders::ALL).title(" recent PRs ");
    render_prs_into(
        f,
        right[1],
        prs_block,
        &detail.prs,
        detail.prs_err.as_deref(),
        "no PRs",
    );
}

fn render_repo_detail_header(f: &mut ratatui::Frame, area: Rect, detail: &RepoDetailState) {
    let title = format!(" {} ", detail.full_name);
    let block = Block::default().borders(Borders::ALL).title(title);

    let mut lines: Vec<Line> = Vec::new();
    if let Some(info) = &detail.info {
        if let Some(desc) = &info.description {
            lines.push(Line::from(Span::raw(desc.clone())));
        }
        let lang = info.primary_language.as_deref().unwrap_or("-");
        let branch = info.default_branch.as_deref().unwrap_or("-");
        let pushed = info
            .pushed_at
            .as_deref()
            .map(humanize_short)
            .unwrap_or_else(|| "?".into());
        let visibility = if info.is_private { "private" } else { "public" };
        lines.push(Line::from(vec![
            kv_span("language", lang),
            Span::raw("   "),
            kv_span("default", branch),
            Span::raw("   "),
            kv_span("pushed", &pushed),
            Span::raw("   "),
            kv_span("visibility", visibility),
        ]));
        lines.push(Line::from(vec![kv_span(
            "stars",
            &info.stargazer_count.to_string(),
        )]));
    } else {
        lines.push(Line::from(Span::styled(
            "(metadata unavailable)",
            Style::default().fg(Color::DarkGray),
        )));
    }

    f.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: false }),
        area,
    );
}

fn render_contributors(f: &mut ratatui::Frame, area: Rect, detail: &RepoDetailState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" top contributors ");
    if let Some(err) = &detail.contributors_err {
        f.render_widget(
            Paragraph::new(format!("error: {}", err))
                .style(Style::default().fg(Color::Red))
                .block(block)
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    if detail.contributors.is_empty() {
        f.render_widget(
            Paragraph::new("none")
                .style(Style::default().fg(Color::DarkGray))
                .block(block),
            area,
        );
        return;
    }
    let items: Vec<ListItem> = detail
        .contributors
        .iter()
        .map(|c| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("@{:<22}", truncate(&c.login, 22)),
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(
                    format!("{} commits", c.contributions),
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();
    f.render_widget(List::new(items).block(block), area);
}

fn render_footer(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let mut spans: Vec<Span> = vec![
        Span::styled("q", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" quit  "),
        Span::styled("r", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" reload  "),
        Span::styled("1/2", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" or "),
        Span::styled("←/→", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" tabs"),
    ];

    match app.screen {
        Screen::Dashboard => {
            if app.dashboard.any_focused() {
                spans.extend([
                    Span::raw("  "),
                    Span::styled("↑↓/jk", Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw(" select  "),
                    Span::styled("Enter", Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw(" detail  "),
                    Span::styled("o", Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw(" open in browser  "),
                    Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw(" unfocus"),
                ]);
            } else {
                spans.extend([
                    Span::raw("  "),
                    Span::styled("v", Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw(" review  "),
                    Span::styled("p", Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw(" my PRs  "),
                    Span::styled("n", Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw(" notifications  "),
                    Span::styled("c", Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw(" my commits"),
                ]);
            }
        }
        Screen::PrDetail | Screen::NotificationDetail => {
            spans.extend([
                Span::raw("  "),
                Span::styled("↑↓/jk PgUp/PgDn", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" scroll  "),
                Span::styled("o", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" open in browser  "),
                Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" back"),
            ]);
        }
        Screen::Repo => {
            if app.commits_focused {
                spans.extend([
                    Span::raw("  "),
                    Span::styled("↑↓/jk", Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw(" select  "),
                    Span::styled("Enter", Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw(" detail  "),
                    Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw(" unfocus"),
                ]);
            } else {
                spans.extend([
                    Span::raw("  "),
                    Span::styled("c", Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw(" focus commits"),
                ]);
            }
        }
        Screen::CommitDetail => {
            spans.extend([
                Span::raw("  "),
                Span::styled("↑↓/jk PgUp/PgDn", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" scroll  "),
                Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" back"),
            ]);
        }
        Screen::Org => {
            spans.extend([
                Span::raw("  "),
                Span::styled("[ ]", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" sub-view"),
            ]);
            match app.org.subview {
                OrgSubview::Activity => spans.extend([
                    Span::raw("  "),
                    Span::styled("↑↓/jk", Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw(" scroll  "),
                    Span::styled("g/G", Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw(" top/bot"),
                ]),
                OrgSubview::Repos => {
                    if app.org.repos.filtering {
                        spans = vec![
                            Span::styled(
                                "type to filter",
                                Style::default().add_modifier(Modifier::BOLD),
                            ),
                            Span::raw("  "),
                            Span::styled("Backspace", Style::default().add_modifier(Modifier::BOLD)),
                            Span::raw(" delete  "),
                            Span::styled("Enter", Style::default().add_modifier(Modifier::BOLD)),
                            Span::raw(" confirm  "),
                            Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
                            Span::raw(" clear"),
                        ];
                    } else {
                        spans.extend([
                            Span::raw("  "),
                            Span::styled("↑↓/jk", Style::default().add_modifier(Modifier::BOLD)),
                            Span::raw(" select  "),
                            Span::styled("Enter", Style::default().add_modifier(Modifier::BOLD)),
                            Span::raw(" open  "),
                            Span::styled("/", Style::default().add_modifier(Modifier::BOLD)),
                            Span::raw(" filter"),
                        ]);
                    }
                }
                OrgSubview::Users => {
                    if app.org.users.filtering {
                        spans = vec![
                            Span::styled(
                                "type to filter",
                                Style::default().add_modifier(Modifier::BOLD),
                            ),
                            Span::raw("  "),
                            Span::styled("Backspace", Style::default().add_modifier(Modifier::BOLD)),
                            Span::raw(" delete  "),
                            Span::styled("Enter", Style::default().add_modifier(Modifier::BOLD)),
                            Span::raw(" confirm  "),
                            Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
                            Span::raw(" clear"),
                        ];
                    } else {
                        spans.extend([
                            Span::raw("  "),
                            Span::styled("↑↓/jk", Style::default().add_modifier(Modifier::BOLD)),
                            Span::raw(" select  "),
                            Span::styled("Enter", Style::default().add_modifier(Modifier::BOLD)),
                            Span::raw(" open  "),
                            Span::styled("/", Style::default().add_modifier(Modifier::BOLD)),
                            Span::raw(" filter"),
                        ]);
                    }
                }
            }
        }
        Screen::RepoDetail | Screen::UserDetail => {
            spans.extend([
                Span::raw("  "),
                Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" back"),
            ]);
        }
    }

    let footer = Paragraph::new(Line::from(spans)).style(Style::default().fg(Color::DarkGray));
    f.render_widget(footer, area);
}

fn kv(key: &str, val: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{:<13}", key),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw(val.to_string()),
    ])
}

fn kv_span(key: &str, val: &str) -> Span<'static> {
    Span::raw(format!("{}: {}", key, val))
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn short_kind(kind: &str) -> String {
    let trimmed = kind.trim_end_matches("Event");
    match trimmed {
        "PullRequest" => "PR".into(),
        "PullRequestReview" => "PR-review".into(),
        "PullRequestReviewComment" => "PR-comment".into(),
        "IssueComment" => "issue-cmt".into(),
        "Issues" => "issue".into(),
        other => other.to_lowercase(),
    }
}

fn humanize_short(ts: &str) -> String {
    if ts.len() >= 16 && ts.as_bytes().get(10) == Some(&b'T') {
        format!("{} {}", &ts[..10], &ts[11..16])
    } else {
        ts.to_string()
    }
}
