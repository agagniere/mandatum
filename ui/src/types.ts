export type TaskStatus = string
export type TaskPriority = 'low' | 'medium' | 'high' | 'critical'
export type AgentRole = string

export interface PipelineStage {
  role: string
  transition: { success: string | null; failure: string | null }
}

export interface Task {
  id: string
  title: string
  description?: string
  status: TaskStatus
  assigned_role?: AgentRole
  assigned_agent_id?: string
  priority: TaskPriority
  created_at: string
  updated_at: string
  output_path?: string
  tags: string[]
  dependencies: string[]
  // git
  branch_name?: string
  base_branch: string
  latest_commit?: string
  commit_count: number
  pr_url?: string
  worktree_path?: string
  activity?: ActivityEntry[]
}

export interface Commit {
  id: string
  task_id: string
  agent_id?: string
  hash: string
  message: string
  timestamp: string
}

export interface ActivityEntry {
  id: string
  task_id: string
  agent_id?: string
  agent_role?: AgentRole
  action: string
  detail?: string
  timestamp: string
}

export interface Agent {
  agent_id: string
  role: AgentRole
  last_seen?: string
  current_task_id?: string
  stop_requested: boolean
}

export interface Stats {
  total: number
  by_status: Record<string, number>
  by_role: Record<string, number>
}

export interface AgentLogLine {
  agent_id: string
  line: string
  ts: string
}

// Activity actions that are git-related
export const GIT_ACTIONS = new Set([
  'branch_created', 'committed', 'review_requested',
  'approved', 'changes_requested', 'pr_opened', 'worktree_created',
])
