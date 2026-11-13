//! The programs a build runs: the user's Blender (with the pipeline's scripts) and the resource size table
//! calculator. Each run's whole output goes to a log file; the lines the caller picks are shown as it goes.

use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use crate::progress::{CANCELLED, Progress};
use crate::{Context, Error, Result};

/// The Blender scripts (`pipeline/`), built in: the app writes them out where Blender can run them.
pub const SCRIPTS: [(&str, &str); 4] = [
    (
        "make_scene.py",
        include_str!("../../../pipeline/make_scene.py"),
    ),
    (
        "prep_for_game.py",
        include_str!("../../../pipeline/prep_for_game.py"),
    ),
    (
        "check_result.py",
        include_str!("../../../pipeline/check_result.py"),
    ),
    (
        "scene_nodes.py",
        include_str!("../../../pipeline/scene_nodes.py"),
    ),
];

/// The folder Blender runs the scripts from: `MODSTUDIO_PIPELINE` if set (a checkout's `pipeline` folder, to
/// work on the scripts without rebuilding), else the built-in copies written to `<cache>/pipeline`.
pub fn scripts_dir(cache: &Path) -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("MODSTUDIO_PIPELINE") {
        return Ok(PathBuf::from(dir));
    }
    let dir = cache.join("pipeline");
    std::fs::create_dir_all(&dir).context(dir.display())?;
    for (name, text) in SCRIPTS {
        let path = dir.join(name);
        if std::fs::read(&path).ok().as_deref() != Some(text.as_bytes()) {
            std::fs::write(&path, text).context(path.display())?;
        }
    }
    Ok(dir)
}

/// Blender installs found in the usual places, newest first.
pub fn find_blender() -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        for var in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
            let Some(base) = std::env::var_os(var).map(PathBuf::from) else {
                continue;
            };
            if let Ok(entries) = std::fs::read_dir(base.join("Blender Foundation")) {
                found.extend(
                    entries
                        .flatten()
                        .map(|e| e.path().join("blender.exe"))
                        .filter(|p| p.is_file()),
                );
            }
            let steam = base.join("Steam/steamapps/common/Blender/blender.exe");
            if steam.is_file() {
                found.push(steam);
            }
        }
    } else {
        if let Some(path) = std::env::var_os("PATH") {
            found.extend(
                std::env::split_paths(&path)
                    .map(|d| d.join("blender"))
                    .filter(|p| p.is_file()),
            );
        }
        for p in [
            "/usr/bin/blender",
            "/usr/local/bin/blender",
            "/snap/bin/blender",
        ] {
            if Path::new(p).is_file() {
                found.push(p.into());
            }
        }
    }
    found.sort_by_key(|p| std::cmp::Reverse(version_key(p)));
    found.dedup();
    found
}

/// "Blender 5.2" in a path, as numbers to sort by.
fn version_key(path: &Path) -> Vec<u32> {
    path.components()
        .filter_map(|c| c.as_os_str().to_str()?.strip_prefix("Blender "))
        .filter(|v| !v.is_empty() && v.split('.').all(|n| n.parse::<u32>().is_ok()))
        .flat_map(|v| v.split('.').map(|n| n.parse().unwrap_or(0)))
        .collect()
}

/// Blender's version line ("Blender 5.2.0"). Errors if it doesn't run or is older than 5.
pub fn blender_version(exe: &Path) -> Result<String> {
    let mut cmd = Command::new(exe);
    cmd.arg("--version")
        .arg("--factory-startup")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    hide_window(&mut cmd);
    let out = cmd.output().context(format!("running {}", exe.display()))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text
        .lines()
        .find(|l| l.starts_with("Blender "))
        .ok_or_else(|| Error(format!("{} didn't say its version", exe.display())))?
        .trim()
        .to_owned();
    let major: u32 = line["Blender ".len()..]
        .split('.')
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0);
    if major < 5 {
        return Err(Error(format!("{line}: Blender 5 or newer is needed")));
    }
    Ok(line)
}

/// Opens `blend` in Blender for editing (Blender keeps running on its own).
pub fn open_in_blender(exe: &Path, blend: &Path) -> Result<()> {
    // blender-launcher.exe starts Blender on Windows without a console window.
    let launcher = exe.with_file_name("blender-launcher.exe");
    let exe = if cfg!(windows) && launcher.is_file() {
        launcher
    } else {
        exe.to_owned()
    };
    Command::new(&exe)
        .arg(blend)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context(format!("starting {}", exe.display()))?;
    Ok(())
}

