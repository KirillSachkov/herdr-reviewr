//! Git access: scopes, changed files, and diffs.
//!
//! The only writes are private refs under `refs/worktree/reviewr/`. Nothing here
//! commits, stages, or mutates the worktree, the index, or any branch.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

use anyhow::{Context, Result, bail};

use crate::model::{ChangeKind, ChangedFile};

/// Every git process reviewr runs, in `repo`. Nothing here may write the real index (the
/// **No writes** invariant): `git diff` would otherwise refresh a stat-dirty entry and take
/// `index.lock` under the agent's own `git add`, and `GIT_OPTIONAL_LOCKS=0` turns off the same
/// opportunistic write in `status`. A run that needs the refresh runs on an [`IndexCopy`].
/// `GIT_DIFF_OPTS` would override the full context the diff sides are read with.
fn git_command(repo: &Path) -> std::process::Command {
    #[cfg(test)]
    GIT_COMMANDS.with(|n| n.set(n.get() + 1));
    let mut cmd = crate::proc::command("git");
    cmd.arg("-C")
        .arg(repo)
        .args(["-c", "diff.autoRefreshIndex=false"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env_remove("GIT_DIFF_OPTS");
    cmd
}

#[cfg(test)]
thread_local! {
    /// Git processes this thread built, for tests that pin a build's spawn budget.
    pub(crate) static GIT_COMMANDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    run(git_command(repo), args)
}

/// Run `cmd` (a [`git_command`]) with `args` and return stdout. Errors on non-zero exit.
fn run(mut cmd: std::process::Command, args: &[&str]) -> Result<String> {
    let out = cmd
        .args(["-c", "core.quotepath=false"])
        .args(args)
        .output()
        .map_err(|e| anyhow::anyhow!(git_error(args, "could not run", e)))?;
    if !out.status.success() {
        bail!(git_error(args, "failed", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A failed git call's message: the subcommand and git's own words for the reviewer, and the
/// whole argv for the log.
fn git_error(args: &[&str], what: &str, detail: impl std::fmt::Display) -> String {
    crate::logln!("git {args:?} {what}: {detail}");
    format!("git {} {what}: {detail}", subcommand(args))
}

/// The git subcommand an argv runs, for an error the reviewer reads: `rev-parse`, not the
/// whole argv in Rust's debug quoting.
fn subcommand<'a>(args: &[&'a str]) -> &'a str {
    let mut rest = args.iter().copied();
    while let Some(arg) = rest.next() {
        match arg {
            // A global option that takes the next word as its value.
            "-c" | "-C" => {
                rest.next();
            }
            arg if arg.starts_with('-') => {}
            arg => return arg,
        }
    }
    ""
}

/// Like [`git`], but returns stdout even on non-zero exit (e.g. `diff --no-index`).
fn git_lenient(repo: &Path, args: &[&str]) -> String {
    git_command(repo)
        .args(["-c", "core.quotepath=false"])
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// Run `git -C <repo> <args>` and return its trimmed stdout, or `None` if the command fails to
/// spawn, exits non-zero, or prints nothing. The one-line query workhorse for `rev-parse`/`merge-base`.
fn git_line(repo: &Path, args: &[&str]) -> Option<String> {
    let out = git_command(repo).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!line.is_empty()).then_some(line)
}

/// Whether `git -C <repo> <args>` spawns and exits zero. The predicate workhorse for existence checks.
fn git_ok(repo: &Path, args: &[&str]) -> bool {
    git_command(repo).args(args).output().is_ok_and(|o| o.status.success())
}

/// Whether `path` is inside a git work tree.
pub fn is_repo(path: &Path) -> bool {
    git_ok(path, &["rev-parse", "--is-inside-work-tree"])
}

/// `core.editor` for `repo` from any config level, or `None` when no level sets it. Git for
/// Windows' installer writes it, and `$EDITOR` is rarely set there, so it is the editor most
/// Windows users picked. reviewr reads it last, after the `editor` key, `$VISUAL`, and `$EDITOR`
/// (`editor::resolve`). git's own order differs: `$GIT_EDITOR`, then `core.editor`, then
/// `$VISUAL`, then `$EDITOR`.
pub fn core_editor(repo: &Path) -> Option<String> {
    git_line(repo, &["config", "--get", "core.editor"])
}

/// The git top-level of `path`, or `None` if it is not a repo. Collapses "git ran and said no"
/// and "git could not run" — use [`worktree_of`] when that difference matters.
pub fn toplevel(path: &Path) -> Option<PathBuf> {
    match worktree_of(path) {
        Worktree::Root(root) => Some(root),
        Worktree::Outside | Worktree::Unknown => None,
    }
}

/// A directory's git top level, keeping "git ran and it is outside any worktree" (`Outside`, a
/// determination) apart from "git could not be run at all" (`Unknown`, the absence of one — a
/// spawn error under load). A caller deciding membership must hold on `Unknown` rather than read
/// it as `Outside`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Worktree {
    Root(PathBuf),
    Outside,
    Unknown,
}

/// Resolve `path` to its worktree, distinguishing the two ways resolution yields no root.
pub fn worktree_of(path: &Path) -> Worktree {
    match git_command(path).args(["rev-parse", "--show-toplevel"]).output() {
        Err(_) => Worktree::Unknown,
        Ok(out) if !out.status.success() => Worktree::Outside,
        Ok(out) => match String::from_utf8_lossy(&out.stdout).trim() {
            "" => Worktree::Outside,
            root => Worktree::Root(PathBuf::from(root)),
        },
    }
}

/// The forge a repository target belongs to. Part of the target's identity: the same path on
/// a different forge is a different target.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Forge {
    /// The default carries the neutral `PR` vocabulary a forgeless state renders under.
    #[default]
    GitHub,
    GitLab,
    AzureDevOps,
}

/// The per-forge display vocabulary — the CLI, noun, and reference table in
impl Forge {
    /// The forge's display name for link labels and failure wording.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::GitHub => "GitHub",
            Self::GitLab => "GitLab",
            Self::AzureDevOps => "Azure DevOps",
        }
    }

    /// The forge's full noun: the word its users say.
    pub fn noun(self) -> &'static str {
        match self {
            Self::GitHub | Self::AzureDevOps => "pull request",
            Self::GitLab => "merge request",
        }
    }

    /// The forge's noun abbreviation: `PR` on GitHub, `MR` on GitLab.
    pub fn abbr(self) -> &'static str {
        match self {
            Self::GitHub | Self::AzureDevOps => "PR",
            Self::GitLab => "MR",
        }
    }

    /// The reference sigil before a number: `#226` on GitHub, `!42` on GitLab.
    pub fn sigil(self) -> char {
        match self {
            Self::GitHub | Self::AzureDevOps => '#',
            Self::GitLab => '!',
        }
    }

    /// The forge CLI's binary name.
    pub fn cli(self) -> &'static str {
        match self {
            Self::GitHub => "gh",
            Self::GitLab => "glab",
            Self::AzureDevOps => "az",
        }
    }
}

/// The self-hosted hostnames one validated config snapshot adds, one per forge
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ForgeHosts<'a> {
    pub github: Option<&'a str>,
    pub gitlab: Option<&'a str>,
    pub azure_devops: Option<&'a str>,
}

/// A canonical forge repository target: the forge, its hostname, and the repository path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoTarget {
    forge: Forge,
    host: String,
    /// Exactly `[owner, name]` for GitHub; the full namespace path (2+ segments) for GitLab;
    /// exactly `[organization, project, repository]` for Azure DevOps.
    path: Vec<String>,
}

impl RepoTarget {
    /// Build one canonical GitHub repository target from a hostname and owner/name pair.
    #[cfg(test)]
    pub(crate) fn new(host: &str, owner: &str, name: &str) -> Option<Self> {
        Self::with_path(Forge::GitHub, host, &[owner, name])
    }

    /// Build one canonical target from a forge, hostname, and validated path segments.
    pub(crate) fn with_path(forge: Forge, host: &str, segments: &[&str]) -> Option<Self> {
        let host = host.to_ascii_lowercase();
        let valid_len = match forge {
            Forge::GitHub => segments.len() == 2,
            // GitLab reserves `-` as the separator between a project path and the rest of a web
            // URL, so a pasted browse link is a malformed remote, not a deep namespace.
            Forge::GitLab => segments.len() >= 2 && !segments.contains(&"-"),
            // Always `[organization, project, repository]`, shaped by `ado_canonicalize`.
            Forge::AzureDevOps => segments.len() == 3,
        };
        // Azure DevOps project and repository names admit spaces and non-ASCII characters,
        // which arrive percent-encoded and are decoded by `ado_canonicalize`.
        let valid_component: fn(&str) -> bool = match forge {
            Forge::AzureDevOps => valid_ado_component,
            _ => valid_repository_component,
        };
        let components_ok = segments.iter().all(|part| valid_component(part));
        (crate::config::valid_host_syntax(&host) && valid_len && components_ok).then(|| Self {
            forge,
            host,
            path: segments.iter().map(|part| (*part).to_string()).collect(),
        })
    }

    /// The forge this target lives on.
    pub fn forge(&self) -> Forge {
        self.forge
    }

    /// The lowercase canonical forge hostname.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The first path segment — the owner at the GitHub API boundary, the organization at
    /// the Azure DevOps one.
    pub fn owner(&self) -> &str {
        &self.path[0]
    }

    /// The last path segment — the repository name at the GitHub API boundary.
    pub fn name(&self) -> &str {
        self.path.last().expect("a target has 2+ segments")
    }

    /// The full slash-joined repository path — the GitLab project identity.
    pub fn full_path(&self) -> String {
        self.path.join("/")
    }

    /// Whether `other` names the same repository — the identity the PR association matches
    /// on. The derived `==` stays exact on purpose: it is the input tag's change detector,
    /// where a respelled remote must still start a fresh fetch. Forge paths and hosts compare
    /// case-insensitively, as every supported forge resolves them. Azure DevOps cloud
    /// serves one organization under two hosts (`dev.azure.com` and the legacy
    /// `{org}.visualstudio.com`); the organization is in the path either way.
    pub fn is(&self, other: &Self) -> bool {
        let azure_cloud =
            |host: &str| host == "dev.azure.com" || host.ends_with(".visualstudio.com");
        let same_host = self.host == other.host
            || (self.forge == Forge::AzureDevOps
                && azure_cloud(&self.host)
                && azure_cloud(&other.host));
        self.forge == other.forge
            && same_host
            && self.path.len() == other.path.len()
            && self.path.iter().zip(&other.path).all(|(a, b)| a.eq_ignore_ascii_case(b))
    }

    /// The second path segment — the project at the Azure DevOps API boundary, whose
    /// targets always carry `[organization, project, repository]`.
    pub fn project(&self) -> &str {
        &self.path[1]
    }
}

fn valid_repository_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

/// An Azure DevOps identity segment after percent-decoding: any visible name, so long as it
/// cannot smuggle a path step, an option-shaped token, or a control sequence into a CLI
/// argument. A segment reaches `az` as the argv token after `--project`/`--repository`, so a
/// leading `-` must never pass — the same rule git applies to its own refnames.
fn valid_ado_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.starts_with('-')
        && !value.contains('/')
        && value.chars().all(|c| !c.is_control())
}

/// Host classification for one candidate repository remote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepositoryIdentity {
    Repository(RepoTarget),
    Missing,
    Hostless,
    Unsupported(String),
    Malformed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RemoteTransport {
    Ssh,
    Hosted,
    Unsupported,
}

/// Classify one repository URL against the built-in forge hosts and the configured
/// self-hosted keys.
fn classify_remote(url: &str, hosts: &ForgeHosts<'_>) -> RepositoryIdentity {
    let Some((transport, host, path, has_port)) = split_remote(url) else {
        return RepositoryIdentity::Hostless;
    };
    if host.is_empty() {
        return RepositoryIdentity::Hostless;
    }
    let host = host.to_ascii_lowercase();
    if transport == RemoteTransport::Unsupported
        || (transport == RemoteTransport::Hosted && has_port)
    {
        return RepositoryIdentity::Unsupported(host);
    }
    let Some(forge) = forge_for_host(&host, hosts) else {
        return RepositoryIdentity::Unsupported(host);
    };
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let segments: Vec<&str> = path.split('/').collect();
    let target = match forge {
        Forge::AzureDevOps => ado_canonicalize(&host, &segments).and_then(|(host, segments)| {
            let segments: Vec<&str> = segments.iter().map(String::as_str).collect();
            RepoTarget::with_path(forge, &host, &segments)
        }),
        _ => RepoTarget::with_path(forge, &host, &segments),
    };
    match target {
        Some(target) => RepositoryIdentity::Repository(target),
        None => RepositoryIdentity::Malformed(host),
    }
}

/// The forge that recognizes `host`, if any — the one authority for the built-in host set.
/// Config validation asks it with default hosts, so the sets cannot drift. Config validation
/// also keeps the host sets disjoint, so at most one forge matches.
/// `*.visualstudio.com` is the one built-in wildcard, matching every legacy Azure DevOps
/// organization host by suffix.
pub(crate) fn forge_for_host(host: &str, hosts: &ForgeHosts<'_>) -> Option<Forge> {
    if host == "github.com" || hosts.github == Some(host) {
        return Some(Forge::GitHub);
    }
    if host == "gitlab.com" || hosts.gitlab == Some(host) {
        return Some(Forge::GitLab);
    }
    if host == "dev.azure.com"
        || host == "ssh.dev.azure.com"
        || host.strip_suffix(".visualstudio.com").is_some_and(|label| !label.is_empty())
        || hosts.azure_devops == Some(host)
    {
        return Some(Forge::AzureDevOps);
    }
    None
}

