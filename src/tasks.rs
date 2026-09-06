//! Windows Task Scheduler integration.
//!
//! The axum backend shells out to Windows PowerShell (which ships the
//! `ScheduledTasks` module) to read/manage the local Task Scheduler.
//!
//! 所有 PowerShell 脚本/命令的构造都放在 `build_*` 纯函数里，
//! 这样无需真实的计划任务服务即可单元测试（见文件末尾 `mod tests`）。

use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TaskSummary {
    pub name: String,
    pub path: String,
    pub state: String,
    pub enabled: bool,
    pub last_run_time: Option<String>,
    pub next_run_time: Option<String>,
    pub last_result: Option<i64>,
    pub executable: Option<String>,
    pub arguments: Option<String>,
    pub description: Option<String>,
    /// 触发器类型推导："once" | "daily" | "interval" | "unknown"（开机/登录等非时间触发）
    pub schedule_type: Option<String>,
    /// 重复间隔（分钟），仅 interval 类型有值
    pub interval_minutes: Option<u32>,
    /// 触发器 StartBoundary（本地时间）
    pub start_boundary: Option<String>,
}

/// 单次执行记录（来自任务计划程序事件日志，按 TaskExecutionId 关联）。
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RunRecord {
    pub task_name: String,
    /// 事件 100（任务启动）的时间
    pub start_time: Option<String>,
    /// 事件 201/101（动作完成/启动失败）的时间
    pub end_time: Option<String>,
    pub result_code: Option<i64>,
    /// "running" | "done" | "failed" | "start_failed" | "unknown"
    pub status: String,
}

#[derive(Deserialize, Debug)]
pub struct HistoryPayload {
    /// 任务计划程序的"历史记录"开关是否已启用
    pub history_enabled: bool,
    pub rows: Vec<RunRecord>,
}

#[derive(Deserialize, Debug)]
pub struct Schedule {
    /// "once" | "interval" | "daily"
    pub ty: String,
    /// Local wall-clock start, "yyyy-MM-ddTHH:mm"
    #[serde(default)]
    pub at: Option<String>,
    #[serde(default)]
    pub every_minutes: Option<u32>,
}

#[derive(Deserialize, Debug)]
pub struct CreateTaskRequest {
    pub name: String,
    pub program: String,
    #[serde(default)]
    pub arguments: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub schedule: Schedule,
}

#[derive(Deserialize, Debug)]
pub struct UpdateTaskRequest {
    pub program: String,
    #[serde(default)]
    pub arguments: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub schedule: Schedule,
}

/// Quote a value as a single-quoted PowerShell string literal.
fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// True when the value is a non-empty string; used to decide whether to embed
/// an optional parameter.
fn has(s: &Option<String>) -> bool {
    matches!(s, Some(v) if !v.trim().is_empty())
}

/// Build a `[datetime]` literal for the given "yyyy-MM-ddTHH:mm" local time.
fn datetime_literal(at: &str) -> String {
    format!(
        "[datetime]::ParseExact('{at}','yyyy-MM-ddTHH:mm',[Globalization.CultureInfo]::InvariantCulture)"
    )
}

/// Same, but wrapped in parentheses so it can be passed as a cmdlet argument.
///
/// 这是必须的：PowerShell 在参数模式下会把 `-At [datetime]::ParseExact(...)`
/// 里的整个表达式当成字符串字面量，必须用 `(...)` 才会求值。
fn datetime_arg(at: &str) -> String {
    format!("({})", datetime_literal(at))
}

/// Normalize the UI-supplied start time. Accepts "yyyy-MM-dd HH:mm" too.
fn parse_datetime(at: &str) -> Result<String, String> {
    let cleaned = at.trim().replace(' ', "T");
    if cleaned.len() != 16 || cleaned.as_bytes().get(10) != Some(&b'T') {
        return Err(format!("开始时间格式应为 yyyy-MM-ddTHH:mm，收到: {at}"));
    }
    Ok(cleaned)
}

