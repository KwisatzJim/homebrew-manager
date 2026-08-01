//! Core logic for talking to the `brew` CLI.
//!
//! Every operation that shells out runs on a background thread and reports
//! back to the GUI thread through an `mpsc::Sender<Event>`. Nothing here
//! touches egui, so this module can be compiled and tested on its own.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::thread;

/// Messages sent from background worker threads back to the GUI.
#[derive(Debug, Clone)]
pub enum Event {
    /// A single line of output from a running command (stdout or stderr).
    Log(String),
    /// A running command finished.
    Finished {
        success: bool,
        exit_code: Option<i32>,
    },
    /// Result of checking whether brew is installed.
    Status(BrewStatus),
    InstalledFormulae(Vec<Package>),
    InstalledCasks(Vec<Package>),
    Outdated(Vec<OutdatedPackage>),
    SearchResults(Vec<String>),
    /// (name, full `brew info` text)
    Info(String, String),
}

#[derive(Debug, Clone, Default)]
pub struct BrewStatus {
    pub installed: bool,
    pub version: String,
    pub prefix: String,
    pub brew_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Package {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone)]
pub struct OutdatedPackage {
    pub name: String,
    pub current: String,
    pub latest: String,
    pub pinned: bool,
    pub is_cask: bool,
}

/// Locate the `brew` binary: first via `PATH`, then via well-known install
/// locations for Apple Silicon macOS, Intel macOS, and Linuxbrew.
pub fn find_brew_binary() -> Option<String> {
    if let Ok(output) = Command::new("sh").arg("-c").arg("command -v brew").output() {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Some(path);
            }
        }
    }
    for candidate in [
        "/opt/homebrew/bin/brew",              // Apple Silicon macOS
        "/usr/local/bin/brew",                 // Intel macOS
        "/home/linuxbrew/.linuxbrew/bin/brew", // Linux
    ] {
        if Path::new(candidate).exists() {
            return Some(candidate.to_string());
        }
    }
    None
}

/// Synchronous status check (fast — a couple of quick subprocess calls).
/// Safe to call from a background thread and send the result back as an
/// `Event::Status`.
pub fn get_status() -> BrewStatus {
    match find_brew_binary() {
        None => BrewStatus {
            installed: false,
            ..Default::default()
        },
        Some(path) => {
            let version = Command::new(&path)
                .arg("--version")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .and_then(|s| s.lines().next().map(|l| l.to_string()))
                .unwrap_or_default();
            let prefix = Command::new(&path)
                .arg("--prefix")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            BrewStatus {
                installed: true,
                version,
                prefix,
                brew_path: Some(path),
            }
        }
    }
}

pub fn refresh_status_async(tx: Sender<Event>) {
    thread::spawn(move || {
        let status = get_status();
        let _ = tx.send(Event::Status(status));
    });
}

/// Run a program with arguments, streaming stdout+stderr line-by-line back
/// as `Event::Log`, followed by a final `Event::Finished`.
pub fn run_streaming(program: String, args: Vec<String>, envs: Vec<(String, String)>, tx: Sender<Event>) {
    thread::spawn(move || {
        let mut cmd = Command::new(&program);
        cmd.args(&args);
        for (k, v) in &envs {
            cmd.env(k, v);
        }
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.stdin(Stdio::null());

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(Event::Log(format!("Failed to start `{program}`: {e}")));
                let _ = tx.send(Event::Finished {
                    success: false,
                    exit_code: None,
                });
                return;
            }
        };

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let out_handle = stdout.map(|s| {
            let tx = tx.clone();
            thread::spawn(move || {
                for line in BufReader::new(s).lines().map_while(Result::ok) {
                    let _ = tx.send(Event::Log(line));
                }
            })
        });
        let err_handle = stderr.map(|s| {
            let tx = tx.clone();
            thread::spawn(move || {
                for line in BufReader::new(s).lines().map_while(Result::ok) {
                    let _ = tx.send(Event::Log(line));
                }
            })
        });

        if let Some(h) = out_handle {
            let _ = h.join();
        }
        if let Some(h) = err_handle {
            let _ = h.join();
        }

        match child.wait() {
            Ok(status) => {
                let _ = tx.send(Event::Finished {
                    success: status.success(),
                    exit_code: status.code(),
                });
            }
            Err(e) => {
                let _ = tx.send(Event::Log(format!("Error waiting on `{program}`: {e}")));
                let _ = tx.send(Event::Finished {
                    success: false,
                    exit_code: None,
                });
            }
        }
    });
}