/// Canonicalize an Azure DevOps remote into its one target identity: the canonical host and
/// the `[organization, project, repository]` path. The ssh hosts
/// fold into their https equivalents, the `v3` and `_git` URL markers drop, a legacy
/// `{org}.visualstudio.com` host contributes the organization segment, and each segment
/// percent-decodes — a project named with a space travels as `%20` in the remote URL but is
/// addressed decoded at the CLI boundary.
fn ado_canonicalize(host: &str, segments: &[&str]) -> Option<(String, Vec<String>)> {
    // The ssh forms carry a leading `v3` marker and their own hostnames.
    let (host, segments): (String, Vec<&str>) = match host {
        "ssh.dev.azure.com" => {
            ("dev.azure.com".to_string(), segments.strip_prefix(&["v3"])?.to_vec())
        }
        "vs-ssh.visualstudio.com" => {
            let rest = segments.strip_prefix(&["v3"])?;
            let org = rest.first()?.to_ascii_lowercase();
            (format!("{org}.visualstudio.com"), rest.to_vec())
        }
        // A legacy https host names the organization; hoist it into the path.
        _ => match host.strip_suffix(".visualstudio.com") {
            Some(org) => {
                let mut with_org = vec![org];
                with_org.extend_from_slice(segments);
                (host.to_string(), with_org)
            }
            None => (host.to_string(), segments.to_vec()),
        },
    };
    let saw_git_marker = segments.contains(&"_git");
    // `DefaultCollection` is URL filler only on the legacy organization hosts, whose
    // organization lives in the hostname. On every other host the first segment is the
    // organization or collection identity and stays.
    let org_host = host.ends_with(".visualstudio.com");
    let mut path: Vec<String> = segments
        .iter()
        .copied()
        .filter(|s| *s != "_git" && !(org_host && *s == "DefaultCollection"))
        .map(percent_decode)
        .collect::<Option<_>>()?;
    // `…/{org}/_git/{repo}` is the short form for a repository named after its project.
    if path.len() == 2 && saw_git_marker {
        path.push(path[1].clone());
    }
    // Azure DevOps treats the organization case-insensitively, and the legacy host form
    // derives it from the lowercased hostname — lowercase it everywhere, so every clone
    // form and casing of one repository is one target.
    if let Some(organization) = path.first_mut() {
        *organization = organization.to_ascii_lowercase();
    }
    (path.len() == 3).then_some((host, path))
}

/// Decode `%XX` escapes in one URL path segment, or `None` when an escape is broken or the
/// bytes are not UTF-8. A segment with no escapes passes through unchanged.
fn percent_decode(segment: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(segment.len());
    let mut rest = segment.bytes();
    while let Some(byte) = rest.next() {
        if byte == b'%' {
            let hex = [rest.next()?, rest.next()?];
            let hex = std::str::from_utf8(&hex).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
        } else {
            bytes.push(byte);
        }
    }
    String::from_utf8(bytes).ok()
}

/// Split a Git remote URL into transport, host, and path for scheme and scp-style forms.
fn split_remote(url: &str) -> Option<(RemoteTransport, &str, &str, bool)> {
    if let Some((scheme, rest)) = url.split_once("://") {
        let rest = rest.split_once('@').map_or(rest, |(_, r)| r); // drop `user@`
        let (hostport, path) = rest.split_once('/').unwrap_or((rest, ""));
        let (host, port) = hostport.split_once(':').map_or((hostport, None), |(h, p)| (h, Some(p)));
        let transport = match scheme.to_ascii_lowercase().as_str() {
            "ssh" => RemoteTransport::Ssh,
            "http" | "https" | "git" => RemoteTransport::Hosted,
            _ => RemoteTransport::Unsupported,
        };
        Some((transport, host, path, port.is_some()))
    } else {
        // scp-like `[user@]host:path` — the first `:` splits host from path.
        let (hostpart, path) = url.split_once(':')?;
        let host = hostpart.split_once('@').map_or(hostpart, |(_, h)| h);
        Some((RemoteTransport::Ssh, host, path, false))
    }
}

// --- PR-fetch local reads (published heads) ------------------------------------
//
// Repository selection and
// branch-state derivation both use the same failure contract: a git command that *fails* is a
// transient [`GitFail`], never read as absence. The caller distinguishes a target read failure
// from a later branch-state failure so only an unproven target replaces the visible snapshot.

/// A git command that failed (spawn error or unexpected non-zero exit) during the PR
/// fetch's local reads — a transient failure per, never absence.
#[derive(Debug)]
pub struct GitFail(pub String);

/// Spawn one PR-fetch git read. `LC_ALL=C` pins Git's messages to English — remote discovery
/// classifies a missing remote by stderr text, which Git otherwise localizes.
fn run_git(repo: &Path, args: &[&str]) -> Result<std::process::Output, GitFail> {
    git_command(repo)
        .env("LC_ALL", "C")
        .args(args)
        .output()
        .map_err(|e| GitFail(git_error(args, "could not run", e)))
}

/// Run git where exit 0 is a value, exit 1 is a designated clean absence (`--verify
/// --quiet`, `symbolic-ref --quiet`, `cat-file -e`), and anything else is a failure.
fn git_tristate(repo: &Path, args: &[&str]) -> Result<Option<String>, GitFail> {
    let out = run_git(repo, args)?;
    if out.status.success() {
        return Ok(Some(String::from_utf8_lossy(&out.stdout).trim().to_string()));
    }
    if out.status.code() == Some(1) {
        return Ok(None);
    }
    Err(GitFail(git_error(args, "failed", String::from_utf8_lossy(&out.stderr).trim())))
}

/// Run git where any non-zero exit is a failure. Exit 0 with empty output is a clean
/// "found nothing" (e.g. `for-each-ref` matching no refs).
fn git_strict(repo: &Path, args: &[&str]) -> Result<String, GitFail> {
    let out = run_git(repo, args)?;
    if !out.status.success() {
        return Err(GitFail(git_error(
            args,
            "failed",
            String::from_utf8_lossy(&out.stderr).trim(),
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Everything that determines one PR fetch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrFetchInput {
    pub repository: RepositoryIdentity,
    /// The `origin` repository, when it is a usable forge identity — on a fork clone it
    /// is the fork, queried beside the target.
    pub origin_repository: Option<RepoTarget>,
    /// The locally derived pins and published heads, read in the same pass.
    pub local: PrLocalState,
}

/// The local identity one PR fetch derives: the pins, the branch, and where its work lives.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrLocalState {
    /// `HEAD` pinned to an OID at the start of the pass; every ancestry test, distance,
    /// and the `sync` count use this pin, so one fetch reads one consistent local state.
    pub head_oid: Option<String>,
    /// The winning base entry pinned to an OID — the paint guard keys on it, so a base
    /// moving mid-fetch never paints a stale verdict.
    pub base_oid: Option<String>,
    /// The checked-out branch. `None` is a detached `HEAD`: no branch, no PR story.
    pub branch: Option<String>,
    /// The branch's published heads: every (repository, branch name) its work was pushed to.
    /// A pull request is this branch's only when its head is one of these.
    pub heads: Vec<Head>,
    /// The pull request a `gh pr checkout` or `glab mr checkout` recorded as the branch's
    /// upstream — an exact key that outranks every name lookup.
    pub pin: Option<PrPin>,
}

/// One published head: a branch name in a forge repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Head {
    pub repo: RepoTarget,
    pub name: String,
}

/// A pull request number in the repository whose pull request ref the branch tracks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrPin {
    pub repo: RepoTarget,
    pub number: u64,
}

impl PrLocalState {
    /// The branch's pin, when it pins a pull request on `forge` — each provider reads only
    /// its own.
    #[must_use]
    pub fn pin_on(&self, forge: Forge) -> Option<&PrPin> {
        self.pin.as_ref().filter(|pin| pin.repo.forge() == forge)
    }

    /// The distinct head branch names, in head order — the forge lookup's query keys.
    #[must_use]
    pub fn head_names(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for head in &self.heads {
            if !names.contains(&head.name) {
                names.push(head.name.clone());
            }
        }
        names
    }
}

/// Derive the pinned `HEAD`, the pinned base, and the branch's published heads — every
/// (repository, branch name) git has evidence the branch's work lives at: its upstream
/// record, its push destination, and the remote-tracking refs at its pushed frontier.
pub(crate) fn pr_local(
    repo: &Path,
    base_flag: Option<&str>,
    hosts: &ForgeHosts<'_>,
) -> Result<PrLocalState, GitFail> {
    let Some(branch) = checked_out_branch(repo)? else {
        return Ok(PrLocalState::default());
    };
    let head_oid = git_tristate(repo, &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"])?;
    let resolution = resolve_base(repo, base_flag)?;
    let bases = resolution.oids();
    let config = GitConfig::read(repo)?;
    let remote_list = remote_names(&config);
    let tips = remote_tips(repo, &remote_list)?;
    let mut remotes = Remotes::new(repo, &config, hosts);
    let push_remote_record = config.get(&format!("branch.{branch}.pushremote"));
    // A base resolved without a configured name (through `origin/HEAD` or a verbatim `--base`
    // rev) is recognized by the recorded remote's tracking tip sitting on it.
    let tracks_base_tip = |remote: &str, name: &str| {
        tips.iter().any(|tip| tip.remote == remote && tip.name == name && bases.contains(&tip.oid))
    };

    // The upstream record. One that only tracks a base (`git switch -c x origin/main`) is no
    // publication — unless the branch also pushes there, which is how `gh`/`glab` record a
    // checked-out fork PR whose head is the fork's `main`.
    let record = match branch_record(&config, &branch) {
        Some((remote, BranchMerge::Branch(name)))
            if push_remote_record != Some(remote.as_str())
                && (resolution.recorded.contains(&name) || tracks_base_tip(&remote, &name)) =>
        {
            None
        }
        record => record,
    };

    let mut heads: Vec<Head> = Vec::new();
    let push_head = |repo: Option<RepoTarget>, name: &str, heads: &mut Vec<Head>| {
        if let Some(repo) = repo
            && !heads.iter().any(|have| have.name == name && have.repo.is(&repo))
        {
            heads.push(Head { repo, name: name.to_string() });
        }
    };
    // Where `git push` sends the branch: git's own chain, ending at `origin` when it exists.
    let push_remote = push_remote_record
        .or_else(|| config.get("remote.pushdefault"))
        .or(record.as_ref().map(|(remote, _)| remote.as_str()))
        .or_else(|| is_named_remote(&config, "origin").then_some("origin"));
    if let Some(value) = push_remote {
        push_head(remotes.resolve(value, true)?, &branch, &mut heads);
    }
    let mut pin = None;
    if let Some((remote, merge)) = &record {
        let recorded = remotes.resolve(remote, false)?;
        match merge {
            BranchMerge::Branch(name) => push_head(recorded, name, &mut heads),
            BranchMerge::Pull(forge, number) => {
                pin = recorded
                    .filter(|repo| repo.forge() == *forge)
                    .map(|repo| PrPin { repo, number: *number });
            }
            BranchMerge::Other => {}
        }
    }
    if let Some(head) = &head_oid
        && !bases.is_empty()
    {
        for (remote, name) in frontier_names(repo, &remote_list, &tips, head, &bases)? {
            push_head(remotes.resolve(&remote, false)?, &name, &mut heads);
        }
    }
    // A frontier of many refs stays bounded, so the per-name forge queries do.
    heads.truncate(8);
    Ok(PrLocalState {
        head_oid,
        base_oid: bases.into_iter().next(),
        branch: Some(branch),
        heads,
        pin,
    })
}

/// The winning base: a branch (origin then local) or any other spelling
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolvedBase {
    Branch { name: String, oid: String },
    Rev { spelling: String, oid: String },
}

impl ResolvedBase {
    fn branch(name: String, oid: String) -> Self {
        Self::Branch { name, oid }
    }

    fn rev(spelling: String, oid: String) -> Self {
        Self::Rev { spelling, oid }
    }

    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Branch { name, .. } => name,
            Self::Rev { spelling, .. } => spelling,
        }
    }

    #[must_use]
    pub fn oid(&self) -> &str {
        match self {
            Self::Branch { oid, .. } | Self::Rev { oid, .. } => oid,
        }
    }
}

/// The chain outcome the header paints: the winner and the first recorded choice the
/// chain skipped because it no longer resolves. The skip rides beside the winner, not
/// inside it, so it survives a chain where nothing resolves at all — a dormant pick
/// never reads as never-chosen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BaseStatus {
    pub winner: Option<ResolvedBase>,
    pub skipped: Option<String>,
}

/// One pass over the base chain. `candidates` keeps every source that resolved, in
/// precedence order and deduped by OID — the PR frontier walk needs all of them, not just
/// the winner (`pr_local`). `recorded` keeps every source name the chain considered —
/// every candidate's name and every dormant one's, since a pick that fails to resolve
/// still shields its name from the PR name lookup.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BaseResolution {
    pub status: BaseStatus,
    /// The default branch the chain ran against ([`default_branch_name`]), so the picker
    /// marks its row from the same pass that resolved the winner.
    pub default: Option<String>,
    candidates: Vec<ResolvedBase>,
    recorded: Vec<String>,
}

impl BaseResolution {
    fn oids(&self) -> Vec<String> {
        self.candidates.iter().map(|c| c.oid().to_string()).collect()
    }
}

/// Resolve the base chain: the `--base` flag, then this worktree's pick, then the default
/// branch ([`default_branch_name`]). A source that does not
/// resolve to a commit is skipped, never an error; a skipped flag or pick that would have
/// outranked the winner is recorded for the header.
///
/// A pick spelling the default branch (one an earlier release wrote, or one the repo
/// re-defaulted onto) resolves to the same base the default step would, so it needs no
/// special case here; [`write_base_pick`] keeps such a ref from being written.
pub fn resolve_base(repo: &Path, base_flag: Option<&str>) -> Result<BaseResolution, GitFail> {
    let mut candidates: Vec<ResolvedBase> = Vec::new();
    let mut recorded: Vec<String> = Vec::new();
    let mut skipped: Option<String> = None;
    let default = default_branch_name(repo)?;
    let push = |c: ResolvedBase, list: &mut Vec<ResolvedBase>| {
        if !list.iter().any(|x| x.oid() == c.oid()) {
            list.push(c);
        }
    };
    let record = |name: String, r: &mut Vec<String>| {
        if !name.is_empty() && !r.contains(&name) {
            r.push(name);
        }
    };
    if let Some(flag) = base_flag.filter(|b| !b.is_empty()) {
        let (hit, skip) = classify_flag(repo, flag)?;
        if let Some(c) = hit {
            record(c.name().to_string(), &mut recorded);
            push(c, &mut candidates);
        }
        if let Some(s) = skip {
            record(s.clone(), &mut recorded);
            skipped = Some(s);
        }
    }
    if let Some(pick) = read_base_pick(repo)? {
        record(pick.clone(), &mut recorded);
        match resolve_spelling(repo, &pick)? {
            Some(c) => push(c, &mut candidates),
            None if candidates.is_empty() => skipped = skipped.or(Some(pick)),
            None => {}
        }
    }
    if let Some(name) = &default {
        record(name.clone(), &mut recorded);
        if let Some(oid) = resolve_base_entry(repo, name)? {
            push(ResolvedBase::branch(name.clone(), oid), &mut candidates);
        }
    }
    let winner = candidates.first().cloned();
    Ok(BaseResolution { status: BaseStatus { winner, skipped }, default, candidates, recorded })
}