/// Build the PowerShell script that lists scheduled tasks as a JSON array.
///
/// 注意：`ConvertTo-Json` 必须用 `-InputObject`。走管道时若只有一个元素，
/// PowerShell 会把数组拆成单个对象输出 `{...}` 而不是 `[{...}]`，
/// 反序列化成 `Vec<TaskSummary>` 会失败。
///
/// 输出用 `[Console]::Out.Write` 直接写 stdout，绕开 PowerShell 的格式化层，
/// 避免长 JSON 被按控制台宽度折行破坏。
pub fn build_list_script(scope_all: bool) -> String {
    let folder_filter = if scope_all {
        String::new()
    } else {
        " -TaskPath '\\'".to_string()
    };
    format!(
        r#"
$ErrorActionPreference = 'Stop'
try {{
  $rows = @()
  $all = @(Get-ScheduledTask{folder_filter})
  foreach ($t in $all) {{
    $info = $t | Get-ScheduledTaskInfo -ErrorAction SilentlyContinue
    $exe = $null
    $arg = $null
    if ($null -ne $t.Actions -and $t.Actions.Count -gt 0) {{
      $exe = [string]$t.Actions[0].Execute
      $arg = [string]$t.Actions[0].Arguments
    }}
    $desc = $null
    try {{ $desc = [string]$t.Description }} catch {{}}
    $lrt = $null
    $nrt = $null
    $res = $null
    if ($null -ne $info) {{
      if ($null -ne $info.LastRunTime -and $info.LastRunTime -ne [datetime]::MinValue) {{
        $lrt = $info.LastRunTime.ToString('yyyy-MM-ddTHH:mm:ss')
      }}
      if ($null -ne $info.NextRunTime -and $info.NextRunTime -ne [datetime]::MinValue) {{
        $nrt = $info.NextRunTime.ToString('yyyy-MM-ddTHH:mm:ss')
      }}
      try {{ $res = [long]$info.LastTaskResult }} catch {{}}
    }}
    $stype = 'unknown'
    $ivmin = $null
    $start = $null
    $trig0 = $null
    if ($null -ne $t.Triggers -and $t.Triggers.Count -gt 0) {{ $trig0 = $t.Triggers[0] }}
    if ($null -ne $trig0) {{
      # StartBoundary 从 CIM 返回的是字符串（可能带时区偏移），先转 datetime 再格式化
      try {{ $start = ([datetime]$trig0.StartBoundary).ToString('yyyy-MM-ddTHH:mm:ss') }} catch {{}}
      if ($null -ne $trig0.Repetition -and $null -ne $trig0.Repetition.Interval -and $trig0.Repetition.Interval -ne [timespan]::Zero) {{
        $stype = 'interval'
        # Repetition.Interval 是 ISO8601 时长字符串（如 PT30M、PT1H40M），不是
        # TimeSpan；对它取 .TotalSeconds 会得到 $null（转成 int 变 0），导致
        # interval_minutes 恒为 0。用 XmlConvert 解析成 TimeSpan 再取分钟。
        try {{
          $ivRaw = $trig0.Repetition.Interval
          $span = if ($ivRaw -is [string]) {{ [System.Xml.XmlConvert]::ToTimeSpan($ivRaw) }} else {{ [timespan]$ivRaw }}
          $ivmin = [int]$span.TotalMinutes
        }} catch {{}}
      }} elseif ($trig0.CimClass.CimClassName -eq 'MSFT_TaskDailyTrigger') {{
        $stype = 'daily'
      }} else {{
        $stype = 'once'
      }}
    }}
    $rows += [pscustomobject]@{{
      name          = [string]$t.TaskName
      path          = [string]$t.TaskPath
      state         = [string]$t.State
      enabled       = [bool]$t.Settings.Enabled
      last_run_time = $lrt
      next_run_time = $nrt
      last_result   = $res
      executable    = $exe
      arguments     = $arg
      description   = $desc
      schedule_type = $stype
      interval_minutes = $ivmin
      start_boundary   = $start
    }}
  }}
  if ($rows.Count -eq 0) {{
    [Console]::Out.Write('[]')
  }} else {{
    [Console]::Out.Write((ConvertTo-Json -InputObject $rows -Depth 3 -Compress))
  }}
}} catch {{
  [Console]::Error.Write('LIST_FAIL: ' + $_.Exception.Message)
  exit 1
}}
"#
    )
}

