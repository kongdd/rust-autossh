---
name: autossh-ui-preview
description: Build、启动并截图 autossh 的原生 egui UI；使用 Xvfb 提供虚拟显示器，使用 xdotool 点击、双击和截图验证交互。适用于检查布局、按钮、弹窗、Start All/Stop All、SSH hosts 和 connection 双击编辑。
---

# autossh UI 截图与交互

本项目是 Rust `eframe/egui` 原生桌面程序，不是 Web 页面。不要使用浏览器或 Playwright；在无桌面环境中使用 `Xvfb + xdotool + ImageMagick import`。

## 1. 构建

在项目根目录执行：

```bash
cargo build -p autossh-ui
```

构建产物：

```text
target/debug/autossh-ui
target/debug/rust-autossh
```

长时间构建使用 `pi-processes` 的 `process start`，不要用 shell 后台符号 `&` 让构建脱离管理。

## 2. 准备隔离配置

不要使用用户真实的 `~/.ssh/config`，也不要让预览启动真实 SSH 连接。创建临时 HOME 和禁用连接：

```bash
preview=/tmp/autossh-ui-preview
rm -rf "$preview"
mkdir -p "$preview/home/.ssh" images

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

注意：每个 connection 至少要有一个 forward 才能通过配置校验；`enabled = false` 可避免 supervisor 预览时启动 SSH。

## 3. 启动并截图

使用 `xvfb-run` 启动虚拟显示器。窗口默认大小为 `1024x720`：

```bash
HOME="$preview/home" xvfb-run -a -s '-screen 0 1024x720x24' sh -c '
  "$PWD/target/debug/autossh-ui" --config "$1" \
    >/tmp/autossh-ui-preview/stdout.log \
    2>/tmp/autossh-ui-preview/stderr.log &
  pid=$!
  trap "kill $pid 2>/dev/null || true; wait $pid 2>/dev/null || true" EXIT
  sleep 2
  import -window root "$PWD/images/autossh-ui-preview.png"
' sh "$preview/config.toml"
```

截图必须放在 `images/`，并在回复中使用相对路径：

```markdown
![autossh UI](images/autossh-ui-preview.png)
```

## 4. 窗口定位与交互

先确认工具存在：

```bash
command -v xvfb-run
command -v xdotool
command -v import
```

查找窗口和几何信息：

```bash
xdotool search --name rust-autossh
xdotool search --name rust-autossh getwindowgeometry --shell
```

在当前 Xvfb 配置中窗口通常位于 `(0, 0)`，因此可以直接使用截图中的屏幕坐标。坐标应以最新截图为准，不要盲目依赖旧坐标。

### 常用交互坐标（1024×720 默认窗口）

- 左侧 `Connections` 面板：`x = 0..511`。
- 左侧标题栏约 `y = 30..60`，`SSH hosts` 按钮通常在 `x ≈ 350..430`。
- connection 第一行约 `y = 65..100`，名称区域约 `x = 60..140`。
- connection 第二行约 `y = 105..140`。
- 中央顶部右侧：`Start All` / `Stop All` 约在 `x ≈ 880..960, y ≈ 38`；`Save` 在最右侧。
- Logs 面板从约 `y = 500` 开始；`follow` 在右上角。

普通点击：

```bash
xdotool mousemove 400 39 click 1
```

双击 connection 名称验证编辑：

```bash
xdotool mousemove 90 82 click --repeat 2 --delay 120 1
sleep 1
import -window root "$PWD/images/autossh-ui-connection-edit.png"
```

验证成功时，截图中应出现 `Edit connection (...)` 窗口。

打开 SSH hosts 并加载：

```bash
xdotool mousemove 400 39 click 1
sleep 0.5
xdotool mousemove 460 129 click 1
sleep 1
import -window root "$PWD/images/autossh-ui-ssh-hosts-loaded.png"
```

## 5. 交互验证原则

- `Start All`：应启动 supervisor；按钮变为 `Stop All`，状态变为 running。
- `Stop All`：应释放 supervisor handle，并终止 supervisor 及其 SSH 子进程。
- `follow`：首次默认勾选；点击后应能取消勾选，再点击可恢复。
- `SSH hosts`：Load 后应显示临时 SSH config 中的 host；名称必须随主题可读，不能硬编码白色。
- connection 名称、地址或 forward 信息双击：应打开编辑 connection 弹窗。
- 修改配置后截图前应等待约 1 秒，避免捕获上一帧 UI。

## 6. 清理

每次预览都必须结束 GUI 进程。优先使用 `trap`；若进程异常残留，再检查：

```bash
pgrep -af autossh-ui || true
pgrep -af rust-autossh || true
```

不要在预览中使用真实 SSH 地址、真实私钥或真实用户配置。
