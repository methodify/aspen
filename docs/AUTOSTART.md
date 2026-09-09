# Auto-start: the daemon at login, as you

`aspen autostart enable | disable | status` (PROPOSALS-2026-09-C.md §3,
v0.25). The node runs the operator's repos with the operator's harness
credentials, so it starts under the operator's own login — never as a
system service, which would put that work under an identity that is not
theirs.

## 1. What is installed

| platform | mechanism | supervisor |
|---|---|---|
| Linux, WSL with systemd | `~/.config/systemd/user/aspen.service` (`XDG_CONFIG_HOME` honoured); `Type=simple`, the daemon in the foreground; `Restart=on-failure`; `KillMode=process`; `WantedBy=default.target` | yes |
| macOS | `~/Library/LaunchAgents/dev.aspen.aspen.plist`; `RunAtLoad`; `KeepAlive` on failure only; `AbandonProcessGroup` | yes |
| Windows | Scheduled Task "Aspen", at logon, as the user, limited rights; runs `conhost.exe --headless aspen up -d` so no window flashes; allowed on battery, no time limit | no |

A data dir other than the default gets a suffixed unit (`aspen-1a2b3c4d`),
so two nodes under one login do not collide. The unit's command line is
what the running daemon was asked for (`--listen`, `--headless`, `--ui`
from daemon.json) when one runs at enable time, else the configured
defaults (`aspen config listen | headless`), which `up` applies itself.

The unit carries `ASPEN_SUPERVISOR=systemd|launchd` and today's `PATH`:
the user manager's environment is not the login shell's, and the harness
binaries (`claude`, `codex`) usually live under `~/.local/bin` or a version
manager. Re-run `enable` after moving them.

The data dir keeps a marker, `autostart.json` (`kind`, `unit`, `path`,
`at`): the cheap answer for `GET /api/node` and the fleet views. The CLI's
`status` asks the platform (`systemctl --user is-enabled`, `launchctl
print`, `Get-ScheduledTask`) and corrects the marker if they disagree.

## 2. Enable, and the running daemon

`enable` installs the unit and brings the daemon under it. A daemon that
was started by hand is stopped (the normal ladder: sessions shut down
with their live marks kept) and started again by the supervisor, which
revives them — the same cycle as `aspen restart`. `enable --no-start`
installs only. On Windows there is no supervisor: the task is registered
and a running daemon is left alone; a daemon not running is started
through the task so what runs is what login will run.

`disable` removes the unit and stops nothing: the running daemon runs on
(under systemd/launchd until it exits on its own); the next login starts
nothing.

## 3. Supervised: down, restart, update

A daemon started by the supervisor writes `"supervisor"` into daemon.json.
The CLI reads it and routes around the supervisor rather than fighting it:

- `aspen down` → `systemctl --user stop` / `launchctl kill TERM`. The
  supervisor signals the daemon into the same ladder as before, and, being
  a clean exit, does not restart it. `Restart=on-failure` / `KeepAlive
  {SuccessfulExit: false}` mean a crash *is* restarted.
- `aspen restart` → stop through the supervisor, then `systemctl --user
  start` / `launchctl kickstart`. No re-spawn from the CLI: there would be
  two owners of one daemon, and the next `systemctl start` would fail on
  the bound port.
- `aspen update --restart` (and the unattended updater the daemon
  launches) → the same; the updater's restart parameters carry the
  supervisor. `KillMode=process` is what lets an updater the daemon
  launched outlive the daemon it is replacing.

A daemon started by hand while a unit exists is not supervised (its
daemon.json has no `supervisor`); `aspen status` and the Mesh chip say so,
and `aspen restart` brings it under.

## 4. Lingering, and a headless box

A `systemd --user` unit starts at the user's first login and stops at the
last logout. For a box that should run headless — a server, a WSL distro
that should carry a node whether or not a shell is open — enable
lingering: `loginctl enable-linger`. `status` says when it is off. WSL
needs `systemd=true` under `[boot]` in `/etc/wsl.conf` for any of this;
without it `enable` refuses and says so.

## 5. Surfaces

- `aspen status`: an `autostart:` line (on/off, mechanism, whether the
  running daemon is under it).
- `GET /api/node` carries `autostart {supported, kind, enabled, unit,
  path, supervised, supervisor}`; `GET /api/node/autostart?node=` asks a
  peer; `PUT /api/node/autostart {enabled, node?}` enables or disables —
  the work runs as a detached CLI (`aspen autostart enable`), since
  enabling restarts the daemon that would otherwise be running it, and
  the answer is `{started: true}`; poll the status.
- Mesh list view: an `autostart on|off` chip with a toggle on this node's
  row and on every linked peer's, with the restart consequence in its
  title before the click.

## 6. Verified (rig, 2026-09-09)

Linux/WSL: enable on a rig node with a non-default data dir wrote the
suffixed unit, stopped the hand-started daemon and started it under
`systemd --user`; daemon.json carried `"supervisor":"systemd"`; `aspen
restart` went through `systemctl`; `disable` removed the unit and left the
daemon running. macOS and Windows paths are written to the same shape
and not yet exercised on hardware.
