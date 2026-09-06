# win-timer

管理本机 Windows 计划任务的轻量控制台：列表、新建、启用/禁用、立即运行、删除。

后端 Rust + axum 直连本机 Task Scheduler，前端 Vue 3 + Vite。

## 快速开始

```bash
# 终端 1 - 后端（监听 http://127.0.0.1:58081）
cd win-timer
cargo run

# 终端 2 - 前端
cd win-timer/web
pnpm install
pnpm dev
```

打开 http://localhost:58173 。

> 端口刻意选在 49152–65535 动态段（后端 58081 / 前端 58173），避开 8080、5173、3000 等常见开发端口。
> 读取任务一般不需要管理员权限；在根目录 `\` 下**新建/删除**任务建议用管理员终端启动后端。

## 功能

- 任务列表：名称、路径、状态、上次/下次运行时间、上次结果、执行的程序与参数；每 15 秒自动刷新。
- 作用域切换：`根目录任务`（仅 `\`）与 `全部任务`（含所有子文件夹）。
- 新建任务：一次性、按间隔重复（每 N 分钟）、每天三种调度；可填程序、参数、说明。
- 操作：运行 / 停止 / 启用 / 禁用 / 删除（删除有二次确认）。
- 编辑任务：点行内「编辑」整体覆盖程序、参数、说明与调度（名称不可改）。
- 执行历史：展示最近 200 次真实运行（开始/结束时间、结果码、状态），数据来自任务计划程序事件日志；
  支持工具栏全局查看或按单个任务过滤。历史记录未启用时界面会给出开启指引。

## 结构

```
win-timer/
├── Cargo.toml
├── src/
│   ├── main.rs        # axum 路由与统一错误响应
│   └── tasks.rs       # PowerShell 脚本构造 + 单元测试
├── web/               # Vue 3 前端
├── e2e_smoke.py       # 端到端冒烟脚本
└── AGENTS.md          # 给 AI Agent 的项目约定
```

## 测试

```bash
cargo test                  # 22 项单元测试，含 2 项真跑 PowerShell 的集成测试
cargo test -- --ignored     # 额外跑需要访问计划任务服务/事件日志的测试
cd web && pnpm build        # vue-tsc + vite 构建
python e2e_smoke.py         # 端到端冒烟（后端需先启动；会临时创建并删除 win-timer-selftest）
```

## 常见问题

**页面提示"后端未连接"** —— 后端没起。`cd win-timer && cargo run`，确认 58081 端口未被占用。

**端口还是冲突了** —— 需同步改三处：`src/main.rs` 的 bind 地址、`web/vite.config.ts` 的 proxy target、`web/src/config.ts` 的 `BACKEND_PORT`。前端端口只改 `web/vite.config.ts` 的 `server.port`（并同步 `web/src/config.ts` 的 `FRONTEND_PORT`，仅用于展示）。

**创建任务失败并提示"拒绝访问"** —— 用管理员终端重新启动后端。

**列表为空** —— 默认只看根目录 `\`，点一下"全部任务"。

**执行历史为空且提示未启用** —— 任务计划程序的历史记录默认关闭。以管理员打开「任务计划程序」，在右侧操作栏点击「启用所有任务历史记录」即可（或管理员终端执行 `wevtutil set-log Microsoft-Windows-TaskScheduler/Operational /enabled:true`）。

更多约定见 [AGENTS.md](./AGENTS.md)。