/// Runs `exe`, writes its command line and whole output to `log_path`, shows the lines `show` picks, and
/// fails (naming the log and the last error lines) if it exits with an error. Stops it if the task is cancelled.
pub fn run(
    exe: &Path,
    args: &[OsString],
    log_path: &Path,
    show: &dyn Fn(&str) -> bool,
    progress: &mut Progress,
) -> Result<()> {
    progress.check()?;
    if let Some(dir) = log_path.parent() {
        std::fs::create_dir_all(dir).context(dir.display())?;
    }
    let mut log = std::fs::File::create(log_path).context(log_path.display())?;
    writeln!(log, "{}\n", command_line(exe, args)).context(log_path.display())?;

    let mut cmd = Command::new(exe);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_window(&mut cmd);
    let mut child = cmd.spawn().context(format!("starting {}", exe.display()))?;

    let (tx, rx) = mpsc::channel::<String>();
    let pipes: [Option<Box<dyn Read + Send>>; 2] = [
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    ];
    for pipe in pipes.into_iter().flatten() {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(pipe);
            let mut buf = Vec::new();
            while matches!(reader.read_until(b'\n', &mut buf), Ok(n) if n > 0) {
                let line = String::from_utf8_lossy(&buf).trim_end().to_owned();
                buf.clear();
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
    }
    drop(tx);

    let mut tail: VecDeque<String> = VecDeque::new();
    loop {
        if progress.cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            let _ = writeln!(log, "\n(cancelled)");
            return Err(Error(CANCELLED.into()));
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => {
                let _ = writeln!(log, "{line}");
                if !line.is_empty() && show(&line) {
                    progress.line(line.clone());
                }
                tail.push_back(line);
                if tail.len() > 40 {
                    tail.pop_front();
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    let status = child
        .wait()
        .context(format!("waiting for {}", exe.display()))?;
    if status.success() {
        return Ok(());
    }
    let errors: Vec<&str> = tail
        .iter()
        .map(String::as_str)
        .filter(|l| l.contains("Error") || l.contains("error:") || l.starts_with("FAILED"))
        .collect();
    let shown = if errors.is_empty() {
        tail.iter()
            .rev()
            .take(5)
            .rev()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        errors[errors.len().saturating_sub(5)..].join("\n")
    };
    let name = exe.file_stem().unwrap_or(exe.as_os_str()).to_string_lossy();
    Err(Error(format!(
        "{name} failed ({status}). See {}{}",
        log_path.display(),
        if shown.is_empty() {
            String::new()
        } else {
            format!(":\n{shown}")
        }
    )))
}

fn command_line(exe: &Path, args: &[OsString]) -> String {
    std::iter::once(exe.as_os_str())
        .chain(args.iter().map(OsString::as_os_str))
        .map(|a: &OsStr| {
            let a = a.to_string_lossy();
            if a.contains(' ') {
                format!("\"{a}\"")
            } else {
                a.into_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// No console window for a background run started from the app.
fn hide_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

/// Arguments from strings and paths: `args!["--python", script]`.
#[macro_export]
macro_rules! args {
    ($($a:expr),* $(,)?) => {
        [$(::std::ffi::OsString::from($a)),*]
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blender_versions_sort_newest_first() {
        assert_eq!(
            version_key(Path::new(
                "C:/Program Files/Blender Foundation/Blender 5.2/blender.exe"
            )),
            vec![5, 2]
        );
        let mut v = [
            PathBuf::from("C:/B/Blender 4.5/blender.exe"),
            PathBuf::from("C:/B/Blender 5.10/blender.exe"),
            PathBuf::from("C:/B/Blender 5.2/blender.exe"),
        ];
        v.sort_by_key(|p| std::cmp::Reverse(version_key(p)));
        assert!(v[0].ends_with("Blender 5.10/blender.exe"));
        assert!(v[2].ends_with("Blender 4.5/blender.exe"));
    }

    #[test]
    fn scripts_are_written_once() {
        let dir = std::env::temp_dir().join(format!("modstudio-scripts-{}", std::process::id()));
        // (MODSTUDIO_PIPELINE isn't set in tests)
        let out = scripts_dir(&dir).unwrap();
        for (name, text) in SCRIPTS {
            assert_eq!(std::fs::read_to_string(out.join(name)).unwrap(), text);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
