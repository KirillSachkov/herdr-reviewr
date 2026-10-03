//! Open a URL in the user's browser — the `PR` tab's only outward action.
//!
//! A configured opener wins; otherwise the host platform's default is used.

use std::process::{Command, Stdio};

use anyhow::{Context, Result};

#[cfg(target_os = "macos")]
const OPENERS: &[&str] = &["open"];
#[cfg(target_os = "linux")]
const OPENERS: &[&str] = &["xdg-open"];
#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
const OPENERS: &[&str] = &["open", "xdg-open"];

/// How a link opens when no `url_opener` is configured.
#[derive(Debug, PartialEq, Eq)]
enum DefaultOpener {
    /// A platform opener program, found on `PATH`.
    #[cfg(not(windows))]
    Tool(&'static str),
    /// The default browser, through `ShellExecuteW`. No shell parses the URL, so an `&` in it
    /// can't cut it short or start a second command, as it would through `cmd /c start`.
    #[cfg(windows)]
    Shell,
}

/// The first platform opener the `present` predicate accepts, in list order. Windows needs
/// none on `PATH`: its shell opens links itself.
#[cfg(not(windows))]
fn default_opener(present: impl Fn(&str) -> bool) -> Result<DefaultOpener> {
    OPENERS
        .iter()
        .copied()
        .find(|candidate| present(candidate))
        .map(DefaultOpener::Tool)
        .context("no link opener: install open or xdg-open, or set `url_opener`")
}

#[cfg(windows)]
#[allow(clippy::unnecessary_wraps)] // One signature on every OS.
fn default_opener(_present: impl Fn(&str) -> bool) -> Result<DefaultOpener> {
    Ok(DefaultOpener::Shell)
}

/// Open `url` through the configured `url_opener`, else the platform default.
pub fn open(url: &str, configured: Option<&str>) -> Result<()> {
    let Some(template) = configured else {
        return match default_opener(crate::proc::on_path)? {
            #[cfg(not(windows))]
            DefaultOpener::Tool(tool) => {
                let mut command = crate::proc::command(tool);
                command.arg(url);
                spawn_detached(tool, command)
            }
            // The call returns once the shell has handed the URL on, leaving nothing to reap.
            #[cfg(windows)]
            DefaultOpener::Shell => opener::open(url)
                .map_err(|error| anyhow::anyhow!("the default browser could not start: {error}")),
        };
    };
    let (program, args) = opener_argv(template, url).context("`url_opener` names no program")?;
    let mut command = crate::proc::user_command(&program)
        .with_context(|| format!("`url_opener` not found: {program}"))?;
    command.args(&args);
    spawn_detached(&program, command)
}

/// Run an opener detached from the frame: it is started and reaped on a background thread,
/// never waited on, so a command that lingers (a browser launched in the foreground, a bridge
/// to an unreachable host) can never freeze the pane. A command that cannot start is reported;
/// what it does once running is its own.
fn spawn_detached(tool: &str, mut command: Command) -> Result<()> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| anyhow::anyhow!("{tool} could not start: {error}"))?;
    // Reaped off the frame thread, so a finished opener never lingers as a zombie.
    std::thread::spawn(move || drop(child.wait()));
    Ok(())
}

/// The configured opener's program and arguments: `template` split the way the `editor` key
/// is, `{url}` substituted per word — so a URL never splits — and appended when absent.
fn opener_argv(template: &str, url: &str) -> Option<(String, Vec<String>)> {
    let names_url = template.contains("{url}");
    let mut words =
        crate::editor::split_command(template).into_iter().map(|w| w.replace("{url}", url));
    let program = words.next().filter(|p| !p.is_empty())?;
    let mut args: Vec<String> = words.collect();
    if !names_url {
        args.push(url.to_string());
    }
    Some((program, args))
}

