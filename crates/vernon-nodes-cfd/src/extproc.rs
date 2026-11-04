//! Running external programs (OpenFOAM) as separate processes. Output goes to
//! log files, never pipes, so a chatty solver cannot deadlock the node.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use vernon_core::process::{self, Exit, RunOptions};

/// Where and how the tools run.
#[derive(Debug, Clone, Default)]
pub struct RunOpts {
    /// `native`, `wsl` or `docker`.
    pub runner: String,
    /// Container image for the `docker` runner.
    pub image: String,
    /// Directory holding the OpenFOAM executables; empty searches PATH.
    pub bin_dir: String,
    /// Overall chain budget; 0 = the shared runner's default (one day).
    pub timeout_min: u64,
}

/// Explicit Linux execution context, kept separate so existing native/container
/// callers keep their current `RunOpts` construction.
#[derive(Debug, Clone, Default)]
pub struct WslOpts {
    pub distribution: String,
    pub user: String,
    pub environment_file: String,
}

/// A WSL command and the attempt identity needed to stop its Linux process group.
#[derive(Debug)]
pub struct WslLaunch {
    pub argv: Vec<String>,
    control_root: PathBuf,
    record_path: PathBuf,
    cancel_path: PathBuf,
    token: String,
    cleanup_argv: Vec<String>,
}

