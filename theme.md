
- `(0,0,0)` + 有窗口 → 后端未生效 / 未进入 render loop
- 中等深灰（如 `(28,32,38)`） → panel 正常，截图无误
- 灰白 → app 在画但配色被改坏

推荐配色档（按亮度阶梯，每档 +14~22），调 theme 时直接拿来对：

```text
panel fill      #252A33  (37,42,51)   整窗底色
noninteractive  #2E3441  (46,52,65)   卡片 / 字段
inactive        #3C4458  (60,68,88)   不可点条目
hovered         #505A72  (80,90,114)  鼠标悬停
active          #64708A  (100,112,138) 按下瞬间
selection       #1E80A8  (30,128,168) 选中底
```

主题改动只动 `ui/src/main.rs::visuals()`，约 12 行 diff。
