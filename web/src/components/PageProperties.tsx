import { useEffect, useMemo, useRef, useState, type ComponentType } from 'react'
import { Link } from 'react-router-dom'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { AlignLeft, Calendar, CircleDot, FileText, Hash, Link2, Plus, Trash2, User, X } from 'lucide-react'
import { api, formatDateOnly, type Page, type PageProperty, type PropertyType, type SearchHit } from '../api'
import { Avatar, useToast } from '../ui'

export const PROPERTY_TYPES: { type: PropertyType; label: string; icon: ComponentType<{ size?: number; className?: string }> }[] = [
  { type: 'text', label: 'Text', icon: AlignLeft },
  { type: 'select', label: 'Select', icon: CircleDot },
  { type: 'number', label: 'Number', icon: Hash },
  { type: 'date', label: 'Date', icon: Calendar },
  { type: 'user', label: 'Person', icon: User },
  { type: 'url', label: 'Web address', icon: Link2 },
  { type: 'page', label: 'Page', icon: FileText },
]

const typeInfo = (t: PropertyType) => PROPERTY_TYPES.find((x) => x.type === t) ?? PROPERTY_TYPES[0]

interface KeyUse {
  key: string
  type: PropertyType
  pages: number
  values: string[]
}

/** Same value, same colour (from the theme's callout tones). */
function tone(v: string) {
  let h = 0
  for (const c of v.toLowerCase()) h = (h * 31 + c.charCodeAt(0)) >>> 0
  return ['info', 'note', 'success', 'warning', 'error', 'neutral'][h % 6]
}

function shortUrl(u: string) {
  try {
    const url = new URL(u)
    if (url.protocol === 'mailto:') return url.pathname
    const path = url.pathname === '/' ? '' : url.pathname
    const s = url.host + path
    return s.length > 48 ? s.slice(0, 47) + '…' : s
  } catch {
    return u
  }
}

/** A property's value, read-only. */
export function PropertyValue({ prop }: { prop: Pick<PageProperty, 'type' | 'value' | 'label'> }) {
  const v = prop.value
  if (v === null || v === undefined || v === '') return <span className="faint">Empty</span>
  switch (prop.type) {
    case 'select':
      return <span className="prop-chip" data-tone={tone(String(v))}>{String(v)}</span>
    case 'number':
      return <span>{typeof v === 'number' ? v.toLocaleString() : String(v)}</span>
    case 'date': {
      return <time dateTime={String(v)}>{formatDateOnly(String(v))}</time>
    }
    case 'user':
      return prop.label ? (
        <Link to={`/people/${v}`} className="prop-person">
          <Avatar name={prop.label} id={String(v)} size={18} /> {prop.label}
        </Link>
      ) : (
        <span className="faint">Unknown person</span>
      )
    case 'url':
      return (
        <a href={String(v)} target="_blank" rel="noopener noreferrer" title={String(v)}>
          {shortUrl(String(v))}
        </a>
      )
    case 'page':
      return prop.label ? (
        <Link to={`/pages/${v}`} className="prop-page">
          <FileText size={13} /> {prop.label}
        </Link>
      ) : (
        <span className="faint">A page you can't open</span>
      )
    default:
      return <span className="prop-text">{String(v)}</span>
  }
}

type Draft = { key: string; type: PropertyType; value: string | number | null; label: string | null }