// These original fixed templates receive all paths and tool arguments as
// positional parameters. No user text is evaluated as shell syntax.
const WSL_START: &str = r#"
_vernon_record=$1
_vernon_cancel=$2
_vernon_token=$3
_vernon_environment=$4
shift 4
readonly -a _vernon_tool_argv=("$@")
[[ ! -e "$_vernon_cancel" ]] || exit 125
_vernon_case=$(pwd -P) || exit 126
IFS= read -r _vernon_stat < "/proc/$$/stat" || exit 126
_vernon_stat_tail=${_vernon_stat##*) }
IFS=' ' read -r -a _vernon_fields <<< "$_vernon_stat_tail"
[[ ${_vernon_fields[2]:-} == "$$" && ${_vernon_fields[3]:-} == "$$" ]] || exit 126
printf '%s %s %s %s\n' "$_vernon_token" "$$" "${_vernon_fields[19]}" "$$" > "$_vernon_record.tmp" || exit 126
/bin/mv -f -- "$_vernon_record.tmp" "$_vernon_record" || exit 126
[[ ! -e "$_vernon_cancel" ]] || exit 125
export VERNON_CFD_ATTEMPT="$_vernon_token"
set +e
set +u
set +o pipefail
_vernon_load_environment() {
    local _vernon_env_path="$1"
    shift
    . "$_vernon_env_path"
}
_vernon_load_environment "$_vernon_environment" || exit 126
builtin cd -- "$_vernon_case" || exit 126
[[ ! -e "$_vernon_cancel" ]] || exit 125
trap '' HUP TERM INT
(exec -- "${_vernon_tool_argv[@]}") &
_vernon_child=$!
if wait "$_vernon_child"; then
    _vernon_child_code=0
else
    _vernon_child_code=$?
fi
# Stay the verifiable session/group leader until every non-zombie member has
# ended. Waiting for the direct tool alone misses its background descendants.
while :; do
    _vernon_group_live=0
    _vernon_scanned=0
    for _vernon_stat_file in /proc/[0-9]*/stat; do
        ((_vernon_scanned+=1))
        (( _vernon_scanned <= 32768 )) || exit 132
        [[ "$_vernon_stat_file" != "/proc/$$/stat" ]] || continue
        IFS= read -r _vernon_stat < "$_vernon_stat_file" 2>/dev/null || continue
        _vernon_stat_tail=${_vernon_stat##*) }
        IFS=' ' read -r -a _vernon_fields <<< "$_vernon_stat_tail"
        [[ ${_vernon_fields[2]:-} == "$$" && ${_vernon_fields[3]:-} == "$$" ]] || continue
        [[ ${_vernon_fields[0]:-} != Z && ${_vernon_fields[0]:-} != X ]] || continue
        _vernon_group_live=1
        break
    done
    (( _vernon_group_live != 0 )) || break
    /bin/sleep 0.05
done
printf 'VERNON_WSL_GROUP_EMPTY %s\n' "$_vernon_token"
exit "$_vernon_child_code"
"#;

const WSL_CLEANUP: &str = r#"
_vernon_record=$1
_vernon_cancel=$2
_vernon_expected=$3
: > "$_vernon_cancel" || exit 129
_vernon_stopped() {
    printf 'VERNON_WSL_GROUP_STOPPED %s\n' "$_vernon_expected"
}
[[ -e "$_vernon_record" ]] || { _vernon_stopped; exit 0; }
IFS=' ' read -r _vernon_token _vernon_pid _vernon_start _vernon_group _vernon_extra < "$_vernon_record" || exit 130
[[ "$_vernon_token" == "$_vernon_expected" && -z "$_vernon_extra" ]] || exit 130
[[ "$_vernon_pid" =~ ^[1-9][0-9]*$ && "$_vernon_start" =~ ^[0-9]+$ && "$_vernon_group" == "$_vernon_pid" ]] || exit 130
_vernon_valid_leader=0
if [[ -r "/proc/$_vernon_pid/stat" ]]; then
    IFS= read -r _vernon_stat < "/proc/$_vernon_pid/stat" || exit 131
    _vernon_stat_tail=${_vernon_stat##*) }
    IFS=' ' read -r -a _vernon_fields <<< "$_vernon_stat_tail"
    [[ ${_vernon_fields[19]:-} == "$_vernon_start" && ${_vernon_fields[2]:-} == "$_vernon_group" && ${_vernon_fields[3]:-} == "$_vernon_group" ]] || exit 131
    _vernon_valid_leader=1
fi
_vernon_scan_group() {
    _vernon_live=0
    _vernon_owned=$_vernon_valid_leader
    _vernon_scanned=0
    for _vernon_stat_file in /proc/[0-9]*/stat; do
        ((_vernon_scanned+=1))
        (( _vernon_scanned <= 32768 )) || return 1
        IFS= read -r _vernon_stat < "$_vernon_stat_file" 2>/dev/null || continue
        _vernon_stat_tail=${_vernon_stat##*) }
        IFS=' ' read -r -a _vernon_fields <<< "$_vernon_stat_tail"
        [[ ${_vernon_fields[2]:-} == "$_vernon_group" && ${_vernon_fields[3]:-} == "$_vernon_group" ]] || continue
        [[ ${_vernon_fields[0]:-} != Z && ${_vernon_fields[0]:-} != X ]] || continue
        _vernon_live=1
        if (( _vernon_owned == 0 )); then
            _vernon_environ="${_vernon_stat_file%/stat}/environ"
            if [[ -r "$_vernon_environ" ]]; then
                while IFS= read -r -d '' _vernon_entry; do
                    if [[ "$_vernon_entry" == "VERNON_CFD_ATTEMPT=$_vernon_expected" ]]; then
                        _vernon_owned=1
                        break
                    fi
                done < "$_vernon_environ" 2>/dev/null
            fi
        fi
    done
}
_vernon_scan_group || exit 132
(( _vernon_live != 0 )) || { _vernon_stopped; exit 0; }
(( _vernon_owned != 0 )) || exit 133
if ! kill -KILL -- "-$_vernon_group" 2>/dev/null; then
    _vernon_scan_group || exit 132
    (( _vernon_live == 0 )) || exit 134
fi
for _vernon_poll in {1..20}; do
    _vernon_scan_group || exit 132
    if (( _vernon_live == 0 )); then
        _vernon_stopped
        exit 0
    fi
    /bin/sleep 0.05
done
exit 135
"#;

fn linux_working_directory(cwd: &Path) -> Result<String, String> {
    let raw = cwd
        .to_str()
        .ok_or("WSL working directory must be valid Unicode")?;
    let text = raw.strip_prefix(r"\\?\").unwrap_or(raw);
    if text.contains('\0') || text.starts_with(r"\\") || text.starts_with("//") {
        return Err("WSL working directory must be an absolute local drive path, not UNC".into());
    }
    let bytes = text.as_bytes();
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\')
    {
        let mut parts = Vec::new();
        for part in text[3..].split(['/', '\\']) {
            match part {
                "" | "." => {}
                ".." => {
                    if parts.pop().is_none() {
                        return Err("WSL working directory escapes its drive root".into());
                    }
                }
                other if other.contains(':') => {
                    return Err("WSL working directory contains an invalid drive component".into());
                }
                other => parts.push(other),
            }
        }
        let suffix = parts.join("/");
        return Ok(if suffix.is_empty() {
            format!("/mnt/{}", (bytes[0] as char).to_ascii_lowercase())
        } else {
            format!("/mnt/{}/{suffix}", (bytes[0] as char).to_ascii_lowercase())
        });
    }
    if text.starts_with('/') {
        return Ok(text.to_string());
    }
    Err("WSL working directory must be absolute".into())
}

fn wsl_prefix(wsl: &WslOpts) -> Vec<String> {
    vec![
        "wsl.exe".into(),
        "--distribution".into(),
        wsl.distribution.clone(),
        "--user".into(),
        wsl.user.clone(),
    ]
}

/// Build a structured WSL invocation without probing executable availability or
/// writing files. Environment paths and tool arguments remain individual argv.
/// `control_root` is supplied by the caller under its stable context work root,
/// outside every case generation (normally `ctx.work_dir/.wsl-control`).
pub fn wrap_with_wsl(
    opts: &RunOpts,
    wsl: &WslOpts,
    cwd: &Path,
    control_root: &Path,
    program: &str,
    args: &[String],
) -> Result<WslLaunch, String> {
    if opts.runner != "wsl" {
        return Err("structured WSL invocation requires the wsl runner".into());
    }
    for (field, value) in [("distribution", &wsl.distribution), ("user", &wsl.user)] {
        if value.trim().is_empty() || value.starts_with('-') || value.contains(['\0', '\r', '\n']) {
            return Err(format!(
                "WSL {field} must be an explicit literal identifier"
            ));
        }
    }
    if !wsl.environment_file.starts_with('/') || wsl.environment_file.contains('\0') {
        return Err("WSL environment file must be an absolute Linux path".into());
    }
    if program.is_empty()
        || program.contains('\0')
        || args.iter().any(|arg| arg.contains('\0'))
        || opts.bin_dir.contains('\0')
    {
        return Err(
            "WSL program and arguments must be nonempty/literal strings without zero bytes".into(),
        );
    }
    if !opts.bin_dir.trim().is_empty() && !opts.bin_dir.starts_with('/') {
        return Err("WSL executable directory must be an absolute Linux path".into());
    }
    let linux_cwd = linux_working_directory(cwd)?;
    let linux_control = linux_working_directory(control_root)?;
    let windows_path = cwd
        .to_str()
        .is_some_and(|s| s.as_bytes().get(1) == Some(&b':') || s.starts_with(r"\\?\"));
    let case_key = if windows_path {
        linux_cwd.to_lowercase()
    } else {
        linux_cwd.clone()
    };
    let control_key = if windows_path {
        linux_control.to_lowercase()
    } else {
        linux_control
    };
    let owner_prefix = case_key
        .rsplit_once('/')
        .map(|(parent, _)| format!("{parent}/"))
        .ok_or("WSL case has no owner directory")?;
    if !control_key.starts_with(&owner_prefix)
        || control_key == case_key
        || control_key.starts_with(&format!("{case_key}/"))
    {
        return Err(
            "WSL control root must be an explicit owner directory outside movable case generations"
                .into(),
        );
    }
    let token = process::container_name("vernon-cfd-wsl");
    let record_path = control_root.join(format!(".{token}.pid"));
    let cancel_path = control_root.join(format!(".{token}.cancel"));
    let linux_record = linux_working_directory(&record_path)?;
    let linux_cancel = linux_working_directory(&cancel_path)?;
    let inner = if opts.bin_dir.trim().is_empty() || program.starts_with('/') {
        program.to_string()
    } else {
        format!("{}/{}", opts.bin_dir.trim_end_matches('/'), program)
    };
    let mut argv = wsl_prefix(wsl);
    argv.extend([
        "--cd".into(),
        linux_cwd,
        "--exec".into(),
        "/usr/bin/env".into(),
        "-u".into(),
        "BASH_ENV".into(),
        "-u".into(),
        "ENV".into(),
        "--".into(),
        "/usr/bin/setsid".into(),
        "--fork".into(),
        "--wait".into(),
        "/bin/bash".into(),
        "--noprofile".into(),
        "--norc".into(),
        "-c".into(),
        WSL_START.into(),
        "vernon-cfd-wsl".into(),
        linux_record.clone(),
        linux_cancel.clone(),
        token.clone(),
        wsl.environment_file.clone(),
        inner,
    ]);
    argv.extend_from_slice(args);
    let mut cleanup_argv = wsl_prefix(wsl);
    cleanup_argv.extend([
        "--exec".into(),
        "/usr/bin/env".into(),
        "-u".into(),
        "BASH_ENV".into(),
        "-u".into(),
        "ENV".into(),
        "--".into(),
        "/bin/bash".into(),
        "--noprofile".into(),
        "--norc".into(),
        "-c".into(),
        WSL_CLEANUP.into(),
        "vernon-cfd-cleanup".into(),
        linux_record,
        linux_cancel,
        token.clone(),
    ]);
    Ok(WslLaunch {
        argv,
        control_root: control_root.into(),
        record_path,
        cancel_path,
        token,
        cleanup_argv,
    })
}

/// Run a chain with an explicit WSL context and a stable caller-owned control
/// directory outside the movable case. Ordinary native/container callers retain
/// the existing `run_chain` API and do not create WSL control files.
pub fn run_chain_with_wsl(
    opts: &RunOpts,
    wsl: &WslOpts,
    case: &Path,
    control_root: &Path,
    steps: &[(String, Vec<String>)],
) -> ChainResult {
    match chain_timeout(opts) {
        Ok(budget) => {
            run_chain_with_budget(opts, Some(wsl), case, Some(control_root), steps, budget)
        }
        Err(error) => ChainResult {
            failed: Some(error),
            ..Default::default()
        },
    }
}

fn chain_timeout(opts: &RunOpts) -> Result<Duration, String> {
    if opts.timeout_min == 0 {
        return Ok(process::DEFAULT_TIMEOUT);
    }
    opts.timeout_min
        .checked_mul(60)
        .map(Duration::from_secs)
        .ok_or_else(|| "timeout minutes overflow the supported duration".into())
}

fn diagnostic_tail(log: &Path) -> String {
    const INPUT_LIMIT: u64 = 4096;
    let Ok(mut file) = File::open(log) else {
        return String::new();
    };
    let Ok(metadata) = file.metadata() else {
        return String::new();
    };
    if file
        .seek(SeekFrom::Start(metadata.len().saturating_sub(INPUT_LIMIT)))
        .is_err()
    {
        return String::new();
    }
    let mut bytes = Vec::new();
    if file.take(INPUT_LIMIT).read_to_end(&mut bytes).is_err() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = text.lines().rev().take(32).collect::<Vec<_>>();
    lines.reverse();
    lines.join("\n")
}

fn exe_names(name: &str) -> Vec<String> {
    if cfg!(windows) {
        vec![
            format!("{name}.exe"),
            format!("{name}.cmd"),
            format!("{name}.bat"),
            name.to_string(),
        ]
    } else {
        vec![name.to_string()]
    }
}

/// Find an executable in `bin_dir` or on PATH.
pub fn find_exe(bin_dir: &str, name: &str) -> Option<PathBuf> {
    let direct = Path::new(name);
    if direct.is_absolute() && direct.is_file() {
        return Some(direct.to_path_buf());
    }
    let mut dirs: Vec<PathBuf> = Vec::new();
    if !bin_dir.trim().is_empty() {
        dirs.push(PathBuf::from(bin_dir.trim()));
    } else if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    for d in dirs {
        for n in exe_names(name) {
            let p = d.join(n);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// Full command line for one step. Errors explain what is missing.
pub fn wrap(
    opts: &RunOpts,
    cwd: &Path,
    program: &str,
    args: &[String],
) -> Result<Vec<String>, String> {
    let inner = if opts.bin_dir.trim().is_empty() || matches!(opts.runner.as_str(), "" | "native") {
        program.to_string()
    } else {
        format!("{}/{}", opts.bin_dir.trim().trim_end_matches('/'), program)
    };
    match opts.runner.as_str() {
        "wsl" => Err("the wsl runner requires explicit distribution/user/environment options; use run_chain_with_wsl".into()),
        "docker" => {
            if opts.image.trim().is_empty() {
                return Err("the docker runner needs an image name".into());
            }
            find_exe("", "docker").ok_or("docker not found")?;
            let mut v = vec![
                "docker".into(),
                "run".into(),
                "--rm".into(),
                // Named, so a run that times out can be stopped: killing
                // the client would leave the container running.
                "--name".into(),
                vernon_core::process::container_name("vernon-foam"),
                "-v".into(),
                format!("{}:/case", cwd.display()),
                "-w".into(),
                "/case".into(),
                opts.image.trim().to_string(),
                inner,
            ];
            v.extend_from_slice(args);
            Ok(v)
        }
        "" | "native" => {
            let exe = find_exe(&opts.bin_dir, program).ok_or_else(|| format!("{program} not found"))?;
            let mut v = vec![exe.display().to_string()];
            v.extend_from_slice(args);
            Ok(v)
        }
        other => Err(format!("unknown runner: {other}")),
    }
}

/// `docker stop NAME` for a `docker run --name NAME ...` command line.
fn docker_cleanup(argv: &[String]) -> Vec<String> {
    match argv.iter().position(|a| a == "--name") {
        Some(i) if argv.first().is_some_and(|a| a == "docker") && i + 1 < argv.len() => {
            vec![
                "docker".into(),
                "stop".into(),
                "-t".into(),
                "2".into(),
                argv[i + 1].clone(),
            ]
        }
        _ => Vec::new(),
    }
}

/// Run `argv` in `cwd` through the shared runner, output written to `log`
/// (started afresh). Returns the exit code. `None` uses the runner's
/// default deadline; a timeout or a cancelled cook kills the whole process
/// tree (and stops a docker container).
pub fn run_logged(
    argv: &[String],
    cwd: &Path,
    log: &Path,
    timeout: Option<Duration>,
) -> Result<i32, String> {
    run_logged_inner(argv, cwd, log, timeout, None)
}

fn failure_with_log(message: impl AsRef<str>, log: &Path) -> String {
    let tail = diagnostic_tail(log);
    let message = format!("{} (see {})", message.as_ref(), log.display());
    if tail.is_empty() {
        message
    } else {
        format!("{message}\n{tail}")
    }
}

/// The caller supplies a stable directory under its own work root. It must not
/// be a case generation or a link into one. Never traverse links/reparse points.
fn prepare_control_root(root: &Path) -> Result<(), String> {
    if !root.is_absolute()
        || root
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(
            "WSL control root must be an absolute owner path without parent components".into(),
        );
    }
    fn ordinary(path: &Path) -> Result<std::fs::Metadata, String> {
        let meta = fs::symlink_metadata(path)
            .map_err(|e| format!("cannot inspect WSL control path {}: {e}", path.display()))?;
        let link = meta.file_type().is_symlink();
        #[cfg(windows)]
        let link = {
            use std::os::windows::fs::MetadataExt;
            link || meta.file_attributes() & 0x400 != 0
        };
        if link || (!meta.is_dir() && !meta.is_file()) {
            return Err(format!(
                "WSL control path refuses links, reparse points and special files: {}",
                path.display()
            ));
        }
        Ok(meta)
    }
    for parent in root.ancestors().skip(1) {
        if !ordinary(parent)?.is_dir() {
            return Err("WSL control root requires ordinary existing owner ancestors".into());
        }
    }
    match fs::create_dir(root) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(format!(
                "cannot create WSL control root {}: {error}",
                root.display()
            ))
        }
    }
    if !ordinary(root)?.is_dir() {
        return Err("WSL control root is not a directory".into());
    }
    let entries = fs::read_dir(root).map_err(|e| format!("cannot list WSL control root: {e}"))?;
    for (index, entry) in entries.enumerate() {
        if process::cancel_requested() {
            return Err("WSL control preparation was cancelled".into());
        }
        if index >= 32_768 {
            return Err("WSL control root exceeds its retained entry budget".into());
        }
        let entry = entry.map_err(|e| format!("cannot read WSL control entry: {e}"))?;
        let meta = ordinary(&entry.path())?;
        if !meta.is_file() || meta.len() > 4096 {
            return Err("WSL control root contains an unexpected or oversized entry".into());
        }
    }
    Ok(())
}

fn cleanup_wsl(launch: &WslLaunch, launcher: &Path, cwd: &Path, log: &Path) -> Result<(), String> {
    // Retain this tombstone even after successful cleanup: a WSL launch delayed
    // before publication must still refuse to execute after this cook returns.
    let tombstone_error = fs::write(&launch.cancel_path, b"cancelled\n").err();
    let cleanup_log = log.with_extension("wsl-cleanup.log");
    let mut command = Command::new(launcher);
    command.args(&launch.cleanup_argv[1..]).current_dir(cwd);
    let opts = RunOptions::with_timeout(Duration::from_secs(10));
    let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let result = process::with_cancel(cancelled, || {
        process::run(&mut command, &cleanup_log, &opts)
    });
    let expected = format!("VERNON_WSL_GROUP_STOPPED {}", launch.token);
    match result {
        Ok(Exit::Code(0))
            if diagnostic_tail(&cleanup_log)
                .lines()
                .any(|line| line == expected) =>
        {
            Ok(())
        }
        other => {
            let extra = tombstone_error
                .map(|e| format!("; host cancellation record failed: {e}"))
                .unwrap_or_default();
            Err(failure_with_log(
                format!("Linux process-group cleanup was not verified: {other:?}{extra}"),
                &cleanup_log,
            ))
        }
    }
}

fn publish_wsl_intent(launch: &WslLaunch) -> Result<(), String> {
    if process::cancel_requested() {
        return Err("WSL intent publication was cancelled before launch".into());
    }
    prepare_control_root(&launch.control_root)?;
    let path = launch.control_root.join(format!(".{}.intent", launch.token));
    // Exclusive creation preserves any prior file. A partial write fails before
    // process::run: no client can have launched without a complete synced intent.
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&path)
        .map_err(|e| format!("cannot create WSL prelaunch intent {}: {e}", path.display()))?;
    file.write_all(format!("{}\n", launch.token).as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|e| format!("cannot persist WSL prelaunch intent: {e}"))?;
    if process::cancel_requested() {
        return Err("WSL intent publication was cancelled before client start".into());
    }
    Ok(())
}

fn discard_completed_control(launch: &WslLaunch) {
    let _ = fs::remove_file(&launch.record_path);
    let _ = fs::remove_file(&launch.cancel_path);
    let temporary = PathBuf::from(format!("{}.tmp", launch.record_path.display()));
    let _ = fs::remove_file(temporary);
    let _ = fs::remove_file(launch.control_root.join(format!(".{}.intent", launch.token)));
}

fn run_logged_inner(
    argv: &[String],
    cwd: &Path,
    log: &Path,
    timeout: Option<Duration>,
    wsl: Option<&WslLaunch>,
) -> Result<i32, String> {
    let (exe, args) = argv.split_first().ok_or("empty command line")?;
    if process::cancel_requested() {
        return Err("command was stopped (cook cancelled before launch)".into());
    }
    let actual = if wsl.is_some() {
        find_exe("", "wsl").ok_or("wsl.exe not found")?
    } else {
        PathBuf::from(exe)
    };
    if let Some(launch) = wsl {
        publish_wsl_intent(launch)?;
    }
    let opts = RunOptions {
        timeout: timeout.unwrap_or(Duration::ZERO),
        cleanup: if wsl.is_some() {
            Vec::new()
        } else {
            docker_cleanup(argv)
        },
        ..Default::default()
    };
    let mut cmd = Command::new(&actual);
    cmd.args(args).current_dir(cwd);
    let result = process::run(&mut cmd, log, &opts);
    match result {
        Ok(Exit::Code(code)) => {
            if let Some(launch) = wsl {
                if code == 0 {
                    let expected = format!("VERNON_WSL_GROUP_EMPTY {}", launch.token);
                    if !diagnostic_tail(log).lines().any(|line| line == expected) {
                        let cleanup = cleanup_wsl(launch, &actual, cwd, log);
                        return Err(failure_with_log(
                            format!("WSL supervisor exited without verified group completion; cleanup: {cleanup:?}"),
                            log,
                        ));
                    }
                    discard_completed_control(launch);
                } else if let Err(error) = cleanup_wsl(launch, &actual, cwd, log) {
                    return Err(failure_with_log(
                        format!("{} exited with code {code}; {error}", actual.display()),
                        log,
                    ));
                }
            }
            Ok(code)
        }
        stopped => {
            let message = match stopped {
                Ok(Exit::TimedOut) => format!("{} timed out", actual.display()),
                Ok(Exit::Cancelled) => format!("{} was stopped (cook cancelled)", actual.display()),
                Err(error) => format!("cannot start or wait for {}: {error}", actual.display()),
                Ok(Exit::Code(_)) => unreachable!(),
            };
            let message = if let Some(launch) = wsl {
                match cleanup_wsl(launch, &actual, cwd, log) {
                    Ok(()) => format!("{message}; Linux process-group cleanup verified"),
                    Err(error) => format!("{message}; {error}"),
                }
            } else {
                message
            };
            Err(failure_with_log(message, log))
        }
    }
}

/// Outcome of a step chain.
#[derive(Debug, Default)]
pub struct ChainResult {
    pub completed: Vec<String>,
    pub failed: Option<String>,
}

/// Run each `(name, program, args)` step in order, logging to `case/logs/<name>.log`
/// (and `case/log.<name>`), stopping at the first failure.
pub fn run_chain(opts: &RunOpts, case: &Path, steps: &[(String, Vec<String>)]) -> ChainResult {
    match chain_timeout(opts) {
        Ok(budget) => run_chain_with_budget(opts, None, case, None, steps, budget),
        Err(error) => ChainResult {
            failed: Some(error),
            ..Default::default()
        },
    }
}

fn valid_step_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.'))
}

fn remaining_budget(start: Instant, budget: Duration) -> Result<Duration, String> {
    if process::cancel_requested() {
        return Err("chain was stopped (cook cancelled)".into());
    }
    budget
        .checked_sub(start.elapsed())
        .filter(|left| !left.is_zero())
        .ok_or_else(|| "overall chain deadline expired".into())
}

fn mirror_log(
    source: &Path,
    target: &Path,
    start: Instant,
    budget: Duration,
) -> Result<(), String> {
    let mut input =
        File::open(source).map_err(|error| format!("cannot read completed log: {error}"))?;
    let mut output =
        File::create(target).map_err(|error| format!("cannot mirror completed log: {error}"))?;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        remaining_budget(start, budget)?;
        let read = input
            .read(&mut buffer)
            .map_err(|error| format!("cannot read completed log: {error}"))?;
        if read == 0 {
            return Ok(());
        }
        output
            .write_all(&buffer[..read])
            .map_err(|error| format!("cannot mirror completed log: {error}"))?;
    }
}

fn run_chain_with_budget(
    opts: &RunOpts,
    wsl: Option<&WslOpts>,
    case: &Path,
    control_root: Option<&Path>,
    steps: &[(String, Vec<String>)],
    budget: Duration,
) -> ChainResult {
    let mut res = ChainResult::default();
    let start = Instant::now();
    if let Err(error) = remaining_budget(start, budget) {
        res.failed = Some(error);
        return res;
    }
    for (name, cmd) in steps {
        if let Err(error) = remaining_budget(start, budget) {
            res.failed = Some(format!("{name}: {error}"));
            return res;
        }
        if !valid_step_name(name) {
            res.failed = Some("invalid step name; log paths must stay within the case".into());
            return res;
        }
        let Some((program, args)) = cmd.split_first().filter(|(program, _)| !program.is_empty())
        else {
            res.failed = Some(format!("{name}: empty command"));
            return res;
        };
        let prepared_wsl = if opts.runner == "wsl" {
            let Some(wsl) = wsl else {
                res.failed = Some(format!("{name}: the wsl runner requires explicit distribution/user/environment options"));
                return res;
            };
            let Some(control_root) = control_root else {
                res.failed = Some(format!(
                    "{name}: the wsl runner requires an explicit stable control root"
                ));
                return res;
            };
            match wrap_with_wsl(opts, wsl, case, control_root, program, args) {
                Ok(launch) => Some(launch),
                Err(error) => {
                    res.failed = Some(format!("{name}: {error}"));
                    return res;
                }
            }
        } else {
            None
        };
        let argv = match &prepared_wsl {
            Some(launch) => launch.argv.clone(),
            None => match wrap(opts, case, program, args) {
                Ok(argv) => argv,
                Err(error) => {
                    res.failed = Some(format!("{name}: {error}"));
                    return res;
                }
            },
        };
        let log = case.join("logs").join(format!("{name}.log"));
        let timeout = match remaining_budget(start, budget) {
            Ok(left) => left,
            Err(error) => {
                res.failed = Some(format!("{name}: {error}"));
                return res;
            }
        };
        let outcome = match prepared_wsl.as_ref() {
            Some(launch) => run_logged_inner(&argv, case, &log, Some(timeout), Some(launch)),
            None => run_logged(&argv, case, &log, Some(timeout)),
        };
        match outcome {
            Ok(code) => {
                if code != 0 {
                    res.failed = Some(failure_with_log(
                        format!("{name} exited with code {code}"),
                        &log,
                    ));
                    return res;
                }
                if let Err(error) =
                    mirror_log(&log, &case.join(format!("log.{name}")), start, budget)
                {
                    res.failed = Some(failure_with_log(format!("{name}: {error}"), &log));
                    return res;
                }
                res.completed.push(name.clone());
            }
            Err(e) => {
                res.failed = Some(format!("{name}: {e}"));
                return res;
            }
        }
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testutil::tmp;

    #[test]
    fn wrapping_per_runner() {
        let cwd = Path::new("/tmp/case");
        let mut o = RunOpts {
            runner: "docker".into(),
            ..Default::default()
        };
        assert!(wrap(&o, cwd, "blockMesh", &[]).is_err(), "image required");
        o.runner = "native".into();
        assert!(wrap(&o, cwd, "definitely_not_a_real_tool_xyz", &[]).is_err());
    }

    #[test]
    fn wsl_and_docker_command_lines() {
        // only the string layout is checked, so fake the availability by pointing PATH search at nothing:
        // build the vectors through the same code path when the launcher exists, else skip.
        let o = RunOpts {
            runner: "docker".into(),
            image: "of:11".into(),
            bin_dir: "/opt/of/bin".into(),
            timeout_min: 0,
        };
        match wrap(&o, Path::new("/c"), "checkMesh", &["-allGeometry".into()]) {
            Ok(v) => {
                assert_eq!(v[0], "docker");
                assert!(v.contains(&"/opt/of/bin/checkMesh".to_string()));
                assert_eq!(v.last().unwrap(), "-allGeometry");
                let stop = docker_cleanup(&v);
                assert_eq!(&stop[..2], ["docker", "stop"]);
                assert!(stop.last().unwrap().starts_with("vernon-foam-"));
            }
            Err(e) => assert_eq!(e, "docker not found"),
        }
    }

    #[test]
    fn run_logged_captures_output_and_exit_codes() {
        let dir = tmp("extproc");
        let (prog, args): (&str, Vec<String>) = if cfg!(windows) {
            ("cmd", vec!["/C".into(), "echo hello".into()])
        } else {
            ("sh", vec!["-c".into(), "echo hello".into()])
        };
        let mut argv = vec![prog.to_string()];
        argv.extend(args);
        let log = dir.join("a.log");
        assert_eq!(run_logged(&argv, &dir, &log, None).unwrap(), 0);
        assert!(std::fs::read_to_string(&log).unwrap().contains("hello"));
        let (prog, args): (&str, Vec<String>) = if cfg!(windows) {
            ("cmd", vec!["/C".into(), "exit 3".into()])
        } else {
            ("sh", vec!["-c".into(), "exit 3".into()])
        };
        let mut argv = vec![prog.to_string()];
        argv.extend(args);
        assert_eq!(
            run_logged(&argv, &dir, &dir.join("b.log"), None).unwrap(),
            3
        );
        assert!(run_logged(
            &["no_such_program_xyz".into()],
            &dir,
            &dir.join("c.log"),
            None
        )
        .is_err());
    }

    fn explicit_wsl() -> WslOpts {
        WslOpts {
            distribution: "Vernon Ubuntu 24.04".into(),
            user: "root".into(),
            environment_file: "/opt/foam's environment/bashrc".into(),
        }
    }

    fn native_shell(script: &str) -> Vec<String> {
        if cfg!(windows) {
            vec!["cmd".into(), "/C".into(), script.into()]
        } else {
            vec!["sh".into(), "-c".into(), script.into()]
        }
    }

    fn control_root(case: &Path) -> PathBuf {
        case.with_extension("wsl-control")
    }

    #[test]
    fn wsl_argv_preserves_environment_program_and_hostile_arguments() {
        let opts = RunOpts {
            runner: "wsl".into(),
            bin_dir: "/opt/foam tools/bin".into(),
            ..Default::default()
        };
        let wsl = explicit_wsl();
        let args = vec![
            "space value".into(),
            "quote'value".into(),
            "$(touch injected); echo bad".into(),
            "line\nbreak".into(),
        ];
        let launch = wrap_with_wsl(
            &opts,
            &wsl,
            Path::new(r"C:\Users\example\case folder"),
            Path::new(r"C:\Users\example\.wsl-control"),
            "checkMesh",
            &args,
        )
        .unwrap();
        assert!(launch
            .argv
            .windows(2)
            .any(|v| v == ["--distribution", wsl.distribution.as_str()]));
        assert!(launch.argv.windows(2).any(|v| v == ["--user", "root"]));
        assert!(launch
            .argv
            .windows(2)
            .any(|v| v == ["--cd", "/mnt/c/Users/example/case folder"]));
        let mut expected = vec![wsl.environment_file, "/opt/foam tools/bin/checkMesh".into()];
        expected.extend(args.clone());
        assert!(
            launch.argv.ends_with(&expected),
            "literal tool argv must be preserved: {:?}",
            launch.argv
        );
        let script_index = launch
            .argv
            .iter()
            .position(|s| s == "-c")
            .expect("fixed Bash template");
        let script = &launch.argv[script_index + 1];
        for arg in args {
            assert!(
                !script.contains(&arg),
                "a tool argument was interpolated into shell code"
            );
        }
    }

    #[test]
    fn wsl_control_paths_are_unique_and_outside_case_generations() {
        let opts = RunOpts {
            runner: "wsl".into(),
            ..Default::default()
        };
        let case = Path::new(r"D:\project\case");
        let control = control_root(case);
        let first =
            wrap_with_wsl(&opts, &explicit_wsl(), case, &control, "checkMesh", &[]).unwrap();
        let second =
            wrap_with_wsl(&opts, &explicit_wsl(), case, &control, "checkMesh", &[]).unwrap();
        assert_eq!(first.record_path.parent(), Some(control.as_path()));
        assert_eq!(first.cancel_path.parent(), Some(control.as_path()));
        assert!(!first.record_path.starts_with(case));
        assert!(wrap_with_wsl(
            &opts,
            &explicit_wsl(),
            case,
            &case.join("control"),
            "checkMesh",
            &[]
        )
        .is_err());
        assert_ne!(first.record_path, second.record_path);
        assert_ne!(first.cancel_path, second.cancel_path);
        assert_ne!(first.token, second.token);
        assert!(!first.token.is_empty());
        assert!(first
            .argv
            .windows(2)
            .any(|v| v == ["--cd", "/mnt/d/project/case"]));
    }

    #[test]
    fn wsl_rejects_relative_and_unc_working_directories() {
        let opts = RunOpts {
            runner: "wsl".into(),
            ..Default::default()
        };
        for case in [r"relative\case", r"\\server\share\case", r"C:relative\case"] {
            let error = wrap_with_wsl(
                &opts,
                &explicit_wsl(),
                Path::new(case),
                &control_root(Path::new(case)),
                "checkMesh",
                &[],
            )
            .unwrap_err();
            assert!(error.contains("working directory"), "{case}: {error}");
        }
    }

    #[test]
    fn wsl_intent_is_published_before_launch_and_discarded_after_completion() {
        let case = tmp("wsl-intent-before-launch");
        let opts = RunOpts { runner: "wsl".into(), ..Default::default() };
        let launch = wrap_with_wsl(&opts, &explicit_wsl(), &case, &control_root(&case), "checkMesh", &[]).unwrap();
        let intent = launch.control_root.join(format!(".{}.intent", launch.token));
        assert!(!intent.exists(), "preview must remain read-only");
        publish_wsl_intent(&launch).unwrap();
        assert!(intent.exists(), "cleanup cannot discover a delayed launch without its durable intent");
        assert_eq!(fs::read_to_string(&intent).unwrap(), format!("{}\n", launch.token));
        assert!(!launch.record_path.exists(), "this fixture never starts Linux");
        assert!(publish_wsl_intent(&launch).is_err(), "an existing attempt must never be overwritten");
        discard_completed_control(&launch);
        assert!(!intent.exists(), "verified completion should discard the pending intent");
    }

    #[test]
    fn wsl_intent_refuses_cancellation_and_existing_unowned_files() {
        let case = tmp("wsl-intent-refusal");
        let opts = RunOpts { runner: "wsl".into(), ..Default::default() };
        let launch = wrap_with_wsl(&opts, &explicit_wsl(), &case, &control_root(&case), "checkMesh", &[]).unwrap();
        let intent = launch.control_root.join(format!(".{}.intent", launch.token));
        let stopped = process::with_cancel(std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)), || publish_wsl_intent(&launch));
        assert!(stopped.is_err());
        assert!(!intent.exists());
        prepare_control_root(&launch.control_root).unwrap();
        fs::write(&intent, "unowned prior content\n").unwrap();
        assert!(publish_wsl_intent(&launch).is_err());
        assert_eq!(fs::read_to_string(intent).unwrap(), "unowned prior content\n");
    }

    #[test]
    fn wsl_requires_distribution_user_and_linux_environment_path() {
        let opts = RunOpts {
            runner: "wsl".into(),
            ..Default::default()
        };
        for (field, wsl) in [
            (
                "distribution",
                WslOpts {
                    distribution: String::new(),
                    ..explicit_wsl()
                },
            ),
            (
                "user",
                WslOpts {
                    user: String::new(),
                    ..explicit_wsl()
                },
            ),
            (
                "environment",
                WslOpts {
                    environment_file: r"C:\foam\bashrc".into(),
                    ..explicit_wsl()
                },
            ),
        ] {
            let error = wrap_with_wsl(
                &opts,
                &wsl,
                Path::new(r"C:\case"),
                Path::new(r"C:\.wsl-control"),
                "checkMesh",
                &[],
            )
            .unwrap_err();
            assert!(
                error.contains(field),
                "expected {field} validation, got {error}"
            );
        }
    }

    #[test]
    fn unknown_runner_is_rejected_instead_of_launching_native() {
        let opts = RunOpts {
            runner: "mistyped-runner".into(),
            ..Default::default()
        };
        let program = if cfg!(windows) { "cmd" } else { "sh" };
        let error = wrap(&opts, Path::new("."), program, &[]).unwrap_err();
        assert!(error.contains("runner"), "{error}");
    }

    #[test]
    fn empty_step_is_a_reported_error_not_a_panic() {
        let case = tmp("empty-step");
        let result = run_chain(&RunOpts::default(), &case, &[("empty".into(), Vec::new())]);
        assert!(
            result
                .failed
                .as_deref()
                .is_some_and(|s| s.contains("empty command")),
            "{result:?}"
        );
        assert!(result.completed.is_empty());
    }

    #[test]
    fn step_names_cannot_escape_the_log_directory() {
        let case = tmp("unsafe-step-name");
        let result = run_chain(
            &RunOpts::default(),
            &case,
            &[("../outside".into(), vec!["no_such_program_xyz".into()])],
        );
        assert!(
            result
                .failed
                .as_deref()
                .is_some_and(|s| s.contains("step name")),
            "{result:?}"
        );
    }

    #[test]
    fn timeout_minutes_overflow_is_reported_before_any_step() {
        let opts = RunOpts {
            timeout_min: u64::MAX,
            ..Default::default()
        };
        let result = run_chain(&opts, &tmp("timeout-overflow"), &[]);
        assert!(
            result
                .failed
                .as_deref()
                .is_some_and(|s| s.contains("timeout")),
            "{result:?}"
        );
    }

    #[test]
    fn cancelled_chain_does_not_even_resolve_a_tool() {
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let result = process::with_cancel(flag, || {
            run_chain(
                &RunOpts::default(),
                &tmp("pre-cancel"),
                &[("blocked".into(), vec!["no_such_program_xyz".into()])],
            )
        });
        assert!(
            result
                .failed
                .as_deref()
                .is_some_and(|s| s.contains("cancel")),
            "{result:?}"
        );
        assert!(result.completed.is_empty());
    }

    #[test]
    fn structured_entry_preserves_native_execution() {
        let case = tmp("structured-native");
        std::fs::create_dir_all(&case).unwrap();
        let opts = RunOpts {
            runner: "native".into(),
            ..Default::default()
        };
        let result = run_chain_with_wsl(
            &opts,
            &WslOpts::default(),
            &case,
            &control_root(&case),
            &[("echo".into(), native_shell("echo native-ok"))],
        );
        assert_eq!(result.completed, ["echo"]);
        assert!(result.failed.is_none(), "{result:?}");
        assert!(std::fs::read_to_string(case.join("logs/echo.log"))
            .unwrap()
            .contains("native-ok"));
    }

    #[test]
    fn one_deadline_covers_all_steps_and_stops_the_next_step() {
        let case = tmp("whole-chain-budget");
        std::fs::create_dir_all(&case).unwrap();
        // Leave process-start scheduling margin under a busy solver workload.
        // Each pause fits the budget; both pauses together must exceed it.
        let pause = if cfg!(windows) {
            "ping -n 4 127.0.0.1 >nul"
        } else {
            "sleep 1.5"
        };
        let budget = if cfg!(windows) {
            Duration::from_millis(5000)
        } else {
            Duration::from_millis(2500)
        };
        let steps = vec![
            ("first".into(), native_shell(pause)),
            ("second".into(), native_shell(pause)),
            (
                "never".into(),
                native_shell("echo should-not-start > next-step.txt"),
            ),
        ];
        let result = run_chain_with_budget(&RunOpts::default(), None, &case, None, &steps, budget);
        assert_eq!(result.completed, ["first"], "{result:?}");
        assert!(
            result
                .failed
                .as_deref()
                .is_some_and(|s| s.contains("timed out") || s.contains("deadline")),
            "{result:?}"
        );
        assert!(!case.join("next-step.txt").exists());
    }

    #[test]
    fn diagnostics_keep_only_a_bounded_tail_with_the_last_error() {
        let case = tmp("diagnostic-tail");
        std::fs::create_dir_all(&case).unwrap();
        let log = case.join("large.log");
        let mut bytes = vec![b'x'; 256 * 1024];
        bytes.extend_from_slice(b"\nfinal diagnostic\xff\n");
        std::fs::write(&log, bytes).unwrap();
        let tail = diagnostic_tail(&log);
        assert!(tail.contains("final diagnostic"));
        assert!(tail.len() <= 16 * 1024, "{} bytes", tail.len());
        assert!(diagnostic_tail(&case.join("missing.log")).is_empty());
    }

    #[test]
    fn nonzero_step_reports_its_bounded_diagnostic_tail() {
        let case = tmp("failed-diagnostics");
        std::fs::create_dir_all(&case).unwrap();
        let script = if cfg!(windows) {
            "echo specific-failure-detail & exit 7"
        } else {
            "echo specific-failure-detail; exit 7"
        };
        let result = run_chain(
            &RunOpts::default(),
            &case,
            &[("failure".into(), native_shell(script))],
        );
        let error = result.failed.unwrap();
        assert!(error.contains("7"), "{error}");
        assert!(error.contains("specific-failure-detail"), "{error}");
    }

    fn runtime_wsl() -> WslOpts {
        WslOpts {
            distribution: std::env::var("VERNON_WSL_DISTRO")
                .expect("set VERNON_WSL_DISTRO to the approved test distribution"),
            user: std::env::var("VERNON_WSL_USER")
                .expect("set VERNON_WSL_USER to the explicit Linux test user"),
            environment_file: "/dev/null".into(),
        }
    }

    #[test]
    #[ignore = "needs WSL"]
    fn wsl_regression_success_does_not_leave_a_background_group() {
        let case = tmp("wsl-zero-exit-background");
        fs::create_dir_all(&case).unwrap();
        let opts = RunOpts {
            runner: "wsl".into(),
            ..Default::default()
        };
        let steps = vec![(
            "background".into(),
            vec![
                "/bin/sh".into(),
                "-c".into(),
                "(printf started > started.txt; sleep 4; printf escaped > escaped.txt) </dev/null >/dev/null 2>&1 & exit 0".into(),
            ],
        )];
        let result = run_chain_with_budget(
            &opts,
            Some(&runtime_wsl()),
            &case,
            Some(&control_root(&case)),
            &steps,
            Duration::from_secs(1),
        );
        let started = case.join("started.txt");
        std::thread::sleep(Duration::from_millis(4500));
        assert!(
            started.exists(),
            "background-child coverage was not established: {result:?}"
        );
        assert!(
            !case.join("escaped.txt").exists(),
            "the background group survived successful launcher completion: {result:?}"
        );
    }

    #[test]
    #[ignore = "needs WSL"]
    fn wsl_regression_cancelled_start_remains_blocked_after_case_replacement() {
        let owner = tmp("wsl-control-case-replacement");
        let case = owner.join("case");
        fs::create_dir_all(&case).unwrap();
        let opts = RunOpts {
            runner: "wsl".into(),
            ..Default::default()
        };
        let launch = wrap_with_wsl(
            &opts,
            &runtime_wsl(),
            &case,
            &control_root(&case),
            "/bin/sh",
            &["-c".into(), "printf escaped > should-not-start.txt".into()],
        )
        .unwrap();
        prepare_control_root(&launch.control_root).unwrap();
        fs::write(&launch.cancel_path, b"cancelled\n").unwrap();
        fs::rename(&case, owner.join("case.previous-0")).unwrap();
        fs::create_dir(&case).unwrap();
        let code = run_logged_inner(
            &launch.argv,
            &case,
            &owner.join("old-launch.log"),
            Some(Duration::from_secs(10)),
            Some(&launch),
        )
        .unwrap();
        assert_eq!(
            code, 125,
            "a delayed original argv must remain cancelled after case archival/replacement"
        );
        assert!(!case.join("should-not-start.txt").exists());
    }

    #[test]
    #[ignore = "needs WSL and OpenFOAM"]
    fn wsl_environment_with_installed_runtime_supports_simplefoam_help() {
        let case = tmp("wsl-real-environment");
        fs::create_dir_all(&case).unwrap();
        let mut wsl = runtime_wsl();
        wsl.environment_file = std::env::var("VERNON_WSL_ENV_FILE")
            .expect("set VERNON_WSL_ENV_FILE to the installed runtime's Linux bashrc path");
        let opts = RunOpts {
            runner: "wsl".into(),
            timeout_min: 1,
            ..Default::default()
        };
        let result = run_chain_with_wsl(
            &opts,
            &wsl,
            &case,
            &control_root(&case),
            &[("help".into(), vec!["simpleFoam".into(), "-help".into()])],
        );
        assert!(result.failed.is_none(), "{result:?}");
        assert_eq!(result.completed, ["help"]);
        let output = fs::read_to_string(case.join("logs/help.log")).unwrap();
        assert!(
            output.contains("simpleFoam"),
            "the installed executable's help was not observed"
        );
        assert!(
            !output.contains("pop_var_context"),
            "runtime environment sourcing failed"
        );
    }

    #[test]
    #[ignore = "needs WSL"]
    fn wsl_environment_may_change_directory_and_positional_parameters() {
        let case = tmp("wsl-startup-state");
        fs::create_dir_all(&case).unwrap();
        let environment = case.join("startup file's.bashrc");
        fs::write(&environment, b"cd /\nset -- replaced parameters\n").unwrap();
        let mut wsl = runtime_wsl();
        wsl.environment_file = linux_working_directory(&environment).unwrap();
        let opts = RunOpts {
            runner: "wsl".into(),
            timeout_min: 1,
            ..Default::default()
        };
        let literal = "literal $(not-a-command); 'value'";
        let steps = vec![(
            "startup".into(),
            vec![
                "/bin/sh".into(),
                "-c".into(),
                "printf '%s' \"$1\" > actual-argument.txt; pwd > actual-cwd.txt".into(),
                "vernon-test".into(),
                literal.into(),
            ],
        )];
        let result = run_chain_with_wsl(&opts, &wsl, &case, &control_root(&case), &steps);
        assert!(result.failed.is_none(), "{result:?}");
        assert_eq!(
            fs::read_to_string(case.join("actual-argument.txt")).unwrap(),
            literal
        );
        assert_eq!(
            fs::read_to_string(case.join("actual-cwd.txt"))
                .unwrap()
                .trim(),
            linux_working_directory(&case).unwrap()
        );
    }

    #[test]
    #[ignore = "needs WSL"]
    fn wsl_cancellation_before_record_publication_prevents_tool_start() {
        let case = tmp("wsl-prepublication-cancel");
        fs::create_dir_all(&case).unwrap();
        let opts = RunOpts {
            runner: "wsl".into(),
            ..Default::default()
        };
        let launch = wrap_with_wsl(
            &opts,
            &runtime_wsl(),
            &case,
            &control_root(&case),
            "/bin/sh",
            &["-c".into(), "printf escaped > should-not-start.txt".into()],
        )
        .unwrap();
        prepare_control_root(&launch.control_root).unwrap();
        fs::write(&launch.cancel_path, b"cancelled\n").unwrap();
        let code = run_logged_inner(
            &launch.argv,
            &case,
            &case.join("prepublication.log"),
            Some(Duration::from_secs(10)),
            Some(&launch),
        )
        .unwrap();
        assert_eq!(code, 125);
        assert!(!case.join("should-not-start.txt").exists());
        assert!(
            !launch.record_path.exists(),
            "cancelled startup must precede identity publication"
        );
        assert!(
            launch.cancel_path.exists(),
            "late-launch protection must survive cleanup"
        );
    }

    #[test]
    #[ignore = "needs WSL"]
    fn wsl_timeout_stops_an_active_linux_grandchild() {
        let case = tmp("wsl-timeout-tree");
        fs::create_dir_all(&case).unwrap();
        let opts = RunOpts {
            runner: "wsl".into(),
            ..Default::default()
        };
        let steps = vec![(
            "wait".into(),
            vec![
                "/bin/sh".into(),
                "-c".into(),
                "printf started > started.txt; (sleep 4; printf escaped > escaped.txt) & wait"
                    .into(),
            ],
        )];
        let result = run_chain_with_budget(
            &opts,
            Some(&runtime_wsl()),
            &case,
            Some(&control_root(&case)),
            &steps,
            Duration::from_secs(2),
        );
        let error = result
            .failed
            .expect("the long-running Linux step must time out");
        assert!(
            case.join("started.txt").exists(),
            "active-child coverage was not established: {error}"
        );
        assert!(error.contains("timed out"), "{error}");
        assert!(error.contains("cleanup verified"), "{error}");
        assert!(result.completed.is_empty());
        std::thread::sleep(Duration::from_millis(4500));
        assert!(
            !case.join("escaped.txt").exists(),
            "a Linux descendant survived timeout cleanup"
        );
    }

    #[test]
    #[ignore = "needs WSL"]
    fn wsl_cook_cancellation_stops_children_and_does_not_cancel_cleanup() {
        let case = tmp("wsl-cancel-tree");
        fs::create_dir_all(&case).unwrap();
        let started = case.join("started.txt");
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let setter_flag = flag.clone();
        let setter = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(8);
            while !started.exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            setter_flag.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        let opts = RunOpts {
            runner: "wsl".into(),
            ..Default::default()
        };
        let steps = vec![(
            "wait".into(),
            vec![
                "/bin/sh".into(),
                "-c".into(),
                "printf started > started.txt; (sleep 4; printf escaped > escaped.txt) & wait"
                    .into(),
            ],
        )];
        let wsl = runtime_wsl();
        let result = process::with_cancel(flag, || {
            run_chain_with_budget(
                &opts,
                Some(&wsl),
                &case,
                Some(&control_root(&case)),
                &steps,
                Duration::from_secs(20),
            )
        });
        setter.join().unwrap();
        let error = result
            .failed
            .expect("the active Linux step must be cancelled");
        assert!(
            case.join("started.txt").exists(),
            "active-child coverage was not established: {error}"
        );
        assert!(error.contains("cancelled"), "{error}");
        assert!(error.contains("cleanup verified"), "{error}");
        assert!(result.completed.is_empty());
        std::thread::sleep(Duration::from_millis(4500));
        assert!(
            !case.join("escaped.txt").exists(),
            "a Linux descendant survived cook cancellation"
        );
        assert!(
            !process::cancel_requested(),
            "the enclosing cancellation context must be restored"
        );
    }
}
