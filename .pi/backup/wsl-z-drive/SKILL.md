---
name: wsl-z-drive
description: WSL bash 下访问 Z 盘 Windows 项目，并通过 pwsh 调用 Windows 原生命令
---

# WSL 下操作 Z 盘项目

> **前提**：本文假定用 **pwsh 7+**（`pwsh.exe`）。

## 一句话原则

> **bash 走 `/mnt/z`；read/edit/write 走 `Z:/`；Windows 原生命令一律 `pwsh.exe -NoProfile -Command '...'`，输出先落盘再 grep。**

---

## 1. 路径双轨

| 工具 | 路径 | 例 |
|---|---|---|
| WSL bash | `/mnt/z/...` | `cat /mnt/z/GitHub/kongdd/autossh/Cargo.toml` |
| `read`/`edit`/`write` | `Z:/...` | `read Z:/GitHub/kongdd/autossh/Cargo.toml` |

常见错：bash 用 `Z:/...`、read 用 `/mnt/z/...`、`cd /z/...`（默认挂载根 `/mnt/<letter>`，即 `/mnt/z`）。

---

## 2. pwsh 四条铁律

1. **必须 `-NoProfile`**：否则 `profile.ps1` 里的 `Write-Host` 污染管道，跟 cmd 那 4 行杂音同类。
2. **单引号包 `-Command`**：bash 双引号会展开 `$`，把 PowerShell 变量（如 `$env:Path`）吞成空串。
3. **无需 `cd /d`**：pwsh 原生支持 UNC（`\\wsl$\Ubuntu\...`），启动 cwd 不会落到 `C:\Windows`。
4. **输出落盘**：grep 前 `> /tmp/out.txt 2>&1`，防 profile 杂音 / 缓冲切断。

```bash
# 错
pwsh.exe -Command '...'                                            # 缺 -NoProfile
pwsh.exe -NoProfile -Command "Set-Location Z:\foo; cargo build"   # 若命令里有 $ 会被 bash 吞
pwsh.exe -NoProfile -Command 'cargo build' | grep "^error"         # grep 拿不到，应先落盘

# 对
pwsh.exe -NoProfile -Command 'Set-Location Z:\foo; cargo build 2>&1' > /tmp/build.txt 2>&1
grep -E "^error" /tmp/build.txt
```

---

## 3. 常用 recipe

```bash
# cargo build / check
pwsh.exe -NoProfile -Command 'Set-Location Z:\GitHub\kongdd\autossh; cargo build 2>&1' > /tmp/build.txt 2>&1
pwsh.exe -NoProfile -Command 'Set-Location Z:\GitHub\kongdd\autossh; cargo check -p friday 2>&1' > /tmp/check.txt 2>&1

# 任意 Windows 工具（git/node/python…）：换命令名，模式不变
pwsh.exe -NoProfile -Command 'Set-Location Z:\path\to\project; git status 2>&1' > /tmp/git.txt 2>&1

# cargo 源码缓存（**在 Windows 的** C:\Users\<user>\.cargo\registry\src\）
pwsh.exe -NoProfile -Command \
  'Get-ChildItem -Directory C:\Users\hydro\.cargo\registry\src | Select-Object -ExpandProperty Name' \
  > /tmp/idx.txt 2>&1
cat /tmp/idx.txt

pwsh.exe -NoProfile -Command \
  'Get-ChildItem -Directory C:\Users\hydro\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f | Select-Object -ExpandProperty Name' \
  > /tmp/src.txt 2>&1
grep -E "tiny_http|socket2" /tmp/src.txt

pwsh.exe -NoProfile -Command \
  'Get-Content C:\Users\hydro\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\tiny_http-0.12.0\src\lib.rs' \
  > /tmp/lib.txt 2>&1
sed -n '120,250p' /tmp/lib.txt
```

---

## 4. 自检

```bash
ls /mnt/z/GitHub/kongdd/autossh/Cargo.toml        # 应存在
ls /mnt/z 2>/dev/null; ls /z 2>/dev/null         # 都应 fail
pwsh.exe -NoProfile -Command 'cargo --version'    # 应输出版本
pwsh.exe -NoProfile -Command 'echo hello' > /tmp/x.txt 2>&1  # 文件应只有 "hello"
cat /tmp/x.txt
```

---

## 坑速查

| # | 症状 | 真因 | 修法 |
|---|---|---|---|
| 1 | `ls Z:/...` → `No such file or directory` | bash 不认 Windows 路径 | bash 用 `/mnt/z/...` |
| 2 | `cd /z/...` 失败 | 默认挂载根是 `/mnt/<letter>` | 用 `/mnt/z/...` |
| 3 | `read /mnt/z/...` ENOENT | read 只认 Windows 路径 | read/edit/write 用 `Z:/...` |
| 4 | `pwsh -Command "..."` 里 `$变量` 被吞成空串 | bash 双引号触发 `$` 展开 | 用单引号 `pwsh -Command '...'` |
| 5 | `pwsh ... \| grep` grep 不到 | `profile.ps1` 噪声 / 没落盘 | 加 `-NoProfile` + `> /tmp/out.txt 2>&1` |
| 6 | pwsh 输出 grep 拿到乱码 | powershell.exe 5.1 默认 UTF-16 LE | 装 pwsh 7+；或 `powershell.exe -Command '$OutputEncoding = [System.Text.Encoding]::UTF8; ...'` |
| 7 | cargo 报 `could not find Cargo.toml` | pwsh cwd 没切到项目根 | `Set-Location Z:\path; cargo ...` |
| 8 | cargo 报 `Access is denied` / `failed to load cargo config` | WSL/NTFS 权限边界不一致 | 不从 WSL 写 `.cargo/`，全部走 `pwsh.exe -Command 'cargo ...'` |