/// Build the `New-ScheduledTaskTrigger` expression for the requested schedule.
fn build_trigger_expr(schedule: &Schedule) -> Result<String, String> {
    let at = schedule
        .at
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(parse_datetime)
        .transpose()?;

    match schedule.ty.as_str() {
        "once" => {
            let at = at.ok_or_else(|| "一次性任务需要开始时间".to_string())?;
            Ok(format!(
                "New-ScheduledTaskTrigger -Once -At {}",
                datetime_arg(&at)
            ))
        }
        "daily" => {
            let at = at.ok_or_else(|| "每日任务需要开始时间".to_string())?;
            Ok(format!(
                "New-ScheduledTaskTrigger -Daily -At {}",
                datetime_arg(&at)
            ))
        }
        "interval" => {
            let m = schedule.every_minutes.unwrap_or(0);
            if m == 0 {
                return Err("间隔(分钟)必须大于 0".to_string());
            }
            // 起始时间可选；不给就从"现在"开始。
            let start = match at {
                Some(a) => datetime_arg(&a),
                None => "([datetime]::Now)".to_string(),
            };
            Ok(format!(
                "New-ScheduledTaskTrigger -Once -At {start} -RepetitionInterval (New-TimeSpan -Seconds {secs}) -RepetitionDuration (New-TimeSpan -Days 3650)",
                start = start,
                secs = m as u64 * 60
            ))
        }
        other => Err(format!("不支持的调度类型: {other}")),
    }
}

/// Build the PowerShell script that registers a new scheduled task.
pub fn build_create_script(req: &CreateTaskRequest) -> Result<String, String> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err("任务名称不能为空".to_string());
    }
    let program = req.program.trim();
    if program.is_empty() {
        return Err("要执行的程序不能为空".to_string());
    }
    let trigger_cmd = build_trigger_expr(&req.schedule)?;

    let mut action_args = String::from("-Execute ");
    action_args.push_str(&ps_quote(program));
    if has(&req.arguments) {
        action_args.push_str(" -Argument ");
        action_args.push_str(&ps_quote(req.arguments.as_deref().unwrap()));
    }

    let mut settings = String::new();
    if has(&req.description) {
        settings.push_str(" -Description ");
        settings.push_str(&ps_quote(req.description.as_deref().unwrap().trim()));
    }

    Ok(format!(
        r#"
$ErrorActionPreference = 'Stop'
try {{
  $action = New-ScheduledTaskAction {action_args}
  $trig   = {trigger_cmd}
  $reg    = Register-ScheduledTask -TaskName {name_q} -Action $action -Trigger $trig{settings_q} -Force
  [Console]::Out.Write('OK|' + [string]$reg.TaskPath + [string]$reg.TaskName)
}} catch {{
  [Console]::Error.Write('CREATE_FAIL: ' + $_.Exception.Message)
  exit 1
}}
"#,
        action_args = action_args,
        trigger_cmd = trigger_cmd,
        name_q = ps_quote(name),
        settings_q = settings,
    ))
}

/// Build the PowerShell command for a verb-style operation against a task.
pub fn build_verb_command(verb: &str, name: &str, path: Option<&str>) -> Result<String, String> {
    let cmd = match verb {
        "run" => "Start-ScheduledTask",
        "end" => "Stop-ScheduledTask",
        "enable" => "Enable-ScheduledTask",
        "disable" => "Disable-ScheduledTask",
        other => return Err(format!("未知操作: {other}")),
    };
    let path = path.filter(|p| !p.trim().is_empty()).unwrap_or("\\");
    Ok(format!(
        "{cmd} -TaskPath {path_q} -TaskName {name_q} -ErrorAction Stop",
        cmd = cmd,
        path_q = ps_quote(path),
        name_q = ps_quote(name),
    ))
}

/// Build the PowerShell command that deletes a task.
pub fn build_delete_command(name: &str, path: Option<&str>) -> String {
    let path = path.filter(|p| !p.trim().is_empty()).unwrap_or("\\");
    format!(
        "$ErrorActionPreference='Stop'\nUnregister-ScheduledTask -TaskPath {path_q} -TaskName {name_q} -Confirm:$false",
        path_q = ps_quote(path),
        name_q = ps_quote(name),
    )
}

