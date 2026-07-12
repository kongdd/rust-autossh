**Rust 很适合在 Windows 上实现一个原生版 autossh**。

我更推荐的方案不是用 Rust 重写 SSH 协议，而是：

> **Rust 负责守护、健康检查、重连和 Windows Service；实际隧道仍调用 Windows 自带的** `ssh.exe`**。**

Windows 10/11 和 Windows Server 已支持 OpenSSH Client；`autossh` 的本质也是启动并监控 SSH 进程，在连接死亡或无法传输数据时重新启动。

## 推荐架构

```text
Windows Service
└── rust-autossh.exe
    ├── 启动 ssh.exe
    │   └── ssh -N -T -R/-L ...
    ├── 监听子进程退出
    ├── 检测本地/远程端口
    ├── 指数退避重连
    ├── 保存日志
    └── 响应 Windows 服务停止信号

```

例如反向映射：

```powershell
ssh.exe `
  -N `
  -T `
  -o BatchMode=yes `
  -o ExitOnForwardFailure=yes `
  -o ServerAliveInterval=30 `
  -o ServerAliveCountMax=3 `
  -R 10022:127.0.0.1:22 `
  user@example.com

```

关键参数：


| 参数                       | 作用                       |
| -------------------------- | -------------------------- |
| `-N`                       | 不执行远程命令，只做转发   |
| `-T`                       | 不分配终端                 |
| `BatchMode=yes`            | 禁止弹出密码交互           |
| `ExitOnForwardFailure=yes` | 端口映射创建失败时立即退出 |
| `ServerAliveInterval=30`   | 每 30 秒发送 SSH 层心跳    |
| `ServerAliveCountMax=3`    | 连续 3 次无响应则退出      |
| `-R`                       | 反向端口映射               |
| `-L`                       | 本地端口映射               |


只要 Rust 守护程序检测到 `ssh.exe` 退出，再重新启动即可。多数情况下甚至不必实现 autossh 原版的双端口回环检测。

## Rust 最小实现

```rust
use std::{
    process::{Child, Command, Stdio},
    thread,
    time::Duration,
};

const SSH_HOST: &str = "user@example.com";

fn spawn_ssh() -> std::io::Result<Child> {
    Command::new("C:\\Windows\\System32\\OpenSSH\\ssh.exe")
        .args([
            "-N",
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "ExitOnForwardFailure=yes",
            "-o",
            "ConnectTimeout=15",
            "-o",
            "ServerAliveInterval=30",
            "-o",
            "ServerAliveCountMax=3",
            "-R",
            "10022:127.0.0.1:22",
            SSH_HOST,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
}

fn main() {
    let mut retry_secs = 1_u64;

    loop {
        eprintln!("starting ssh tunnel...");

        match spawn_ssh() {
            Ok(mut child) => {
                match child.wait() {
                    Ok(status) => {
                        eprintln!("ssh exited: {status}");
                    }
                    Err(error) => {
                        eprintln!("failed waiting for ssh: {error}");
                        let _ = child.kill();
                    }
                }
            }
            Err(error) => {
                eprintln!("failed to start ssh: {error}");
            }
        }

        eprintln!("reconnecting in {retry_secs}s");
        thread::sleep(Duration::from_secs(retry_secs));

        retry_secs = (retry_secs * 2).min(60);
    }
}

```

`Cargo.toml` 不需要第三方依赖：

```toml
[package]
name = "rust-autossh"
version = "0.1.0"
edition = "2024"

[dependencies]

```

这个版本已经能够：

- SSH 断开后自动重启；
- 网络中断后自动恢复；
- 映射创建失败时退出并重试；
- 避免无限高速重启；
- 只生成一个很小的 exe。

不过还缺少一个细节：**连接成功稳定运行一段时间后，应将退避时间重置为 1 秒**。可以记录进程运行时长，例如运行超过 60 秒就认为连接曾经稳定。

## Windows Service

可以使用 `windows-service` crate，将守护程序注册成真正的 Windows 服务。该 crate 专门提供 Windows Service 的实现和管理接口。([Docs.rs](http://Docs.rs))

```toml
[dependencies]
windows-service = "0.8"

```

安装后可实现：

```powershell
rust-autossh.exe install
rust-autossh.exe start
rust-autossh.exe stop
rust-autossh.exe uninstall

```

相比“任务计划程序”，Windows Service 更适合你的场景：

- 开机即可运行，不需要用户登录；
- Service Control Manager 可在程序异常退出后自动重启；
- 可以正确处理停止、关机事件；
- 日志和配置更容易统一管理。



调用系统 `ssh.exe` 可以天然复用：

```text
%USERPROFILE%\.ssh\config
%USERPROFILE%\.ssh\known_hosts
%USERPROFILE%\.ssh\id_ed25519
Windows OpenSSH Authentication Agent

```

所以最佳分工是：

```text
OpenSSH：负责可靠、安全、兼容的 SSH 连接
Rust：负责进程监督、配置管理、日志和 Windows 服务

```

## 建议做成的配置格式

```toml
[[tunnels]]
name = "server-reverse-ssh"
host = "myserver"
forward = "10022:127.0.0.1:22"
mode = "remote"
enabled = true

[tunnels.keepalive]
interval = 30
count_max = 3

[tunnels.retry]
initial_seconds = 1
maximum_seconds = 60
stable_seconds = 60

```

然后生成：

```powershell
ssh.exe -N -T `
  -o BatchMode=yes `
  -o ExitOnForwardFailure=yes `
  -o ServerAliveInterval=30 `
  -o ServerAliveCountMax=3 `
  -R 10022:127.0.0.1:22 `
  myserver

```

完成多隧道、自动重连、配置热加载、日志记录和 Windows Service；无需自己实现 SSH 协议。
