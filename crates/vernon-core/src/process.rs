//! Running external programs (solvers, meshers, slicers, photogrammetry).
//!
//! Every node that starts a program goes through [`run`], which
//! - sends stdout and stderr to a log file, never to a pipe, so neither a
//!   chatty program nor a grandchild that keeps a pipe open can block;
//! - always has a deadline (a zero timeout means [`DEFAULT_TIMEOUT`]);
//! - stops on a cancel flag, either passed in [`RunOptions`] or set for the
//!   whole cook thread with [`with_cancel`];
//! - kills the whole process tree on a timeout or cancel (on Windows
//!   `Child::kill` ends only the direct child, so a launcher such as
//!   `wsl.exe`, `mpirun` or a `docker` client would leave the real work
//!   running), then runs an optional clean-up command such as `docker stop`.

use std::cell::RefCell;
use std::fs::{File, OpenOptions};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Deadline used when a caller gives none (a zero timeout): one day.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(24 * 3600);

/// Longest a clean-up command (or the tree kill) may take.
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(30);

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// The program exited with this code (-1 when ended by a signal).
    Code(i32),
    /// The deadline passed; the process tree was killed.
    TimedOut,
    /// The cancel flag was set; the process tree was killed.
    Cancelled,
}

impl Exit {
    /// True for exit code 0.
    pub fn success(self) -> bool {
        self == Exit::Code(0)
    }
}

/// Options of [`run`].
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// Deadline; zero means [`DEFAULT_TIMEOUT`].
    pub timeout: Duration,
    /// Stop when this flag is set (besides the cook thread's flag, see [`with_cancel`]).
    pub cancel: Option<Arc<AtomicBool>>,
    /// Append to the log instead of starting it afresh.
    pub append: bool,
    /// Command run after a timeout or cancel kill, e.g. `docker stop NAME`
    /// for a container that outlives its client.
    pub cleanup: Vec<String>,
}

impl RunOptions {
    pub fn with_timeout(timeout: Duration) -> Self {
        Self { timeout, ..Default::default() }
    }
}

thread_local! {
    static THREAD_CANCEL: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

/// Run `f` with `flag` as the cancel flag of every [`run`] on this thread
/// (the app sets it around a background cook, so stopping the cook also
/// stops a running solver).
pub fn with_cancel<R>(flag: Arc<AtomicBool>, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Arc<AtomicBool>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let old = self.0.take();
            THREAD_CANCEL.with(|c| *c.borrow_mut() = old);
        }
    }
    let old = THREAD_CANCEL.with(|c| c.borrow_mut().replace(flag));
    let _restore = Restore(old);
    f()
}

/// True when the cook running on this thread was asked to stop (see
/// [`with_cancel`]); long loops inside a node may check it.
pub fn cancel_requested() -> bool {
    THREAD_CANCEL.with(|c| c.borrow().as_ref().is_some_and(|f| f.load(Ordering::Relaxed)))
}

fn quiet(cmd: &mut Command) -> &mut Command {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: no console flashes up from a GUI app.
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

/// Run a helper command with output discarded, killing it after `limit`.
fn run_bounded(argv: &[String], limit: Duration) {
    let Some((exe, args)) = argv.split_first() else { return };
    let Ok(mut child) = quiet(Command::new(exe).args(args)).spawn() else { return };
    let start = Instant::now();
    while start.elapsed() < limit {
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Kill `child` and every process it started, then reap it.
pub fn kill_tree(child: &mut Child) {
    let pid = child.id().to_string();
    #[cfg(windows)]
    run_bounded(&["taskkill".into(), "/PID".into(), pid, "/T".into(), "/F".into()], CLEANUP_TIMEOUT);
    // The child leads its own process group (see `run`).
    #[cfg(unix)]
    run_bounded(&["kill".into(), "-s".into(), "KILL".into(), "--".into(), format!("-{pid}")], CLEANUP_TIMEOUT);
    #[cfg(not(any(windows, unix)))]
    let _ = pid;
    let _ = child.kill();
    let _ = child.wait();
}

/// Run `cmd` with stdin closed and both output streams going to `log`
/// (created with its folder when missing), until it exits, the deadline
/// passes or a cancel flag is set. An `Err` means the program could not be
/// started or waited for.
pub fn run(cmd: &mut Command, log: &Path, opts: &RunOptions) -> std::io::Result<Exit> {
    if let Some(dir) = log.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let out = if opts.append { OpenOptions::new().create(true).append(true).open(log)? } else { File::create(log)? };
    let err = out.try_clone()?;
    cmd.stdin(Stdio::null()).stdout(Stdio::from(out)).stderr(Stdio::from(err));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn()?;
    let deadline = if opts.timeout.is_zero() { DEFAULT_TIMEOUT } else { opts.timeout };
    let start = Instant::now();
    let mut nap = Duration::from_millis(2);
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Exit::Code(status.code().unwrap_or(-1)));
        }
        let cancelled = opts.cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) || cancel_requested();
        let stop = if cancelled {
            Some(Exit::Cancelled)
        } else if start.elapsed() >= deadline {
            Some(Exit::TimedOut)
        } else {
            None
        };
        if let Some(why) = stop {
            kill_tree(&mut child);
            run_bounded(&opts.cleanup, CLEANUP_TIMEOUT);
            return Ok(why);
        }
        std::thread::sleep(nap);
        nap = (nap * 2).min(Duration::from_millis(50));
    }
}

