//! Windows Firewall awareness (RELAY.md §11). The first-run "allow this
//! app" prompt writes an Allow rule for the Private profile and a Block
//! rule for the Public one; the WSL virtual adapter and most Wi-Fi are
//! Public, so a WSL node or a LAN peer dialing this node times out while
//! everything looks fine locally. We ask the firewall once per process
//! whether such a Block rule names our executable, and say so in `aspen
//! status` and in the advertised hint the Mesh panel shows.

use std::sync::OnceLock;

/// Profiles on which an inbound Block rule names this executable
/// (empty off Windows, or when there is none).
pub fn block_profiles() -> &'static [String] {
    static CELL: OnceLock<Vec<String>> = OnceLock::new();
    CELL.get_or_init(probe)
}

#[cfg(windows)]
fn probe() -> Vec<String> {
    let exe = std::env::current_exe().map(|p| p.to_string_lossy().to_lowercase()).unwrap_or_default();
    if exe.is_empty() {
        return Vec::new();
    }
    let script = "Get-NetFirewallRule -Direction Inbound -Action Block -Enabled True -ErrorAction SilentlyContinue | ForEach-Object { $a = $_ | Get-NetFirewallApplicationFilter; if ($a.Program) { $_.Profile.ToString() + '|' + $a.Program } }";
    let Ok(out) = aspen_core::quiet_command("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
    else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut profiles: Vec<String> = text
        .lines()
        .filter_map(|l| l.split_once('|'))
        .filter(|(_, prog)| prog.trim().to_lowercase() == exe)
        .map(|(p, _)| p.trim().to_owned())
        .collect();
    profiles.sort();
    profiles.dedup();
    profiles
}

#[cfg(not(windows))]
fn probe() -> Vec<String> {
    Vec::new()
}

/// The fix, as one PowerShell line.
pub fn fix_command(port: u16) -> String {
    format!("New-NetFirewallRule -DisplayName \"Aspen node {port}\" -Direction Inbound -Protocol TCP -LocalPort {port} -Action Allow -Profile Any")
}