/// Convenience wrapper for running `brew <args...>` with streamed output.
fn run_brew(brew_path: &str, args: &[&str], tx: Sender<Event>) {
    run_streaming(
        brew_path.to_string(),
        args.iter().map(|s| s.to_string()).collect(),
        Vec::new(),
        tx,
    );
}

// ---------------------------------------------------------------------
// Installing Homebrew itself
// ---------------------------------------------------------------------

/// Run the official Homebrew install script non-interactively
/// (`NONINTERACTIVE=1` skips the "Press RETURN to continue" prompt).
/// Note: on some systems the script may still invoke `sudo` for steps like
/// installing Xcode Command Line Tools, which will hang waiting for a
/// password if run headlessly — this is a limitation of the upstream
/// installer, not this app.
pub fn install_homebrew(tx: Sender<Event>) {
    let script = "curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh | bash";
    run_streaming(
        "bash".to_string(),
        vec!["-c".to_string(), script.to_string()],
        vec![("NONINTERACTIVE".to_string(), "1".to_string())],
        tx,
    );
}

// ---------------------------------------------------------------------
// Listing installed packages
// ---------------------------------------------------------------------

pub fn refresh_installed(brew_path: String, tx: Sender<Event>) {
    thread::spawn(move || {
        if let Ok(o) = Command::new(&brew_path)
            .args(["list", "--formula", "--versions"])
            .output()
        {
            let text = String::from_utf8_lossy(&o.stdout);
            let _ = tx.send(Event::InstalledFormulae(parse_versions_list(&text)));
        }
        if let Ok(o) = Command::new(&brew_path)
            .args(["list", "--cask", "--versions"])
            .output()
        {
            let text = String::from_utf8_lossy(&o.stdout);
            let _ = tx.send(Event::InstalledCasks(parse_versions_list(&text)));
        }
    });
}

fn parse_versions_list(text: &str) -> Vec<Package> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let mut parts = l.split_whitespace();
            let name = parts.next().unwrap_or("").to_string();
            let version = parts.collect::<Vec<_>>().join(", ");
            Package { name, version }
        })
        .collect()
}

// ---------------------------------------------------------------------
// Outdated packages
// ---------------------------------------------------------------------

pub fn refresh_outdated(brew_path: String, tx: Sender<Event>) {
    thread::spawn(move || {
        let mut all = Vec::new();
        if let Ok(o) = Command::new(&brew_path)
            .args(["outdated", "--formula", "--verbose"])
            .output()
        {
            let text = String::from_utf8_lossy(&o.stdout);
            all.extend(parse_outdated(&text, false));
        }
        if let Ok(o) = Command::new(&brew_path)
            .args(["outdated", "--cask", "--verbose"])
            .output()
        {
            let text = String::from_utf8_lossy(&o.stdout);
            all.extend(parse_outdated(&text, true));
        }
        let _ = tx.send(Event::Outdated(all));
    });
}

/// Parses lines like:
///   `git (2.40.0) < 2.42.0`
///   `some-cask (1.2) != 1.3 [pinned]`
fn parse_outdated(text: &str, is_cask: bool) -> Vec<OutdatedPackage> {
    let mut result = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let pinned = line.contains("[pinned");
        let Some(paren_start) = line.find('(') else {
            continue;
        };
        let Some(paren_end) = line.find(')') else {
            continue;
        };
        let name = line[..paren_start].trim().to_string();
        let current = line[paren_start + 1..paren_end].to_string();
        let rest = line[paren_end + 1..].trim_start();
        let rest = rest.trim_start_matches("!=").trim_start_matches('<').trim_start();
        let latest = rest.split_whitespace().next().unwrap_or("").to_string();
        result.push(OutdatedPackage {
            name,
            current,
            latest,
            pinned,
            is_cask,
        });
    }
    result
}

// ---------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------