/// Build the PowerShell script that overwrites action/trigger/description of an
/// existing task (GET the registered object, mutate it, pipe to Set-ScheduledTask).
pub fn build_update_script(
    name: &str,
    path: Option<&str>,
    req: &UpdateTaskRequest,
) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("任务名称不能为空".to_string());
    }
    let program = req.program.trim();
    if program.is_empty() {
        return Err("要执行的程序不能为空".to_string());
    }
    let trigger_cmd = build_trigger_expr(&req.schedule)?;

    let mut action_args = String::from("-Execute ");
    action_args.push_str(&ps_quote(program));
    if has(&req.arguments) {
        action_args.push_str(" -Argument ");
        action_args.push_str(&ps_quote(req.arguments.as_deref().unwrap()));
    }

    let desc_stmt = match &req.description {
        Some(d) if !d.trim().is_empty() => {
            format!("\n  $task.Description = {}", ps_quote(d.trim()))
        }
        _ => String::new(),
    };

    let path = path.filter(|p| !p.trim().is_empty()).unwrap_or("\\");
    Ok(format!(
        r#"
$ErrorActionPreference = 'Stop'
try {{
  $task = Get-ScheduledTask -TaskPath {path_q} -TaskName {name_q} -ErrorAction Stop
  $task.Actions = @(New-ScheduledTaskAction {action_args})
  $task.Triggers = @({trigger_cmd}){desc_stmt}
  $task | Set-ScheduledTask | Out-Null
  [Console]::Out.Write('OK|' + [string]$task.TaskPath + [string]$task.TaskName)
}} catch {{
  [Console]::Error.Write('UPDATE_FAIL: ' + $_.Exception.Message)
  exit 1
}}
"#,
        path_q = ps_quote(path),
        name_q = ps_quote(name),
        action_args = action_args,
        trigger_cmd = trigger_cmd,
        desc_stmt = desc_stmt,
    ))
}

/// Run a PowerShell script and return stdout on success.
fn run_ps(script: &str) -> Result<String, String> {
    let output = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ])
        .output()
        .map_err(|e| format!("无法启动 PowerShell: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if output.status.success() {
        Ok(stdout)
    } else {
        let mut msg = stderr;
        if msg.is_empty() {
            msg = stdout;
        }
        if msg.is_empty() {
            msg = format!("PowerShell 退出码 {}", output.status.code().unwrap_or(-1));
        }
        Err(msg)
    }
}

/// List scheduled tasks.
pub fn list_tasks(scope_all: bool) -> Result<Vec<TaskSummary>, String> {
    let out = run_ps(&build_list_script(scope_all))?;
    if out.is_empty() {
        return Err("PowerShell 未返回任何输出".to_string());
    }
    serde_json::from_str(&out).map_err(|e| {
        format!(
            "解析任务列表失败: {e}（原始输出前 200 字符: {}）",
            &out[..out.len().min(200)]
        )
    })
}

/// Create a new scheduled task that runs `program` (with optional arguments)
/// according to the requested schedule.
pub fn create_task(req: &CreateTaskRequest) -> Result<String, String> {
    let out = run_ps(&build_create_script(req)?)?;
    if let Some(rest) = out.strip_prefix("OK|") {
        Ok(rest.to_string())
    } else {
        Err(out
            .strip_prefix("CREATE_FAIL: ")
            .unwrap_or(&out)
            .trim()
            .to_string())
    }
}

/// Update an existing scheduled task's action/trigger/description.
pub fn update_task(
    name: &str,
    path: Option<&str>,
    req: &UpdateTaskRequest,
) -> Result<String, String> {
    let out = run_ps(&build_update_script(name, path, req)?)?;
    if let Some(rest) = out.strip_prefix("OK|") {
        Ok(rest.to_string())
    } else {
        Err(out
            .strip_prefix("UPDATE_FAIL: ")
            .unwrap_or(&out)
            .trim()
            .to_string())
    }
}

/// Run one of the simple verb-style operations against a task.
pub fn task_verb(verb: &str, name: &str, path: Option<&str>) -> Result<(), String> {
    run_ps(&build_verb_command(verb, name, path)?).map(|_| ())
}

/// Delete a task.
pub fn delete_task(name: &str, path: Option<&str>) -> Result<(), String> {
    run_ps(&build_delete_command(name, path)).map(|_| ())
}

