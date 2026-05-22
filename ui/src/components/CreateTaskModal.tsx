import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { createTask, fetchTasks, fetchInfo } from '../api'
import { TaskPriority, AgentRole } from '../types'
import { X, PlusCircle } from 'lucide-react'

interface CreateTaskModalProps {
  onClose: () => void
}

export default function CreateTaskModal({ onClose }: CreateTaskModalProps) {
  const queryClient = useQueryClient()
  const [form, setForm] = useState({
    title: '',
    description: '',
    priority: 'medium' as TaskPriority,
    assigned_role: '' as AgentRole | '',
    branch_name: '',
    tags: '',
    dependencies: [] as string[],
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
  const pipeline = info?.pipeline ?? []

  const mutation = useMutation({
    mutationFn: () => createTask({
      title: form.title,
      description: form.description || undefined,
      priority: form.priority,
      assigned_role: (form.assigned_role as AgentRole) || undefined,
      branch_name: form.branch_name || undefined,
      tags: form.tags ? form.tags.split(',').map(t => t.trim()).filter(Boolean) : [],
      dependencies: form.dependencies,
    }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['tasks'] })
      queryClient.invalidateQueries({ queryKey: ['stats'] })
      onClose()
    },
  })

  function toggleDependency(id: string) {
    setForm(f => ({
      ...f,
      dependencies: f.dependencies.includes(id)
        ? f.dependencies.filter(d => d !== id)
        : [...f.dependencies, id],
    }))
  }

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault()
    if (!form.title.trim()) return
    mutation.mutate()
  }

  return (
    <div
      className="fixed inset-0 bg-black/60 backdrop-blur-sm z-50 flex items-center justify-center p-4"
      onClick={e => e.target === e.currentTarget && onClose()}
    >
      <div className="bg-slate-900 border border-slate-700 rounded-xl w-full max-w-md shadow-2xl">
        <div className="flex items-center justify-between p-5 border-b border-slate-800">
          <div className="flex items-center gap-2">
            <PlusCircle className="w-4 h-4 text-indigo-400" />
            <h2 className="font-semibold text-white">New Task</h2>
          </div>
          <button onClick={onClose} className="p-1.5 rounded-md text-slate-400 hover:text-white hover:bg-slate-800 transition-colors">
            <X className="w-4 h-4" />
          </button>
        </div>

        <form onSubmit={handleSubmit} className="p-5 space-y-4">
          <div>
            <label className="text-xs text-slate-500 uppercase font-medium block mb-1">Title *</label>
            <input value={form.title} onChange={e => setForm(f => ({ ...f, title: e.target.value }))}
              placeholder="Task title…" required
              className="w-full bg-slate-800 border border-slate-600 rounded-lg px-3 py-2 text-sm text-white outline-none focus:border-indigo-500 placeholder:text-slate-600" />
          </div>

          <div>
            <label className="text-xs text-slate-500 uppercase font-medium block mb-1">Description</label>
            <textarea value={form.description} onChange={e => setForm(f => ({ ...f, description: e.target.value }))}
              placeholder="Describe the task…" rows={3}
              className="w-full bg-slate-800 border border-slate-600 rounded-lg px-3 py-2 text-sm text-white outline-none focus:border-indigo-500 placeholder:text-slate-600 resize-none" />
          </div>

          <div className="grid grid-cols-2 gap-3">
            <div>
              <label className="text-xs text-slate-500 uppercase font-medium block mb-1">Priority</label>
              <select value={form.priority} onChange={e => setForm(f => ({ ...f, priority: e.target.value as TaskPriority }))}
                className="w-full bg-slate-800 border border-slate-600 rounded-lg px-3 py-2 text-sm text-white outline-none">
                <option value="low">Low</option>
                <option value="medium">Medium</option>
                <option value="high">High</option>
                <option value="critical">Critical</option>
              </select>
            </div>
            <div>
              <label className="text-xs text-slate-500 uppercase font-medium block mb-1">Place in</label>
              <select value={form.assigned_role} onChange={e => setForm(f => ({ ...f, assigned_role: e.target.value as AgentRole | '' }))}
                className="w-full bg-slate-800 border border-slate-600 rounded-lg px-3 py-2 text-sm text-white outline-none">
                <option value="">Backlog</option>
                {pipeline.map(s => (
                  <option key={s.role} value={s.role}>
                    {s.role.replace(/_/g, ' ').replace(/\b\w/g, c => c.toUpperCase())}
                  </option>
                ))}
              </select>
            </div>
          </div>

          <div>
            <label className="text-xs text-slate-500 uppercase font-medium block mb-1">Branch</label>
            <input value={form.branch_name} onChange={e => setForm(f => ({ ...f, branch_name: e.target.value }))}
              placeholder="auto (leave blank for coder tasks)"
              className="w-full bg-slate-800 border border-slate-600 rounded-lg px-3 py-2 text-sm text-white outline-none focus:border-indigo-500 placeholder:text-slate-600 font-mono" />
          </div>

          <div>
            <label className="text-xs text-slate-500 uppercase font-medium block mb-1">Tags</label>
            <input value={form.tags} onChange={e => setForm(f => ({ ...f, tags: e.target.value }))}
              placeholder="auth, backend (comma-separated)"
              className="w-full bg-slate-800 border border-slate-600 rounded-lg px-3 py-2 text-sm text-white outline-none focus:border-indigo-500 placeholder:text-slate-600" />
          </div>

          {allTasks.length > 0 && (
            <div>
              <label className="text-xs text-slate-500 uppercase font-medium block mb-1">
                Depends on
                {form.dependencies.length > 0 && (
                  <span className="ml-1.5 text-indigo-400">({form.dependencies.length} selected)</span>
                )}
              </label>
              <div className="max-h-36 overflow-y-auto rounded-lg border border-slate-600 bg-slate-800 divide-y divide-slate-700">
                {allTasks.map(t => (
                  <label key={t.id} className="flex items-center gap-2.5 px-3 py-2 cursor-pointer hover:bg-slate-700/50 transition-colors">
                    <input
                      type="checkbox"
                      checked={form.dependencies.includes(t.id)}
                      onChange={() => toggleDependency(t.id)}
                      className="accent-indigo-500 w-3.5 h-3.5 shrink-0"
                    />
                    <span className="text-sm text-slate-300 truncate flex-1">{t.title}</span>
                    <span className={`text-xs shrink-0 px-1.5 py-0.5 rounded ${
                      t.status === 'done' ? 'text-emerald-400 bg-emerald-900/40' : 'text-slate-500 bg-slate-700'
                    }`}>{t.status.replace(/_/g, ' ')}</span>
                  </label>
                ))}
              </div>
            </div>
          )}

          <div className="flex gap-2 pt-1">
            <button type="submit" disabled={!form.title.trim() || mutation.isPending}
              className="flex-1 px-4 py-2 bg-indigo-600 hover:bg-indigo-500 text-white rounded-lg text-sm font-medium transition-colors disabled:opacity-50">
              {mutation.isPending ? 'Creating…' : 'Create Task'}
            </button>
            <button type="button" onClick={onClose}
              className="px-4 py-2 bg-slate-800 hover:bg-slate-700 text-slate-300 rounded-lg text-sm font-medium transition-colors">
              Cancel
            </button>
          </div>
        </form>
      </div>
    </div>
  )
}