pub fn search(brew_path: String, query: String, tx: Sender<Event>) {
    thread::spawn(move || {
        if let Ok(o) = Command::new(&brew_path).args(["search", &query]).output() {
            let text = String::from_utf8_lossy(&o.stdout);
            let results: Vec<String> = text
                .lines()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty() && !l.starts_with("==>"))
                .map(|s| s.to_string())
                .collect();
            let _ = tx.send(Event::SearchResults(results));
        }
    });
}

pub fn info(brew_path: String, name: String, tx: Sender<Event>) {
    thread::spawn(move || {
        let text = Command::new(&brew_path)
            .args(["info", &name])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();
        let _ = tx.send(Event::Info(name, text));
    });
}

// ---------------------------------------------------------------------
// Mutating operations (all streamed so the console shows live progress)
// ---------------------------------------------------------------------

pub fn update(brew_path: String, tx: Sender<Event>) {
    run_brew(&brew_path, &["update"], tx);
}

pub fn upgrade_all(brew_path: String, tx: Sender<Event>) {
    run_brew(&brew_path, &["upgrade"], tx);
}

pub fn upgrade_packages(brew_path: String, names: Vec<String>, tx: Sender<Event>) {
    let mut args = vec!["upgrade".to_string()];
    args.extend(names);
    let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
    run_brew(&brew_path, &args_ref, tx);
}

pub fn install_packages(brew_path: String, names: Vec<String>, cask: bool, tx: Sender<Event>) {
    let mut args = vec!["install".to_string()];
    if cask {
        args.push("--cask".to_string());
    }
    args.extend(names);
    let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
    run_brew(&brew_path, &args_ref, tx);
}

pub fn uninstall_packages(brew_path: String, names: Vec<String>, cask: bool, tx: Sender<Event>) {
    let mut args = vec!["uninstall".to_string()];
    if cask {
        args.push("--cask".to_string());
    }
    args.extend(names);
    let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
    run_brew(&brew_path, &args_ref, tx);
}

pub fn pin_packages(brew_path: String, names: Vec<String>, pin: bool, tx: Sender<Event>) {
    let mut args = vec![if pin { "pin".to_string() } else { "unpin".to_string() }];
    args.extend(names);
    let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
    run_brew(&brew_path, &args_ref, tx);
}

pub fn cleanup(brew_path: String, tx: Sender<Event>) {
    run_brew(&brew_path, &["cleanup", "-s"], tx);
}

pub fn autoremove(brew_path: String, tx: Sender<Event>) {
    run_brew(&brew_path, &["autoremove"], tx);
}

pub fn doctor(brew_path: String, tx: Sender<Event>) {
    run_brew(&brew_path, &["doctor"], tx);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_versions_list() {
        let input = "wget 1.24.5\nfish 3.7.0\n";
        let pkgs = parse_versions_list(input);
        assert_eq!(pkgs.len(), 2);
        assert_eq!(pkgs[0].name, "wget");
        assert_eq!(pkgs[0].version, "1.24.5");
    }

    #[test]
    fn parses_multi_version_line() {
        // brew can list multiple installed versions of one formula on one line
        let input = "python@3.11 3.11.6 3.11.7\n";
        let pkgs = parse_versions_list(input);
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].name, "python@3.11");
        assert_eq!(pkgs[0].version, "3.11.6, 3.11.7");
    }

    #[test]
    fn parses_outdated_formula_line() {
        let input = "git (2.40.0) < 2.42.0\n";
        let out = parse_outdated(input, false);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "git");
        assert_eq!(out[0].current, "2.40.0");
        assert_eq!(out[0].latest, "2.42.0");
        assert!(!out[0].pinned);
    }

    #[test]
    fn parses_outdated_pinned_line() {
        let input = "node (18.0.0) < 20.0.0 [pinned at 18.0.0]\n";
        let out = parse_outdated(input, false);
        assert_eq!(out.len(), 1);
        assert!(out[0].pinned);
    }

    #[test]
    fn parses_outdated_cask_line() {
        let input = "firefox (120.0) != 121.0\n";
        let out = parse_outdated(input, true);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "firefox");
        assert_eq!(out[0].latest, "121.0");
        assert!(out[0].is_cask);
    }

    #[test]
    fn ignores_blank_lines() {
        let input = "\n\nwget 1.0\n\n";
        assert_eq!(parse_versions_list(input).len(), 1);
    }
}
