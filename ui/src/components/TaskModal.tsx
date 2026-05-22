import { useState } from 'react'
import ReactMarkdown from 'react-markdown'
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { fetchTask, fetchCommits, fetchAgents, fetchTasks, updateTask, deleteTask, resetTask, fetchInfo } from '../api'
import { Task, TaskStatus, TaskPriority, AgentRole, Agent, GIT_ACTIONS, PipelineStage } from '../types'
import AgentBadge from './AgentBadge'
import { PRIORITY_CONFIG } from './TaskCard'
import { formatDistanceToNow } from 'date-fns'
import { X, Copy, Pencil, Trash2, ChevronDown, GitBranch, GitCommit, ExternalLink, FolderOpen, AlertTriangle, RotateCcw, Link, CheckCircle2, Clock } from 'lucide-react'

const PRIORITIES: TaskPriority[] = ['low', 'medium', 'high', 'critical']

const ACTION_ICON: Record<string, string> = {
  branch_created:    '⎇',
  committed:         '●',
  review_requested:  '⇒',
  approved:          '✓',
  changes_requested: '✗',
  pr_opened:         '↑',
  worktree_created:  '⊞',
}

function copyToClipboard(text: string) {
  navigator.clipboard.writeText(text)
}

interface TaskModalProps {
  task: Task
  onClose: () => void
}

