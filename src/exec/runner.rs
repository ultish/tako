//! Spawn a planned command, stream stdout/stderr lines, kill process group on cancel.

use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, watch};

use super::args::PlannedCommand;

/// Options for [`spawn_and_stream`].
pub struct SpawnOpts {
    pub planned: PlannedCommand,
    /// Job id (caller-owned; kept for API symmetry / future structured events).
    #[allow(dead_code)]
    pub job_id: u64,
    /// Cooperative cancel: set to `true` to kill the process group.
    pub cancel_rx: watch::Receiver<bool>,
    /// Line callback channel — runner sends each stdout/stderr line.
    pub line_tx: mpsc::UnboundedSender<String>,
}

/// Kill an entire process group (Unix). Best-effort no-op elsewhere.
///
/// Children are started in their own session/process group via `pre_exec` +
/// `setsid`, so `kill(-pid, SIGTERM)` reaps skaffold/gradle grandchildren.
pub fn kill_process_group(pid: u32) {
    if pid == 0 {
        return;
    }
    // Use the system `kill` so we don't need a libc dependency.
    // Negative PID = process group on Unix.
    let _ = std::process::Command::new("kill")
        .args(["-TERM", &format!("-{pid}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Spawn `planned`, stream merged stdout/stderr lines, honor cancel.
///
/// Returns `(ok, summary)` where `summary` is a short exit description
/// (exit code, signal, cancel, or spawn error).
pub async fn spawn_and_stream(mut opts: SpawnOpts) -> (bool, String) {
    let planned = &opts.planned;
    let mut cmd = Command::new(&planned.program);
    cmd.args(&planned.args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .stdin(Stdio::null());

    if let Some(cwd) = &planned.cwd {
        cmd.current_dir(cwd);
    }

    // New process group so cancel can reap the whole tree (gradle/skaffold kids).
    #[cfg(unix)]
    {
        // Safety: runs in the child just after fork, before exec — setsid is the
        // standard way to become a process-group leader for later killpg.
        unsafe {
            cmd.pre_exec(|| {
                // setsid() fails if already a leader; ignore and continue.
                libc_setsid();
                Ok(())
            });
        }
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(err) => {
            let summary = format!("spawn failed: {err}");
            let _ = opts.line_tx.send(summary.clone());
            return (false, summary);
        }
    };

    let pid = child.id().unwrap_or(0);
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    // Pump stdout / stderr concurrently into the line channel.
    let line_tx_out = opts.line_tx.clone();
    let out_task = tokio::spawn(async move {
        if let Some(out) = stdout {
            let mut lines = BufReader::new(out).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line_tx_out.send(line).is_err() {
                    break;
                }
            }
        }
    });

    let line_tx_err = opts.line_tx.clone();
    let err_task = tokio::spawn(async move {
        if let Some(err) = stderr {
            let mut lines = BufReader::new(err).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line_tx_err.send(line).is_err() {
                    break;
                }
            }
        }
    });

    let mut cancelled = false;
    let status = loop {
        tokio::select! {
            biased;
            changed = opts.cancel_rx.changed() => {
                if changed.is_ok() && *opts.cancel_rx.borrow() {
                    cancelled = true;
                    if pid != 0 {
                        kill_process_group(pid);
                    }
                    let _ = child.start_kill();
                    // Fall through to wait so we don't leave a zombie.
                    break child.wait().await;
                }
            }
            status = child.wait() => {
                break status;
            }
        }
    };

    // Drain pumps (they exit when pipes close after wait).
    let _ = out_task.await;
    let _ = err_task.await;

    if cancelled {
        return (false, "cancelled".into());
    }

    match status {
        Ok(st) if st.success() => (true, "exit 0".into()),
        Ok(st) => {
            let code = st
                .code()
                .map(|c| format!("exit {c}"))
                .unwrap_or_else(|| "terminated by signal".into());
            (false, code)
        }
        Err(err) => (false, format!("wait failed: {err}")),
    }
}

/// Call setsid(2) without linking libc directly (inline syscall via nix-less shim).
#[cfg(unix)]
fn libc_setsid() {
    // libc is a transitive dep of tokio/nix on unix; use the `libc` crate if
    // present, else raw syscall via the kill-compatible approach.
    // We depend on `libc` lightly — see Cargo.toml.
    unsafe {
        let _ = libc::setsid();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::args::PlannedCommand;
    use std::time::Duration;
    use tokio::sync::{mpsc, watch};

    #[tokio::test]
    async fn streams_echo_lines_and_succeeds() {
        let (line_tx, mut line_rx) = mpsc::unbounded_channel();
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let planned = PlannedCommand {
            program: "echo".into(),
            args: vec!["hello-tako".into()],
            cwd: None,
        };
        let (ok, summary) = spawn_and_stream(SpawnOpts {
            planned,
            job_id: 1,
            cancel_rx,
            line_tx,
        })
        .await;
        assert!(ok, "summary={summary}");
        // Collect lines with a short timeout.
        let mut lines = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_millis(200);
        while tokio::time::Instant::now() < deadline {
            match line_rx.try_recv() {
                Ok(l) => lines.push(l),
                Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
        assert!(
            lines.iter().any(|l| l.contains("hello-tako")),
            "lines={lines:?}"
        );
    }

    #[tokio::test]
    async fn cancel_kills_long_running_sleep() {
        let (line_tx, _line_rx) = mpsc::unbounded_channel();
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let planned = PlannedCommand {
            program: "sleep".into(),
            args: vec!["30".into()],
            cwd: None,
        };
        let handle = tokio::spawn(async move {
            spawn_and_stream(SpawnOpts {
                planned,
                job_id: 2,
                cancel_rx,
                line_tx,
            })
            .await
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        let _ = cancel_tx.send(true);
        let (ok, summary) = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("join timeout")
            .expect("task join");
        assert!(!ok);
        assert_eq!(summary, "cancelled");
    }
}
