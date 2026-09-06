export type ScheduleType = 'once' | 'interval' | 'daily' | 'unknown'

export interface TaskSummary {
  name: string
  path: string
  state: string
  enabled: boolean
  last_run_time: string | null
  next_run_time: string | null
  last_result: number | null
  executable: string | null
  arguments: string | null
  description: string | null
  schedule_type: ScheduleType | null
  interval_minutes: number | null
  start_boundary: string | null
}

export interface Schedule {
  /** "once" | "interval" | "daily" */
  ty: 'once' | 'interval' | 'daily'
  /** Local "yyyy-MM-ddTHH:mm" */
  at?: string | null
  every_minutes?: number | null
}

export interface CreateTaskInput {
  name: string
  program: string
  arguments?: string | null
  description?: string | null
  schedule: Schedule
}

export interface TaskListResponse {
  tasks: TaskSummary[]
}

export interface CreateResponse {
  ok: boolean
  name: string
  path: string
}

export interface ErrorBody {
  error: string
}

/** 单次执行记录（来自任务计划程序事件日志） */
export interface RunRecord {
  task_name: string
  start_time: string | null
  end_time: string | null
  result_code: number | null
  status: 'running' | 'done' | 'failed' | 'start_failed' | 'unknown'
}

export interface HistoryResponse {
  /** 任务计划程序的历史记录开关是否已启用 */
  history_enabled: boolean
  rows: RunRecord[]
}
