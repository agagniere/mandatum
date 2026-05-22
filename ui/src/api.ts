import { Task, TaskStatus, TaskPriority, AgentRole, Agent, Stats, ActivityEntry, PipelineStage } from './types'

const BASE = '/api'

export async function fetchTasks(filters?: { status?: string; role?: string; agent_id?: string }): Promise<Task[]> {
  const params = new URLSearchParams()
  if (filters?.status) params.set('status', filters.status)
  if (filters?.role) params.set('role', filters.role)
  if (filters?.agent_id) params.set('agent_id', filters.agent_id)
  const q = params.toString()
  const res = await fetch(`${BASE}/tasks${q ? '?' + q : ''}`)
  if (!res.ok) throw new Error('Failed to fetch tasks')
  return res.json()
}

export async function fetchTask(id: string): Promise<Task> {
  const res = await fetch(`${BASE}/tasks/${id}`)
  if (!res.ok) throw new Error('Failed to fetch task')
  return res.json()
}

export async function createTask(data: {
  title: string
  description?: string
  priority?: TaskPriority
  assigned_role?: AgentRole
  branch_name?: string
  tags?: string[]
  dependencies?: string[]
}): Promise<Task> {
  const res = await fetch(`${BASE}/tasks`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(data),
  })
  if (!res.ok) throw new Error('Failed to create task')
  return res.json()
}

export async function updateTask(id: string, data: Partial<{
  title: string
  description: string
  status: TaskStatus
  priority: TaskPriority
  assigned_role: AgentRole
  tags: string[]
  dependencies: string[]
}>): Promise<Task> {
  const res = await fetch(`${BASE}/tasks/${id}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(data),
  })
  if (!res.ok) throw new Error('Failed to update task')
  return res.json()
}

export async function deleteTask(id: string): Promise<void> {
  const res = await fetch(`${BASE}/tasks/${id}`, { method: 'DELETE' })
  if (!res.ok) throw new Error('Failed to delete task')
}

export async function fetchActivity(): Promise<ActivityEntry[]> {
  const res = await fetch(`${BASE}/activity`)
  if (!res.ok) throw new Error('Failed to fetch activity')
  return res.json()
}

export async function fetchAgents(): Promise<Agent[]> {
  const res = await fetch(`${BASE}/agents`)
  if (!res.ok) throw new Error('Failed to fetch agents')
  return res.json()
}

export async function fetchStats(): Promise<Stats> {
  const res = await fetch(`${BASE}/stats`)
  if (!res.ok) throw new Error('Failed to fetch stats')
  return res.json()
}

export async function fetchCommits(taskId: string): Promise<import('./types').Commit[]> {
  const res = await fetch(`${BASE}/tasks/${taskId}/commits`)
  if (!res.ok) throw new Error('Failed to fetch commits')
  return res.json()
}

export async function resetTask(id: string): Promise<Task> {
  const res = await fetch(`${BASE}/tasks/${id}/reset`, { method: 'POST' })
  if (!res.ok) throw new Error('Failed to reset task')
  return res.json()
}

export async function reapTasks(): Promise<{ reaped: string[] }> {
  const res = await fetch(`${BASE}/tasks/reap`, { method: 'POST' })
  if (!res.ok) throw new Error('Failed to reap tasks')
  return res.json()
}

export async function stopAgent(id: string): Promise<Agent> {
  const res = await fetch(`${BASE}/agents/${id}/stop`, { method: 'POST' })
  if (!res.ok) throw new Error('Failed to stop agent')
  return res.json()
}

export async function unstopAgent(id: string): Promise<Agent> {
  const res = await fetch(`${BASE}/agents/${id}/stop`, { method: 'DELETE' })
  if (!res.ok) throw new Error('Failed to resume agent')
  return res.json()
}

export async function fetchInfo(): Promise<{ repo_path: string | null; base_branch: string; pipeline: PipelineStage[] }> {
  const res = await fetch(`${BASE}/info`)
  if (!res.ok) return { repo_path: null, base_branch: 'master', pipeline: [] }
  return res.json()
}

export async function fetchAgentLog(id: string): Promise<string[]> {
  const res = await fetch(`${BASE}/agents/${id}/log`)
  if (!res.ok) return []
  return res.json()
}