/// Gate a markdown link destination before it reaches the OS opener
/// : trimmed, case-insensitive `http://`/`https://` with something
/// after the scheme, and no control or bidirectional-override character anywhere — a
/// destination the display would sanitize must never open as different bytes.
pub fn openable_url(url: &str) -> Result<&str, &'static str> {
    let trimmed = url.trim();
    let hostile = trimmed.chars().any(crate::markdown::hostile_char);
    let b = trimmed.as_bytes();
    let schemed = (b.len() > 7 && b[..7].eq_ignore_ascii_case(b"http://"))
        || (b.len() > 8 && b[..8].eq_ignore_ascii_case(b"https://"));
    if !hostile && schemed { Ok(trimmed) } else { Err("unsupported link scheme") }
}

#[cfg(test)]
mod tests {
    use super::{default_opener, open, openable_url, opener_argv};

    #[test]
    fn a_configured_opener_that_cannot_start_is_reported_never_replaced() {
        let error = open("https://x.dev", Some("reviewr-no-such-opener {url}")).unwrap_err();
        assert!(error.to_string().contains("reviewr-no-such-opener"), "{error}");
    }

    #[cfg(not(windows))]
    #[test]
    fn the_default_opener_is_the_first_one_on_path_and_its_absence_says_what_to_install() {
        use super::DefaultOpener::Tool;
        let all = |_: &str| true;
        #[cfg(target_os = "macos")]
        assert_eq!(default_opener(all).unwrap(), Tool("open"));
        #[cfg(target_os = "linux")]
        assert_eq!(default_opener(all).unwrap(), Tool("xdg-open"));
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        assert_eq!(default_opener(all).unwrap(), Tool("open"));
        assert_eq!(
            default_opener(|_| false).unwrap_err().to_string(),
            "no link opener: install open or xdg-open, or set `url_opener`"
        );
    }

    /// Windows opens a link through its shell, so nothing has to be on `PATH`.
    #[cfg(windows)]
    #[test]
    fn the_default_opener_on_windows_is_the_shell_with_nothing_on_path() {
        assert_eq!(default_opener(|_| false).unwrap(), super::DefaultOpener::Shell);
    }

    #[test]
    fn the_opener_template_splits_like_editor_and_places_the_url() {
        let url = "https://example.com/pr?a=1&b=2";
        let argv = |t: &str| opener_argv(t, url).map(|(p, a)| (p, a.join("|")));
        assert_eq!(argv("remote-open"), Some(("remote-open".into(), url.into())), "appended");
        assert_eq!(
            argv("ssh laptop 'open -g' {url}"),
            Some(("ssh".into(), format!("laptop|open -g|{url}"))),
            "quoted words stay whole",
        );
        assert_eq!(
            argv("bridge --url={url} --new"),
            Some(("bridge".into(), format!("--url={url}|--new"))),
            "placed where named, never appended twice",
        );
        assert_eq!(argv("   "), None, "no program");
        // A URL is one word whatever it holds, and is substituted once.
        let odd = "https://x/a b'c{url}";
        assert_eq!(opener_argv("bridge {url}", odd).map(|(_, a)| a), Some(vec![odd.to_string()]));
    }

    #[test]
    fn the_url_guard_admits_http_and_https_case_insensitively() {
        assert_eq!(openable_url("https://ci.example/1"), Ok("https://ci.example/1"));
        assert_eq!(openable_url("HTTP://ci.example"), Ok("HTTP://ci.example"));
        assert_eq!(openable_url("  https://x.dev  "), Ok("https://x.dev"), "trimmed");
    }

    #[test]
    fn the_url_guard_rejects_other_schemes_and_hostile_bytes() {
        for bad in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "https:evil", // scheme without authority
            "https://",   // nothing after the scheme
            "ftp://host",
            "https://a\u{202e}b",   // bidi override
            "https://a\u{1b}[31mb", // control character
            "",
        ] {
            assert!(openable_url(bad).is_err(), "{bad:?} must not open");
        }
    }
}
