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
