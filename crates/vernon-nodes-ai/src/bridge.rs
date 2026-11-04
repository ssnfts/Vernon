//! Runs the installed model launchers as separate processes.
//!
//! Each model is driven by a small Python bridge (`scripts/ai/*.py`, embedded in
//! this crate) that prints one JSON object per line: `progress` events, then one
//! `result` or `error`. The Rust side never loads a model itself, so a crash,
//! an out-of-memory error or a cancel can only ever kill the child process.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::Value;

pub const PARTCRAFTER_BRIDGE: &str = include_str!("../../../scripts/ai/partcrafter_bridge.py");
pub const DOMINO_BRIDGE: &str = include_str!("../../../scripts/ai/domino_bridge.py");

/// Environment variable naming the AI-3D install folder when a node leaves its path blank.
pub const ROOT_ENV: &str = "VERNON_AI3D_ROOT";
/// Environment variable overriding the Linux folder that holds each model's Python environment.
pub const LINUX_ROOT_ENV: &str = "VERNON_AI3D_LINUX_ROOT";

static CANCEL: AtomicBool = AtomicBool::new(false);

/// Ask every running model job to stop. The child process tree is killed within a
/// fraction of a second and the node fails with [`BridgeError::Cancelled`]. The app's
/// own cancel button works too: it sets the flag read by
/// `vernon_core::process::cancel_requested`, which the job loop also honours.
pub fn request_cancel() {
    CANCEL.store(true, Ordering::SeqCst);
}

/// Clear a previous cancel request. Every job also clears it when it starts.
pub fn clear_cancel() {
    CANCEL.store(false, Ordering::SeqCst);
}

type ProgressHook = Box<dyn Fn(&Progress) + Send + Sync>;
static HOOK: OnceLock<Mutex<Option<ProgressHook>>> = OnceLock::new();

/// Register a callback that receives every progress event (replaces the previous one).
pub fn set_progress_hook(hook: Option<ProgressHook>) {
    let slot = HOOK.get_or_init(|| Mutex::new(None));
    if let Ok(mut g) = slot.lock() {
        *g = hook;
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Progress {
    pub stage: String,
    pub fraction: f64,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Progress(Progress),
    Result(Value),
    Error { code: String, message: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum BridgeError {
    /// The model install, WSL distribution or GPU is missing or too small.
    Unavailable(String),
    Cancelled,
    Timeout(u64),
    /// The model process reported a failure.
    Model { code: String, message: String },
    /// The process ended without a result or error line.
    Protocol(String),
    Spawn(String),
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BridgeError::Unavailable(m) => write!(f, "model unavailable: {m}"),
            BridgeError::Cancelled => write!(f, "cancelled by user"),
            BridgeError::Timeout(s) => write!(f, "model job exceeded its {s} s time limit and was stopped"),
            BridgeError::Model { code, message } => write!(f, "model error ({code}): {message}"),
            BridgeError::Protocol(m) => write!(f, "model process ended without a result: {m}"),
            BridgeError::Spawn(m) => write!(f, "could not start the model process: {m}"),
        }
    }
}

/// Parse one stdout line of a bridge. Non-JSON lines (library chatter) give `None`.
pub fn parse_line(line: &str) -> Option<Event> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    match v.get("event")?.as_str()? {
        "progress" => Some(Event::Progress(Progress {
            stage: v.get("stage").and_then(Value::as_str).unwrap_or("").to_string(),
            fraction: v.get("fraction").and_then(Value::as_f64).unwrap_or(0.0).clamp(0.0, 1.0),
            message: v.get("message").and_then(Value::as_str).unwrap_or("").to_string(),
        })),
        "result" => Some(Event::Result(v)),
        "error" => Some(Event::Error {
            code: v.get("code").and_then(Value::as_str).unwrap_or("unknown").to_string(),
            message: v.get("message").and_then(Value::as_str).unwrap_or("").to_string(),
        }),
        _ => None,
    }
}

/// `E:\AI-3D\x` becomes `/mnt/e/AI-3D/x`; other paths pass through with `/` separators.
pub fn to_wsl_path(p: &str) -> String {
    let b = p.as_bytes();
    if b.len() > 2 && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/') && b[0].is_ascii_alphabetic() {
        format!("/mnt/{}/{}", (b[0] as char).to_ascii_lowercase(), p[3..].replace('\\', "/"))
    } else {
        p.replace('\\', "/")
    }
}

/// Where the models live and how to reach their Linux environment.
#[derive(Debug, Clone, PartialEq)]
pub struct Install {
    pub root: PathBuf,
    /// WSL distribution name; empty runs the Python directly (Linux hosts).
    pub distro: String,
}