/** Typed key/value fields above the page body; editors add, change and remove them. */
export function PageProperties({ page, canEdit, adding, onAddingDone }: { page: Page; canEdit: boolean; adding: boolean; onAddingDone: () => void }) {
  const qc = useQueryClient()
  const toast = useToast()
  const props = page.properties ?? []
  const [editing, setEditing] = useState<number | 'new' | null>(null)
  useEffect(() => {
    if (adding) {
      setEditing('new')
      onAddingDone()
    }
  }, [adding, onAddingDone])
  const uses = useQuery({
    queryKey: ['space-properties', page.spaceKey],
    queryFn: () => api.get<KeyUse[]>(`/spaces/${page.spaceKey}/properties`),
    enabled: canEdit && editing !== null,
    staleTime: 30_000,
  })
  if (!props.length && editing === null) return null

  const save = async (next: Draft[]) => {
    try {
      const r = await api.put<{ properties: PageProperty[] }>(`/pages/${page.id}/properties`, {
        properties: next.map((p) => ({ key: p.key, type: p.type, value: p.value })),
      })
      qc.setQueryData(['page', page.id], (old: Page | undefined) => (old ? { ...old, properties: r.properties } : old))
      qc.invalidateQueries({ queryKey: ['space-properties', page.spaceKey] })
      qc.invalidateQueries({ queryKey: ['report-props'] })
      setEditing(null)
      return true
    } catch (e) {
      toast(e instanceof Error ? e.message : 'Could not save the property')
      return false
    }
  }
  const asDrafts = (): Draft[] => props.map((p) => ({ key: p.key, type: p.type, value: p.value, label: p.label }))

  return (
    <div className="page-props" role="table" aria-label="Page properties">
      {props.map((pr, i) =>
        editing === i ? (
          <PropertyEditor
            key={pr.key}
            initial={pr}
            uses={uses.data ?? []}
            taken={props.filter((_, j) => j !== i).map((p) => p.key.toLowerCase())}
            onCancel={() => setEditing(null)}
            onSave={(d) => save(asDrafts().map((x, j) => (j === i ? d : x)))}
            onRemove={() => save(asDrafts().filter((_, j) => j !== i))}
          />
        ) : (
          <div className="page-prop" role="row" key={pr.key}>
            <div className="page-prop-key" role="rowheader">
              {canEdit ? (
                <button className="page-prop-key-btn" title="Change this property" onClick={() => setEditing(i)}>
                  <PropIcon type={pr.type} /> <span>{pr.key}</span>
                </button>
              ) : (
                <span className="page-prop-key-btn">
                  <PropIcon type={pr.type} /> <span>{pr.key}</span>
                </span>
              )}
            </div>
            <div className="page-prop-value" role="cell">
              {canEdit && (pr.value === null || pr.value === '') ? (
                <button className="btn btn-ghost btn-sm faint" onClick={() => setEditing(i)}>Add a value</button>
              ) : (
                <PropertyValue prop={pr} />
              )}
            </div>
          </div>
        ),
      )}
      {editing === 'new' && (
        <PropertyEditor
          uses={uses.data ?? []}
          taken={props.map((p) => p.key.toLowerCase())}
          onCancel={() => setEditing(null)}
          onSave={(d) => save([...asDrafts(), d])}
        />
      )}
      {canEdit && editing === null && (
        <div>
          <button className="btn btn-ghost btn-sm" onClick={() => setEditing('new')}>
            <Plus size={13} /> Add property
          </button>
        </div>
      )}
    </div>
  )
}

function PropIcon({ type }: { type: PropertyType }) {
  const Icon = typeInfo(type).icon
  return <Icon size={13} className="faint" />
}

function PropertyEditor({
  initial,
  uses,
  taken,
  onSave,
  onCancel,
  onRemove,
}: {
  initial?: PageProperty
  uses: KeyUse[]
  taken: string[]
  onSave: (d: Draft) => Promise<boolean>
  onCancel: () => void
  onRemove?: () => void
}) {
  const [key, setKey] = useState(initial?.key ?? '')
  const [type, setType] = useState<PropertyType>(initial?.type ?? 'text')
  const [typeTouched, setTypeTouched] = useState(!!initial)
  const [value, setValue] = useState<string | number | null>(initial?.value ?? null)
  const [label, setLabel] = useState<string | null>(initial?.label ?? null)
  const [busy, setBusy] = useState(false)
  const keyRef = useRef<HTMLInputElement>(null)
  useEffect(() => keyRef.current?.focus(), [])
  const use = uses.find((u) => u.key.toLowerCase() === key.trim().toLowerCase())
  // a name used elsewhere in the space brings its type along
  useEffect(() => {
    if (use && !typeTouched) setType(use.type)
  }, [use, typeTouched])
  const changeType = (t: PropertyType) => {
    setTypeTouched(true)
    if (t === 'user' || t === 'page' || type === 'user' || type === 'page') {
      setValue(null)
      setLabel(null)
    } else if (value !== null && t !== 'number' && typeof value === 'number') {
      setValue(String(value))
    }
    setType(t)
  }
  const duplicate = taken.includes(key.trim().toLowerCase())
  const submit = async () => {
    if (!key.trim() || duplicate || busy) return
    setBusy(true)
    const ok = await onSave({ key: key.trim(), type, value: value === '' ? null : value, label })
    if (!ok) setBusy(false)
  }
  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Escape') {
      e.preventDefault()
      onCancel()
    } else if (e.key === 'Enter' && !(e.target instanceof HTMLTextAreaElement && !e.metaKey && !e.ctrlKey)) {
      e.preventDefault()
      submit()
    }
  }
  const listId = useMemo(() => `prop-keys-${Math.random().toString(36).slice(2)}`, [])
  return (
    <div className="page-prop-editor" onKeyDown={onKeyDown}>
      <div className="row" style={{ gap: 6, flexWrap: 'wrap' }}>
        <input
          ref={keyRef}
          className="input"
          style={{ flex: '1 1 160px' }}
          placeholder="Property name"
          aria-label="Property name"
          maxLength={100}
          list={listId}
          value={key}
          onChange={(e) => setKey(e.target.value)}
        />
        <datalist id={listId}>
          {uses.filter((u) => !taken.includes(u.key.toLowerCase())).map((u) => <option key={u.key} value={u.key} />)}
        </datalist>
        <select className="select" style={{ width: 150 }} aria-label="Property type" value={type} onChange={(e) => changeType(e.target.value as PropertyType)}>
          {PROPERTY_TYPES.map((t) => <option key={t.type} value={t.type}>{t.label}</option>)}
        </select>
      </div>
      <div style={{ marginTop: 6 }}>
        <ValueInput type={type} value={value} label={label} suggestions={use?.values ?? []} onChange={(v, l) => { setValue(v); setLabel(l ?? null) }} />
      </div>
      {duplicate && <div className="small" style={{ color: 'var(--danger)', marginTop: 4 }}>This page already has a property with that name.</div>}
      <div className="row" style={{ gap: 6, marginTop: 8 }}>
        <button className="btn btn-primary btn-sm" disabled={!key.trim() || duplicate || busy} onClick={submit}>Save</button>
        <button className="btn btn-sm" onClick={onCancel}>Cancel</button>
        <span className="spacer" />
        {onRemove && (
          <button className="btn btn-ghost btn-sm" onClick={onRemove}>
            <Trash2 size={13} /> Remove
          </button>
        )}
      </div>
    </div>
  )
}

