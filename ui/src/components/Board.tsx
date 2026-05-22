import { useState } from 'react'
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { fetchTasks, fetchAgents, fetchInfo, updateTask } from '../api'
import { Task, TaskStatus, Agent, PipelineStage } from '../types'
import TaskCard from './TaskCard'
import TaskModal from './TaskModal'

// Fixed non-role columns that always appear
const PREFIX_COLUMNS = [{ status: 'backlog', label: 'Backlog' }]
const SUFFIX_COLUMNS = [
  { status: 'done',    label: 'Done'    },
  { status: 'blocked', label: 'Blocked' },
]

// Cycle through a simple palette for pipeline role columns
const ROLE_ACCENT_PALETTE = [
  'border-t-blue-500',
  'border-t-purple-500',
  'border-t-amber-500',
  'border-t-green-500',
  'border-t-cyan-500',
  'border-t-rose-500',
  'border-t-indigo-500',
]

const FIXED_ACCENT: Record<string, string> = {
  backlog: 'border-t-slate-500',
  done:    'border-t-emerald-500',
  blocked: 'border-t-red-500',
}

function accentFor(status: string, roleIndex: number): string {
  return FIXED_ACCENT[status] ?? ROLE_ACCENT_PALETTE[roleIndex % ROLE_ACCENT_PALETTE.length]
}

function columnsFromPipeline(pipeline: PipelineStage[]) {
  const roles = pipeline.map((s, i) => ({
    status: s.role,
    label: s.role.replace(/_/g, ' ').replace(/\b\w/g, c => c.toUpperCase()),
    accentIndex: i,
  }))
  return [
    ...PREFIX_COLUMNS.map(c => ({ ...c, accentIndex: -1 })),
    ...roles,
    ...SUFFIX_COLUMNS.map(c => ({ ...c, accentIndex: -1 })),
  ]
}

interface BoardProps {
  onTaskSelect: (task: Task | null) => void
  selectedTask: Task | null
}

export default function Board({ onTaskSelect, selectedTask }: BoardProps) {
  const queryClient = useQueryClient()
  const [draggedTaskId, setDraggedTaskId] = useState<string | null>(null)
  const [dragOverColumn, setDragOverColumn] = useState<TaskStatus | null>(null)

  const { data: tasks = [], isLoading } = useQuery({
    queryKey: ['tasks'],
    queryFn: () => fetchTasks(),
  })

  const { data: agents = [] } = useQuery({
    queryKey: ['agents'],
    queryFn: fetchAgents,
    refetchInterval: 15_000,
  })

  const { data: info } = useQuery({
    queryKey: ['info'],
    queryFn: fetchInfo,
    staleTime: Infinity,
  })

  const pipeline: PipelineStage[] = info?.pipeline ?? []
  const columns = columnsFromPipeline(pipeline)

  const agentMap = Object.fromEntries((agents as Agent[]).map(a => [a.agent_id, a]))

  const updateMutation = useMutation({
    mutationFn: ({ id, status }: { id: string; status: TaskStatus }) =>
      updateTask(id, { status }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['tasks'] })
      queryClient.invalidateQueries({ queryKey: ['stats'] })
    },
  })

  // Build a set of known column statuses so tasks with unknown statuses land in a catch-all
  const knownStatuses = new Set(columns.map(c => c.status))
  const tasksByStatus = Object.fromEntries(
    columns.map(col => [
      col.status,
      tasks.filter(t => t.status === col.status),
    ])
  ) as Record<string, Task[]>

  // Tasks with statuses not in any column go into backlog as a safety net
  for (const task of tasks) {
    if (!knownStatuses.has(task.status)) {
      tasksByStatus['backlog'] = [...(tasksByStatus['backlog'] ?? []), task]
    }
  }

  const handleDragStart = (e: React.DragEvent, taskId: string) => {
    setDraggedTaskId(taskId)
    e.dataTransfer.effectAllowed = 'move'
  }

  const handleDrop = (e: React.DragEvent, status: TaskStatus) => {
    e.preventDefault()
    if (draggedTaskId) {
      const task = tasks.find(t => t.id === draggedTaskId)
      if (task && task.status !== status) {
        updateMutation.mutate({ id: draggedTaskId, status })
      }
    }
    setDraggedTaskId(null)
    setDragOverColumn(null)
  }

  if (isLoading) {
    return <div className="flex items-center justify-center h-full text-slate-500">Loading…</div>
  }

  return (
    <>
      <div className="flex gap-3 h-full overflow-x-auto p-4 pb-6">
        {columns.map(({ status, label, accentIndex }) => {
          const col = tasksByStatus[status] ?? []
          const isDragOver = dragOverColumn === status
          const accent = accentFor(status, accentIndex)
          return (
            <div
              key={status}
              className={`flex flex-col w-60 shrink-0 rounded-lg border-t-2 ${accent} bg-slate-900 border border-slate-800 transition-colors duration-150 ${isDragOver ? 'ring-1 ring-indigo-500 bg-slate-800' : ''}`}
              onDragOver={e => { e.preventDefault(); setDragOverColumn(status) }}
              onDragLeave={() => setDragOverColumn(null)}
              onDrop={e => handleDrop(e, status)}
            >
              <div className="flex items-center justify-between px-3 py-2.5 border-b border-slate-800">
                <span className="text-xs font-semibold text-slate-300 uppercase tracking-wider">{label}</span>
                <span className="text-xs bg-slate-800 text-slate-400 rounded-full px-2 py-0.5 font-mono">{col.length}</span>
              </div>
              <div className="flex-1 overflow-y-auto p-2 space-y-2 min-h-0">
                {col.map(task => (
                  <TaskCard
                    key={task.id}
                    task={task}
                    agent={task.assigned_agent_id ? agentMap[task.assigned_agent_id] : undefined}
                    allTasks={tasks}
                    onClick={() => onTaskSelect(task)}
                    onDragStart={e => handleDragStart(e, task.id)}
                  />
                ))}
                {col.length === 0 && (
                  <div className={`flex items-center justify-center h-16 rounded-md border-2 border-dashed text-xs ${isDragOver ? 'border-indigo-600 text-indigo-500' : 'border-slate-800 text-slate-600'}`}>
                    Drop here
                  </div>
                )}
              </div>
            </div>
          )
        })}
      </div>

      {selectedTask && (
        <TaskModal task={selectedTask} onClose={() => onTaskSelect(null)} />
      )}
    </>
  )
}
