---
name: autossh-ui-preview
description: Build、启动并截图 autossh 的原生 egui UI；使用 Xvfb 提供虚拟显示器，使用 xdotool 点击、双击和 `xwd` 截图验证交互。适用于检查布局、按钮、弹窗、Start All/Stop All、SSH hosts 和 connection 双击编辑。
---

# autossh UI 截图与交互

Rust `eframe/egui` 原生桌面程序。不用浏览器 / Playwright。流程：**cargo build → xvfb + app via `process start` → `xwd + convert PNG24:` → `images/`**。

先看最后一节「[踩坑速查](#Qsnapshot-先看)」。其余章节按需翻。

## 1. 构建

```bash
cargo build -p autossh-ui
```

产物：`target/debug/autossh-ui`、`target/debug/rust-autossh`。长构建用 `process start`，不要 `&`。

## 2. 隔离配置

不用真实 `~/.ssh/`；建临时 HOME、关闭连接：

```bash
preview=/tmp/autossh-ui-preview
rm -rf "$preview" && mkdir -p "$preview/home/.ssh"
cat > "$preview/home/.ssh/config" <<'EOF'
Host demo-server
    HostName demo.example.com
    User demo
    Port 2222
Host backup-server
    HostName backup.example.com
    User backup
EOF
cat > "$preview/config.toml" <<'EOF'
[[connections]]
name = "demo-server"
host = "demo.example.com"
enabled = false
forwards = [{ mode = "local", forward = "8080:127.0.0.1:80" }]
[[connections]]
name = "backup-server"
host = "backup.example.com"
enabled = false
forwards = [{ mode = "remote", forward = "10022:127.0.0.1:22" }]
EOF
```

注意：每个 connection 至少一个 forward；`enabled=false` 防真连。

## 3. 启动 + 截图（canonical）

Xvfb 与应用拆两个 `process start`，强制 X11 后端，截图走 `xwd`：

```text
process start name=xvfb-server   command="Xvfb :77 -screen 0 1024x720x24 +extension RANDR +extension RENDER -ac"
process start name=autossh-ui    command="env WAYLAND_DISPLAY= DISPLAY=:77 XDG_SESSION_TYPE=x11 WINIT_UNIX_BACKEND=x11 HOME=/tmp/autossh-ui-preview/home /abs/path/to/autossh-ui --config /tmp/autossh-ui-preview/config.toml"
```

截图：

```bash
DISPLAY=:77 xwd -root -out /tmp/raw.xwd
convert /tmp/raw.xwd PNG24:/abs/path/to/images/autossh-ui-preview.png
identify /abs/path/to/images/autossh-ui-preview.png
```

最后一行应输出 `8-bit/color RGB`；输出 `1-bit grayscale` 就回坑速查 1。

回复里引用图片：

```markdown
![autossh UI](images/autossh-ui-preview.png)
```

清理：

```bash
process kill id=autossh-ui
process kill id=xvfb-server
```

## 4. 交互坐标

工具依赖：`xdotool`、`xdotool search --name rust-autossh`、`xdotool search --name rust-autossh getwindowgeometry --shell`。窗口默认 1024×720、原点 (0,0)。

| 元素 | 位置 |
|---|---|
| Connections 面板 | `x ∈ [0,512)` |
| 标题栏 + SSH hosts 按钮 | `y ∈ [30,60]`，SSH `x ≈ 350..430` |
| connection 名称 | `y ≈ 65..100`，`x ≈ 60..140` |
| connection 详情行 | `y ≈ 105..140` |
| Start All / Stop All | `x ≈ 880..960`，`y ≈ 38`；Save 最右侧 |
| Logs 面板 | `y ≥ 500`，follow 右上角 |

点击：

```bash
xdotool mousemove 400 39 click 1
```

双击 connection 验证编辑弹窗：

```bash
xdotool mousemove 90 82 click --repeat 2 --delay 120 1
sleep 1
DISPLAY=:77 xwd -root -out /tmp/raw.xwd
convert /tmp/raw.xwd PNG24:images/autossh-ui-connection-edit.png
```

预期截图含 `Edit connection (...)` 弹窗。改完配置等 ~1s 再拍，避免抓到上一帧。

## 5. 交互验证

- Start All → 起 supervisor，按钮变 Stop All，状态 running。
- Stop All → 释放 supervisor handle，终止子 SSH 进程。
- follow 默认勾选；可取消、再勾回。
- SSH hosts Load 后显示临时 `~/.ssh/config` 的 host；名称随主题可读，不硬编白。
- connection 字段双击 → 编辑弹窗。

## 6. 卫生

- 不预览真实 SSH 地址 / 私钥 / 用户配置。
- 换版本前清理 X 端残留：`pkill -9 -f autossh-ui && DISPLAY=:77 xdotool search --name rust-autossh getwindowpid %@ 2>/dev/null | xargs -r kill -9`。
- 残留多窗口会让截图拍到旧版本。

---

## 坑 snapshot（先看）

5 条症状 → 真因 → 修法。表格序号是子节错交叉引用错（例：「回 §Q3」）。

| # | 症状 | 真因 | 修法 |
|---|---|---|---|
| 1 | 截图 `1-bit grayscale`，227 字节 | `import` 走 PNG8 | `xwd` + `convert PNG24:` |
| 2 | 进程活、截图全黑；`xwininfo` 显 `IsUnMapped` / `1x1` | eframe 0.29 默认 Wayland，Xvfb 只 X11 | 强制 X11 env |
| 3 | 下条命令发现 Xvfb / app 没了 | `bash &` 子进程随会话回收 | 全程 `process start` |
| 4 | `xdotool search` 返 ≥2 个窗口、截图拍到旧版 | 旧版窗口被 X server 持有 | 清 X 端残留 |
| 5 | 改 `visuals()` 一调到底、多处失谐 | 视觉未对齐、参数互相吃 | 一次只动 1~2 色阶 |

### §Q1 `import -window root` 1-bit 黑屏

本机 ImageMagick 7.1.x 默认 PNG8 编码 XWD dump，再彩也输出 227 字节 1-bit B/W。**永远走 `xwd + convert PNG24:`**：

```bash
DISPLAY=:77 xwd -root -out /tmp/raw.xwd
convert /tmp/raw.xwd PNG24:images/autossh-ui-preview.png
```

判定：`identify` 输出 `8-bit/color RGB` 正常；任何 `1-bit grayscale` 都本坑。

### §Q2 eframe 0.29 默认 Wayland

eframe/winit 在 Linux 默认 Wayland 后端，Xvfb 只 X11。进程活、窗口未映射、截图纯黑。强制 X11：

```bash
env WAYLAND_DISPLAY= \
    DISPLAY=:77 \
    XDG_SESSION_TYPE=x11 \
    WINIT_UNIX_BACKEND=x11 \
    /path/to/autossh-ui --config config.toml
```

判定：`xwininfo -tree -root` 看到 `Map State: IsUnMapped` 或窗口 1×1。

### §Q3 bash `&` 子进程会被会话回收

`bash &` 起的进程在 bash 命令返回后被回收，下条命令发现 Xvfb / app 没了。**X server 与 app 都用 `process start`**：

```text
process start name=xvfb-server   command="Xvfb :77 -screen 0 1024x720x24 +extension RANDR +extension RENDER -ac"
process start name=autossh-ui    command="env WAYLAND_DISPLAY= DISPLAY=:77 XDG_SESSION_TYPE=x11 WINIT_UNIX_BACKEND=x11 HOME=/tmp/autossh-ui-preview/home /abs/path/to/autossh-ui --config /tmp/autossh-ui-preview/config.toml"
```

查看 / 终止：`process list` / `process kill id=<name>`。崩溃自动通知，无需 poll。

### §Q4 X server 持有杀进程的窗口

`kill` 旧 `autossh-ui` 后，X server 仍持其 1×1 或全屏窗口。新版启动后 X 里叠两份，截图可能拍到旧。清理：

```bash
pkill -9 -f autossh-ui
sleep 1
DISPLAY=:77 xdotool search --name rust-autossh getwindowpid %@ 2>/dev/null \
  | xargs -r kill -9 2>/dev/null
```

判定：`xdotool search --name rust-autossh` 返 ≥2 ID、新旧 PID 都对应有窗口。


### §Q5 一次改太多色，`visuals()` 全面失谐

每改一档 `visuals()` → build → 截图 → 比对像素。一次只动 1~2 个色阶。下面的档位推荐 = 阶梯式亮度。

像素取样一行确认故障层：

```bash
identify -format "bg=%[fx:int(255*r)],%[fx:int(255*g)],%[fx:int(255*b)]\n" "out.png[1x1+600+450]"
```
