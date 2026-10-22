//! Running external programs (OpenFOAM) as separate processes. Output goes to
//! log files, never pipes, so a chatty solver cannot deadlock the node.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Where and how the tools run.
#[derive(Debug, Clone, Default)]
pub struct RunOpts {
    /// `native`, `wsl` or `docker`.
    pub runner: String,
    /// Container image for the `docker` runner.
    pub image: String,
    /// Directory holding the OpenFOAM executables; empty searches PATH.
    pub bin_dir: String,
    /// 0 = no timeout.
    pub timeout_min: u64,
}

fn exe_names(name: &str) -> Vec<String> {
    if cfg!(windows) {
        vec![format!("{name}.exe"), format!("{name}.cmd"), format!("{name}.bat"), name.to_string()]
    } else {
        vec![name.to_string()]
    }
}

/// Find an executable in `bin_dir` or on PATH.
pub fn find_exe(bin_dir: &str, name: &str) -> Option<PathBuf> {
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
pub fn wrap(opts: &RunOpts, cwd: &Path, program: &str, args: &[String]) -> Result<Vec<String>, String> {
    let inner = if opts.bin_dir.trim().is_empty() || opts.runner == "native" {
        program.to_string()
    } else {
        format!("{}/{}", opts.bin_dir.trim().trim_end_matches('/'), program)
    };
    match opts.runner.as_str() {
        "wsl" => {
            find_exe("", "wsl").ok_or("wsl.exe not found")?;
            let mut v = vec!["wsl.exe".into(), "--cd".into(), cwd.display().to_string(), "--".into(), inner];
            v.extend_from_slice(args);
            Ok(v)
        }
        "docker" => {
            if opts.image.trim().is_empty() {
                return Err("the docker runner needs an image name".into());
            }
            find_exe("", "docker").ok_or("docker not found")?;
            let mut v = vec![
                "docker".into(),
                "run".into(),
                "--rm".into(),
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
        _ => {
            let exe = find_exe(&opts.bin_dir, program).ok_or_else(|| format!("{program} not found"))?;
            let mut v = vec![exe.display().to_string()];
            v.extend_from_slice(args);
            Ok(v)
        }
    }
}

/// Run `argv` in `cwd`, output appended to `log`. Returns the exit code.
pub fn run_logged(argv: &[String], cwd: &Path, log: &Path, timeout: Option<Duration>) -> Result<i32, String> {
    if let Some(parent) = log.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let out = File::create(log).map_err(|e| format!("cannot create {}: {e}", log.display()))?;
    let err = out.try_clone().map_err(|e| e.to_string())?;
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err))
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", argv[0]))?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status.code().unwrap_or(-1)),
            Ok(None) => {
                if timeout.is_some_and(|t| start.elapsed() > t) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("{} timed out", argv[0]));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e.to_string()),
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
    let mut res = ChainResult::default();
    let timeout = (opts.timeout_min > 0).then(|| Duration::from_secs(opts.timeout_min * 60));
    for (name, cmd) in steps {
        let argv = match wrap(opts, case, &cmd[0], &cmd[1..]) {
            Ok(a) => a,
            Err(e) => {
                res.failed = Some(format!("{name}: {e}"));
                return res;
            }
        };
        let log = case.join("logs").join(format!("{name}.log"));
        match run_logged(&argv, case, &log, timeout) {
            Ok(0) => {
                let _ = std::fs::copy(&log, case.join(format!("log.{name}")));
                res.completed.push(name.clone());
            }
            Ok(code) => {
                let _ = std::fs::copy(&log, case.join(format!("log.{name}")));
                res.failed = Some(format!("{name} exited with code {code} (see {})", log.display()));
                return res;
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
        let mut o = RunOpts { runner: "docker".into(), ..Default::default() };
        assert!(wrap(&o, cwd, "blockMesh", &[]).is_err(), "image required");
        o.runner = "native".into();
        assert!(wrap(&o, cwd, "definitely_not_a_real_tool_xyz", &[]).is_err());
    }

    #[test]
    fn wsl_and_docker_command_lines() {
        // only the string layout is checked, so fake the availability by pointing PATH search at nothing:
        // build the vectors through the same code path when the launcher exists, else skip.
        let o = RunOpts { runner: "docker".into(), image: "of:11".into(), bin_dir: "/opt/of/bin".into(), timeout_min: 0 };
        match wrap(&o, Path::new("/c"), "checkMesh", &["-allGeometry".into()]) {
            Ok(v) => {
                assert_eq!(v[0], "docker");
                assert!(v.contains(&"/opt/of/bin/checkMesh".to_string()));
                assert_eq!(v.last().unwrap(), "-allGeometry");
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
        assert_eq!(run_logged(&argv, &dir, &dir.join("b.log"), None).unwrap(), 3);
        assert!(run_logged(&["no_such_program_xyz".into()], &dir, &dir.join("c.log"), None).is_err());
    }
}
