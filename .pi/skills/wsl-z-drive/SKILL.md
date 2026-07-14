---
name: wsl-z-drive
description: WSL bash 下访问 Z 盘 Windows 项目的路径与 cmd.exe 规范
---

# WSL 下操作 Z 盘项目

## 一句话原则

> **bash 走 `/mnt/z`；read/edit/write 走 `Z:/`；Windows 原生命令一律 `cmd.exe /c 'cd /d Z:\... && ...'`，输出先落盘再 grep。**

---

## 1. 路径双轨

| 工具 | 路径 | 例 |
|---|---|---|
| WSL bash | `/mnt/z/...` | `cat /mnt/z/GitHub/kongdd/autossh/Cargo.toml` |
| `read`/`edit`/`write` | `Z:/...` | `read Z:/GitHub/kongdd/autossh/Cargo.toml` |

常见错：`ls Z:/...`（bash 不认）、`read /mnt/z/...`（read 不认）、`cd /z/...`（挂载根在 `/mnt/<letter>`，默认 `/mnt/z`）。

---

## 2. cmd.exe 三条铁律

1. **单引号**：bash 会把双引号里的 `/c` 转义成 `//c`，整串被当成启动消息。
2. **显式 `cd /d`**：cmd 启动 cwd 是 `\\wsl.localhost\...`，不支持 UNC，会落到 `C:\Windows`。
3. **输出落盘**：cmd 在主输出前会 echo 4 行诊断（wsl.exe 1 行 + cmd UNC 抱怨 3 行），污染管道。

```bash
# 错
cmd.exe /c "cd /d Z:\GitHub\kongdd\autossh && cargo build"
cmd.exe //c "cargo --version"
cmd.exe /c 'cargo build' | grep -E "^error"          # grep 不到，被启动消息挤到后面
cmd.exe /c 'dir | findstr foo'                       # findstr 不在 bash PATH

# 对
cmd.exe /c 'cd /d Z:\GitHub\kongdd\autossh && cargo build 2>&1' > /tmp/build.txt 2>&1
grep -E "^error" /tmp/build.txt
cmd.exe /c 'dir /B /AD C:\path' > /tmp/out.txt 2>&1
grep foo /tmp/out.txt
```

---

## 3. 常用 recipe

```bash
# cargo build / check
cmd.exe /c 'cd /d Z:\GitHub\kongdd\autossh && cargo build 2>&1' > /tmp/build.txt 2>&1
cmd.exe /c 'cd /d Z:\GitHub\kongdd\autossh && cargo check -p friday 2>&1' > /tmp/check.txt 2>&1

# 任意 Windows 工具（git/node/python…）：换命令名，模式不变
cmd.exe /c 'cd /d Z:\path\to\project && git status 2>&1' > /tmp/git.txt 2>&1

# cargo 源码缓存（**在 Windows 的** `C:\Users\<user>\.cargo\registry\src\`）
cmd.exe /c 'dir /B /AD C:\Users\hydro\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f' > /tmp/src.txt 2>&1
grep -E "tiny_http|socket2" /tmp/src.txt
cmd.exe /c 'type C:\Users\hydro\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\tiny_http-0.12.0\src\lib.rs' > /tmp/lib.txt 2>&1
```

---

## 4. 自检

```bash
ls /mnt/z/GitHub/kongdd/autossh/Cargo.toml      # 应存在
ls /mnt/z 2>/dev/null; ls /z 2>/dev/null       # 都应 fail
cmd.exe /c 'cargo --version'                   # 应输出版本
cmd.exe /c 'echo hello' > /tmp/x.txt 2>&1      # 文件应只有 "hello"
cat /tmp/x.txt
```

---

## 坑速查

| # | 症状 | 真因 | 修法 |
|---|---|---|---|
| 1 | `ls Z:/...` → `No such file or directory` | bash 不认 Windows 路径 | bash 用 `/mnt/z/...` |
| 2 | `cd /z/...` 失败 | 默认挂载根是 `/mnt/<letter>` | 用 `/mnt/z/...` |
| 3 | `read /mnt/z/...` ENOENT | read 只认 Windows 路径 | read/edit/write 用 `Z:/...` |
| 4 | `cmd.exe /c "..."` 命令不执行 | bash 把 `/c` 转义成 `//c` | 用单引号 `cmd.exe /c '...'` |
| 5 | `cmd ... | grep` grep 不到 | cmd echo 4 行启动消息污染管道 | `> /tmp/out.txt 2>&1` 落盘再 grep |
| 6 | `cmd ... | findstr ...` `command not found` | bash 把 findstr 当本地命令 | 用 bash 的 grep/awk，cmd 只产文本 |
| 7 | cargo 报 `could not find Cargo.toml` | cmd 启动 cwd 落到 `C:\Windows` | `cmd.exe /c 'cd /d Z:\path && ...'` |
| 8 | cargo 报 `Access is denied` / `failed to load cargo config` | WSL/NTFS 权限边界不一致 | 不从 WSL 写 `.cargo/`，全部走 `cmd.exe /c cargo ...` |