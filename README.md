# rust-autossh

`rust-autossh` supervises one or more OpenSSH port-forwarding processes. It does **not** implement SSH: it starts the system `ssh` (`ssh.exe` on Windows), detects its exit, and restarts it with bounded exponential backoff.

## Features

- Multiple `-R` and `-L` tunnels from one TOML configuration;
- `BatchMode`, `ExitOnForwardFailure`, connect timeout, and SSH-layer keepalives by default;
- Retry backoff with reset after a stable connection;
- Configuration hot reload every two seconds: saving a valid TOML file restarts connections; an invalid update keeps the current set alive;
- Optional rotating log file;
- Foreground operation on Linux and Windows; Windows Service installation commands on Windows.

## Build

```bash
cargo build --release
# Cross-compile from Linux only after installing a Windows target and linker:
# cargo build --release --target x86_64-pc-windows-gnu
```

On Windows, install [OpenSSH Client](https://learn.microsoft.com/windows-server/administration/openssh/openssh_install_firstuse) if `ssh.exe` is absent.

## Configuration

Copy and edit [`config.example.toml`](config.example.toml), then validate it:

```powershell
.\rust-autossh.exe check --config C:\ProgramData\rust-autossh\config.toml
.\rust-autossh.exe run --config C:\ProgramData\rust-autossh\config.toml
```

Each `[[connections]]` block creates one `ssh` process; `name` doubles as both the log identifier and the SSH destination (host/IP/alias). Its `forwards` array may mix any number of `-L` and `-R` mappings. SSH host aliases and all ordinary settings in `%USERPROFILE%\.ssh\config` work unchanged. Configure key-based authentication or `ssh-agent`; password prompts are disabled by `BatchMode=yes`.

`extra_args` is an array of individual arguments, not one shell command. For example:

```toml
extra_args = ["-i", "C:\\Users\\alice\\.ssh\\id_ed25519", "-o", "StrictHostKeyChecking=yes"]
```

## Windows Service

Open an **elevated** PowerShell. Keep the configuration in a system-readable protected directory, validate it, then install and start the service:

```powershell
.\rust-autossh.exe check --config C:\ProgramData\rust-autossh\config.toml
.\rust-autossh.exe install --config C:\ProgramData\rust-autossh\config.toml
.\rust-autossh.exe start
```

The service is automatic and runs as `LocalSystem` by default. This account has a different home directory from your user, therefore user-specific `%USERPROFILE%\.ssh` keys and `ssh-agent` are normally unavailable. For production, either configure the service **Log On** account to the account owning the key, or use an explicit locked-down `ssh_path`/`extra_args` and key readable by the service account.

```powershell
.\rust-autossh.exe stop
.\rust-autossh.exe uninstall
```

The Windows Service Control Manager does not restart a service that remains alive while it reconnects. `rust-autossh` performs those tunnel restarts itself. Optionally configure SCM recovery separately for a program crash:

```powershell
sc.exe failure rust-autossh reset= 86400 actions= restart/5000/restart/5000/restart/5000
```

## Operational notes

- Config content is checked every two seconds. A reload stops old `ssh` children and starts latest valid connection definitions. Apply changes atomically (write a temporary file, then rename it) to avoid a transient syntax error.
- `ExitOnForwardFailure=yes` is especially important for detecting a failed initial forwarding request.
- A remote forward may still require `GatewayPorts` / `AllowTcpForwarding` on the SSH server.
- Tunnel stdout/stderr are discarded. Supervisor events are written to stderr and, when configured, to the rotating log file. A service startup failure is additionally written to `rust-autossh.service-error.log` beside config.