/// The repo's default branch: what `origin/HEAD` names, else `init.defaultBranch`, else
/// `main`, else `master` — the last three only when a branch of exactly that name exists,
/// on origin or locally. `origin/HEAD` is the best evidence of the trunk, not its
/// definition: a clone with no remote still has one, and without this fallback such a
/// repo has no base at all.
///
/// Existence is read back from the ref list, never probed with `rev-parse`: a loose-ref
/// lookup on a case-insensitive filesystem resolves `refs/heads/main` to a branch named
/// `Main`, and a name no ref spells would then paint the header and match no row.
pub fn default_branch_name(repo: &Path) -> Result<Option<String>, GitFail> {
    if let Some(name) = origin_default_branch(repo)? {
        return Ok(Some(name));
    }
    let configured = git_tristate(repo, &["config", "--get", "init.defaultBranch"])?
        .filter(|name| is_branch_label(name));
    let names: Vec<&str> =
        configured.iter().map(String::as_str).chain(["main", "master"]).collect();
    // One listing for every candidate: `for-each-ref` takes several patterns, and it
    // matches them case-sensitively and by whole path, so the output is checked for the
    // exact ref (the pattern alone would also match a branch `main/foo`).
    let patterns: Vec<String> = names
        .iter()
        .flat_map(|name| BRANCH_REF_PREFIXES.iter().map(move |prefix| format!("{prefix}{name}")))
        .collect();
    let mut args = vec!["for-each-ref", "--format=%(refname)"];
    args.extend(patterns.iter().map(String::as_str));
    let out = git_strict(repo, &args)?;
    let listed: std::collections::HashSet<&str> = out.lines().collect();
    Ok(names
        .into_iter()
        .find(|name| {
            BRANCH_REF_PREFIXES.iter().any(|p| listed.contains(format!("{p}{name}").as_str()))
        })
        .map(str::to_string))
}

/// The branch name `origin/HEAD` points at. Some
/// clones carry `origin/HEAD` as a plain ref instead of a symref — then the name is the
/// origin tip whose commit matches it.
fn origin_default_branch(repo: &Path) -> Result<Option<String>, GitFail> {
    let target = git_tristate(repo, &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"])?;
    if let Some(name) =
        target.and_then(|t| t.strip_prefix("refs/remotes/origin/").map(str::to_string))
    {
        // `fetch --prune` can delete the target and leave the symref dangling: a name
        // that resolves to nothing is no default, or the picker could never mark the
        // default row and the name shield would carry a phantom.
        let probe = format!("refs/remotes/origin/{name}^{{commit}}");
        let resolves = git_tristate(repo, &["rev-parse", "--verify", "--quiet", &probe])?;
        return Ok(resolves.map(|_| name));
    }
    let Some(oid) = git_tristate(
        repo,
        &["rev-parse", "--verify", "--quiet", "refs/remotes/origin/HEAD^{commit}"],
    )?
    else {
        return Ok(None);
    };
    Ok(origin_tips(repo)?.into_iter().find_map(|(tip, name)| (tip == oid).then_some(name)))
}

/// Strip the ref prefixes a `--base` branch name may carry.
pub(crate) fn strip_base_prefix(entry: &str) -> String {
    ["refs/remotes/origin/", "refs/heads/", "origin/"]
        .iter()
        .find_map(|p| entry.strip_prefix(p))
        .unwrap_or(entry)
        .to_string()
}

/// One base picker row: a bare branch name and the unix time of its tip commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchRow {
    pub name: String,
    pub tip_secs: u64,
}

/// Every branch for the base picker: `refs/heads` and `refs/remotes/origin` merged by
/// bare name, newest tip first, `origin/HEAD` excluded. A name on both sides keeps
/// origin's tip, the one the chain resolves it to. The checked-out branch is listed: it is
/// a legitimate base (the diff is then the uncommitted one), and excluding it is what left
/// a one-branch repo with no rows.
pub fn list_branches(repo: &Path) -> Result<Vec<BranchRow>, GitFail> {
    let out = git_strict(
        repo,
        &[
            "for-each-ref",
            "refs/remotes/origin",
            "refs/heads",
            "--sort=-committerdate",
            "--format=%(refname)%00%(committerdate:unix)",
        ],
    )?;
    // The sort interleaves origin and local refs by date, so origin's rows are taken in a
    // first pass and local ones fill in after: the merge keeps origin's tip by rule
    // (`BRANCH_REF_PREFIXES`), not by whichever side happens to be newer. A tip whose
    // date does not parse (a ref at a non-commit) keeps `0`, which paints as no age.
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut rows: Vec<BranchRow> = Vec::new();
    for prefix in BRANCH_REF_PREFIXES {
        for line in out.lines() {
            let Some((refname, secs)) = line.split_once('\0') else { continue };
            let Some(name) = refname.strip_prefix(prefix) else { continue };
            if name == "HEAD" || !seen.insert(name) {
                continue;
            }
            rows.push(BranchRow { name: name.to_string(), tip_secs: secs.parse().unwrap_or(0) });
        }
    }
    rows.sort_by_key(|r| std::cmp::Reverse(r.tip_secs));
    Ok(rows)
}

/// The checked-out branch's bare name, `None` when `HEAD` is detached.
pub fn checked_out_branch(repo: &Path) -> Result<Option<String>, GitFail> {
    git_tristate(repo, &["symbolic-ref", "--quiet", "--short", "HEAD"])
}

/// The `origin` remote-tracking tips as `(OID, bare name)`, `origin/HEAD` excluded.
fn origin_tips(repo: &Path) -> Result<Vec<(String, String)>, GitFail> {
    Ok(remote_tips(repo, &["origin"])?.into_iter().map(|tip| (tip.oid, tip.name)).collect())
}

/// One remote-tracking tip: its OID, its remote, and its branch name there.
struct RemoteTip {
    oid: String,
    remote: String,
    name: String,
}

/// The remote-tracking tips of the given remotes, `<remote>/HEAD` excluded — one listing per
/// pass. `remotes` must come longest first, so a remote name containing `/` splits right;
/// a ref left behind by a remote no longer configured belongs to none of them.
fn remote_tips(repo: &Path, remotes: &[&str]) -> Result<Vec<RemoteTip>, GitFail> {
    let out =
        git_strict(repo, &["for-each-ref", "refs/remotes", "--format=%(objectname) %(refname)"])?;
    Ok(out
        .lines()
        .filter_map(|line| {
            let (oid, refname) = line.split_once(' ')?;
            let rest = refname.strip_prefix("refs/remotes/")?;
            let (remote, name) = remotes.iter().find_map(|remote| {
                Some((*remote, rest.strip_prefix(remote)?.strip_prefix('/')?))
            })?;
            (name != "HEAD").then(|| RemoteTip {
                oid: oid.to_string(),
                remote: remote.to_string(),
                name: name.to_string(),
            })
        })
        .collect())
}

/// The remote-tracking branches at the pushed frontier, on every configured remote, as
/// `(remote, name)`:
/// the tips at the boundary of the unpushed range — or at `head` itself when nothing is
/// unpushed. A tip on base history carries no work of this branch and contributes nothing.
/// Bounded at 32 boundary commits, so a merge-heavy frontier stays cheap.
fn frontier_names(
    repo: &Path,
    remotes: &[&str],
    tips: &[RemoteTip],
    head: &str,
    bases: &[String],
) -> Result<Vec<(String, String)>, GitFail> {
    if tips.is_empty() {
        // Nothing is published at all; skip the history walk, which `--not --remotes` would
        // otherwise run unbounded.
        return Ok(Vec::new());
    }
    // Only configured remotes bound the walk: a ref a removed remote left behind is no
    // publication and must not hide the real frontier.
    let excluded: Vec<String> = remotes.iter().map(|r| format!("--remotes={r}")).collect();
    let mut args = vec!["rev-list", "--boundary", head, "--not"];
    args.extend(excluded.iter().map(String::as_str));
    let out = git_strict(repo, &args)?;
    let mut oids: Vec<String> = Vec::new();
    let mut saw_unpushed = false;
    for line in out.lines() {
        match line.strip_prefix('-') {
            Some(boundary) => oids.push(boundary.to_string()),
            None if !line.is_empty() => saw_unpushed = true,
            None => {}
        }
    }
    if !saw_unpushed && oids.is_empty() {
        // Nothing is unpushed: HEAD itself is published.
        oids.push(head.to_string());
    }
    oids.truncate(32);
    let mut names: Vec<(String, String)> = Vec::new();
    for oid in oids {
        // The caller keeps at most 8 heads, so stop paying git calls past that.
        if names.len() >= 8 {
            break;
        }
        if !beyond_all_bases(repo, &oid, bases)? {
            continue;
        }
        for tip in tips {
            let pair = (tip.remote.clone(), tip.name.clone());
            if tip.oid == oid && !names.contains(&pair) {
                names.push(pair);
            }
        }
    }
    Ok(names)
}

/// Whether `commit` is an ancestor of (or equal to) `of`.
fn is_ancestor(repo: &Path, commit: &str, of: &str) -> Result<bool, GitFail> {
    Ok(git_tristate(repo, &["merge-base", "--is-ancestor", commit, of])?.is_some())
}

/// Whether the pinned `HEAD` contains `commit` — the merged/closed admission guard: a
/// reused branch name never resurrects a PR whose commits this branch does not hold
/// A commit absent from the object database is not
/// contained; an unfetched head proves nothing.
pub fn contains_commit(repo: &Path, head: &str, commit: &str) -> Result<bool, GitFail> {
    if git_tristate(repo, &["cat-file", "-e", commit])?.is_none() {
        return Ok(false);
    }
    is_ancestor(repo, commit, head)
}