/// Build the PowerShell script that reads recent task runs from the
/// Task Scheduler operational event log.
///
/// 数据源是事件日志 `Microsoft-Windows-TaskScheduler/Operational`：
/// - 事件 100：任务启动（拿到开始时间）
/// - 事件 201：动作完成（拿到结束时间与 ResultCode）
/// - 事件 101：任务启动失败
///
/// 事件按 `TaskName + TaskExecutionId` 关联成一次运行。历史记录开关未启用时
/// 日志为空/不存在，此时返回 `history_enabled = false` 与空列表而不是报错。
///
/// `task` 可选：按任务名精确过滤（用户输入，必须经 `ps_quote` 转义）。
pub fn build_history_script(task: Option<&str>) -> String {
    let filter = match task {
        Some(t) if !t.trim().is_empty() => ps_quote(t.trim()),
        _ => "''".to_string(),
    };
    format!(
        r#"
$ErrorActionPreference = 'Stop'
$histEnabled = $false
try {{ $histEnabled = [bool](Get-LogProperties 'Microsoft-Windows-TaskScheduler/Operational').Enabled }} catch {{ $histEnabled = $false }}
$taskFilter = {filter}
$events = @()
try {{
  $events = @(Get-WinEvent -FilterHashtable @{{ LogName = 'Microsoft-Windows-TaskScheduler/Operational'; Id = 100,101,201 }} -MaxEvents 3000 -ErrorAction Stop)
}} catch {{ $events = @() }}
$runs = @{{}}
foreach ($e in $events) {{
  $x = [xml]$e.ToXml()
  $data = @{{}}
  foreach ($d in @($x.Event.EventData.Data)) {{
    if ($null -ne $d -and $d.Name) {{ $data[[string]$d.Name] = [string]$d.'#text' }}
  }}
  $tn = [string]$data['TaskName']
  if ($taskFilter -ne '' -and $tn -ne $taskFilter) {{ continue }}
  $key = $tn + '|' + [string]$data['TaskExecutionId']
  if (-not $runs.ContainsKey($key)) {{
    $runs[$key] = @{{ task = $tn; start = $null; end = $null; result = $null; status = 'unknown' }}
  }}
  $r = $runs[$key]
  $tstr = $e.TimeCreated.ToString('yyyy-MM-ddTHH:mm:ss')
  $rc = $null
  try {{ $rc = [long]$data['ResultCode'] }} catch {{}}
  if ($e.Id -eq 100) {{
    $r.start = $tstr
    if ($r.status -eq 'unknown') {{ $r.status = 'running' }}
  }} elseif ($e.Id -eq 201) {{
    $r.end = $tstr
    $r.result = $rc
    $r.status = 'done'
    if ($null -ne $rc -and $rc -ne 0) {{ $r.status = 'failed' }}
  }} else {{
    $r.end = $tstr
    $r.result = $rc
    $r.status = 'start_failed'
  }}
}}
$rows = @()
foreach ($k in $runs.Keys) {{
  $r = $runs[$k]
  if ($null -eq $r.start -and $null -eq $r.end) {{ continue }}
  $rows += [pscustomobject]@{{
    task_name   = [string]$r.task
    start_time  = $r.start
    end_time    = $r.end
    result_code = $r.result
    status      = [string]$r.status
  }}
}}
$rows = @($rows | Sort-Object {{ if ($_.start_time) {{ $_.start_time }} else {{ $_.end_time }} }} -Descending | Select-Object -First 200)
$out = [pscustomobject]@{{ history_enabled = $histEnabled; rows = $rows }}
[Console]::Out.Write((ConvertTo-Json -InputObject $out -Depth 4 -Compress))
"#,
        filter = filter,
    )
}