impl Install {
    /// `root` blank falls back to `VERNON_AI3D_ROOT`. Checks the folder exists.
    pub fn resolve(root: &str, distro: &str) -> Result<Install, BridgeError> {
        let root = if root.trim().is_empty() {
            std::env::var(ROOT_ENV).unwrap_or_default()
        } else {
            root.trim().to_string()
        };
        if root.is_empty() {
            return Err(BridgeError::Unavailable(format!(
                "no AI model folder set: fill the 'AI-3D folder' parameter or set {ROOT_ENV}"
            )));
        }
        let path = PathBuf::from(&root);
        if !path.is_dir() {
            return Err(BridgeError::Unavailable(format!("AI model folder not found: {root}")));
        }
        Ok(Install { root: path, distro: distro.trim().to_string() })
    }

    pub fn require_model_dir(&self, model: &str) -> Result<(), BridgeError> {
        let d = self.root.join(model);
        if d.is_dir() {
            Ok(())
        } else {
            Err(BridgeError::Unavailable(format!("{} not installed under {}", model, self.root.display())))
        }
    }

    /// The Python interpreter of the model's environment, inside the Linux side.
    pub fn python(&self, model: &str) -> String {
        let base = std::env::var(LINUX_ROOT_ENV).unwrap_or_else(|_| "/opt/ai3d".to_string());
        format!("{}/{}/bin/python", base.trim_end_matches('/'), model)
    }

    fn command(&self, model: &str, script: &Path, request: &Path) -> Command {
        let py = self.python(model);
        let (script_s, req_s) = (script.to_string_lossy().to_string(), request.to_string_lossy().to_string());
        let mut cmd;
        if self.distro.is_empty() {
            cmd = Command::new(py);
            cmd.arg(script_s).arg("--request").arg(req_s);
        } else {
            cmd = Command::new("wsl.exe");
            cmd.args(["-d", &self.distro, "-u", "root", "--exec", &py]);
            cmd.arg(to_wsl_path(&script_s)).arg("--request").arg(to_wsl_path(&req_s));
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        cmd
    }

    fn kill_remote(&self, script_name: &str) {
        // Killing wsl.exe normally ends the child; this removes a straggler.
        if !self.distro.is_empty() {
            let mut c = Command::new("wsl.exe");
            c.args(["-d", &self.distro, "-u", "root", "--exec", "pkill", "-f", script_name]);
            let _ = c.stdout(Stdio::null()).stderr(Stdio::null()).status();
        }
    }
}

/// Free GPU memory and RAM visible to the model process, in MiB.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Resources {
    pub vram_free_mib: u64,
    pub vram_total_mib: u64,
    pub ram_available_mib: u64,
}

/// First line of `nvidia-smi --query-gpu=memory.free,memory.total --format=csv,noheader,nounits`.
pub fn parse_nvidia_smi(out: &str) -> Option<(u64, u64)> {
    let line = out.lines().find(|l| !l.trim().is_empty())?;
    let mut it = line.split(',').map(|s| s.trim().parse::<u64>());
    let free = it.next()?.ok()?;
    let total = it.next()?.ok()?;
    Some((free, total))
}

/// `available` column (MiB) of the `Mem:` row of `free -m`.
pub fn parse_free_m(out: &str) -> Option<u64> {
    let line = out.lines().find(|l| l.trim_start().starts_with("Mem:"))?;
    line.split_whitespace().nth(6)?.parse().ok()
}

pub fn query_resources(install: &Install) -> Result<Resources, BridgeError> {
    let smi = Command::new("nvidia-smi")
        .args(["--query-gpu=memory.free,memory.total", "--format=csv,noheader,nounits"])
        .output()
        .map_err(|e| BridgeError::Unavailable(format!("nvidia-smi not found, cannot check GPU memory: {e}")))?;
    let (vram_free_mib, vram_total_mib) = parse_nvidia_smi(&String::from_utf8_lossy(&smi.stdout))
        .ok_or_else(|| BridgeError::Unavailable("could not read GPU memory from nvidia-smi".into()))?;
    let mut free = if install.distro.is_empty() {
        Command::new("free")
    } else {
        let mut c = Command::new("wsl.exe");
        c.args(["-d", &install.distro, "--exec", "free"]);
        c
    };
    free.arg("-m");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        free.creation_flags(0x0800_0000);
    }
    let out = free
        .output()
        .map_err(|e| BridgeError::Unavailable(format!("cannot read memory of the model environment: {e}")))?;
    let text = String::from_utf8_lossy(&out.stdout).replace('\0', "");
    let ram_available_mib = parse_free_m(&text)
        .ok_or_else(|| BridgeError::Unavailable("could not read available RAM from `free -m`".into()))?;
    Ok(Resources { vram_free_mib, vram_total_mib, ram_available_mib })
}

/// Peak memory measured on this project's RTX 4090 Laptop (16 GB) runs; see docs/specs/ai-models.md.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Need {
    pub vram_mib: u64,
    pub ram_mib: u64,
}