/// Whether `oid` lies beyond every resolved base — an ancestor of none of them. Decides which
/// frontier tips carry provable work.
fn beyond_all_bases(repo: &Path, oid: &str, bases: &[String]) -> Result<bool, GitFail> {
    for base in bases {
        if is_ancestor(repo, oid, base)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The target and origin identities from one read of each remote. The target resolves from
/// a readable supported `upstream`, falling back to `origin`; unusable identities fall
/// back, read errors do not. The origin identity rides along for the fork lookup —
/// on a fork clone the fork's own PRs live there. A usable `upstream`
/// already fixes the target, so an `origin` read that fails then costs only that fetch's
/// association source, not the whole read.
pub(crate) fn remote_identities(
    repo: &Path,
    hosts: &ForgeHosts<'_>,
) -> Result<(RepositoryIdentity, Option<RepoTarget>), GitFail> {
    let upstream = remote_identity(repo, "upstream", hosts)?;
    let origin = remote_identity(repo, "origin", hosts);
    let origin_target = match &origin {
        Ok(RepositoryIdentity::Repository(target)) => Some(target.clone()),
        _ => None,
    };
    let repository =
        if matches!(upstream, RepositoryIdentity::Repository(_)) { upstream } else { origin? };
    Ok((repository, origin_target))
}

/// Classify one rewritten primary fetch URL. A missing remote is a clean state; every other
/// `remote get-url` failure is transient. The command applies `url.*.insteadOf` rewrites.
fn remote_identity(
    repo: &Path,
    remote: &str,
    hosts: &ForgeHosts<'_>,
) -> Result<RepositoryIdentity, GitFail> {
    let args = ["remote", "get-url", "--", remote];
    let out = run_git(repo, &args)?;
    if out.status.success() {
        let url = std::str::from_utf8(&out.stdout)
            .map_err(|e| GitFail(git_error(&args, "returned invalid UTF-8", e)))?;
        return Ok(classify_remote(url.trim(), hosts));
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    if stderr.to_lowercase().contains("no such remote") {
        return Ok(RepositoryIdentity::Missing);
    }
    Err(GitFail(git_error(&args, "failed", stderr.trim())))
}

/// Peel `rev` to a commit object id. A leading `-` is not a
/// rev. An ambiguous abbreviated SHA is a miss, not an error.
pub fn resolve_commit(repo: &Path, rev: &str) -> Result<Option<String>, GitFail> {
    if rev.is_empty() || rev.starts_with('-') {
        return Ok(None);
    }
    let probe = format!("{rev}^{{commit}}");
    git_tristate(repo, &["rev-parse", "--verify", "--quiet", &probe])
}

/// The abbreviated object id the header and the picker paint.
#[must_use]
pub fn abbreviate_oid(oid: &str) -> String {
    const N: usize = 7;
    if oid.len() <= N { oid.to_string() } else { oid[..N].to_string() }
}

/// Whether `spelling` is a hex prefix of `oid`.
#[must_use]
pub fn spelling_is_sha_prefix(spelling: &str, oid: &str) -> bool {
    let s = spelling.to_ascii_lowercase();
    !s.is_empty()
        && s.bytes().all(|b| b.is_ascii_hexdigit())
        && oid.to_ascii_lowercase().starts_with(&s)
}

/// Shown name and optional abbreviated SHA for a non-branch spelling. A SHA prefix paints once; anything else keeps the spelling and
/// carries the mark.
#[must_use]
pub fn rev_paint(spelling: &str, oid: &str) -> (String, Option<String>) {
    let abbrev = abbreviate_oid(oid);
    if spelling_is_sha_prefix(spelling, oid) {
        (abbrev, None)
    } else {
        (spelling.to_string(), Some(abbrev))
    }
}

/// Complete a unique SHA prefix to the abbreviated object id. A spelling that is
/// already that abbrev, or a longer hex prefix of the oid (a pasted 40-hex), is kept
#[must_use]
pub fn complete_sha_prefix(spelling: &str, oid: &str) -> String {
    let abbrev = abbreviate_oid(oid);
    if spelling_is_sha_prefix(spelling, oid) && spelling.len() < abbrev.len() {
        abbrev
    } else {
        spelling.to_string()
    }
}

/// A branch name the picker would list, not `HEAD` and not a rev-walk.
#[must_use]
pub fn is_branch_label(value: &str) -> bool {
    branch_name_shaped(value) && !value.eq_ignore_ascii_case("HEAD")
}

/// Origin then local, else a verbatim commit.
pub(crate) fn resolve_spelling(
    repo: &Path,
    spelling: &str,
) -> Result<Option<ResolvedBase>, GitFail> {
    if let Some(oid) = resolve_base_entry(repo, spelling)? {
        return Ok(Some(ResolvedBase::branch(spelling.to_string(), oid)));
    }
    Ok(resolve_commit(repo, spelling)?.map(|oid| ResolvedBase::rev(spelling.to_string(), oid)))
}

/// `--base`: verbatim first, else prefix-stripped as a branch. A miss keeps the flag
/// spelling unless the stripped form is a branch name.
fn classify_flag(
    repo: &Path,
    flag: &str,
) -> Result<(Option<ResolvedBase>, Option<String>), GitFail> {
    let entry = strip_base_prefix(flag);
    let verbatim = resolve_commit(repo, flag)?;
    let via_branch = resolve_base_entry(repo, &entry)?;
    Ok(match (verbatim, via_branch) {
        (Some(oid), None) => (Some(ResolvedBase::rev(flag.to_string(), oid)), None),
        (Some(oid), Some(_)) | (None, Some(oid)) => (Some(ResolvedBase::branch(entry, oid)), None),
        (None, None) => {
            let skip = if is_branch_label(&entry) { entry } else { flag.to_string() };
            (None, Some(skip))
        }
    })
}

/// Where a bare branch name is looked up, in the order that decides a name on both
/// sides: origin's tip is what the PR sees, so it wins. One list serves the resolve, the
/// default fallback, and the picker's merge.
const BRANCH_REF_PREFIXES: [&str; 2] = ["refs/remotes/origin/", "refs/heads/"];

fn resolve_base_entry(repo: &Path, name: &str) -> Result<Option<String>, GitFail> {
    if !is_branch_label(name) {
        return Ok(None);
    }
    for prefix in BRANCH_REF_PREFIXES {
        let probe = format!("{prefix}{name}^{{commit}}");
        if let Some(oid) = git_tristate(repo, &["rev-parse", "--verify", "--quiet", &probe])? {
            return Ok(Some(oid));
        }
    }
    Ok(None)
}

/// One read of the repository's effective git config. Keys arrive as git prints them:
/// section and variable lowercased, the subsection (a branch or remote name) verbatim.
struct GitConfig(Vec<(String, String)>);

impl GitConfig {
    fn read(repo: &Path) -> Result<Self, GitFail> {
        let out = git_strict(repo, &["config", "-z", "--list"])?;
        Ok(Self(
            out.split('\0')
                .filter(|entry| !entry.is_empty())
                .map(|entry| match entry.split_once('\n') {
                    Some((key, value)) => (key.to_string(), value.to_string()),
                    // A bare boolean key carries no value line.
                    None => (entry.to_string(), String::new()),
                })
                .collect(),
        ))
    }

    /// The last value of `key` — git's own precedence for a single-valued key.
    fn get(&self, key: &str) -> Option<&str> {
        self.0.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// The first value of a multi-valued key — git's upstream is the first `merge`.
    fn first(&self, key: &str) -> Option<&str> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

/// What `branch.<name>.merge` names on the recorded remote.
#[derive(Debug)]
enum BranchMerge {
    /// A branch on that remote (`refs/heads/<name>`, or a bare `<name>` as git reads it).
    Branch(String),
    /// A forge's pull request ref — what `gh pr checkout` (`refs/pull/<N>/head`) and
    /// `glab mr checkout` (`refs/merge-requests/<N>/head`) record without push access.
    Pull(Forge, u64),
    /// Any other ref: a remote, but no branch or pull request on it.
    Other,
}

/// The branch's upstream record from config — `branch.<name>.remote` and what its first
/// `merge` names, as git reads it — or `None` when no remote is recorded or it is local (`.`).
fn branch_record(config: &GitConfig, branch: &str) -> Option<(String, BranchMerge)> {
    let remote = config.get(&format!("branch.{branch}.remote"))?;
    if remote.is_empty() || remote == "." {
        return None;
    }
    let merge = config.first(&format!("branch.{branch}.merge")).unwrap_or_default();
    let pull =
        |prefix: &str| merge.strip_prefix(prefix)?.strip_suffix("/head")?.parse::<u64>().ok();
    let kind = if let Some(number) = pull("refs/pull/") {
        BranchMerge::Pull(Forge::GitHub, number)
    } else if let Some(number) = pull("refs/merge-requests/") {
        BranchMerge::Pull(Forge::GitLab, number)
    } else if let Some(name) = merge.strip_prefix("refs/heads/").filter(|name| !name.is_empty()) {
        BranchMerge::Branch(name.to_string())
    } else if !merge.is_empty() && !merge.starts_with("refs/") {
        BranchMerge::Branch(merge.to_string())
    } else {
        BranchMerge::Other
    };
    Some((remote.to_string(), kind))
}

/// git's own rule for a remote-valued setting: a configured remote name, else a URL.
fn is_named_remote(config: &GitConfig, value: &str) -> bool {
    config.get(&format!("remote.{value}.url")).is_some()
}

/// The configured remote names, longest first — the order that splits a remote-tracking
/// refname whose remote name itself contains `/`.
fn remote_names(config: &GitConfig) -> Vec<&str> {
    let mut names: Vec<&str> = config
        .0
        .iter()
        .filter_map(|(key, _)| key.strip_prefix("remote.")?.strip_suffix(".url"))
        .collect();
    names.sort_unstable();
    names.dedup();
    names.sort_by_key(|name| std::cmp::Reverse(name.len()));
    names
}

/// Resolves remote-valued settings to forge repositories, once each per derivation.
struct Remotes<'a> {
    repo: &'a Path,
    config: &'a GitConfig,
    hosts: &'a ForgeHosts<'a>,
    seen: Vec<((String, bool), Option<RepoTarget>)>,
}

impl<'a> Remotes<'a> {
    fn new(repo: &'a Path, config: &'a GitConfig, hosts: &'a ForgeHosts<'a>) -> Self {
        Self { repo, config, hosts, seen: Vec::new() }
    }

    /// The forge repository `value` (a remote name or a URL) names, the way git resolves it:
    /// `ls-remote --get-url` (`insteadOf`), and on the push side of a configured remote
    /// `remote get-url --push` (`pushurl`, `pushInsteadOf`; git offers no command that applies
    /// `pushInsteadOf` to a bare URL). When the push side names no forge repository — an ssh
    /// Host alias, a mirror — the fetch side does. A remote that no longer exists or a host
    /// this config does not support names nothing — never a fallback to another remote.
    fn resolve(&mut self, value: &str, push: bool) -> Result<Option<RepoTarget>, GitFail> {
        let key = (value.to_string(), push);
        if let Some((_, repo)) = self.seen.iter().find(|(k, _)| *k == key) {
            return Ok(repo.clone());
        }
        let url = if push && is_named_remote(self.config, value) {
            git_strict(self.repo, &["remote", "get-url", "--push", "--", value])?
        } else {
            // A configured name or a URL alike. A deleted remote's leftover name prints back
            // verbatim and names no host.
            git_strict(self.repo, &["ls-remote", "--get-url", "--", value])?
        };
        let repo = match classify_remote(url.trim(), self.hosts) {
            RepositoryIdentity::Repository(target) => Some(target),
            _ if push && is_named_remote(self.config, value) => self.resolve(value, false)?,
            _ => None,
        };
        self.seen.push((key, repo.clone()));
        Ok(repo)
    }
}

/// Commits `local` (the pinned `HEAD` OID) is ahead and behind `other` (the PR head OID).
/// `Ok(None)` when `other` is not in the object database — the PR head was never fetched
/// locally, a clean absence. Backs the PR `sync` indicator.
pub fn ahead_behind_oids(
    repo: &Path,
    local: &str,
    other: &str,
) -> Result<Option<(u32, u32)>, GitFail> {
    // Plain `-e` (no `^{commit}` peel): peeling a missing object exits 128, not the
    // clean-absence 1 this check relies on.
    if git_tristate(repo, &["cat-file", "-e", other])?.is_none() {
        return Ok(None);
    }
    let range = format!("{local}...{other}");
    let args = ["rev-list", "--left-right", "--count", range.as_str()];
    let out = git_strict(repo, &args)?;
    let mut it = out.split_whitespace();
    let parse = |s: Option<&str>| {
        s.and_then(|v| v.parse().ok())
            .ok_or_else(|| GitFail(git_error(&args, "returned unexpected output", out.trim())))
    };
    let ahead = parse(it.next())?;
    let behind = parse(it.next())?;
    Ok(Some((ahead, behind)))
}

/// The merge-base commit of the resolved base OID and `HEAD`
pub fn merge_base(repo: &Path, base_oid: &str) -> Option<String> {
    git_line(repo, &["merge-base", base_oid, "HEAD"])
}

/// The content of `path` at `rev` (`git show <rev>:<path>`). Empty when the path does
/// not exist at that rev — an added file against its old side, say.
pub fn file_content(repo: &Path, rev: &str, path: &str) -> String {
    git_lenient(repo, &["show", &format!("{rev}:{path}")])
}

// --- diff sides ----------------------------------------------------------------
//
// A tracked file's two sides come from git: one full-context `git diff`, whose context and
// `-` lines are the old side and whose context and `+` lines are the new side. git cleans the
// worktree side the way every `git diff` does (line endings under `core.autocrlf` and the
// `text`/`eol` attributes, `ident`, filter drivers, working-tree encoding), so the sides
// compare exactly what `git diff` compares. reviewr never replays that step itself.

/// One file's diff sides, or git's verdict that the change has no text diff.
#[derive(Debug, PartialEq, Eq)]
pub enum DiffSides {
    Text {
        old: String,
        new: String,
    },
    /// "Binary files … differ": binary content, or a path whose `diff` attribute is unset.
    Binary,
}

/// Where a changed path's old side lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin<'a> {
    /// At the path itself.
    Same,
    /// At the source of a rename, which the change deleted.
    Renamed(&'a str),
    /// At the source of a copy, which is still there.
    Copied(&'a str),
}

impl<'a> Origin<'a> {
    /// The origin of a changeset entry of `kind` with `previous_path`. Only a rename or a
    /// copy has a source, so any other kind reads at its own path.
    pub fn of(kind: ChangeKind, previous_path: Option<&'a str>) -> Self {
        match (kind, previous_path) {
            (ChangeKind::Renamed, Some(source)) => Self::Renamed(source),
            (ChangeKind::Copied, Some(source)) => Self::Copied(source),
            _ => Self::Same,
        }
    }
}

/// `path`'s diff sides from the tree-ish `old` to the tree-ish `new`, or to the worktree when
/// `new` is `None`, its old side read where `origin` says.
///
/// One `git diff`. A second, a `git show`, only where that diff spells no side: a rename's or
/// copy's source, and a file git reports no line of (a mode change), whose two sides are the
/// one blob. An error is a git that could not answer: a missing revision, an unborn `HEAD`.
pub fn diff_sides(
    repo: &Path,
    old: &str,
    new: Option<&str>,
    path: &str,
    origin: Origin<'_>,
) -> Result<DiffSides> {
    // Context as wide as the byte budget holds every line of a file the diff can render
    // (each line is at least a byte), so the one hunk is the whole file. A file past the
    // budget renders its too-large notice, whatever part of it this reads.
    let source = match origin {
        Origin::Same => path,
        Origin::Renamed(source) | Origin::Copied(source) => source,
    };
    let context = format!("-U{}", crate::diff::MAX_BYTES);
    let mut args = vec![
        // An empty context line prints as a lone space, the form unified diff defines, whatever
        // the user set; the parser reads a bare newline too.
        "-c",
        "diff.suppressBlankEmpty=false",
        // A path is a path, never a glob: `a[1].txt` must not match `a1.txt`.
        "--literal-pathspecs",
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--ignore-submodules",
        // A rename spells its source as a deleted file and its target as an added one, so
        // even a pure rename prints both sides in full.
        "--no-renames",
        &context,
        old,
    ];
    args.extend(new);
    args.extend(["--", path]);
    let out = git(repo, &args)?;
    let same = |rev: &str, at: &str| {
        let text = file_content(repo, rev, at);
        DiffSides::Text { old: text.clone(), new: text }
    };
    Ok(match (parse_sides(&out), origin) {
        // A rename's or copy's target diffs as a new file, and its old side is the source's
        // blob, read rather than diffed: whatever stands at the source path now (a copy's
        // source, a re-created file) is another file's change.
        (
            Some(DiffSides::Text { new: text, .. }),
            Origin::Renamed(source) | Origin::Copied(source),
        ) => DiffSides::Text { old: file_content(repo, old, source), new: text },
        (Some(sides), _) => sides,
        (None, _) => match new {
            Some(new) => same(new, path),
            None => same(old, source),
        },
    })
}

/// A unified diff's hunk header, `@@ -l,s +l,s @@`: each side's (first line, line count),
/// a count of one left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HunkHeader {
    pub old: (usize, usize),
    pub new: (usize, usize),
}

impl HunkHeader {
    pub(crate) fn parse(line: &str) -> Option<Self> {
        let range = |r: &str| -> Option<(usize, usize)> {
            match r.split_once(',') {
                Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
                None => Some((r.parse().ok()?, 1)),
            }
        };
        let mut parts = line.strip_prefix("@@ ")?.split(' ');
        let old = range(parts.next()?.strip_prefix('-')?)?;
        let new = range(parts.next()?.strip_prefix('+')?)?;
        Some(Self { old, new })
    }
}

/// The sides a full-context `git diff` spells, `None` when it printed no hunk (the sides are
/// equal). Each hunk header counts its lines, so a body line is never mistaken for a header:
/// `--- x` inside a hunk is the deletion of `-- x`. `\ No newline at end of file` takes the
/// newline off the line before it, on the side or sides that line belongs to. A CR git keeps
/// (under `-text`) stays in the line's text.
fn parse_sides(out: &str) -> Option<DiffSides> {
    let (mut old, mut new) = (String::new(), String::new());
    let mut hunks = false;
    // Lines the open hunk still holds on each side.
    let (mut old_left, mut new_left) = (0usize, 0usize);
    // The side or sides the last body line went to: (old, new).
    let mut last = (false, false);
    for line in out.split_inclusive('\n') {
        if line.starts_with('\\') {
            for (side, took) in [(&mut old, last.0), (&mut new, last.1)] {
                if took && side.ends_with('\n') {
                    side.pop();
                }
            }
            continue;
        }
        if old_left + new_left > 0 {
            let (tag, body) = line.split_at(line.len().min(1));
            last = match tag {
                "-" => (true, false),
                "+" => (false, true),
                // A context line; a bare newline is an empty one.
                _ => (true, true),
            };
            let body = if tag == "\n" { line } else { body };
            if last.0 {
                old.push_str(body);
                old_left = old_left.saturating_sub(1);
            }
            if last.1 {
                new.push_str(body);
                new_left = new_left.saturating_sub(1);
            }
        } else if line.starts_with("@@ ") {
            let hunk = HunkHeader::parse(line)?;
            (old_left, new_left) = (hunk.old.1, hunk.new.1);
            hunks = true;
        } else if line.starts_with("Binary files ") {
            return Some(DiffSides::Binary);
        }
    }
    hunks.then_some(DiffSides::Text { old, new })
}

// --- base pick (branch scope) --------------------------------------------------
//
// One revision spelling per worktree: a blob under `refs/worktree/reviewr/base-pick`.
// Git isolates that namespace, so sibling worktrees do not share a pick.

const BASE_PICK_REF: &str = "refs/worktree/reviewr/base-pick";
const TURN_BASE_REF: &str = "refs/worktree/reviewr/turn-base";

/// The recorded pick's spelling, or `None` when no pick is recorded. One git call, so a
/// concurrent write from another pane of this worktree can never split the read the way
/// an exists-then-read pair would; a failed read is no pick, matching the chain's
/// skip-never-error contract.
pub fn read_base_pick(repo: &Path) -> Result<Option<String>, GitFail> {
    let out = run_git(repo, &["cat-file", "blob", BASE_PICK_REF])?;
    if !out.status.success() {
        return Ok(None);
    }
    let name = String::from_utf8_lossy(&out.stdout);
    let name = name.trim();
    Ok(pick_spelling_shaped(name).then(|| name.to_string()))
}

/// One printable line, not a git option. `HEAD~1` and a tag are
/// picks. Control bytes are not.
fn pick_spelling_shaped(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && value.bytes().all(|byte| byte > b' ' && byte != 0x7f)
}

/// Shape of a branch name the origin-then-local walk will accept. `HEAD` and rev-walk
/// spellings (`HEAD~1`) are not: git would parse them through `origin/HEAD`.
fn branch_name_shaped(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && !value.contains("..")
        && !value.contains("@{")
        && !value.contains(['~', '^', ':', '?', '*', '[', '\\'])
        && value.bytes().all(|byte| byte > b' ' && byte != 0x7f)
}

/// Record `name` as this worktree's pick. The ref write lands before the pick applies,
/// so a crash between the two loses nothing.
///
/// A name spelling the default branch is no pick: the ref is deleted instead, so the pane
/// follows the repo's next re-default. The default is read here, at the write, so a
/// picker row marked at open cannot go stale under a fetch that moved `origin/HEAD`.
pub fn write_base_pick(repo: &Path, name: &str) -> Result<(), GitFail> {
    if Some(name) == default_branch_name(repo)?.as_deref() {
        return delete_base_pick(repo);
    }
    let blob = git_stdin(repo, &["hash-object", "-w", "--stdin"], name)?;
    git_strict(repo, &["update-ref", BASE_PICK_REF, blob.trim()])?;
    Ok(())
}

/// Forget this worktree's pick, so the base is the default branch again. Deleting a ref
/// that does not exist succeeds: git's `-d` without an old value is idempotent.
pub fn delete_base_pick(repo: &Path) -> Result<(), GitFail> {
    git_strict(repo, &["update-ref", "-d", BASE_PICK_REF])?;
    Ok(())
}

/// Run git with `input` piped to stdin, any non-zero exit a failure.
///
/// The write runs on its own thread. An input large enough to fill the stdin pipe — the
/// untracked path set of [`diff_unset`] — would otherwise block here before anything read
/// stdout, while git blocks writing the stdout it cannot flush: a deadlock with no timeout
/// to break it, on the thread that draws the frame.
fn git_stdin(repo: &Path, args: &[&str], input: &str) -> Result<String, GitFail> {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = git_command(repo)
        .env("LC_ALL", "C")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| GitFail(git_error(args, "could not run", e)))?;
    let mut stdin = child.stdin.take().expect("stdin piped");
    let owned = input.to_string();
    // A git that answers and exits before reading it all closes the pipe. That is its answer,
    // not a failure of ours, so the write's result is dropped and the exit status decides.
    let writer = std::thread::spawn(move || drop(stdin.write_all(owned.as_bytes())));
    let out = child.wait_with_output().map_err(|e| GitFail(git_error(args, "could not run", e)))?;
    let _ = writer.join();
    if !out.status.success() {
        return Err(GitFail(git_error(
            args,
            "failed",
            String::from_utf8_lossy(&out.stderr).trim(),
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

// --- turn baseline (last-turn scope) -------------------------------------------
//
// The snapshot is non-disruptive: it writes a tree object from the worktree through
// a temporary index, never touching the real index, the worktree, or any branch, and
// persists the baseline at `refs/worktree/reviewr/turn-base`.

/// A non-disruptive snapshot of the worktree as a tree object: `add -A` and `write-tree` on
/// an [`IndexCopy`], so unchanged files keep their cached hash. Captures staged, unstaged,
/// and untracked content alike. Touches only the object database and the copy, never the
/// real index or any ref.
pub fn snapshot_worktree(repo: &Path) -> Result<String> {
    IndexCopy::with(repo, Purpose::Snapshot, |index| {
        index.git(repo, &["add", "-A"])?;
        Ok(index.git(repo, &["write-tree"])?.trim().to_string())
    })
}

/// What an [`IndexCopy`] is for. A snapshot's `add -A` stages untracked files into its copy,
/// which a worktree diff would then take for tracked ones, so the two never share a copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Purpose {
    Diff,
    Snapshot,
}

/// The prefix of every index copy's directory in the OS temp dir.
const COPY_PREFIX: &str = "reviewr-index-";

/// A private copy of the worktree's index, in a private directory of the OS temp dir, named to
/// git by `GIT_INDEX_FILE` for the runs that write an index: a snapshot's `add`, and the
/// refresh a worktree diff needs to tell a touched file from a changed one. git writes only
/// the copy (the **No writes** invariant).
///
/// A copy lives for the session, one per worktree and [`Purpose`], and is copied again only
/// when the real index changes, so the refresh git writes into it holds: a file touched with
/// the same content is hashed once, not on every poll. A run that finds its copy busy takes a
/// fresh one for itself instead of waiting. The directory holds an OS-locked `lock` file for
/// as long as the copy lives, so a later run can tell a killed process's leftover (unlocked)
/// from a live copy and sweep it.
struct IndexCopy {
    dir: tempfile::TempDir,
    /// The lock that marks this copy live, released on drop or with the process.
    _live: std::fs::File,
    /// The real index's (mtime, size) when it was last copied; `None` before the first copy,
    /// or while the repository has no index.
    seeded: Option<(std::time::SystemTime, u64)>,
}

impl IndexCopy {
    /// Run `f` on `repo`'s copy for `purpose`, brought up to date with the real index.
    fn with<T>(repo: &Path, purpose: Purpose, f: impl FnOnce(&Self) -> Result<T>) -> Result<T> {
        type Slot = std::sync::Arc<std::sync::Mutex<Option<IndexCopy>>>;
        static COPIES: OnceLock<Mutex<HashMap<(PathBuf, Purpose), Slot>>> = OnceLock::new();
        let real = git_dir(repo)?.join("index");
        let slot = COPIES
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry((real.clone(), purpose))
            .or_default()
            .clone();
        let fresh;
        let mut kept;
        let copy = match slot.try_lock() {
            Ok(guard) => {
                kept = guard;
                if kept.is_none() {
                    *kept = Some(Self::new()?);
                }
                kept.as_mut().expect("filled above")
            }
            Err(std::sync::TryLockError::Poisoned(guard)) => {
                kept = guard.into_inner();
                *kept = Some(Self::new()?);
                kept.as_mut().expect("filled above")
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                fresh = Self::new()?;
                &mut { fresh }
            }
        };
        copy.seed(&real)?;
        f(copy)
    }

    /// A new, empty copy, after sweeping the copies killed processes left behind.
    fn new() -> Result<Self> {
        sweep_dead_copies();
        let dir = tempfile::Builder::new()
            .prefix(COPY_PREFIX)
            .tempdir()
            .context("creating the index copy")?;
        // Locked under a temporary name, then renamed into place, so a sweep never finds a
        // `lock` file that is not yet held.
        let pending = dir.path().join("lock.new");
        let live = std::fs::File::create(&pending).context("creating the copy's lock")?;
        live.lock().context("locking the index copy")?;
        std::fs::rename(&pending, dir.path().join("lock")).context("placing the copy's lock")?;
        Ok(Self { dir, _live: live, seeded: None })
    }

    /// Copy the real index again if it changed since the last copy.
    fn seed(&mut self, real: &Path) -> Result<()> {
        // The copy keeps the index's mtime, which git's racy-clean check reads: an entry no
        // older than its index gets its content compared, since a same-size edit in that tick
        // matches every stat field. A copy stamped now would pass that edit as clean. Read
        // before the copy: an index the agent swaps in meanwhile is newer, so the stamp errs
        // old, the safe side, never new.
        let stamp = match std::fs::metadata(real) {
            Ok(meta) => (meta.modified().context("reading the index's mtime")?, meta.len()),
            // A fresh repository has no index yet, and git reads a missing one as empty.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let _ = std::fs::remove_file(self.path());
                self.seeded = None;
                return Ok(());
            }
            // Any other failure is no answer: an empty index would list every file deleted.
            Err(e) => return Err(e).context("reading the index"),
        };
        if self.seeded == Some(stamp) {
            return Ok(());
        }
        // Read through a handle that shares delete, so the agent's git can still rename a new
        // index over the real one while this copies it.
        let mut from = std::fs::File::open(real).context("opening the index")?;
        let mut to = std::fs::File::create(self.path()).context("creating the index copy")?;
        std::io::copy(&mut from, &mut to).context("copying the index")?;
        // Best effort: an undated copy costs the racy-clean edge case, never the run.
        let _ = to.set_modified(stamp.0);
        self.seeded = Some(stamp);
        Ok(())
    }

    fn path(&self) -> PathBuf {
        self.dir.path().join("index")
    }

    /// Like [`git`], on the copy, with the diff refresh on: a stat-dirty entry whose content
    /// is unchanged drops out of the diff, as in the reviewer's own `git diff`.
    fn git(&self, repo: &Path, args: &[&str]) -> Result<String> {
        let mut cmd = git_command(repo);
        cmd.args(["-c", "diff.autoRefreshIndex=true"]).env("GIT_INDEX_FILE", self.path());
        run(cmd, args)
    }
}

/// Remove the index copies whose owner is gone: a copy's `lock` is held for as long as its
/// process lives, so one this can take was left by a process that died without its cleanup.
/// A directory with no `lock` yet is skipped, never taken for dead.
fn sweep_dead_copies() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else { return };
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with(COPY_PREFIX) {
            continue;
        }
        let dir = entry.path();
        let Ok(lock) = std::fs::File::open(dir.join("lock")) else { continue };
        if lock.try_lock().is_ok() {
            drop(lock);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

/// `repo`'s git dir, asked once per worktree: it is fixed for the session.
fn git_dir(repo: &Path) -> Result<PathBuf> {
    static DIRS: OnceLock<Mutex<HashMap<PathBuf, PathBuf>>> = OnceLock::new();
    let dirs = DIRS.get_or_init(Mutex::default);
    if let Some(dir) = dirs.lock().unwrap_or_else(PoisonError::into_inner).get(repo) {
        return Ok(dir.clone());
    }
    let dir = PathBuf::from(git(repo, &["rev-parse", "--absolute-git-dir"])?.trim());
    dirs.lock().unwrap_or_else(PoisonError::into_inner).insert(repo.to_path_buf(), dir.clone());
    Ok(dir)
}

/// The persisted turn baseline tree for this worktree, if a baseline exists.
pub fn read_baseline_ref(repo: &Path) -> Option<String> {
    git_line(repo, &["rev-parse", "--verify", "--quiet", TURN_BASE_REF])
}

/// Persist the turn baseline tree under this worktree's private ref. `update-ref` is
/// atomic, so the baseline is never half-written.
pub fn write_baseline_ref(repo: &Path, sha: &str) -> Result<()> {
    git(repo, &["update-ref", TURN_BASE_REF, sha])?;
    Ok(())
}

/// git's well-known empty-tree object, used as the diff base when a repo has no commits.
pub const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// The commit `HEAD` names, else the empty tree in a repository with no commits: the old end
/// of the `uncommitted` scope. An oid, never the name `HEAD`, so a file's diff reads the same
/// tree its counts came from even after `HEAD` moves.
pub fn diff_base(repo: &Path) -> String {
    head_oid(repo).unwrap_or_else(|| EMPTY_TREE.to_string())
}

/// The changed files from the tree-ish `base` to the worktree, untracked files included,
/// sorted by path: the changeset of the `uncommitted` and `branch` scopes.
pub fn changed_from(repo: &Path, base: &str) -> Result<Vec<ChangedFile>> {
    let out = IndexCopy::with(repo, Purpose::Diff, |index| index.git(repo, &diff_args(&[base])))?;
    assemble(repo, &out, true)
}

/// The changed files between two trees, `old` against `new`: the `commits` scope's run, and
/// `last-turn`'s baseline against a worktree snapshot. Both sides are trees, so no untracked
/// pass runs. `old` may be the empty tree for a root commit.
pub fn changed_between(repo: &Path, old: &str, new: &str) -> Result<Vec<ChangedFile>> {
    assemble(repo, &git(repo, &diff_args(&[old, new]))?, false)
}

/// The one `git diff` a changeset reads, between `ends`: each path's raw record (its status,
/// rename source, and blobs), then its line counts, in one run.
fn diff_args<'a>(ends: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec!["diff"];
    args.extend(ends);
    args.extend(["--raw", "--numstat", "--no-abbrev", "-z"]);
    args
}

/// `sha`'s first parent, or the empty tree when `sha` is a root commit: the old side of a
/// run whose oldest commit is `sha`. `None` when the
/// commit itself is missing. The parent is read from the raw commit object, so a parent the
/// repository lacks (a shallow clone's cut) is named, not mistaken for a root: the caller's
/// existence check then reports it `gone`.
pub fn parent_or_empty(repo: &Path, sha: &str) -> Option<String> {
    let object = git(repo, &["cat-file", "-p", &format!("{sha}^{{commit}}")]).ok()?;
    let parent = object
        .lines()
        .take_while(|l| !l.is_empty())
        .find_map(|l| l.strip_prefix("parent "))
        .map_or(EMPTY_TREE, str::trim);
    Some(parent.to_string())
}

/// The commit `HEAD` names, or `None` in an unborn repository. The commit picker's universe
/// is keyed by it, so a poll re-lists only when it moved.
pub fn head_oid(repo: &Path) -> Option<String> {
    git_line(repo, &["rev-parse", "--verify", "-q", "HEAD"])
}

/// `sha`'s subject line, for the header paint.
pub fn commit_subject(repo: &Path, sha: &str) -> Option<String> {
    git_line(repo, &["log", "-1", "--format=%s", sha])
}

/// Whether `sha` names a commit the repository still holds (`gone`).
pub fn commit_exists(repo: &Path, sha: &str) -> bool {
    git_ok(repo, &["cat-file", "-e", &format!("{sha}^{{commit}}")])
}

/// Whether `sha` is reachable from `HEAD` (`off branch`). A missing
/// commit is unreachable.
pub fn is_reachable(repo: &Path, sha: &str) -> bool {
    git_ok(repo, &["merge-base", "--is-ancestor", sha, "HEAD"])
}

/// How many commits `oldest..=newest` spans along the first-parent walk from `newest`
/// `None` when either end is missing, or `oldest` is not behind `newest`.
pub fn run_length(repo: &Path, oldest: &str, newest: &str) -> Option<usize> {
    let old = parent_or_empty(repo, oldest)?;
    run_length_from(repo, &old, oldest, newest)
}

/// [`run_length`] with `oldest`'s parent already resolved, so a build that has it spawns
/// nothing twice.
pub fn run_length_from(repo: &Path, old: &str, oldest: &str, newest: &str) -> Option<usize> {
    if oldest == newest {
        return Some(1);
    }
    let mut args = vec!["rev-list", "--count", "--first-parent", newest];
    let exclude;
    if old != EMPTY_TREE {
        if !git_ok(repo, &["merge-base", "--is-ancestor", oldest, newest]) {
            return None;
        }
        exclude = format!("^{old}");
        args.push(&exclude);
    }
    git_line(repo, &args)?.parse().ok()
}

/// One row of the commit picker: the full id, the subject,
/// the committer time as unix seconds, the author, the refs pointing at it, and whether it
/// is a merge.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CommitRow {
    pub sha: String,
    pub subject: String,
    pub time: u64,
    pub author: String,
    /// The refs pointing at the commit, `HEAD` and the checked-out branch dropped.
    pub refs: Vec<CommitRef>,
    pub merge: bool,
}

/// A ref a picker row can show, by kind, so the row's one ref ranks by what it is rather
/// than by how it is spelled.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CommitRef {
    /// A remote-tracking tip, shown as `origin/feature`.
    Remote(String),
    /// A tag, shown as `tag: v1`.
    Tag(String),
    /// A local branch other than the one checked out, shown by name.
    Branch(String),
}

impl CommitRef {
    pub fn label(&self) -> String {
        match self {
            Self::Remote(r) | Self::Branch(r) => r.clone(),
            Self::Tag(t) => format!("tag: {t}"),
        }
    }
}

/// The picker's universe, newest first, along the first-parent walk from `HEAD`:
/// `merge_base..HEAD` when the base has one, or the last 50 commits without. First-parent only, so any contiguous run of rows is one ancestor
/// chain and diffs as `A^..B`. An unborn repository lists nothing.
pub fn list_commits(repo: &Path, merge_base: Option<&str>) -> Result<Vec<CommitRow>> {
    if head_oid(repo).is_none() {
        return Ok(Vec::new());
    }
    let range = merge_base.map(|mb| format!("{mb}..HEAD"));
    let mut args = vec![
        "log",
        "--first-parent",
        "--decorate=full",
        "--format=%H%x00%s%x00%ct%x00%an%x00%D%x00%P",
        "-z",
    ];
    match &range {
        Some(r) => args.push(r),
        None => args.extend(["-50", "HEAD"]),
    }
    let out = git(repo, &args)?;
    Ok(parse_commit_log(&out))
}

/// Parse `git log --format=%H%x00%s%x00%ct%x00%an%x00%D%x00%P -z` output: six NUL-separated
/// fields per commit, commits themselves NUL-terminated.
fn parse_commit_log(out: &str) -> Vec<CommitRow> {
    let fields: Vec<&str> = out.split('\0').collect();
    fields
        .chunks(6)
        .filter(|c| c.len() == 6 && !c[0].is_empty())
        .map(|c| CommitRow {
            sha: c[0].to_string(),
            subject: c[1].to_string(),
            time: c[2].trim().parse().unwrap_or(0),
            author: c[3].to_string(),
            refs: parse_decorations(c[4]),
            merge: c[5].split_whitespace().count() > 1,
        })
        .collect()
}

/// `%D` under `--decorate=full` as typed refs: `HEAD -> refs/heads/feature,
/// refs/remotes/origin/feature, tag: refs/tags/v1` becomes `Remote("origin/feature")`,
/// `Tag("v1")`. `HEAD` and the branch it is on are dropped, since the top row is `HEAD` by
/// construction and its branch is the one being reviewed.
fn parse_decorations(d: &str) -> Vec<CommitRef> {
    d.split(", ")
        .map(str::trim)
        .filter(|r| !r.is_empty() && *r != "HEAD" && !r.starts_with("HEAD -> "))
        .filter_map(|r| {
            if let Some(t) = r.strip_prefix("tag: refs/tags/") {
                Some(CommitRef::Tag(t.to_string()))
            } else if let Some(t) = r.strip_prefix("refs/remotes/") {
                Some(CommitRef::Remote(t.to_string()))
            } else {
                r.strip_prefix("refs/heads/").map(|b| CommitRef::Branch(b.to_string()))
            }
        })
        .collect()
}

/// One entry in the `All files` worktree listing: a path plus whether git ignores it and
/// whether it is a (lazily-expanded) directory placeholder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreeEntry {
    pub path: String,
    pub ignored: bool,
    pub is_dir: bool,
}

/// Every entry in the worktree for the `All files` tab: tracked and
/// untracked-not-ignored files from one `ls-files --cached --others` pass, and the ignored
/// entries from [`ignored_entries`] — a wholly-ignored directory collapsed to one `is_dir`
/// placeholder, an individually-ignored file as itself. `.git` is never reported. Deduped and
/// sorted; `-z` keeps paths with spaces or special characters verbatim.
pub fn all_files(repo: &Path) -> Result<Vec<WorktreeEntry>> {
    // One spawn for tracked + untracked. `--others --exclude-standard` applies the same
    // standard exclude rules as the untracked pass `changed_from` runs, so the
    // untracked sets match without a status walk.
    let listed = git(repo, &["ls-files", "--cached", "--others", "--exclude-standard", "-z"])?;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for path in listed.split('\0').filter(|s| !s.is_empty()) {
        if seen.insert(path.to_string()) {
            out.push(WorktreeEntry { path: path.to_string(), ignored: false, is_dir: false });
        }
    }
    for (path, is_dir) in ignored_entries(repo)? {
        if seen.insert(path.clone()) {
            out.push(WorktreeEntry { path, ignored: true, is_dir });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// The ignored entries: a wholly-ignored directory comes back as `dir/` (mapped to
/// `is_dir = true`), an individually-ignored file as itself.
///
/// `ls-files --directory` prunes at each ignored directory instead of walking inside it, where
/// `git status --ignored` enumerates the whole tree — seconds against a large `node_modules`.
/// `--no-empty-directory` matches `status`'s output exactly, which skips empty ignored dirs.
fn ignored_entries(repo: &Path) -> Result<Vec<(String, bool)>> {
    let out = git(
        repo,
        &[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
            "--no-empty-directory",
            "-z",
        ],
    )?;
    Ok(out
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(|path| match path.strip_suffix('/') {
            Some(dir) => (dir.to_string(), true),
            None => (path.to_string(), false),
        })
        .collect())
}

/// The immediate children of a wholly-ignored directory, for lazy expansion in `All files`
/// Everything under an ignored directory is ignored, so this reads the
/// filesystem directly; sub-directories come back as `is_dir` placeholders to expand in turn.
/// An unreadable directory yields no children rather than failing the reload, so expansion is
/// best-effort.
pub fn list_ignored_dir(repo: &Path, dir: &str) -> Vec<WorktreeEntry> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(repo.join(dir)) else { return out };
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else { continue };
        let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
        out.push(WorktreeEntry { path: format!("{dir}/{name}"), ignored: true, is_dir });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Build the sorted `ChangedFile` list from one `git diff --raw --numstat` ([`diff_args`]). A `worktree`
/// diff's new side is the worktree, sized when it is read, and it appends the untracked files
/// a `git diff` never reports. Every blob is sized here, on the world worker, by object id: a
/// file's diff then knows whether its sides fit the render budget before reading either.
fn assemble(repo: &Path, out: &str, worktree: bool) -> Result<Vec<ChangedFile>> {
    let (rows, numstat) = parse_raw(out);
    let counts = parse_numstat(numstat);
    let blobs: Vec<&str> = rows
        .iter()
        .flat_map(|row| [Some(row.old_oid.as_str()), (!worktree).then_some(row.new_oid.as_str())])
        .flatten()
        .collect();
    let sizes = blob_sizes(repo, &blobs)?;
    let size = |oid: &str| sizes.get(oid).copied().unwrap_or(0);
    let mut seen = HashSet::new();
    let mut files = Vec::new();
    for row in rows {
        if !seen.insert(row.path.clone()) {
            continue;
        }
        let verdict = counts.get(&row.path).copied().unwrap_or(Some((0, 0)));
        let (additions, deletions) = verdict.unwrap_or((0, 0));
        files.push(ChangedFile {
            kind: row.kind,
            additions,
            deletions,
            binary: verdict.is_none(),
            old_size: size(&row.old_oid),
            new_size: if worktree { 0 } else { size(&row.new_oid) },
            path: row.path,
            previous_path: row.previous_path,
        });
    }

    if worktree {
        // Untracked-not-ignored files list as additions. One `ls-files --others` pass — the
        // same definition of untracked `all_files` uses, so the two views can't disagree.
        // `-z` keeps paths with spaces or special characters verbatim, and files inside a
        // brand-new directory list individually (.gitignore still applies).
        let others = git(repo, &["ls-files", "--others", "--exclude-standard", "-z"])?;
        let new_paths: Vec<&str> =
            others.split('\0').filter(|p| !p.is_empty() && !seen.contains(*p)).collect();
        // A failed attribute read costs the verdict, never the whole changeset.
        let undiffable = diff_unset(repo, &new_paths).unwrap_or_default();
        let mut buf = vec![0; 64 * 1024];
        for path in new_paths {
            let path = path.to_string();
            if !seen.insert(path.clone()) {
                continue;
            }
            // An unset `diff` attribute is git's no-text-diff verdict before any content read:
            // no lines to count, as a tracked `-diff` path shows none.
            let additions = if undiffable.contains(path.as_str()) {
                None
            } else {
                untracked_additions(repo, &path, &mut buf)
            };
            let binary = additions.is_none();
            files.push(ChangedFile {
                path,
                kind: ChangeKind::Untracked,
                additions: additions.unwrap_or(0),
                deletions: 0,
                previous_path: None,
                binary,
                old_size: 0,
                new_size: 0,
            });
        }
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// Of `paths`, those whose `diff` attribute git reports as unset — `-diff`, or the `binary`
/// macro that implies it. Empty when `paths` is, so a repository with nothing untracked pays
/// nothing. A `diff=<driver>` whose driver sets `binary` counts only once the path is tracked,
/// where `--numstat` reports it.
///
/// One `check-attr` for the whole set, never one per path — the same rule
/// [`untracked_additions`] follows, and for the same reason. Only untracked paths come here:
/// a tracked path already carries git's verdict in its `--numstat` record, at no cost.
///
/// Under `-z` the answer is `PATH\0diff\0VALUE\0` per path, and the input must be
/// NUL-terminated too — a newline-separated list makes git read the newline as part of the
/// path and answer `unspecified` for every one of them.
fn diff_unset(repo: &Path, paths: &[&str]) -> Result<HashSet<String>> {
    if paths.is_empty() {
        return Ok(HashSet::new());
    }
    let mut input = String::new();
    for path in paths {
        input.push_str(path);
        input.push('\0');
    }
    let out = git_stdin(repo, &["check-attr", "-z", "--stdin", "diff"], &input)
        .map_err(|e| anyhow::anyhow!("{}", e.0))?;
    let mut fields = out.split('\0');
    let mut unset = HashSet::new();
    while let (Some(path), Some(_attr), Some(value)) = (fields.next(), fields.next(), fields.next())
    {
        if value == "unset" {
            unset.insert(path.to_string());
        }
    }
    Ok(unset)
}

/// git's default `core.bigFileThreshold`: past it git takes a file for binary without reading
/// it.
const BIG_FILE_THRESHOLD: u64 = 512 * 1024 * 1024;

/// Addition count of an untracked file: its line count, which is what `git diff` against
/// nothing reports. `None` where git would report no countable diff — binary content, or a
/// file past git's default [`BIG_FILE_THRESHOLD`] — matching the `-`/`-` numstat record a tracked binary produces. Read locally rather than
/// shelling `git diff --no-index` per file — with `--untracked-files=all` a large untracked
/// tree would otherwise fork git once per file on every poll and freeze the UI.
///
/// This is the content half of the untracked verdict only. An untracked path never reaches a
/// `git diff`, so no numstat speaks for it; [`diff_unset`] asks git for the attribute half
///.
fn untracked_additions(repo: &Path, path: &str, buf: &mut [u8]) -> Option<u32> {
    use std::io::Read;
    let at = repo.join(path);
    // Only a regular file has lines: a link to a device would read without end.
    let Some(meta) = std::fs::metadata(&at).ok().filter(std::fs::Metadata::is_file) else {
        return Some(0);
    };
    // git takes a file past its default threshold for binary without reading it, and so does
    // this, so a huge log costs a build nothing. A `diff` attribute set on the path, or a
    // threshold the repository raised, would make git read it; here it still reads as binary.
    if meta.len() > BIG_FILE_THRESHOLD {
        return None;
    }
    let Ok(mut file) = std::fs::File::open(at) else { return Some(0) };
    // Counted a buffer at a time, so a large file never sits in memory whole.
    let (mut newlines, mut read, mut last) = (0usize, 0usize, None);
    loop {
        let n = match file.read(buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Some(0),
        };
        let chunk = &buf[..n];
        // git's own sniff: a NUL within the first 8000 bytes.
        if read < 8000 && chunk[..n.min(8000 - read)].contains(&0) {
            return None; // binary — git reports no line additions
        }
        #[allow(clippy::naive_bytecount)]
        let count = chunk.iter().filter(|&&b| b == b'\n').count();
        newlines += count;
        read += n;
        last = chunk.last().copied();
    }
    // Lines = newline count, plus one for a final line with no trailing newline.
    let trailing = usize::from(last.is_some_and(|b| b != b'\n'));
    Some(u32::try_from(newlines + trailing).unwrap_or(u32::MAX))
}

// --- pure parsers (unit-tested without a repo) ---------------------------------

/// Map of new-path to its line counts from `git diff --numstat -z`, `None` where git
/// reports no countable diff — the `-`/`-` record.
///
/// Under `-z` a non-rename record is `ADDS\tDELS\tPATH\0`; a rename/copy record is
/// `ADDS\tDELS\t\0OLD\0NEW\0` — the counts ride the front, then old and new arrive as
/// their own NUL fields (no `=>` arrow, no brace factoring). The counts key under the new
/// path, matching `parse_raw`.
///
/// `-`/`-` is git's own no-text-diff verdict, and it already accounts for `.gitattributes`:
/// binary content, an unset `diff` attribute (`-diff`, or the `binary` macro), and a driver
/// git will not text-diff all land there. Reading it as `0`/`0` — as this once did — loses
/// that verdict and leaves a `-diff` file to be diffed as text anyway.
fn parse_numstat(out: &str) -> HashMap<String, Option<(u32, u32)>> {
    let mut map = HashMap::new();
    let mut it = out.split('\0');
    while let Some(field) = it.next() {
        // `splitn(3)` keeps any tabs inside the path (verbatim under `-z`) intact.
        let mut parts = field.splitn(3, '\t');
        let add = parts.next().unwrap_or("");
        let del = parts.next().unwrap_or("");
        // Either side reading `-` is the whole record's verdict; git never mixes them.
        let counts = match (add.parse(), del.parse()) {
            (Ok(a), Ok(d)) => Some((a, d)),
            _ => None,
        };
        match parts.next() {
            // Non-rename: the path rode this same field.
            Some(path) if !path.is_empty() => {
                map.insert(path.to_string(), counts);
            }
            // Rename/copy: the next two fields are the old and new paths.
            Some(_) => {
                let _old = it.next();
                if let Some(new) = it.next().filter(|n| !n.is_empty()) {
                    map.insert(new.to_string(), counts);
                }
            }
            // No tab fields — a trailing empty record after the final NUL.
            None => {}
        }
    }
    map
}

/// One record of `git diff --raw --no-abbrev -z`.
#[derive(Debug, PartialEq, Eq)]
struct RawRow {
    kind: ChangeKind,
    path: String,
    /// The old path of a rename or copy, whose content is the old side.
    previous_path: Option<String>,
    /// Each side's blob, all zeros where the side is absent or is the worktree.
    old_oid: String,
    new_oid: String,
}

/// The raw records that lead `git diff --raw --numstat --no-abbrev -z`, and the numstat
/// records after them. Each raw record is `:MODE MODE OID OID STATUS\0PATH\0`, except a rename
/// or copy, `:… R<score>\0OLD\0NEW\0`, which takes the new path and carries its old one; every
/// other kind has `previous_path == None`. The first field not opening with `:` starts the
/// numstat, which no raw path can be mistaken for: a path follows its record's meta field.
fn parse_raw(out: &str) -> (Vec<RawRow>, &str) {
    /// One NUL-terminated field off the front of `rest`.
    fn field<'a>(rest: &mut &'a str) -> Option<&'a str> {
        let (head, tail) = rest.split_once('\0')?;
        *rest = tail;
        Some(head)
    }
    let mut rows = Vec::new();
    let mut rest = out;
    while rest.starts_with(':') {
        let mut next = rest;
        let Some(meta) = field(&mut next) else { break };
        let fields: Vec<&str> = meta[1..].split(' ').collect();
        let [_, _, old_oid, new_oid, status] = fields[..] else { break };
        let (kind, previous_path) = match status.chars().next() {
            Some('A') => (ChangeKind::Added, None),
            Some('D') => (ChangeKind::Deleted, None),
            Some(code @ ('R' | 'C')) => {
                let kind = if code == 'R' { ChangeKind::Renamed } else { ChangeKind::Copied };
                let Some(source) = field(&mut next) else { break };
                (kind, Some(source.to_string()))
            }
            // Modified, type-changed, etc.
            _ => (ChangeKind::Modified, None),
        };
        let Some(path) = field(&mut next) else { break };
        rest = next;
        rows.push(RawRow {
            kind,
            path: path.to_string(),
            previous_path,
            old_oid: old_oid.to_string(),
            new_oid: new_oid.to_string(),
        });
    }
    (rows, rest)
}

/// The size of each blob in `oids`, by object id, which no path can garble. A blob's size is
/// fixed for its id, so each is asked once per session, in one `cat-file` per build for the
/// ids it has not seen. An all-zeros id (no blob) is left out, and reads as none.
fn blob_sizes(repo: &Path, oids: &[&str]) -> Result<HashMap<String, u64>> {
    static KNOWN: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    let known = KNOWN.get_or_init(Mutex::default);
    let named = oids.iter().copied().filter(|oid| oid.bytes().any(|b| b != b'0'));
    let unknown: Vec<&str> = {
        let known = known.lock().unwrap_or_else(PoisonError::into_inner);
        named.clone().filter(|oid| !known.contains_key(*oid)).collect()
    };
    if !unknown.is_empty() {
        let input = unknown.join("\n") + "\n";
        let args = ["cat-file", "--batch-check=%(objectname) %(objectsize)"];
        let out = git_stdin(repo, &args, &input).map_err(|e| anyhow::anyhow!(e.0))?;
        let mut known = known.lock().unwrap_or_else(PoisonError::into_inner);
        for (oid, size) in out.lines().filter_map(|line| line.split_once(' ')) {
            if let Ok(size) = size.parse() {
                known.insert(oid.to_string(), size);
            }
        }
    }
    let known = known.lock().unwrap_or_else(PoisonError::into_inner);
    Ok(named.filter_map(|oid| Some((oid.to_string(), *known.get(oid)?))).collect())
}

#[cfg(test)]
mod tests {
    use super::{
        ChangeKind, Forge, ForgeHosts, RepoTarget, RepositoryIdentity, classify_remote,
        parse_numstat, parse_raw,
    };

    const NONE: ForgeHosts<'_> = ForgeHosts { github: None, gitlab: None, azure_devops: None };

    #[test]
    fn a_git_error_names_the_subcommand_not_the_argv() {
        assert_eq!(super::subcommand(&["-c", "x=y", "rev-parse", "--verify", "HEAD"]), "rev-parse");
        assert_eq!(super::subcommand(&["for-each-ref"]), "for-each-ref");
    }

    #[test]
    fn repo_identity_ignores_case_but_not_forge_host_or_depth() {
        let gl =
            |host: &str, path: &[&str]| RepoTarget::with_path(Forge::GitLab, host, path).unwrap();
        let acme = RepoTarget::new("github.com", "Acme", "Widgets").unwrap();
        assert!(acme.is(&RepoTarget::new("github.com", "acme", "widgets").unwrap()));
        assert!(!acme.is(&RepoTarget::new("ghe.corp.test", "acme", "widgets").unwrap()));
        assert!(!acme.is(&gl("github.com", &["acme", "widgets"])), "another forge");
        assert!(
            !gl("gitlab.com", &["group", "sub", "repo"]).is(&gl("gitlab.com", &["group", "sub"]))
        );
        let ado = |host: &str| {
            RepoTarget::with_path(Forge::AzureDevOps, host, &["org", "proj", "app"]).unwrap()
        };
        assert!(ado("org.visualstudio.com").is(&ado("dev.azure.com")), "one cloud org, two hosts");
        assert!(!ado("ado.corp.test").is(&ado("dev.azure.com")), "a server is its own namespace");
    }

    fn github(host: &str) -> ForgeHosts<'_> {
        ForgeHosts { github: Some(host), ..NONE }
    }

    fn gitlab(host: &str) -> ForgeHosts<'_> {
        ForgeHosts { gitlab: Some(host), ..NONE }
    }

    fn azure_devops(host: &str) -> ForgeHosts<'_> {
        ForgeHosts { azure_devops: Some(host), ..NONE }
    }

    #[test]
    fn worktree_of_distinguishes_a_repo_from_a_plain_directory() {
        use super::{Worktree, worktree_of};
        // A plain directory git can read but that holds no worktree.
        let outside = tempfile::tempdir().unwrap();
        assert_eq!(worktree_of(outside.path()), Worktree::Outside);
        // A real worktree resolves to its root. Compare through std canonicalization, an oracle
        // independent of `worktree_of`: both sides resolve the temp dir's symlinks, and on
        // Windows git's `C:/…` and std's `\\?\C:\…` spell the same directory.
        let repo = tempfile::tempdir().unwrap();
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(repo.path())
            .args(["init", "-q"])
            .status()
            .unwrap();
        assert!(status.success());
        let Worktree::Root(root) = worktree_of(repo.path()) else {
            panic!("a fresh repository resolves to a worktree root");
        };
        let canonical = |p: &std::path::Path| std::fs::canonicalize(p).unwrap();
        assert_eq!(canonical(&root), canonical(repo.path()));
    }

    #[test]
    fn core_editor_reads_the_configured_value_verbatim() {
        // The repository's own level outranks whatever the machine's global config says, so
        // this reads the same on every runner. The value keeps its quotes: splitting it is the
        // editor module's job.
        let repo = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let status =
                std::process::Command::new("git").arg("-C").arg(repo.path()).args(args).status();
            assert!(status.unwrap().success());
        };
        git(&["init", "-q"]);
        let value = r#""C:\Program Files\Microsoft VS Code\Code.exe" --wait"#;
        git(&["config", "core.editor", value]);
        assert_eq!(super::core_editor(repo.path()).as_deref(), Some(value));
    }

    #[test]
    fn repository_identity_parses_github_and_enterprise_remote_forms() {
        let repo = |host: &str, owner: &str, name: &str| {
            RepositoryIdentity::Repository(RepoTarget::new(host, owner, name).unwrap())
        };
        // HTTPS, with and without `.git` and a trailing slash.
        assert_eq!(
            classify_remote("https://github.com/owner/repo.git", &NONE),
            repo("github.com", "owner", "repo")
        );
        assert_eq!(
            classify_remote("https://github.com/owner/repo", &NONE),
            repo("github.com", "owner", "repo")
        );
        assert_eq!(
            classify_remote("https://github.com/owner/repo/", &NONE),
            repo("github.com", "owner", "repo")
        );
        // scp-like SSH, and the `ssh://` scheme form with a port.
        assert_eq!(
            classify_remote("git@github.com:owner/repo.git", &NONE),
            repo("github.com", "owner", "repo")
        );
        assert_eq!(
            classify_remote("ssh://git@github.com/owner/repo.git", &NONE),
            repo("github.com", "owner", "repo")
        );
        assert_eq!(
            classify_remote("ssh://git@github.com:22/owner/repo.git", &NONE),
            repo("github.com", "owner", "repo")
        );
        assert_eq!(
            classify_remote("git://github.com/owner/repo", &NONE),
            repo("github.com", "owner", "repo")
        );
        assert_eq!(
            classify_remote(
                "https://github.company.com/owner/repo.git",
                &github("github.company.com")
            ),
            repo("github.company.com", "owner", "repo")
        );
    }

    #[test]
    fn repository_identity_parses_gitlab_remote_forms() {
        let repo = |host: &str, segments: &[&str]| {
            RepositoryIdentity::Repository(
                RepoTarget::with_path(Forge::GitLab, host, segments).unwrap(),
            )
        };
        assert_eq!(
            classify_remote("https://gitlab.com/owner/repo.git", &NONE),
            repo("gitlab.com", &["owner", "repo"])
        );
        assert_eq!(
            classify_remote("git@gitlab.com:owner/repo.git", &NONE),
            repo("gitlab.com", &["owner", "repo"])
        );
        // Nested groups keep the full namespace path.
        assert_eq!(
            classify_remote("https://gitlab.com/group/subgroup/project.git", &NONE),
            repo("gitlab.com", &["group", "subgroup", "project"])
        );
        assert_eq!(
            classify_remote("git@git.corp.example:team/sub/repo.git", &gitlab("git.corp.example")),
            repo("git.corp.example", &["team", "sub", "repo"])
        );
        // A GitHub target and a GitLab target on the same path are different targets.
        let RepositoryIdentity::Repository(on_github) =
            classify_remote("https://github.com/owner/repo", &NONE)
        else {
            panic!("expected a repository identity");
        };
        let RepositoryIdentity::Repository(on_gitlab) =
            classify_remote("https://gitlab.com/owner/repo", &NONE)
        else {
            panic!("expected a repository identity");
        };
        assert_ne!(on_github, on_gitlab);
        assert_eq!(on_github.forge(), Forge::GitHub);
        assert_eq!(on_gitlab.forge(), Forge::GitLab);
        // A single-segment GitLab path is malformed, not unsupported.
        assert_eq!(
            classify_remote("https://gitlab.com/owner", &NONE),
            RepositoryIdentity::Malformed("gitlab.com".to_string())
        );
    }

    #[test]
    fn repository_identity_rejects_aliases_and_keeps_failure_states_distinct() {
        assert_eq!(
            classify_remote("git@github.com-work:owner/repo.git", &NONE),
            RepositoryIdentity::Unsupported("github.com-work".to_string())
        );
        assert_eq!(
            classify_remote(
                "git@github.company.com-work:owner/repo.git",
                &github("github.company.com")
            ),
            RepositoryIdentity::Unsupported("github.company.com-work".to_string())
        );
        assert_eq!(
            classify_remote("https://github.com-attacker/owner/repo", &NONE),
            RepositoryIdentity::Unsupported("github.com-attacker".to_string())
        );
        assert_eq!(
            classify_remote(
                "https://github.company.com-work/owner/repo",
                &github("github.company.com")
            ),
            RepositoryIdentity::Unsupported("github.company.com-work".to_string())
        );
        assert_eq!(
            classify_remote("git@gitlab.com-work:owner/repo.git", &NONE),
            RepositoryIdentity::Unsupported("gitlab.com-work".to_string())
        );
        assert_eq!(
            classify_remote("https://bitbucket.org/owner/repo", &NONE),
            RepositoryIdentity::Unsupported("bitbucket.org".to_string())
        );
        assert_eq!(
            classify_remote("https://github.com/owner", &NONE),
            RepositoryIdentity::Malformed("github.com".to_string())
        );
        assert_eq!(
            classify_remote("https://github.com", &NONE),
            RepositoryIdentity::Malformed("github.com".to_string())
        );
        assert_eq!(
            classify_remote(
                "https://github.company.com:8443/owner/repo.git",
                &github("github.company.com")
            ),
            RepositoryIdentity::Unsupported("github.company.com".to_string())
        );
        assert_eq!(classify_remote("/tmp/repo", &NONE), RepositoryIdentity::Hostless);
        assert_eq!(classify_remote("file:///tmp/repo", &NONE), RepositoryIdentity::Hostless);
        assert_eq!(
            classify_remote("file://github.com/owner/repo", &NONE),
            RepositoryIdentity::Unsupported("github.com".to_string())
        );
        assert_eq!(
            classify_remote("ftp://github.com/owner/repo", &NONE),
            RepositoryIdentity::Unsupported("github.com".to_string())
        );
    }

    #[test]
    fn repository_identity_parses_azure_devops_remote_forms_to_one_target() {
        let repo = |host: &str, org: &str, project: &str, name: &str| {
            RepositoryIdentity::Repository(
                RepoTarget::with_path(Forge::AzureDevOps, host, &[org, project, name]).unwrap(),
            )
        };
        // The https `_git` form, with and without `.git`, plus case-insensitive hosts.
        assert_eq!(
            classify_remote("https://dev.azure.com/org/project/_git/repo", &NONE),
            repo("dev.azure.com", "org", "project", "repo")
        );
        assert_eq!(
            classify_remote("https://DEV.AZURE.COM/org/project/_git/repo.git", &NONE),
            repo("dev.azure.com", "org", "project", "repo")
        );
        // A repository named after its project omits the project segment.
        assert_eq!(
            classify_remote("https://dev.azure.com/org/_git/repo", &NONE),
            repo("dev.azure.com", "org", "repo", "repo")
        );
        // The v3 ssh forms normalize to the https host, so both clones are one target.
        assert_eq!(
            classify_remote("git@ssh.dev.azure.com:v3/org/project/repo", &NONE),
            repo("dev.azure.com", "org", "project", "repo")
        );
        assert_eq!(
            classify_remote("ssh://git@ssh.dev.azure.com/v3/org/project/repo", &NONE),
            repo("dev.azure.com", "org", "project", "repo")
        );
        // The legacy organization hosts, with the wildcard match and the org hoist.
        assert_eq!(
            classify_remote("https://org.visualstudio.com/project/_git/repo", &NONE),
            repo("org.visualstudio.com", "org", "project", "repo")
        );
        assert_eq!(
            classify_remote(
                "https://org.visualstudio.com/DefaultCollection/project/_git/repo",
                &NONE
            ),
            repo("org.visualstudio.com", "org", "project", "repo")
        );
        assert_eq!(
            classify_remote("org@vs-ssh.visualstudio.com:v3/org/project/repo", &NONE),
            repo("org.visualstudio.com", "org", "project", "repo")
        );
        // A self-hosted server recognized through `azure_devops_host`, collection first.
        assert_eq!(
            classify_remote(
                "https://tfs.corp.example/collection/project/_git/repo",
                &azure_devops("tfs.corp.example")
            ),
            repo("tfs.corp.example", "collection", "project", "repo")
        );
        // A project named with a space travels percent-encoded and is addressed decoded.
        assert_eq!(
            classify_remote("https://dev.azure.com/extruct/Extruct%20AI/_git/reviewr-qa", &NONE),
            repo("dev.azure.com", "extruct", "Extruct AI", "reviewr-qa")
        );
        // The organization is case-insensitive on Azure DevOps and the legacy host derives
        // it lowercased, so every casing and clone form is one target.
        assert_eq!(
            classify_remote("https://dev.azure.com/Extruct/project/_git/repo", &NONE),
            repo("dev.azure.com", "extruct", "project", "repo")
        );
        assert_eq!(
            classify_remote("Org@vs-ssh.visualstudio.com:v3/Extruct/project/repo", &NONE),
            repo("extruct.visualstudio.com", "extruct", "project", "repo")
        );
        // On a self-hosted server the first segment is the collection identity, so a
        // literal `DefaultCollection` collection survives canonicalization.
        assert_eq!(
            classify_remote(
                "https://tfs.corp.example/DefaultCollection/proj/_git/repo",
                &azure_devops("tfs.corp.example")
            ),
            repo("tfs.corp.example", "defaultcollection", "proj", "repo")
        );
        // A broken escape is a malformed remote, not a silent misread.
        assert_eq!(
            classify_remote("https://dev.azure.com/org/Bad%2/_git/repo", &NONE),
            RepositoryIdentity::Malformed("dev.azure.com".to_string())
        );
    }

    #[test]
    fn repository_identity_rejects_malformed_azure_devops_paths() {
        // A project URL is not a repository, and extra segments are not an identity.
        assert_eq!(
            classify_remote("https://dev.azure.com/org/project", &NONE),
            RepositoryIdentity::Malformed("dev.azure.com".to_string())
        );
        assert_eq!(
            classify_remote("https://dev.azure.com/org/project/_git/repo/extra", &NONE),
            RepositoryIdentity::Malformed("dev.azure.com".to_string())
        );
        // A self-hosted virtual directory is not supported: its extra path segment leaves a
        // four-part path, which is malformed, not a silently misread target.
        assert_eq!(
            classify_remote(
                "https://tfs.corp.example/tfs/collection/project/_git/repo",
                &azure_devops("tfs.corp.example")
            ),
            RepositoryIdentity::Malformed("tfs.corp.example".to_string())
        );
        assert_eq!(
            classify_remote("https://dev.azure.com", &NONE),
            RepositoryIdentity::Malformed("dev.azure.com".to_string())
        );
        // The wildcard needs an organization label; the bare domain stays unsupported.
        assert_eq!(
            classify_remote("https://visualstudio.com/org/project/_git/repo", &NONE),
            RepositoryIdentity::Unsupported("visualstudio.com".to_string())
        );
        // An unrecognized host never reaches the Azure DevOps path shaping.
        assert_eq!(
            classify_remote("https://dev.azure.com.evil.example/org/project/_git/repo", &NONE),
            RepositoryIdentity::Unsupported("dev.azure.com.evil.example".to_string())
        );
        // An option-shaped segment can never become an `az` argument.
        assert_eq!(
            classify_remote("https://dev.azure.com/org/--project/_git/repo", &NONE),
            RepositoryIdentity::Malformed("dev.azure.com".to_string())
        );
    }

    #[test]
    fn azure_devops_vocabulary_matches_the_provider_contract() {
        assert_eq!(Forge::AzureDevOps.display_name(), "Azure DevOps");
        assert_eq!(Forge::AzureDevOps.noun(), "pull request");
        assert_eq!(Forge::AzureDevOps.abbr(), "PR");
        assert_eq!(Forge::AzureDevOps.sigil(), '#');
        assert_eq!(Forge::AzureDevOps.cli(), "az");
    }

    #[test]
    fn repository_target_enforces_its_canonical_shape() {
        let target = RepoTarget::new("GitHub.COM", "owner", "repo").unwrap();
        assert_eq!(target.host(), "github.com");
        assert_eq!(target.owner(), "owner");
        assert_eq!(target.name(), "repo");
        assert!(RepoTarget::new("bad host", "owner", "repo").is_none());
        assert!(RepoTarget::new("github.com", ".", "repo").is_none());
        assert!(RepoTarget::new("github.com", "owner/name", "repo").is_none());
        assert!(RepoTarget::new("github.com", "owner", "bad\nname").is_none());
        assert!(RepoTarget::new("github.com", "owner", "bad\u{202e}name").is_none());
        // A GitHub path is exactly two segments; a GitLab path is two or more.
        assert!(RepoTarget::with_path(Forge::GitHub, "github.com", &["a", "b", "c"]).is_none());
        let nested =
            RepoTarget::with_path(Forge::GitLab, "gitlab.com", &["group", "sub", "repo"]).unwrap();
        assert_eq!(nested.full_path(), "group/sub/repo");
        assert_eq!(nested.name(), "repo");
        assert!(RepoTarget::with_path(Forge::GitLab, "gitlab.com", &["only"]).is_none());
    }

    #[test]
    fn numstat_parses_counts_and_keeps_the_no_text_diff_verdict() {
        let m = parse_numstat("18\t8\tsrc/a.rs\0-\t-\tassets/logo.png\0");
        assert_eq!(m["src/a.rs"], Some((18, 8)));
        // `-`/`-` is git refusing to text-diff, not a change of zero lines. Collapsing the
        // two is what left a `-diff` path (`.gitattributes`) diffed as text.
        assert_eq!(m["assets/logo.png"], None);
    }

    #[test]
    fn numstat_separates_the_no_text_diff_verdict_from_an_empty_change() {
        let m = parse_numstat("0\t0\tsrc/touched.rs\0-\t-\tflake.lock\0");
        assert_eq!(m["src/touched.rs"], Some((0, 0)));
        assert_eq!(m["flake.lock"], None);
    }

    #[test]
    fn numstat_keys_renames_under_the_new_path() {
        // Under `-z` a rename is `ADDS\tDELS\t\0OLD\0NEW`: old and new are their own fields,
        // no `=>` arrow or brace form. Counts must key under the new path.
        let m = parse_numstat("3\t1\t\0src/old.rs\0src/new.rs\0");
        assert_eq!(m["src/new.rs"], Some((3, 1)));
        assert!(!m.contains_key("src/old.rs"));
    }

    #[test]
    fn numstat_dir_removing_rename_has_no_double_slash() {
        // Regression: the old brace parser produced `a//file.rs` here, so counts never matched.
        let m = parse_numstat("4\t2\t\0a/b/file.rs\0a/file.rs\0");
        assert_eq!(m["a/file.rs"], Some((4, 2)));
        assert!(!m.contains_key("a//file.rs"));
    }

    #[test]
    fn numstat_handles_a_mixed_stream() {
        // binary, plain, rename, in sequence — the rename lookahead must stay aligned.
        // `\x00` (= NUL) is used as the separator so the digits after it read clearly.
        let m = parse_numstat("-\t-\tlogo.png\x009\t1\tsrc/a.rs\x005\t4\t\x00o.rs\x00n.rs\x00");
        assert_eq!(m["logo.png"], None);
        assert_eq!(m["src/a.rs"], Some((9, 1)));
        assert_eq!(m["n.rs"], Some((5, 4)));
    }

    #[test]
    fn a_raw_record_reads_its_kind_its_path_and_a_renames_source() {
        let meta =
            |status: &str| format!(":100644 100644 {} {} {status}", "a".repeat(40), "0".repeat(40));
        let raw = [
            meta("M"),
            "src/a.rs".into(),
            meta("A"),
            "src/b.rs".into(),
            meta("D"),
            "src/c.rs".into(),
            meta("R100"),
            "old.rs".into(),
            "new.rs".into(),
            meta("M"),
            "with\nnewline".into(),
            meta("M"),
            ":colon-led".into(),
            // The numstat records that follow the raw ones in the same run.
            "1\t1\tsrc/a.rs".into(),
            String::new(),
        ]
        .join("\0");
        let (rows, numstat) = parse_raw(&raw);
        assert_eq!(numstat, "1\t1\tsrc/a.rs\0");
        assert_eq!(rows[0].old_oid, "a".repeat(40));
        let rows: Vec<_> = rows.into_iter().map(|r| (r.kind, r.path, r.previous_path)).collect();
        assert_eq!(rows[0], (ChangeKind::Modified, "src/a.rs".to_string(), None));
        assert_eq!(rows[1], (ChangeKind::Added, "src/b.rs".to_string(), None));
        assert_eq!(rows[2], (ChangeKind::Deleted, "src/c.rs".to_string(), None));
        assert_eq!(
            rows[3],
            (ChangeKind::Renamed, "new.rs".to_string(), Some("old.rs".to_string()))
        );
        assert_eq!(rows[4], (ChangeKind::Modified, "with\nnewline".to_string(), None));
        assert_eq!(rows[5], (ChangeKind::Modified, ":colon-led".to_string(), None));
    }

    #[test]
    fn a_copy_keys_under_its_new_path() {
        // A copy carries old + new like a rename; it must key under the new path, not collapse
        // to a Modified entry on the source path.
        let raw = format!(":100644 100644 {0} {0} C75\0orig.rs\0copy.rs\0", "b".repeat(40));
        let row = &parse_raw(&raw).0[0];
        assert_eq!(
            (row.kind, row.path.as_str(), row.previous_path.as_deref()),
            (ChangeKind::Copied, "copy.rs", Some("orig.rs"))
        );
    }

    #[test]
    fn a_full_context_diff_spells_both_sides_exactly() {
        use super::{DiffSides, parse_sides};
        let text = |old: &str, new: &str| {
            Some(DiffSides::Text { old: old.to_string(), new: new.to_string() })
        };
        let header = "diff --git a/f b/f\nindex 1..2 100644\n--- a/f\n+++ b/f\n";
        let cases: &[(&str, Option<DiffSides>)] = &[
            // Context lands on both sides, `-` on the old, `+` on the new. A body line that
            // looks like a header is still body: here the deletion of `-- x`.
            (" a\n--- x\n+b\n c\n", text("a\n-- x\nc\n", "a\nb\nc\n")),
            // A bare newline is an empty context line.
            (" a\n\n-b\n", text("a\n\nb\n", "a\n\n")),
            // The marker takes the newline off the line before it, on that line's side only.
            (" x\n-y\n\\ No newline at end of file\n+z\n", text("x\ny", "x\nz\n")),
            (" x\n\\ No newline at end of file\n", text("x", "x")),
            // A CR git keeps is the line's text.
            ("-one\n+one\r\n", text("one\n", "one\r\n")),
        ];
        for (body, want) in cases {
            let lines = body.lines().filter(|l| !l.starts_with('\\')).count();
            let olds = body.lines().filter(|l| !l.starts_with(['+', '\\'])).count();
            let news = lines - body.lines().filter(|l| l.starts_with('-')).count();
            let out = format!("{header}@@ -1,{olds} +1,{news} @@\n{body}");
            assert_eq!(parse_sides(&out), *want, "{body:?}");
        }
        // An added file, its one-line hunk count left out.
        let added =
            "diff --git a/f b/f\nnew file mode 100644\n--- /dev/null\n+++ b/f\n@@ -0,0 +1 @@\n+a\n";
        assert_eq!(parse_sides(added), text("", "a\n"));
        // Two sections (a rename git did not pair): the old path's lines, then the new one's.
        let unpaired = format!("{header}@@ -1 +0,0 @@\n-old\n{header}@@ -0,0 +1 @@\n+new\n");
        assert_eq!(parse_sides(&unpaired), text("old\n", "new\n"));
        assert_eq!(
            parse_sides(&format!("{header}Binary files a/f and b/f differ\n")),
            Some(DiffSides::Binary)
        );
        // No hunk: the sides are equal, and the caller reads the one blob.
        assert_eq!(parse_sides("diff --git a/f b/g\nsimilarity index 100%\n"), None);
        assert_eq!(parse_sides(""), None);
    }

    #[test]
    fn a_hunk_header_reads_both_ranges_a_count_of_one_left_out() {
        use super::HunkHeader;
        let rows = [
            ("@@ -105,11 +105,12 @@\n", Some(((105, 11), (105, 12)))),
            ("@@ -0,0 +1 @@\n", Some(((0, 0), (1, 1)))),
            ("@@ -7 +7,0 @@ fn main() {\n", Some(((7, 1), (7, 0)))),
            ("@@ +1 -1 @@\n", None),
            ("@@@ -1 -1 +1 @@@\n", None),
        ];
        for (line, want) in rows {
            let got = HunkHeader::parse(line).map(|h| (h.old, h.new));
            assert_eq!(got, want, "{line:?}");
        }
    }
}
