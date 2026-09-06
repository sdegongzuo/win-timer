<script setup lang="ts">
import { computed, onMounted, onUnmounted, reactive, ref } from 'vue'
import {
  createTask,
  deleteTask,
  fetchHealth,
  listTasks,
  runVerb,
} from './api'
import { BACKEND_PORT } from './config'
import type { TaskSummary } from './types'

const tasks = ref<TaskSummary[]>([])
const loading = ref(false)
const backendOk = ref<boolean | null>(null)
const scope = ref<'root' | 'all'>('root')
const error = ref<string | null>(null)
const busyName = ref<string | null>(null)

async function refresh() {
  loading.value = true
  error.value = null
  try {
    tasks.value = await listTasks(scope.value)
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
  } finally {
    loading.value = false
  }
}

function stateLabel(t: TaskSummary): { text: string; cls: string } {
  const s = t.state
  if (s === 'Running') return { text: '运行中', cls: 'badge running' }
  if (s === 'Disabled') return { text: '已禁用', cls: 'badge disabled' }
  if (s === 'Ready') return { text: '就绪', cls: 'badge ready' }
  return { text: s || '未知', cls: 'badge unknown' }
}

function fmt(dt: string | null): string {
  if (!dt) return '—'
  return dt.replace('T', ' ').replace(/-/g, '/')
}

function resultText(n: number | null): { text: string; cls: string } {
  if (n === null) return { text: '—', cls: '' }
  if (n === 0) return { text: '成功 (0)', cls: 'ok' }
  return { text: `失败 (${n})`, cls: 'bad' }
}

async function act(verb: 'run' | 'end' | 'enable' | 'disable', t: TaskSummary) {
  busyName.value = `${verb}:${t.path}${t.name}`
  error.value = null
  try {
    await runVerb(verb, t.name, t.path)
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
  } finally {
    busyName.value = null
    await refresh()
  }
}

async function onDelete(t: TaskSummary) {
  const full = `${t.path}${t.name}`
  if (!confirm(`确定删除计划任务「${full}」？此操作不可撤销。`)) return
  busyName.value = `del:${full}`
  error.value = null
  try {
    await deleteTask(t.name, t.path)
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
  } finally {
    busyName.value = null
    await refresh()
  }
}

/* ---------------- create form ---------------- */
const showCreate = ref(false)
const form = reactive({
  name: '',
  program: '',
  arguments: '',
  description: '',
  ty: 'once' as 'once' | 'interval' | 'daily',
  at: '',
  every_minutes: 5,
  busy: false,
})

