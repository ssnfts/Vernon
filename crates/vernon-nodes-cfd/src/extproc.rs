//! Running external programs (OpenFOAM) as separate processes. Output goes to
//! log files, never pipes, so a chatty solver cannot deadlock the node.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

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
    /// 0 = the shared runner's default deadline (one day).
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
        _ => {
            let exe = find_exe(&opts.bin_dir, program).ok_or_else(|| format!("{program} not found"))?;
            let mut v = vec![exe.display().to_string()];
            v.extend_from_slice(args);
            Ok(v)
        }
    }
}

/// `docker stop NAME` for a `docker run --name NAME ...` command line.
fn docker_cleanup(argv: &[String]) -> Vec<String> {
    match argv.iter().position(|a| a == "--name") {
        Some(i) if argv.first().is_some_and(|a| a == "docker") && i + 1 < argv.len() => {
            vec!["docker".into(), "stop".into(), "-t".into(), "2".into(), argv[i + 1].clone()]
        }
        _ => Vec::new(),
    }
}

/// Run `argv` in `cwd` through the shared runner, output written to `log`
/// (started afresh). Returns the exit code. `None` uses the runner's
/// default deadline; a timeout or a cancelled cook kills the whole process
/// tree (and stops a docker container).
pub fn run_logged(argv: &[String], cwd: &Path, log: &Path, timeout: Option<Duration>) -> Result<i32, String> {
    let (exe, args) = argv.split_first().ok_or("empty command line")?;
    let opts = RunOptions { timeout: timeout.unwrap_or(Duration::ZERO), cleanup: docker_cleanup(argv), ..Default::default() };
    let mut cmd = Command::new(exe);
    cmd.args(args).current_dir(cwd);
    match process::run(&mut cmd, log, &opts).map_err(|e| format!("cannot start {exe}: {e}"))? {
        Exit::Code(c) => Ok(c),
        Exit::TimedOut => Err(format!("{exe} timed out")),
        Exit::Cancelled => Err(format!("{exe} was stopped (cook cancelled)")),
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
        assert_eq!(run_logged(&argv, &dir, &dir.join("b.log"), None).unwrap(), 3);
        assert!(run_logged(&["no_such_program_xyz".into()], &dir, &dir.join("c.log"), None).is_err());
    }
}
