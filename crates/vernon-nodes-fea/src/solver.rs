//! Running an external solver as a separate process: executable discovery,
//! log files instead of pipes (no deadlock on chatty solvers) and a deadline.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use vernon_core::process::{self, Exit, RunOptions};

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    /// The process exited with this code (-1 when killed by a signal).
    Exited(i32),
    /// The deadline passed and the process tree was killed.
    TimedOut,
    /// The cook was cancelled and the process tree was killed.
    Cancelled,
}

/// A configured path that exists, or the first of `names` found on `PATH`.
pub fn find_executable(configured: &str, names: &[&str]) -> Option<PathBuf> {
    let configured = configured.trim();
    if !configured.is_empty() {
        let p = PathBuf::from(configured);
        return p.is_file().then_some(p);
    }
    let path = std::env::var_os("PATH")?;
    let exts: &[&str] = if cfg!(windows) { &["", ".exe", ".bat", ".cmd"] } else { &[""] };
    for dir in std::env::split_paths(&path) {
        for name in names {
            for ext in exts {
                let cand = dir.join(format!("{name}{ext}"));
                if cand.is_file() {
                    return Some(cand);
                }
            }
        }
    }
    None
}

/// Run `exe args` in `cwd` through the shared runner
/// ([`vernon_core::process`]): both output streams go to `log`, and at the
/// deadline or when the cook is cancelled the whole process tree is killed.
pub fn run_with_timeout(exe: &Path, args: &[String], cwd: &Path, log: &Path, timeout: Duration, envs: &[(String, String)]) -> std::io::Result<RunStatus> {
    let mut cmd = Command::new(exe);
    cmd.args(args).current_dir(cwd);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    // A zero timeout would mean "the default"; keep it a real deadline.
    let opts = RunOptions::with_timeout(timeout.max(Duration::from_millis(1)));
    Ok(match process::run(&mut cmd, log, &opts)? {
        Exit::Code(c) => RunStatus::Exited(c),
        Exit::TimedOut => RunStatus::TimedOut,
        Exit::Cancelled => RunStatus::Cancelled,
    })
}

/// Last `n` lines of a log file (empty when unreadable).
pub fn tail(log: &Path, n: usize) -> String {
    process::tail(log, n)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Instant;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("vernon_fea_{name}_{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Child used by the timeout test: sleeps far longer than the deadline.
    #[test]
    #[ignore]
    fn child_sleeps() {
        std::thread::sleep(Duration::from_secs(60));
    }

    #[test]
    fn runner_reports_exit_codes_and_captures_output() {
        let dir = tmp("run_ok");
        let exe = std::env::current_exe().unwrap();
        let log = dir.join("ok.log");
        let st = run_with_timeout(&exe, &["--list".into()], &dir, &log, Duration::from_secs(60), &[]).unwrap();
        assert_eq!(st, RunStatus::Exited(0));
        assert!(tail(&log, 5).contains("test"), "{}", tail(&log, 5));
        let st = run_with_timeout(&exe, &["--definitely-not-a-flag".into()], &dir, &log, Duration::from_secs(60), &[]).unwrap();
        assert!(matches!(st, RunStatus::Exited(c) if c != 0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_silent_hanging_child_is_killed_at_the_deadline() {
        let dir = tmp("run_hang");
        let exe = std::env::current_exe().unwrap();
        let args: Vec<String> = ["--ignored", "--exact", "solver::tests::child_sleeps"].iter().map(|s| s.to_string()).collect();
        let t0 = Instant::now();
        let st = run_with_timeout(&exe, &args, &dir, &dir.join("hang.log"), Duration::from_millis(800), &[]).unwrap();
        assert_eq!(st, RunStatus::TimedOut);
        assert!(t0.elapsed() < Duration::from_secs(20));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_executables_are_none() {
        assert!(find_executable("definitely/not/here.exe", &[]).is_none());
        assert!(find_executable("", &["vernon_no_such_solver_xyz"]).is_none());
        let exe = std::env::current_exe().unwrap();
        assert_eq!(find_executable(exe.to_str().unwrap(), &[]), Some(exe));
    }
}