/// Required free memory = measured peak x 1.25 + 512 MiB. `None` when the
/// workload has no measurement yet (the caller then warns instead of guessing).
pub fn with_margin(peak_vram_gb: f64, peak_rss_gb: f64) -> Need {
    let m = |gb: f64| (gb * 1024.0 * 1.25 + 512.0).ceil() as u64;
    Need { vram_mib: m(peak_vram_gb), ram_mib: m(peak_rss_gb) }
}

pub fn check_resources(need: Need, have: Resources, what: &str) -> Result<(), BridgeError> {
    if have.vram_free_mib < need.vram_mib {
        return Err(BridgeError::Unavailable(format!(
            "{what} needs about {} MiB of free GPU memory but only {} MiB of {} MiB is free; close other GPU programs, lower the quality or enable CPU offload",
            need.vram_mib, have.vram_free_mib, have.vram_total_mib
        )));
    }
    if have.ram_available_mib < need.ram_mib {
        return Err(BridgeError::Unavailable(format!(
            "{what} needs about {} MiB of free RAM in the model environment but only {} MiB is available",
            need.ram_mib, have.ram_available_mib
        )));
    }
    Ok(())
}

fn spawn_readers(child: &mut Child, tx: mpsc::Sender<String>, stderr_log: PathBuf) {
    if let Some(out) = child.stdout.take() {
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
    }
    if let Some(mut err) = child.stderr.take() {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = err.read_to_end(&mut buf);
            let _ = std::fs::write(stderr_log, buf);
        });
    }
}

/// Last `n` bytes of a log, for error messages.
pub fn tail(path: &Path, n: usize) -> String {
    let s = std::fs::read(path).map(|b| String::from_utf8_lossy(&b).replace('\0', "")).unwrap_or_default();
    let s = s.trim_end();
    if s.len() <= n {
        s.to_string()
    } else {
        let mut i = s.len() - n;
        while !s.is_char_boundary(i) {
            i += 1;
        }
        s[i..].to_string()
    }
}

/// Run one bridge job and return its `result` object. `model` is the model folder
/// name (`PartCrafter` or `DoMINO`); `work` receives the script, request, logs and progress.
pub fn run_job(
    install: &Install,
    model: &str,
    script_name: &str,
    script_text: &str,
    request: &Value,
    work: &Path,
    timeout: Duration,
) -> Result<Value, BridgeError> {
    install.require_model_dir(model)?;
    std::fs::create_dir_all(work).map_err(|e| BridgeError::Spawn(e.to_string()))?;
    let script = work.join(script_name);
    let req_path = work.join("request.json");
    std::fs::write(&script, script_text).map_err(|e| BridgeError::Spawn(e.to_string()))?;
    std::fs::write(&req_path, serde_json::to_vec_pretty(request).unwrap_or_default())
        .map_err(|e| BridgeError::Spawn(e.to_string()))?;
    let cmd = install.command(model, &script, &req_path);
    run_process(cmd, work, timeout, &|| install.kill_remote(script_name))
}

/// Spawn `cmd`, follow its JSON lines and return the `result` object.
/// `on_kill` runs after a cancel or timeout to remove any straggler process.
pub fn run_process(mut cmd: Command, work: &Path, timeout: Duration, on_kill: &dyn Fn()) -> Result<Value, BridgeError> {
    let stderr_log = work.join("stderr.log");
    clear_cancel();
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| BridgeError::Spawn(e.to_string()))?;
    let (tx, rx) = mpsc::channel();
    spawn_readers(&mut child, tx, stderr_log.clone());
    let deadline = Instant::now() + timeout;
    let mut progress_log = String::new();
    let mut outcome: Option<Event> = None;
    loop {
        if CANCEL.load(Ordering::SeqCst) || vernon_core::process::cancel_requested() {
            vernon_core::process::kill_tree(&mut child);
            on_kill();
            let _ = child.wait();
            return Err(BridgeError::Cancelled);
        }
        if Instant::now() >= deadline {
            vernon_core::process::kill_tree(&mut child);
            on_kill();
            let _ = child.wait();
            return Err(BridgeError::Timeout(timeout.as_secs()));
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => match parse_line(&line) {
                Some(Event::Progress(p)) => {
                    progress_log.push_str(&line);
                    progress_log.push('\n');
                    if let Some(Ok(g)) = HOOK.get().map(|m| m.lock()) {
                        if let Some(h) = g.as_ref() {
                            h(&p);
                        }
                    }
                }
                Some(e) => outcome = Some(e),
                None => {}
            },
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let status = child.wait().map_err(|e| BridgeError::Spawn(e.to_string()))?;
    let _ = std::fs::write(work.join("progress.jsonl"), progress_log);
    match outcome {
        Some(Event::Result(v)) => Ok(v),
        Some(Event::Error { code, message }) => Err(BridgeError::Model { code, message }),
        _ => Err(BridgeError::Protocol(format!(
            "exit status {status}; {}",
            tail(&stderr_log, 600)
        ))),
    }
}
