import { BACKEND_PORT } from './config'
import type {
  CreateResponse,
  CreateTaskInput,
  ErrorBody,
  HistoryResponse,
  TaskListResponse,
  TaskSummary,
} from './types'

async function request<T>(url: string, init?: RequestInit): Promise<T> {
  let res: Response
  try {
    res = await fetch(url, init)
  } catch {
    throw new Error(
      `无法连接后端服务(win-timer axum)。请确认它在 ${BACKEND_PORT} 端口运行。`,
    )
  }
  if (!res.ok) {
    let message = `请求失败 (HTTP ${res.status})`
    try {
      const body = (await res.json()) as ErrorBody
      if (body?.error) message = body.error
    } catch {
      /* keep default message */
    }
    throw new Error(message)
  }
  return (await res.json()) as T
}

const json = (method: string, body: unknown): RequestInit => ({
  method,
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify(body),
})

export async function fetchHealth(): Promise<boolean> {
  try {
    const r = await fetch('/api/health')
    return r.ok
  } catch {
    return false
  }
}

export async function listTasks(scope: 'root' | 'all'): Promise<TaskSummary[]> {
  const data = await request<TaskListResponse>(`/api/tasks?scope=${scope}`)
  return data.tasks ?? []
}

export async function createTask(input: CreateTaskInput): Promise<CreateResponse> {
  return request<CreateResponse>('/api/tasks', json('POST', input))
}

export async function updateTask(
  name: string,
  path: string,
  input: CreateTaskInput,
): Promise<void> {
  const qs = path && path !== '\\' ? `?path=${encodeURIComponent(path)}` : ''
  await request<{ ok: boolean }>(`/api/tasks/${encodeURIComponent(name)}${qs}`, json('PUT', input))
}

export async function runVerb(
  verb: 'run' | 'end' | 'enable' | 'disable',
  name: string,
  path: string,
): Promise<void> {
  const qs = path && path !== '\\' ? `?path=${encodeURIComponent(path)}` : ''
  await request<{ ok: boolean }>(`/api/tasks/${verb}/${encodeURIComponent(name)}${qs}`, { method: 'POST' })
}

export async function deleteTask(name: string, path: string): Promise<void> {
  const qs = path && path !== '\\' ? `?path=${encodeURIComponent(path)}` : ''
  await request<{ ok: boolean }>(`/api/tasks/${encodeURIComponent(name)}${qs}`, { method: 'DELETE' })
}

/** 读取任务执行历史；task 可选，按任务名精确过滤 */
export async function fetchHistory(task?: string): Promise<HistoryResponse> {
  const qs = task ? `?task=${encodeURIComponent(task)}` : ''
  return request<HistoryResponse>(`/api/history${qs}`)
}