/// [`run`] for a program and its arguments in `cwd`.
pub fn run_program(exe: &Path, args: &[String], cwd: &Path, log: &Path, opts: &RunOptions) -> std::io::Result<Exit> {
    let mut cmd = Command::new(exe);
    cmd.args(args).current_dir(cwd);
    run(&mut cmd, log, opts)
}

/// Last `n` lines of a log file (empty when unreadable).
pub fn tail(log: &Path, n: usize) -> String {
    let text = std::fs::read(log).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// A unique container name for `docker run --name`, so a run that times
/// out can be stopped with `docker stop` (the container outlives a killed
/// client).
pub fn container_name(prefix: &str) -> String {
    use std::sync::atomic::AtomicU64;
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    format!("{prefix}-{}-{nanos:x}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `script` run by the platform shell, passed through unquoted.
    fn shell(script: &str) -> Command {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let mut c = Command::new("cmd");
            c.raw_arg(format!("/C {script}"));
            c
        }
        #[cfg(not(windows))]
        {
            let mut c = Command::new("sh");
            c.arg("-c").arg(script);
            c
        }
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("vernon-process-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn output_goes_to_the_log_and_codes_come_back() {
        let dir = tmp("codes");
        let log = dir.join("logs").join("a.log");
        let opts = RunOptions::with_timeout(Duration::from_secs(60));
        assert_eq!(run(&mut shell("echo hello"), &log, &opts).unwrap(), Exit::Code(0));
        assert!(tail(&log, 5).contains("hello"));
        let append = RunOptions { append: true, ..opts.clone() };
        assert_eq!(run(&mut shell("echo second && exit 3"), &log, &append).unwrap(), Exit::Code(3));
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains("hello") && text.contains("second"), "{text}");
        assert!(run(&mut Command::new("no_such_program_vernon_xyz"), &dir.join("b.log"), &opts).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A shell that starts a second shell which writes a marker file after
    /// two seconds: a timeout must end the grandchild too.
    #[test]
    fn timeout_and_cancel_kill_the_tree() {
        let dir = tmp("tree");
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("marker.txt");
        let nested = if cfg!(windows) {
            "cmd /C \"ping -n 3 127.0.0.1 >nul & echo late>marker.txt\"".to_string()
        } else {
            "sh -c 'sleep 2; echo late > marker.txt'; true".to_string()
        };
        let t0 = Instant::now();
        let mut cmd = shell(&nested);
        cmd.current_dir(&dir);
        let st = run(&mut cmd, &dir.join("t.log"), &RunOptions::with_timeout(Duration::from_millis(500))).unwrap();
        assert_eq!(st, Exit::TimedOut);
        assert!(t0.elapsed() < Duration::from_secs(20), "{:?}", t0.elapsed());
        std::thread::sleep(Duration::from_millis(3500));
        assert!(!marker.exists(), "the grandchild outlived the timeout");

        let nested = if cfg!(windows) { "cmd /C \"ping -n 30 127.0.0.1 >nul\"" } else { "sh -c 'sleep 30'; true" };

        let flag = Arc::new(AtomicBool::new(false));
        let setter = flag.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            setter.store(true, Ordering::Relaxed);
        });
        let t0 = Instant::now();
        let st = with_cancel(flag, || run(&mut shell(nested), &dir.join("c.log"), &RunOptions::default())).unwrap();
        assert_eq!(st, Exit::Cancelled);
        assert!(t0.elapsed() < Duration::from_secs(20));
        assert!(!cancel_requested(), "the thread flag is restored");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn container_names_are_unique() {
        assert_ne!(container_name("vernon"), container_name("vernon"));
    }
}
