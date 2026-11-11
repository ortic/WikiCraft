import { t as tr } from '../i18n'
import { useState } from 'react'
import { Link } from 'react-router-dom'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { CalendarDays, ListChecks } from 'lucide-react'
import { api, formatDateOnly } from '../api'
import { Spinner } from '../ui'

type Task = { pageId: string; idx: number; text: string; done: boolean; due: string | null; pageTitle: string; spaceKey: string; spaceName: string }

const STATUSES = [
  ['open', tr('Open')],
  ['done', tr('Done')],
  ['all', tr('All')],
] as const

function dueLabel(due: string) {
  const today = new Date().toISOString().slice(0, 10)
  const text = formatDateOnly(due, { omitYear: due.slice(0, 4) === today.slice(0, 4) })
  return { text, overdue: due < today, today: due === today }
}

/** Tasks assigned to me (task items mentioning me, optional date = due) across all pages I can see. */
export default function TasksPage() {
  const qc = useQueryClient()
  const [status, setStatus] = useState<'open' | 'done' | 'all'>('open')
  const tasks = useQuery({ queryKey: ['tasks', status], queryFn: () => api.get<Task[]>(`/tasks?status=${status}`) })
  const toggle = useMutation({
    mutationFn: (t: Task) => api.post(`/pages/${t.pageId}/tasks/${t.idx}/toggle`, {}),
    onSettled: () => qc.invalidateQueries({ queryKey: ['tasks'] }),
  })
  const groups = new Map<string, Task[]>()
  for (const t of tasks.data ?? []) {
    const k = `${t.pageId}`
    groups.set(k, [...(groups.get(k) ?? []), t])
  }
  return (
    <div className="content-pad">
      <h1 className="row">
        <ListChecks size={22} />{' '}{tr('My tasks')}</h1>
      <div className="tabs">
        {STATUSES.map(([k, label]) => (
          <button key={k} className={`tab${status === k ? ' active' : ''}`} onClick={() => setStatus(k)}>
            {label}
          </button>
        ))}
      </div>
      {tasks.isLoading && <Spinner />}
      {[...groups.values()].map((items) => (
        <section key={items[0].pageId} className="task-group">
          <Link to={`/pages/${items[0].pageId}`} className="task-page">
            {items[0].pageTitle} <span className="faint small">· {items[0].spaceName}</span>
          </Link>
          <ul className="task-list">
            {items.map((t) => {
              const due = t.due ? dueLabel(t.due) : null
              return (
                <li key={t.idx} className={t.done ? 'done' : ''}>
                  <input type="checkbox" checked={t.done} disabled={toggle.isPending} onChange={() => toggle.mutate(t)} aria-label={t.done ? tr('Mark “{text}” open', { text: t.text }) : tr('Mark “{text}” done', { text: t.text })} />
                  <span className="task-text">{t.text || <em className="faint">{tr('(empty task)')}</em>}</span>
                  {due && (
                    <span className={`due${due.overdue && !t.done ? ' overdue' : ''}${due.today ? ' today' : ''}`}>
                      <CalendarDays size={12} /> {due.text}
                    </span>
                  )}
                </li>
              )
            })}
          </ul>
        </section>
      ))}
      {tasks.data?.length === 0 && (
        <div className="empty">{status === 'open' ? tr('Nothing assigned to you. A task shows up here when a task item mentions you (@name); add a date to give it a due date.') : tr('No tasks.')}</div>
      )}
    </div>
  )
}
