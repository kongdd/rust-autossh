# rust-autossh

`rust-autossh` supervises one or more OpenSSH port-forwarding processes. It does **not** implement SSH: it starts the system `ssh` (`ssh.exe` on Windows), detects its exit, and restarts it with bounded exponential backoff.

## Features

- Multiple `-R`, `-L`, and `-D` (SOCKS proxy) tunnels from one TOML configuration;
- `BatchMode`, `ExitOnForwardFailure`, connect timeout, and SSH-layer keepalives by default;
- Retry backoff with reset after a stable connection;
- Configuration hot reload every two seconds: saving a valid TOML file restarts connections; an invalid update keeps the current set alive;
- Optional log file, replaced on each program start;
- Foreground operation on Linux and Windows; Windows Service installation commands on Windows.

## Build

```bash
cargo build --release
# Cross-compile from Linux only after installing a Windows target and linker:
# cargo build --release --target x86_64-pc-windows-gnu
```

On Windows, install [OpenSSH Client](https://learn.microsoft.com/windows-server/administration/openssh/openssh_install_firstuse) if `ssh.exe` is absent.

## Configuration

Copy and edit [`config.example.toml`](config.example.toml), then validate it. The default location is `$HOME/.config/autossh/config.toml` on every platform (Windows resolves `%USERPROFILE%` when `HOME` is unset):

```powershell
.\rust-autossh.exe check --config %USERPROFILE%\.config\autossh\config.toml
.\rust-autossh.exe run --config %USERPROFILE%\.config\autossh\config.toml
```

Each `[[connections]]` block creates one `ssh` process. `name` is its unique log identifier; optional `host` is the SSH destination (host/IP/alias) and defaults to `name` for backward compatibility. Its `forwards` array may mix any number of `-L`, `-R`, and `-D` (local SOCKS proxy) mappings.

For `-L` and `-R` the spec is `[bind:]listen_port:target_host:target_port`; the format is identical — what changes is *which side* listens and *which side* owns the target: `-L` listens on the client and targets the server; `-R` listens on the server and targets the client. For `-D` the spec is `[bind:]port` only — a local SOCKS proxy whose target is chosen by the SOCKS client per request.

SSH host aliases and all ordinary settings in `%USERPROFILE%\.ssh\config` work unchanged. Configure key-based authentication or `ssh-agent`; password prompts are disabled by `BatchMode=yes`.

`extra_args` is an array of individual arguments, not one shell command. For example:

```toml
extra_args = ["-i", "C:\\Users\\alice\\.ssh\\id_ed25519", "-o", "StrictHostKeyChecking=yes"]
```

## Windows Service

Open an **elevated** PowerShell. Service-mode runs as `LocalSystem` whose `%USERPROFILE%` is `C:\Windows\System32\config\systemprofile`, so put the configuration somewhere reachable by the service account and pass it explicitly. Service registration and management are delegated to the system `sc.exe` through [`scripts/service.ps1`](scripts/service.ps1):

```powershell
mkdir C:\ProgramData\autossh
copy .\config.example.toml C:\ProgramData\autossh\config.toml
.\scripts\service.ps1 install -Exe .\rust-autossh.exe -Config C:\ProgramData\autossh\config.toml
.\scripts\service.ps1 start
.\scripts\service.ps1 status
```

The service is automatic and runs as `LocalSystem` by default. This account has a different home directory from your user, therefore user-specific `%USERPROFILE%\.ssh` keys and `ssh-agent` are normally unavailable. For production, either configure the service **Log On** account to the account owning the key, or use an explicit locked-down `ssh_path`/`extra_args` and key readable by the service account.

```powershell
.\scripts\service.ps1 stop
.\scripts\service.ps1 restart
.\scripts\service.ps1 disable  # Disable automatic startup.
.\scripts\service.ps1 enable   # Enable automatic startup.
.\scripts\service.ps1 uninstall
```

The Windows Service Control Manager does not restart a service that remains alive while it reconnects. `rust-autossh` performs those tunnel restarts itself. Optionally configure SCM recovery separately for a program crash:

```powershell
sc.exe failure rust-autossh reset= 86400 actions= restart/5000/restart/5000/restart/5000
```

## Operational notes

- Config content is checked every two seconds. A reload restarts only changed connections; changing log settings restarts all connections. Apply changes atomically (write a temporary file, then rename it) to avoid a transient syntax error.
- `ExitOnForwardFailure=yes` is especially important for detecting a failed initial forwarding request.
- A remote forward may still require `GatewayPorts` / `AllowTcpForwarding` on the SSH server.
- Tunnel stdout is discarded. Supervisor events are written to stderr and, when configured, to a log file that is replaced on each program start. SSH stderr is captured; OpenSSH DEBUG1 forwarding traces are used internally to annotate `channel ... open failed` messages with the matching `-L`/`-R` listen port, target, and originator where available. A service startup failure is additionally written to `rust-autossh.service-error.log` beside config.
