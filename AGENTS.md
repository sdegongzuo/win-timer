# AGENTS.md

本仓库是 **win-timer**：一个管理本机 Windows 计划任务的最小全栈应用。

- 后端：Rust + axum，监听 `127.0.0.1:58081`，通过 PowerShell 的 `ScheduledTasks` 模块操作本机计划任务。
- 前端：Vue 3 + Vite，位于 `web/`，dev server 在 `58173`，把 `/api` 代理到后端。

端口刻意选在 49152–65535 动态段，避开 8080 / 5173 / 3000 等常见开发端口。

## 常用命令

后端：

```bash
cd win-timer
cargo run          # 启动，监听 http://127.0.0.1:58081
cargo test         # 单元测试（25 项，其中 2 项会真跑 PowerShell）
cargo test -- --ignored   # 额外跑需要访问计划任务服务/事件日志的集成测试
cargo clippy --all-targets
cargo fmt
```

前端：

```bash
cd win-timer/web
pnpm install
pnpm dev           # http://localhost:58173
pnpm build         # vue-tsc -b && vite build
```

端到端冒烟（**会真实创建并删除一个名为 `win-timer-selftest` 的计划任务**）：

```bash
# 先确保后端已在 58081 运行
python e2e_smoke.py
```

## API

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/api/health` | 健康检查 |
| GET | `/api/tasks?scope=root\|all` | 列出任务，`root` 只含根目录 `\`。结果带 30 秒 TTL 缓存（`src/main.rs` 的 `LIST_CACHE_TTL`），创建/删除/启停/编辑会立即失效缓存；PowerShell 枚举在 `spawn_blocking` 中执行 |
| POST | `/api/tasks` | 创建任务 |
| DELETE | `/api/tasks/{name}?path=\` | 删除任务 |
| POST | `/api/tasks/{verb}/{name}?path=\` | `verb` ∈ `run`\|`end`\|`enable`\|`disable` |
| PUT | `/api/tasks/{name}?path=\` | 编辑任务（程序/参数/说明/调度整体覆盖），请求体同创建但无 `name` |
| GET | `/api/history?task=名称` | 执行历史，`task` 可选（按任务名精确过滤）。返回 `{history_enabled, rows}`；数据源是事件日志 `Microsoft-Windows-TaskScheduler/Operational`（事件 100/101/201 按 `TaskExecutionId` 关联成一次运行）。历史记录未启用时 `history_enabled=false` 且 `rows` 为空，**不是错误**——前端据此显示开启指引 |

错误统一返回 `{"error": "中文说明"}`（HTTP 500），前端直接展示 `error` 字段。

## 必须遵守的不变量

1. **后端必须能访问本机计划任务服务。** 读取通常不需要提权；创建/删除根目录 `\` 的任务在非提权会话下可能失败。失败时后端会把 PowerShell 的错误原文转成中文提示返回给前端，不要吞掉。

2. **前端只通过相对路径 `/api` 访问后端**，不要写死 `127.0.0.1:58081`。代理配置在 `web/vite.config.ts`。

3. **改端口必须同步三处**：`src/main.rs` 的 bind 地址、`web/vite.config.ts` 的 proxy target、
   `web/src/config.ts` 的 `BACKEND_PORT`（后者只用于界面展示与错误提示）。前端端口在
   `web/vite.config.ts` 的 `server.port`，并开了 `strictPort`——被占用时直接报错而不是静默换端口。

4. **所有 PowerShell 脚本的构造都放在 `src/tasks.rs` 的 `build_*` 纯函数里**，不要内联在业务函数里——这些函数是可单元测试的边界。

5. **PowerShell 语法陷阱（已踩过，勿回退）**：
   - `ConvertTo-Json` 必须用 `-InputObject $rows`，不能用管道。走管道时若只有 1 个元素，输出会是 `{...}` 而不是 `[{...}]`，反序列化成 `Vec<TaskSummary>` 直接失败。
   - 传给 cmdlet 的表达式必须加括号：`-At ([datetime]::ParseExact(...))`。不括号时 PowerShell 在参数模式下会把整个表达式当字符串字面量。
   - 输出用 `[Console]::Out.Write(...)`、错误用 `[Console]::Error.Write(...)`，不要用 `Write-Output` / `Write-Error`。后者会经过格式化层（长 JSON 可能被按控制台宽度折行破坏），且 `Write-Error` 会把整段脚本拼进错误消息。

6. **所有用户输入必须经 `ps_quote()` 转义**后再拼进 PowerShell 脚本（单引号翻倍），否则任务名可以注入任意命令。

7. **UI 与 CLI 文案一律简体中文**，代码注释也用中文。

8. **UI 是亮色主题**，配色集中在 `web/src/style.css` 的 `:root` 变量里，并在 `:root` 上声明了
   `color-scheme: light`。改配色只动变量，不要在组件里写死颜色。

## 已知环境注意事项

- 后端调用的是 `powershell.exe`（Windows PowerShell 5.1），不是 `pwsh`。5.1 自带 `ScheduledTasks` 模块，兼容性最好。
- 时间格式：`datetime-local` 输入框产出 `yyyy-MM-ddTHH:mm`，后端用 `ParseExact` + `InvariantCulture` 解析，不接受其他格式。
- `cargo` 使用 `D:\app\cargo\config.toml` 里配置的 `rsproxy-sparse` 镜像。
- 若 `cargo build` 报 TLS 失败（schannel `SEC_E_NO_CREDENTIALS`），是镜像的临时网络问题，重试通常即可。
