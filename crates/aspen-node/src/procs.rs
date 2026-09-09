//! The process tree under a session (PROPOSALS-MCP.md §6.4): what a
//! harness's background tasks and a plugin's hook-launched monitors look
//! like from the outside — a child of the session's process with a
//! command line. Linux/WSL read `/proc`; Windows asks CIM. Used to list
//! them and to stop one; inference, not a contract, and labelled so.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Proc {
    pub pid: u32,
    pub parent: u32,
    pub cmdline: String,
    /// Seconds since the process started, when the platform says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub age_secs: Option<u64>,
}

/// Every descendant of `root`, breadth-first.
pub fn descendants(root: u32) -> Vec<Proc> {
    let all = list_all();
    let mut out = Vec::new();
    let mut frontier = vec![root];
    let mut seen = std::collections::HashSet::new();
    while let Some(p) = frontier.pop() {
        for c in all.iter().filter(|c| c.parent == p) {
            if seen.insert(c.pid) {
                out.push(c.clone());
                frontier.push(c.pid);
            }
        }
    }
    out
}

#[cfg(target_os = "linux")]
fn list_all() -> Vec<Proc> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir("/proc") else { return out };
    let uptime = std::fs::read_to_string("/proc/uptime").ok().and_then(|s| s.split_whitespace().next()?.parse::<f64>().ok());
    let hz = 100.0;
    for e in rd.flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else { continue };
        let Ok(stat) = std::fs::read_to_string(e.path().join("stat")) else { continue };
        // "pid (comm) state ppid ..." — comm may contain spaces/parens.
        let Some(close) = stat.rfind(')') else { continue };
        let rest: Vec<&str> = stat[close + 1..].split_whitespace().collect();
        let Some(parent) = rest.get(1).and_then(|p| p.parse::<u32>().ok()) else { continue };
        let start_ticks = rest.get(19).and_then(|s| s.parse::<f64>().ok());
        let cmdline = std::fs::read(e.path().join("cmdline"))
            .map(|b| b.split(|c| *c == 0).filter(|p| !p.is_empty()).map(|p| String::from_utf8_lossy(p).into_owned()).collect::<Vec<_>>().join(" "))
            .unwrap_or_default();
        if cmdline.is_empty() {
            continue; // kernel threads
        }
        let age_secs = match (uptime, start_ticks) {
            (Some(u), Some(st)) => Some((u - st / hz).max(0.0) as u64),
            _ => None,
        };
        out.push(Proc { pid, parent, cmdline, age_secs });
    }
    out
}

#[cfg(windows)]
fn list_all() -> Vec<Proc> {
    let script = "Get-CimInstance Win32_Process | ForEach-Object { $age = if ($_.CreationDate) { [int]((Get-Date) - $_.CreationDate).TotalSeconds } else { -1 }; \"$($_.ProcessId)|$($_.ParentProcessId)|$age|$($_.CommandLine)\" }";
    let Ok(out) = aspen_core::quiet_command("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut it = l.splitn(4, '|');
            let pid = it.next()?.trim().parse().ok()?;
            let parent = it.next()?.trim().parse().ok()?;
            let age: i64 = it.next()?.trim().parse().ok()?;
            let cmdline = it.next().unwrap_or("").trim().to_owned();
            if cmdline.is_empty() {
                return None;
            }
            Some(Proc { pid, parent, cmdline, age_secs: (age >= 0).then_some(age as u64) })
        })
        .collect()
}

#[cfg(not(any(target_os = "linux", windows)))]
fn list_all() -> Vec<Proc> {
    let Ok(out) = std::process::Command::new("ps").args(["-axo", "pid=,ppid=,etimes=,args="]).output() else { return Vec::new() };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let pid = it.next()?.parse().ok()?;
            let parent = it.next()?.parse().ok()?;
            let age = it.next()?.parse().ok();
            let cmdline = it.collect::<Vec<_>>().join(" ");
            Some(Proc { pid, parent, cmdline, age_secs: age })
        })
        .collect()
}

/// Terminate a process (and, on Unix, its own children): SIGTERM, then
/// SIGKILL after a grace period; Windows `taskkill /T /F`.
pub fn terminate(pid: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let alive = |p: u32| std::path::Path::new(&format!("/proc/{p}")).exists();
        for c in descendants(pid) {
            let _ = std::process::Command::new("kill").args(["-TERM", &c.pid.to_string()]).status();
        }
        // The parent (a shell wrapper) often exits on its own once its
        // child is gone; a missing process is the outcome we wanted.
        let _ = std::process::Command::new("kill").args(["-TERM", &pid.to_string()]).status();
        std::thread::sleep(std::time::Duration::from_millis(1500));
        if alive(pid) {
            let _ = std::process::Command::new("kill").args(["-KILL", &pid.to_string()]).status();
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
        if alive(pid) {
            return Err(std::io::Error::other(format!("process {pid} is still running after SIGKILL")));
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        let st = aspen_core::quiet_command("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]).status()?;
        if st.success() { Ok(()) } else { Err(std::io::Error::other(format!("taskkill {pid} failed"))) }
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err(std::io::Error::other("not supported on this platform"))
    }
}

/// A loose match between a ledger row's script and a process's command
/// line: the script's first meaningful token and a long enough shared
/// substring. Shell wrappers (`bash -c '…'`) carry the script verbatim.
pub fn matches_script(cmdline: &str, script: &str) -> bool {
    let s = script.trim();
    if s.is_empty() {
        return false;
    }
    if cmdline.contains(s) {
        return true;
    }
    // The first 60 chars, if distinctive.
    let head: String = s.chars().take(60).collect();
    head.len() >= 12 && cmdline.contains(head.trim())
}