function ValueInput({
  type,
  value,
  label,
  suggestions,
  onChange,
}: {
  type: PropertyType
  value: string | number | null
  label: string | null
  suggestions: string[]
  onChange: (v: string | number | null, label?: string | null) => void
}) {
  const listId = useMemo(() => `prop-values-${Math.random().toString(36).slice(2)}`, [])
  const str = value === null ? '' : String(value)
  switch (type) {
    case 'number':
      return <input className="input" type="number" step="any" aria-label="Value" value={str} onChange={(e) => onChange(e.target.value === '' ? null : e.target.value)} />
    case 'date':
      return <input className="input" type="date" aria-label="Value" style={{ width: 180 }} value={str} onChange={(e) => onChange(e.target.value || null)} />
    case 'url':
      return <input className="input" type="url" aria-label="Value" placeholder="https://" value={str} onChange={(e) => onChange(e.target.value)} />
    case 'select':
      return (
        <>
          <input className="input" aria-label="Value" placeholder="Pick or type a value" list={listId} maxLength={200} value={str} onChange={(e) => onChange(e.target.value)} />
          <datalist id={listId}>{suggestions.map((s) => <option key={s} value={s} />)}</datalist>
          {suggestions.length > 0 && (
            <div className="row" style={{ gap: 4, flexWrap: 'wrap', marginTop: 6 }}>
              {suggestions.slice(0, 12).map((s) => (
                <button key={s} type="button" className="prop-chip prop-chip-btn" data-tone={tone(s)} aria-pressed={s === str} onClick={() => onChange(s)}>{s}</button>
              ))}
            </div>
          )}
        </>
      )
    case 'user':
      return <Picker kind="user" value={str} label={label} onChange={onChange} />
    case 'page':
      return <Picker kind="page" value={str} label={label} onChange={onChange} />
    default:
      return <textarea className="input" rows={2} aria-label="Value" maxLength={5000} value={str} onChange={(e) => onChange(e.target.value)} />
  }
}

/** Search people or pages; shows the pick with a way to clear it. */
function Picker({ kind, value, label, onChange }: { kind: 'user' | 'page'; value: string; label: string | null; onChange: (v: string | null, label?: string | null) => void }) {
  const [q, setQ] = useState('')
  const hits = useQuery({
    queryKey: ['prop-pick', kind, q],
    queryFn: async () =>
      kind === 'user'
        ? (await api.get<{ id: string; name: string }[]>(`/users?q=${encodeURIComponent(q)}`)).map((u) => ({ id: u.id, name: u.name, detail: '' }))
        : (await api.get<SearchHit[]>(`/search?q=${encodeURIComponent(q)}&limit=8`)).map((h) => ({ id: h.pageId, name: h.title, detail: h.spaceName })),
    enabled: q.trim().length > (kind === 'page' ? 1 : 0),
  })
  if (value) {
    return (
      <div className="row" style={{ gap: 6 }}>
        <PropertyValue prop={{ type: kind, value, label }} />
        <button type="button" className="icon-btn" title={kind === 'user' ? 'Pick someone else' : 'Pick another page'} onClick={() => onChange(null, null)}>
          <X size={13} />
        </button>
      </div>
    )
  }
  return (
    <div>
      <input
        className="input"
        aria-label={kind === 'user' ? 'Search people' : 'Search pages'}
        placeholder={kind === 'user' ? 'Search people' : 'Search pages by title or text'}
        value={q}
        onChange={(e) => setQ(e.target.value)}
      />
      {(hits.data ?? []).length > 0 && (
        <div className="list-plain" role="listbox" style={{ marginTop: 4, maxHeight: 220, overflow: 'auto' }}>
          {hits.data!.slice(0, 8).map((h) => (
            <button key={h.id} type="button" role="option" aria-selected={false} className="menu-item" style={{ width: '100%' }} onClick={() => onChange(h.id, h.name)}>
              {kind === 'user' ? <Avatar name={h.name} id={h.id} size={18} /> : <FileText size={14} />}
              <span>{h.name}</span>
              {h.detail && <span className="faint small">{h.detail}</span>}
            </button>
          ))}
        </div>
      )}
    </div>
  )
}