export default function TaskModal({ task: initialTask, onClose }: TaskModalProps) {
  const queryClient = useQueryClient()
  const [editing, setEditing] = useState(false)
  const [showStatusMenu, setShowStatusMenu] = useState(false)
  const [showRoleMenu, setShowRoleMenu] = useState(false)
  const [editForm, setEditForm] = useState({
    title: initialTask.title,
    description: initialTask.description ?? '',
    priority: initialTask.priority,
    assigned_role: initialTask.assigned_role ?? '' as AgentRole | '',
    tags: initialTask.tags.join(', '),
    dependencies: initialTask.dependencies ?? [] as string[],
  })

  const { data: task = initialTask } = useQuery({
    queryKey: ['task', initialTask.id],
    queryFn: () => fetchTask(initialTask.id),
  })

  const { data: commits = [] } = useQuery({
    queryKey: ['commits', initialTask.id],
    queryFn: () => fetchCommits(initialTask.id),
  })

  const { data: agents = [] } = useQuery({
    queryKey: ['agents'],
    queryFn: fetchAgents,
    refetchInterval: 15_000,
  })

  const { data: allTasks = [] } = useQuery({
    queryKey: ['tasks'],
    queryFn: () => fetchTasks(),
  })

  const { data: info } = useQuery({
    queryKey: ['info'],
    queryFn: fetchInfo,
    staleTime: Infinity,
  })
  const pipeline: PipelineStage[] = info?.pipeline ?? []

  const assignedAgent = task.assigned_agent_id
    ? (agents as Agent[]).find(a => a.agent_id === task.assigned_agent_id)
    : undefined
  const agentIsStale = assignedAgent?.last_seen
    ? Date.now() - new Date(assignedAgent.last_seen).getTime() > 10 * 60 * 1000
    : false

  const updateMutation = useMutation({
    mutationFn: (data: Parameters<typeof updateTask>[1]) => updateTask(task.id, data),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['tasks'] })
      queryClient.invalidateQueries({ queryKey: ['task', task.id] })
      queryClient.invalidateQueries({ queryKey: ['stats'] })
      setEditing(false)
    },
  })

  const deleteMutation = useMutation({
    mutationFn: () => deleteTask(task.id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['tasks'] })
      queryClient.invalidateQueries({ queryKey: ['stats'] })
      onClose()
    },
  })

  const resetMutation = useMutation({
    mutationFn: () => resetTask(task.id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['tasks'] })
      queryClient.invalidateQueries({ queryKey: ['task', task.id] })
      queryClient.invalidateQueries({ queryKey: ['stats'] })
    },
  })

  const handleSave = () => {
    updateMutation.mutate({
      title: editForm.title,
      description: editForm.description || undefined,
      priority: editForm.priority as TaskPriority,
      assigned_role: (editForm.assigned_role as AgentRole) || undefined,
      tags: editForm.tags ? editForm.tags.split(',').map(t => t.trim()).filter(Boolean) : [],
      dependencies: editForm.dependencies,
    })
  }

  function toggleDepEdit(id: string) {
    setEditForm(f => ({
      ...f,
      dependencies: f.dependencies.includes(id)
        ? f.dependencies.filter(d => d !== id)
        : [...f.dependencies, id],
    }))
  }

  const handleStatusChange = (status: TaskStatus) => {
    updateMutation.mutate({ status })
    setShowStatusMenu(false)
  }

  const handleRoleChange = (role: AgentRole | '') => {
    updateMutation.mutate({ assigned_role: role || undefined })
    setShowRoleMenu(false)
  }

  const p = PRIORITY_CONFIG[task.priority]

  return (
    <div
      className="fixed inset-0 bg-black/60 backdrop-blur-sm z-50 flex items-center justify-center p-4"
      onClick={e => e.target === e.currentTarget && onClose()}
    >
      <div className="bg-slate-900 border border-slate-700 rounded-xl w-full max-w-2xl max-h-[90vh] flex flex-col shadow-2xl">
        {/* Header */}
        <div className="flex items-start justify-between p-5 border-b border-slate-800">
          {editing ? (
            <input value={editForm.title} onChange={e => setEditForm(f => ({ ...f, title: e.target.value }))}
              className="flex-1 bg-slate-800 border border-slate-600 rounded-lg px-3 py-1.5 text-white text-lg font-semibold mr-2 outline-none focus:border-indigo-500" />
          ) : (
            <h2 className="text-lg font-semibold text-white flex-1 mr-3 leading-snug">{task.title}</h2>
          )}
          <div className="flex items-center gap-1.5 shrink-0">
            {!editing && (
              <button onClick={() => setEditing(true)} className="p-1.5 rounded-md text-slate-400 hover:text-white hover:bg-slate-800 transition-colors">
                <Pencil className="w-4 h-4" />
              </button>
            )}
            <button onClick={() => { if (confirm('Delete this task?')) deleteMutation.mutate() }}
              className="p-1.5 rounded-md text-slate-400 hover:text-red-400 hover:bg-slate-800 transition-colors">
              <Trash2 className="w-4 h-4" />
            </button>
            <button onClick={onClose} className="p-1.5 rounded-md text-slate-400 hover:text-white hover:bg-slate-800 transition-colors">
              <X className="w-4 h-4" />
            </button>
          </div>
        </div>

        <div className="flex-1 overflow-y-auto">
          <div className="p-5 space-y-5">
            {/* Meta */}
            <div className="flex flex-wrap items-center gap-2">
              {editing ? (
                <>
                  <select value={editForm.priority} onChange={e => setEditForm(f => ({ ...f, priority: e.target.value as TaskPriority }))}
                    className="bg-slate-800 border border-slate-600 rounded px-2 py-1 text-sm text-white outline-none">
                    {PRIORITIES.map(pr => <option key={pr} value={pr}>{pr}</option>)}
                  </select>
                  <select value={editForm.assigned_role} onChange={e => setEditForm(f => ({ ...f, assigned_role: e.target.value as AgentRole | '' }))}
                    className="bg-slate-800 border border-slate-600 rounded px-2 py-1 text-sm text-white outline-none">
                    <option value="">No role</option>
                    {pipeline.map(s => <option key={s.role} value={s.role}>{s.role.replace(/_/g, ' ')}</option>)}
                  </select>
                </>
              ) : (
                <>
                  <span className={`inline-flex items-center gap-1 text-xs rounded px-2 py-0.5 font-medium ${p.bg} ${p.text}`}>
                    <span className={`w-1.5 h-1.5 rounded-full ${p.dot}`} />{p.label}
                  </span>
                  <div className="relative">
                    <button onClick={() => setShowRoleMenu(!showRoleMenu)}
                      className="flex items-center gap-1 hover:opacity-80 transition-opacity">
                      {task.assigned_role
                        ? <AgentBadge role={task.assigned_role as AgentRole} size="md" />
                        : <span className="text-xs text-slate-500 bg-slate-800 rounded px-2 py-0.5">No role</span>
                      }
                    </button>
                    {showRoleMenu && (
                      <div className="absolute left-0 top-full mt-1 bg-slate-800 border border-slate-700 rounded-lg shadow-xl z-10 py-1 min-w-[8rem]">
                        <button onClick={() => handleRoleChange('')}
                          className="block w-full text-left px-3 py-1.5 text-sm text-slate-400 hover:text-white hover:bg-slate-700">
                          No role
                        </button>
                        {pipeline.map(s => (
                          <button key={s.role} onClick={() => handleRoleChange(s.role)}
                            className="block w-full text-left px-3 py-1.5 text-sm text-slate-300 hover:text-white hover:bg-slate-700 capitalize">
                            {s.role.replace(/_/g, ' ')}
                          </button>
                        ))}
                      </div>
                    )}
                  </div>
                  <span className="text-xs text-slate-500 bg-slate-800 rounded px-2 py-0.5 uppercase font-medium">
                    {task.status.replace(/_/g, ' ')}
                  </span>
                </>
              )}
              {!editing && (
                <div className="relative ml-auto">
                  <button onClick={() => setShowStatusMenu(!showStatusMenu)}
                    className="flex items-center gap-1 text-xs text-slate-400 hover:text-white bg-slate-800 hover:bg-slate-700 rounded px-2 py-1 transition-colors">
                    Move to… <ChevronDown className="w-3 h-3" />
                  </button>
                  {showStatusMenu && (
                    <div className="absolute right-0 top-full mt-1 bg-slate-800 border border-slate-700 rounded-lg shadow-xl z-10 py-1 min-w-[9rem]">
                      {(['backlog', ...pipeline.map(s => s.role), 'done', 'blocked'] as TaskStatus[])
                        .filter(s => s !== task.status).map(s => (
                        <button key={s} onClick={() => handleStatusChange(s)}
                          className="block w-full text-left px-3 py-1.5 text-sm text-slate-300 hover:text-white hover:bg-slate-700 capitalize">
                          {s.replace(/_/g, ' ')}
                        </button>
                      ))}
                    </div>
                  )}
                </div>
              )}
            </div>

            {/* Assigned agent last-seen / stale warning */}
            {!editing && assignedAgent && (
              <div className={`flex items-center justify-between rounded-lg px-3 py-2 text-xs ${agentIsStale ? 'bg-amber-950/40 border border-amber-800/50' : 'bg-slate-800/50'}`}>
                <div className="flex items-center gap-2">
                  {agentIsStale && <AlertTriangle className="w-3.5 h-3.5 text-amber-400 shrink-0" />}
                  <span className={agentIsStale ? 'text-amber-300' : 'text-slate-400'}>
                    <span className="font-mono">{assignedAgent.agent_id}</span>
                    {assignedAgent.last_seen && (
                      <span className="ml-2 text-slate-500">
                        last seen {formatDistanceToNow(new Date(assignedAgent.last_seen), { addSuffix: true })}
                      </span>
                    )}
                  </span>
                </div>
                {(agentIsStale || task.status === 'in_progress') && (
                  <button
                    onClick={() => { if (confirm('Reset task to its role queue and unassign agent?')) resetMutation.mutate() }}
                    disabled={resetMutation.isPending}
                    className="flex items-center gap-1 text-xs text-slate-400 hover:text-amber-300 hover:bg-amber-900/30 rounded px-2 py-1 transition-colors disabled:opacity-50"
                  >
                    <RotateCcw className="w-3 h-3" /> Reset
                  </button>
                )}
              </div>
            )}

            {/* Description */}
            <div>
              <label className="text-xs text-slate-500 uppercase font-medium block mb-1">Description</label>
              {editing ? (
                <textarea value={editForm.description} onChange={e => setEditForm(f => ({ ...f, description: e.target.value }))}
                  rows={3} className="w-full bg-slate-800 border border-slate-600 rounded-lg px-3 py-2 text-sm text-slate-200 outline-none focus:border-indigo-500 resize-none" />
              ) : task.description ? (
                <div className="prose prose-invert prose-sm max-w-none text-slate-300
                  prose-p:my-1 prose-p:leading-relaxed
                  prose-headings:text-slate-200 prose-headings:font-semibold
                  prose-h1:text-base prose-h2:text-sm prose-h3:text-sm
                  prose-strong:text-slate-200
                  prose-code:text-slate-300 prose-code:bg-slate-800 prose-code:px-1 prose-code:rounded prose-code:text-xs prose-code:before:content-none prose-code:after:content-none
                  prose-pre:bg-slate-800 prose-pre:text-xs prose-pre:p-3 prose-pre:rounded-lg
                  prose-ul:my-1 prose-ul:pl-4 prose-ol:my-1 prose-ol:pl-4 prose-li:my-0
                  prose-a:text-indigo-400 prose-a:no-underline hover:prose-a:underline
                  prose-blockquote:border-slate-600 prose-blockquote:text-slate-400">
                  <ReactMarkdown>{task.description}</ReactMarkdown>
                </div>
              ) : (
                <span className="text-slate-600 text-sm italic">No description</span>
              )}
            </div>

            {/* Tags */}
            <div>
              <label className="text-xs text-slate-500 uppercase font-medium block mb-1">Tags</label>
              {editing ? (
                <input value={editForm.tags} onChange={e => setEditForm(f => ({ ...f, tags: e.target.value }))}
                  placeholder="auth, backend (comma-separated)"
                  className="w-full bg-slate-800 border border-slate-600 rounded-lg px-3 py-1.5 text-sm text-slate-200 outline-none focus:border-indigo-500" />
              ) : (
                <div className="flex flex-wrap gap-1">
                  {task.tags.length > 0
                    ? task.tags.map(tag => <span key={tag} className="text-xs bg-slate-800 text-slate-400 rounded px-2 py-0.5">{tag}</span>)
                    : <span className="text-slate-600 text-sm italic">None</span>}
                </div>
              )}
            </div>

            {/* Dependencies */}
            <div>
              <label className="text-xs text-slate-500 uppercase font-medium block mb-1.5">
                Dependencies
              </label>
              {editing ? (
                <div className="max-h-40 overflow-y-auto rounded-lg border border-slate-600 bg-slate-800/50 divide-y divide-slate-700">
                  {(allTasks as Task[]).filter(t => t.id !== task.id).map(t => (
                    <label key={t.id} className="flex items-center gap-2.5 px-3 py-2 cursor-pointer hover:bg-slate-700/50 transition-colors">
                      <input
                        type="checkbox"
                        checked={editForm.dependencies.includes(t.id)}
                        onChange={() => toggleDepEdit(t.id)}
                        className="accent-indigo-500 w-3.5 h-3.5 shrink-0"
                      />
                      <span className="text-sm text-slate-300 truncate flex-1">{t.title}</span>
                      <span className={`text-xs shrink-0 px-1.5 py-0.5 rounded ${
                        t.status === 'done' ? 'text-emerald-400 bg-emerald-900/40' : 'text-slate-500 bg-slate-700'
                      }`}>{t.status.replace(/_/g, ' ')}</span>
                    </label>
                  ))}
                  {(allTasks as Task[]).filter(t => t.id !== task.id).length === 0 && (
                    <p className="px-3 py-2 text-xs text-slate-600 italic">No other tasks</p>
                  )}
                </div>
              ) : task.dependencies.length > 0 ? (
                <div className="space-y-1.5">
                  {task.dependencies.map(depId => {
                    const dep = (allTasks as Task[]).find(t => t.id === depId)
                    const done = dep?.status === 'done'
                    return (
                      <div key={depId} className={`flex items-center gap-2 rounded-md px-3 py-2 text-sm ${done ? 'bg-emerald-900/20 border border-emerald-800/40' : 'bg-slate-800 border border-slate-700'}`}>
                        {done
                          ? <CheckCircle2 className="w-3.5 h-3.5 text-emerald-500 shrink-0" />
                          : <Clock className="w-3.5 h-3.5 text-amber-500 shrink-0" />
                        }
                        <span className={`flex-1 truncate ${done ? 'text-emerald-300' : 'text-slate-300'}`}>
                          {dep ? dep.title : <span className="font-mono text-xs text-slate-500">{depId}</span>}
                        </span>
                        {dep && (
                          <span className={`text-xs shrink-0 ${done ? 'text-emerald-500' : 'text-slate-500'}`}>
                            {dep.status.replace(/_/g, ' ')}
                          </span>
                        )}
                      </div>
                    )
                  })}
                </div>
              ) : (
                <p className="text-sm text-slate-600 italic">None</p>
              )}
            </div>

            {editing && (
              <div className="flex gap-2">
                <button onClick={handleSave} disabled={updateMutation.isPending}
                  className="px-4 py-2 bg-indigo-600 hover:bg-indigo-500 text-white rounded-lg text-sm font-medium transition-colors disabled:opacity-50">
                  Save Changes
                </button>
                <button onClick={() => setEditing(false)}
                  className="px-4 py-2 bg-slate-800 hover:bg-slate-700 text-slate-300 rounded-lg text-sm font-medium transition-colors">
                  Cancel
                </button>
              </div>
            )}

            {/* ── Git section ─────────────────────────────────────────── */}
            <div className="border border-slate-800 rounded-lg overflow-hidden">
              <div className="flex items-center gap-2 px-3 py-2 bg-slate-800/50 border-b border-slate-800">
                <GitBranch className="w-3.5 h-3.5 text-slate-500" />
                <span className="text-xs font-semibold text-slate-400 uppercase tracking-wider">Git</span>
                {task.commit_count > 0 && (
                  <span className="ml-auto text-xs text-slate-600 flex items-center gap-1">
                    <GitCommit className="w-3 h-3" />{task.commit_count} commit{task.commit_count !== 1 ? 's' : ''}
                  </span>
                )}
              </div>
              <div className="p-3 space-y-3">
                {/* Branch */}
                {task.branch_name ? (
                  <div className="flex items-center gap-2">
                    <GitBranch className="w-3.5 h-3.5 text-emerald-500 shrink-0" />
                    <code className="text-sm text-emerald-400 font-mono flex-1 truncate">{task.branch_name}</code>
                    <button onClick={() => copyToClipboard(task.branch_name!)}
                      className="text-slate-600 hover:text-slate-300 transition-colors shrink-0">
                      <Copy className="w-3.5 h-3.5" />
                    </button>
                  </div>
                ) : (
                  <p className="text-xs text-slate-600 italic">No branch yet — agent should call <code className="text-slate-500">create_branch</code></p>
                )}

                {/* Base branch + diff command */}
                {task.branch_name && (
                  <div className="bg-slate-900 rounded-md px-3 py-2 text-xs font-mono text-slate-500 space-y-0.5">
                    <div className="flex items-center justify-between">
                      <span className="text-slate-600">$ git checkout {task.branch_name}</span>
                      <button onClick={() => copyToClipboard(`git checkout ${task.branch_name}`)}
                        className="text-slate-700 hover:text-slate-400 transition-colors ml-2 shrink-0">
                        <Copy className="w-3 h-3" />
                      </button>
                    </div>
                    <div className="flex items-center justify-between">
                      <span className="text-slate-600">$ git diff {task.base_branch}</span>
                      <button onClick={() => copyToClipboard(`git diff ${task.base_branch}`)}
                        className="text-slate-700 hover:text-slate-400 transition-colors ml-2 shrink-0">
                        <Copy className="w-3 h-3" />
                      </button>
                    </div>
                  </div>
                )}

                {/* Latest commit */}
                {task.latest_commit && (
                  <div className="flex items-center gap-2">
                    <GitCommit className="w-3.5 h-3.5 text-slate-500 shrink-0" />
                    <code className="text-xs text-slate-400 font-mono">{task.latest_commit.slice(0, 12)}</code>
                    <button onClick={() => copyToClipboard(task.latest_commit!)}
                      className="text-slate-600 hover:text-slate-300 transition-colors">
                      <Copy className="w-3 h-3" />
                    </button>
                  </div>
                )}

                {/* PR URL */}
                {task.pr_url && (
                  <div className="flex items-center gap-2">
                    <ExternalLink className="w-3.5 h-3.5 text-indigo-400 shrink-0" />
                    <a href={task.pr_url} target="_blank" rel="noopener noreferrer"
                      className="text-xs text-indigo-400 hover:text-indigo-300 underline truncate flex-1">
                      {task.pr_url}
                    </a>
                  </div>
                )}

                {/* Worktree path */}
                {task.worktree_path && (
                  <div className="flex items-center gap-2">
                    <FolderOpen className="w-3.5 h-3.5 text-amber-500 shrink-0" />
                    <code className="text-xs text-amber-400 font-mono flex-1 truncate">{task.worktree_path}</code>
                    <button onClick={() => copyToClipboard(task.worktree_path!)}
                      className="text-slate-600 hover:text-slate-300 transition-colors shrink-0">
                      <Copy className="w-3 h-3" />
                    </button>
                  </div>
                )}

                {/* Commits list */}
                {commits.length > 0 && (
                  <div className="space-y-1.5 border-t border-slate-800 pt-3 mt-1">
                    <p className="text-xs text-slate-600 uppercase font-medium mb-2">Commits</p>
                    {commits.map(c => (
                      <div key={c.id} className="flex items-start gap-2 text-xs">
                        <code className="text-slate-600 font-mono shrink-0 mt-0.5">{c.hash.slice(0, 8)}</code>
                        <span className="text-slate-300 flex-1 leading-snug">{c.message}</span>
                        <span className="text-slate-600 shrink-0">
                          {formatDistanceToNow(new Date(c.timestamp), { addSuffix: true })}
                        </span>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            </div>

            {/* Output path */}
            {task.output_path && (
              <div>
                <label className="text-xs text-slate-500 uppercase font-medium block mb-1">Output Path</label>
                <div className="flex items-center gap-2 bg-slate-800 rounded-lg px-3 py-2">
                  <code className="text-sm text-emerald-400 flex-1 font-mono truncate">{task.output_path}</code>
                  <button onClick={() => copyToClipboard(task.output_path!)}
                    className="text-slate-500 hover:text-white transition-colors">
                    <Copy className="w-3.5 h-3.5" />
                  </button>
                </div>
              </div>
            )}

            {/* Activity log */}
            {task.activity && task.activity.length > 0 && (
              <div>
                <label className="text-xs text-slate-500 uppercase font-medium block mb-2">Activity</label>
                <div className="space-y-2">
                  {task.activity.map(entry => {
                    const isGit = GIT_ACTIONS.has(entry.action)
                    return (
                      <div key={entry.id} className={`flex gap-3 text-xs rounded-md px-2 py-1.5 ${isGit ? 'bg-slate-800/60 border border-slate-800' : ''}`}>
                        <span className={`shrink-0 w-4 text-center font-mono ${isGit ? 'text-emerald-600' : 'text-slate-700'}`}>
                          {ACTION_ICON[entry.action] ?? '·'}
                        </span>
                        <span className="text-slate-600 font-mono shrink-0 pt-0.5">
                          {formatDistanceToNow(new Date(entry.timestamp), { addSuffix: true })}
                        </span>
                        <div className="flex-1 min-w-0">
                          <span className="text-slate-400">{entry.agent_id ?? 'System'}</span>
                          {entry.agent_role && <span className="text-slate-600 ml-1">({entry.agent_role})</span>}
                          <span className="text-slate-500 mx-1">·</span>
                          <span className={`font-medium ${isGit ? 'text-emerald-400' : 'text-slate-300'}`}>
                            {entry.action.replace(/_/g, ' ')}
                          </span>
                          {entry.detail && (
                            <div className="text-slate-500 mt-0.5 prose prose-invert prose-xs max-w-none
                              prose-p:my-0.5 prose-p:leading-snug
                              prose-code:text-slate-400 prose-code:bg-slate-800 prose-code:px-1 prose-code:rounded prose-code:text-[0.7rem]
                              prose-pre:bg-slate-800 prose-pre:text-xs prose-pre:my-1 prose-pre:p-2 prose-pre:rounded
                              prose-ul:my-0.5 prose-ul:pl-4 prose-ol:my-0.5 prose-ol:pl-4
                              prose-li:my-0 prose-strong:text-slate-300 prose-a:text-indigo-400">
                              <ReactMarkdown>{entry.detail}</ReactMarkdown>
                            </div>
                          )}
                        </div>
                      </div>
                    )
                  })}
                </div>
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  )
}