function nowLocal(): string {
  const d = new Date()
  d.setMinutes(d.getMinutes() + 1)
  const pad = (x: number) => String(x).padStart(2, '0')
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`
}

function openCreate() {
  Object.assign(form, {
    name: '',
    program: '',
    arguments: '',
    description: '',
    ty: 'once',
    at: nowLocal(),
    every_minutes: 5,
    busy: false,
  })
  showCreate.value = true
}

async function submitCreate() {
  if (form.busy) return
  form.busy = true
  error.value = null
  try {
    const needsAt = form.ty !== 'interval'
    if (needsAt && !form.at) {
      throw new Error('请选择开始时间')
    }
    await createTask({
      name: form.name.trim(),
      program: form.program.trim(),
      arguments: form.arguments.trim() || null,
      description: form.description.trim() || null,
      schedule: {
        ty: form.ty,
        at: form.at || null,
        every_minutes: form.ty === 'interval' ? form.every_minutes : null,
      },
    })
    showCreate.value = false
    await refresh()
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
  } finally {
    form.busy = false
  }
}

const runningVerb = (verb: string, t: TaskSummary) => busyName.value === `${verb}:${t.path}${t.name}`
const deletingVerb = (t: TaskSummary) => busyName.value === `del:${t.path}${t.name}`

const filteredCount = computed(() => tasks.value.length)

/* auto refresh + health probe */
let timer: number | undefined
onMounted(async () => {
  backendOk.value = await fetchHealth()
  await refresh()
  timer = window.setInterval(async () => {
    backendOk.value = await fetchHealth()
    await refresh()
  }, 15000)
})
onUnmounted(() => {
  if (timer) window.clearInterval(timer)
})
</script>

<template>
  <div class="shell">
    <header class="topbar">
      <div class="brand">
        <span class="logo">⏱</span>
        <div>
          <h1>Win-Timer</h1>
          <p class="sub">Windows 计划任务管理控制台</p>
        </div>
      </div>
      <div class="health">
        <span
          class="dot"
          :class="backendOk === null ? 'dot-unknown' : backendOk ? 'dot-ok' : 'dot-bad'"
        ></span>
        <span v-if="backendOk === null">检测后端…</span>
        <span v-else-if="backendOk">后端已连接 (127.0.0.1:{{ BACKEND_PORT }})</span>
        <span v-else>后端未连接</span>
      </div>
    </header>

    <div v-if="error" class="banner error">{{ error }}</div>

    <main class="content">
      <div class="toolbar">
        <div class="left">
          <button class="seg" :class="{ active: scope === 'root' }" @click="scope = 'root'; refresh()">
            根目录任务
          </button>
          <button class="seg" :class="{ active: scope === 'all' }" @click="scope = 'all'; refresh()">
            全部任务
          </button>
          <span class="count">{{ filteredCount }} 项</span>
        </div>
        <div class="right">
          <button class="btn ghost" :disabled="loading" @click="refresh()">⟳ 刷新</button>
          <button class="btn primary" @click="openCreate">＋ 新建任务</button>
        </div>
      </div>

      <div class="table-wrap">
        <table>
          <thead>
            <tr>
              <th>名称</th>
              <th>路径</th>
              <th>状态</th>
              <th>上次运行</th>
              <th>下次运行</th>
              <th>上次结果</th>
              <th>执行的程序</th>
              <th class="actions-col">操作</th>
            </tr>
          </thead>
          <tbody>
            <tr v-if="loading">
              <td colspan="8" class="empty">加载中…</td>
            </tr>
            <tr v-else-if="tasks.length === 0">
              <td colspan="8" class="empty">暂无任务。点击「新建任务」创建一个计划任务。</td>
            </tr>
            <tr v-for="t in tasks" :key="t.path + t.name">
              <td class="name">
                {{ t.name }}
                <div v-if="t.description" class="desc" :title="t.description">{{ t.description }}</div>
              </td>
              <td class="mono dim">{{ t.path }}</td>
              <td>
                <span :class="stateLabel(t).cls">{{ stateLabel(t).text }}</span>
              </td>
              <td>{{ fmt(t.last_run_time) }}</td>
              <td>{{ fmt(t.next_run_time) }}</td>
              <td>
                <span :class="resultText(t.last_result).cls">{{ resultText(t.last_result).text }}</span>
              </td>
              <td class="mono dim cmd-cell" :title="`${t.executable ?? ''} ${t.arguments ?? ''}`">
                <div class="cmd-exe">{{ t.executable || '—' }}</div>
                <div v-if="t.arguments" class="cmd-arg">{{ t.arguments }}</div>
              </td>
              <td class="actions">
                <button
                  class="mini"
                  :disabled="t.state !== 'Ready' && t.state !== 'Running'"
                  @click="act('run', t)"
                >
                  {{ runningVerb('run', t) ? '…' : '▶ 运行' }}
                </button>
                <button
                  class="mini"
                  :disabled="t.state !== 'Running'"
                  @click="act('end', t)"
                >
                  {{ runningVerb('end', t) ? '…' : '■ 停止' }}
                </button>
                <button
                  v-if="t.enabled"
                  class="mini"
                  @click="act('disable', t)"
                >
                  {{ runningVerb('disable', t) ? '…' : '禁用' }}
                </button>
                <button v-else class="mini accent" @click="act('enable', t)">
                  {{ runningVerb('enable', t) ? '…' : '启用' }}
                </button>
                <button class="mini danger" :disabled="deletingVerb(t)" @click="onDelete(t)">
                  {{ deletingVerb(t) ? '…' : '删除' }}
                </button>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </main>

    <!-- create modal -->
    <div v-if="showCreate" class="overlay" @click.self="showCreate = false">
      <div class="modal">
        <h2>新建计划任务</h2>
        <form @submit.prevent="submitCreate">
          <label>
            任务名称
            <input v-model="form.name" required placeholder="例如: 备份数据" />
          </label>
          <label>
            要执行的程序
            <input v-model="form.program" required placeholder="例如: C:\Windows\System32\cmd.exe" />
          </label>
          <label>
            参数 (可选)
            <input v-model="form.arguments" placeholder="例如: /c backup.bat" />
          </label>
          <label>
            说明 (可选)
            <input v-model="form.description" placeholder="给任务一个说明" />
          </label>

          <fieldset class="schedule">
            <legend>调度计划</legend>
            <div class="type-row">
              <label class="radio">
                <input v-model="form.ty" type="radio" value="once" />
                <span>一次性</span>
              </label>
              <label class="radio">
                <input v-model="form.ty" type="radio" value="interval" />
                <span>按间隔重复</span>
              </label>
              <label class="radio">
                <input v-model="form.ty" type="radio" value="daily" />
                <span>每天</span>
              </label>
            </div>

            <label v-if="form.ty === 'once'">
              执行时间
              <input v-model="form.at" type="datetime-local" />
            </label>

            <label v-if="form.ty === 'interval'" class="inline">
              每
              <input v-model.number="form.every_minutes" type="number" min="1" class="num" />
              分钟重复
            </label>
            <label v-if="form.ty === 'interval'">
              首次执行时间 (可选)
              <input v-model="form.at" type="datetime-local" />
            </label>

            <label v-if="form.ty === 'daily'">
              每天开始时间
              <input v-model="form.at" type="datetime-local" />
            </label>
          </fieldset>

          <div class="modal-actions">
            <button type="button" class="btn ghost" @click="showCreate = false">取消</button>
            <button type="submit" class="btn primary" :disabled="form.busy">
              {{ form.busy ? '创建中…' : '创建' }}
            </button>
          </div>
        </form>
      </div>
    </div>
  </div>
</template>