/// Read recent execution history, optionally filtered by exact task name.
pub fn task_history(task: Option<&str>) -> Result<HistoryPayload, String> {
    let out = run_ps(&build_history_script(task))?;
    if out.is_empty() {
        return Err("PowerShell 未返回任何输出".to_string());
    }
    serde_json::from_str(&out).map_err(|e| {
        format!(
            "解析执行历史失败: {e}（原始输出前 200 字符: {}）",
            &out[..out.len().min(200)]
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ps_quote_escapes_single_quotes() {
        assert_eq!(ps_quote("abc"), "'abc'");
        assert_eq!(ps_quote("it's"), "'it''s'");
        // 两次转义可防止提前闭合字符串 literal
        assert_eq!(
            ps_quote("'; Remove-Item C:\\ -Recurse; '"),
            "'''; Remove-Item C:\\ -Recurse; '''"
        );
    }

    #[test]
    fn ps_quote_neutralizes_injection_in_task_name() {
        let evil = "x'; Start-Process calc; #";
        let script = build_delete_command(evil, None);
        // 单引号被翻倍，因此字面量不会提前闭合
        assert!(script.contains("'x''; Start-Process calc; #'"));
        assert!(!script.contains("'x'; Start-Process"));
    }

    #[test]
    fn verb_command_uses_default_root_path() {
        assert_eq!(
            build_verb_command("run", "Foo", None).unwrap(),
            "Start-ScheduledTask -TaskPath '\\' -TaskName 'Foo' -ErrorAction Stop"
        );
        assert_eq!(
            build_verb_command("disable", "Foo", Some("")).unwrap(),
            "Disable-ScheduledTask -TaskPath '\\' -TaskName 'Foo' -ErrorAction Stop"
        );
        assert_eq!(
            build_verb_command("enable", "Foo", Some("\\My")).unwrap(),
            "Enable-ScheduledTask -TaskPath '\\My' -TaskName 'Foo' -ErrorAction Stop"
        );
    }

    #[test]
    fn unknown_verb_is_rejected() {
        let err = build_verb_command("nuke", "Foo", None).unwrap_err();
        assert_eq!(err, "未知操作: nuke");
    }

    #[test]
    fn delete_command_shape() {
        assert_eq!(
            build_delete_command("Foo", None),
            "$ErrorActionPreference='Stop'\nUnregister-ScheduledTask -TaskPath '\\' -TaskName 'Foo' -Confirm:$false"
        );
    }

    #[test]
    fn list_script_uses_inputobject_so_single_task_is_an_array() {
        // 回归测试：走管道时单元素会被降级成对象，导致反序列化失败。
        let s = build_list_script(false);
        assert!(s.contains("ConvertTo-Json -InputObject $rows"));
        assert!(!s.contains("$rows | ConvertTo-Json"));
        assert!(s.contains("-TaskPath '\\'"));
        // scope=all 时不带 TaskPath 过滤
        let a = build_list_script(true);
        assert!(!a.contains("-TaskPath"));
    }

    fn req(ty: &str, at: Option<&str>, every: Option<u32>) -> CreateTaskRequest {
        CreateTaskRequest {
            name: "TestTask".into(),
            program: "notepad.exe".into(),
            arguments: None,
            description: None,
            schedule: Schedule {
                ty: ty.into(),
                at: at.map(|s| s.to_string()),
                every_minutes: every,
            },
        }
    }

    #[test]
    fn create_script_once_and_daily_need_a_start_time() {
        assert!(build_create_script(&req("once", None, None))
            .unwrap_err()
            .contains("一次性任务需要开始时间"));
        assert!(build_create_script(&req("daily", None, None))
            .unwrap_err()
            .contains("每日任务需要开始时间"));
    }

    #[test]
    fn create_script_once_shape() {
        let s = build_create_script(&req("once", Some("2026-09-06T12:30"), None)).unwrap();
        assert!(s.contains("New-ScheduledTaskAction -Execute 'notepad.exe'"));
        // 回归测试：-At 后的表达式必须加括号，否则 PowerShell 当成字符串字面量
        assert!(s.contains("New-ScheduledTaskTrigger -Once -At ([datetime]::ParseExact('2026-09-06T12:30','yyyy-MM-ddTHH:mm',[Globalization.CultureInfo]::InvariantCulture))"));
        assert!(s.contains("Register-ScheduledTask -TaskName 'TestTask'"));
        assert!(s.contains("-Force"));
        // 无参数/无说明时不带这两个可选开关
        assert!(!s.contains(" -Argument"));
        assert!(!s.contains(" -Description"));
    }

    #[test]
    fn create_script_daily_shape() {
        let s = build_create_script(&req("daily", Some("2026-09-06T12:30"), None)).unwrap();
        assert!(s.contains(
            "New-ScheduledTaskTrigger -Daily -At ([datetime]::ParseExact('2026-09-06T12:30'"
        ));
    }

    #[test]
    fn create_script_interval_defaults_to_now() {
        let s = build_create_script(&req("interval", None, Some(5))).unwrap();
        assert!(s.contains("-At ([datetime]::Now)"));
        assert!(s.contains("New-TimeSpan -Seconds 300"));
        assert!(s.contains("-RepetitionDuration (New-TimeSpan -Days 3650)"));
    }

    #[test]
    fn create_script_interval_with_explicit_start() {
        let s = build_create_script(&req("interval", Some("2026-09-06 08:00"), Some(1))).unwrap();
        // "yyyy-MM-dd HH:mm" 会被规范化成带 T 的形式
        assert!(s.contains("ParseExact('2026-09-06T08:00'"));
        assert!(s.contains("New-TimeSpan -Seconds 60"));
    }

    #[test]
    fn create_script_rejects_bad_interval_and_type() {
        assert_eq!(
            build_create_script(&req("interval", None, Some(0))).unwrap_err(),
            "间隔(分钟)必须大于 0"
        );
        let e = build_create_script(&req("weekly", Some("2026-09-06T12:30"), None)).unwrap_err();
        assert_eq!(e, "不支持的调度类型: weekly");
    }

    #[test]
    fn create_script_validates_name_and_program() {
        let mut r = req("once", Some("2026-09-06T12:30"), None);
        r.name = "   ".into();
        assert_eq!(build_create_script(&r).unwrap_err(), "任务名称不能为空");
        r.name = "Ok".into();
        r.program = "".into();
        assert_eq!(build_create_script(&r).unwrap_err(), "要执行的程序不能为空");
    }

    #[test]
    fn create_script_accepts_space_separated_datetime() {
        let s = build_create_script(&req("once", Some("2026-09-06 12:30"), None)).unwrap();
        assert!(s.contains("'2026-09-06T12:30'"));
    }

    #[test]
    fn create_script_rejects_malformed_datetime() {
        let e = build_create_script(&req("once", Some("2026-09-06"), None)).unwrap_err();
        assert!(e.starts_with("开始时间格式应为 yyyy-MM-ddTHH:mm"));
    }

    #[test]
    fn list_script_is_valid_powershell_syntax() {
        // 花括号必须成对（format! 里 {{ }} 转义后应为单花括号）
        let s = build_list_script(false);
        let opens = s.chars().filter(|c| *c == '{').count();
        let closes = s.chars().filter(|c| *c == '}').count();
        assert_eq!(opens, closes, "花括号不成对，脚本无法解析");
        assert!(!s.contains("{{"));
        assert!(!s.contains("}}"));
    }

    #[test]
    fn history_script_quotes_task_filter() {
        let s = build_history_script(Some("Foo"));
        assert!(s.contains("$taskFilter = 'Foo'"));
        // 注入防护：任务名里的单引号必须翻倍，防止提前闭合字符串字面量
        let evil = build_history_script(Some("x'; Start-Process calc; #"));
        assert!(evil.contains("'x''; Start-Process calc; #'"));
        assert!(!evil.contains("'x'; Start-Process"));
    }

    #[test]
    fn history_script_without_filter_and_syntax_shape() {
        let s = build_history_script(None);
        assert!(s.contains("$taskFilter = ''"));
        // 花括号必须成对（format! 里 {{ }} 转义后应为单花括号）
        let opens = s.chars().filter(|c| *c == '{').count();
        let closes = s.chars().filter(|c| *c == '}').count();
        assert_eq!(opens, closes, "花括号不成对，脚本无法解析");
        assert!(!s.contains("{{"));
        assert!(!s.contains("}}"));
        // JSON 输出规范：-InputObject + [Console]::Out.Write（勿回退到管道/Write-Output）
        assert!(s.contains("ConvertTo-Json -InputObject $out"));
        assert!(s.contains("[Console]::Out.Write"));
    }

    fn update_req(ty: &str, at: Option<&str>, every: Option<u32>) -> UpdateTaskRequest {
        UpdateTaskRequest {
            program: "notepad.exe".into(),
            arguments: None,
            description: None,
            schedule: Schedule {
                ty: ty.into(),
                at: at.map(|s| s.to_string()),
                every_minutes: every,
            },
        }
    }

    #[test]
    fn update_script_shape_and_injection_guard() {
        let mut r = update_req("once", Some("2026-09-06T12:30"), None);
        r.arguments = Some("/c backup".into());
        r.description = Some("改过的说明".into());
        let s = build_update_script("Foo", None, &r).unwrap();
        assert!(s.contains("Get-ScheduledTask -TaskPath '\\' -TaskName 'Foo'"));
        assert!(s.contains("New-ScheduledTaskAction -Execute 'notepad.exe' -Argument '/c backup'"));
        assert!(s.contains("$task.Triggers = @(New-ScheduledTaskTrigger -Once -At ([datetime]::ParseExact('2026-09-06T12:30'"));
        assert!(s.contains("$task.Description = '改过的说明'"));
        assert!(s.contains("$task | Set-ScheduledTask"));
        // 任务名注入防护：单引号翻倍
        let evil = build_update_script(
            "x'; Start-Process calc; #",
            None,
            &update_req("once", Some("2026-09-06T12:30"), None),
        )
        .unwrap();
        assert!(evil.contains("'x''; Start-Process calc; #'"));
        assert!(!evil.contains("'x'; Start-Process"));
    }

    #[test]
    fn update_script_validates_and_rejects_bad_schedule() {
        let mut r = update_req("once", Some("2026-09-06T12:30"), None);
        r.program = "  ".into();
        assert_eq!(
            build_update_script("Foo", None, &r).unwrap_err(),
            "要执行的程序不能为空"
        );
        let e = build_update_script(
            "Foo",
            None,
            &update_req("weekly", Some("2026-09-06T12:30"), None),
        )
        .unwrap_err();
        assert_eq!(e, "不支持的调度类型: weekly");
        assert_eq!(
            build_update_script(
                "  ",
                None,
                &update_req("once", Some("2026-09-06T12:30"), None)
            )
            .unwrap_err(),
            "任务名称不能为空"
        );
        // 无说明时不带 Description 赋值
        let s = build_update_script(
            "Foo",
            Some("\\My"),
            &update_req("daily", Some("2026-09-06T12:30"), None),
        )
        .unwrap();
        assert!(!s.contains("$task.Description"));
        assert!(s.contains("Get-ScheduledTask -TaskPath '\\My' -TaskName 'Foo'"));
    }

    #[test]
    fn update_script_braces_balanced() {
        let s = build_update_script("Foo", None, &update_req("interval", None, Some(5))).unwrap();
        let opens = s.chars().filter(|c| *c == '{').count();
        let closes = s.chars().filter(|c| *c == '}').count();
        assert_eq!(opens, closes, "花括号不成对，脚本无法解析");
        assert!(!s.contains("{{"));
        assert!(!s.contains("}}"));
    }

    /* ---------- 真正调用 PowerShell 的集成测试 ---------- */

    #[test]
    #[cfg(windows)]
    fn once_trigger_expr_is_accepted_by_real_powershell() {
        let at = "2026-09-06T12:30";
        let expr = build_trigger_expr(&req("once", Some(at), None).schedule).unwrap();
        // StartBoundary 是 UTC，因此把期望值也换算成 UTC 再比，避免时区耦合。
        let out = run_ps(&format!(
            "$ErrorActionPreference='Stop'\n$t = {expr}\n$expected = {lit}.ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')\n[Console]::Out.Write($t.StartBoundary + '|' + $expected)",
            lit = datetime_literal(at)
        ))
        .expect("PowerShell 应接受生成的触发器表达式");
        let (actual, expected) = out
            .split_once('|')
            .unwrap_or_else(|| panic!("输出格式异常: {out}"));
        assert_eq!(actual, expected, "StartBoundary 与传入时间不符");
    }

    #[test]
    #[cfg(windows)]
    fn interval_trigger_expr_is_accepted_by_real_powershell() {
        let expr =
            build_trigger_expr(&req("interval", Some("2026-09-06T08:00"), Some(30)).schedule)
                .unwrap();
        let out = run_ps(&format!(
            "$ErrorActionPreference='Stop'\n$t = {expr}\n[Console]::Out.Write($t.Repetition.Interval)"
        ))
        .expect("PowerShell 应接受生成的间隔触发器表达式");
        assert!(out.contains("PT30M"), "重复间隔不是 30 分钟: {out}");
    }

    /// 需要能访问本机计划任务服务，默认跳过：
    /// `cargo test -- --ignored`
    #[test]
    #[cfg(windows)]
    #[ignore = "需要能访问本机计划任务服务"]
    fn list_tasks_roundtrip_against_real_scheduler() {
        let tasks = list_tasks(true).expect("应能列出计划任务");
        assert!(!tasks.is_empty());
        for t in &tasks {
            assert!(!t.name.is_empty(), "任务名为空: {t:?}");
            assert!(t.path.starts_with('\\'), "任务路径异常: {t:?}");
        }
    }

    /// 需要能访问本机事件日志，默认跳过：
    /// `cargo test -- --ignored`
    /// 历史记录未启用时应返回 `history_enabled = false` + 空列表，而不是报错。
    #[test]
    #[cfg(windows)]
    #[ignore = "需要能访问本机事件日志"]
    fn task_history_roundtrip_against_real_event_log() {
        let payload = task_history(None).expect("应能读取执行历史");
        for r in &payload.rows {
            assert!(!r.task_name.is_empty(), "执行记录任务名为空: {r:?}");
            assert!(
                matches!(
                    r.status.as_str(),
                    "running" | "done" | "failed" | "start_failed" | "unknown"
                ),
                "未知执行状态: {r:?}"
            );
        }
        let filtered =
            task_history(Some("win-timer-selftest-nonexistent")).expect("过滤查询应成功");
        assert!(
            filtered
                .rows
                .iter()
                .all(|r| r.task_name == "win-timer-selftest-nonexistent"),
            "按任务名过滤后不应出现其他任务: {filtered:?}"
        );
    }
}
