import { useEffect, useMemo, useState } from 'react'
import { Link, useParams } from 'react-router-dom'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Briefcase, Clock, FileText, Mail, MapPin, Search, Users } from 'lucide-react'
import { api, formatDate } from '../api'
import { Avatar, Spinner, useMe, useToast } from '../ui'

export interface Person {
  id: string
  name: string
  jobTitle: string | null
  department: string | null
  location: string | null
  timezone: string | null
  avatarUrl: string | null
}

export interface Profile extends Person {
  bio: string | null
  email: string | null
  deactivated: boolean
  personalSpaceKey: string | null
  recentPages: { id: string; title: string; spaceKey: string; spaceName: string; editedAt: string }[]
}

/** The time where someone is, e.g. "14:05", or null for an unknown zone. */
function localTime(timezone: string | null, now: Date): string | null {
  if (!timezone) return null
  try {
    return new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit', timeZone: timezone }).format(now)
  } catch {
    return null
  }
}

function useMinute() {
  const [now, setNow] = useState(() => new Date())
  useEffect(() => {
    const t = setInterval(() => setNow(new Date()), 30_000)
    return () => clearInterval(t)
  }, [])
  return now
}

/** People directory: everybody with an account, searchable, by department. */
export function PeoplePage() {
  const [q, setQ] = useState('')
  const [department, setDepartment] = useState<string | null>(null)
  const params = new URLSearchParams()
  if (q.trim()) params.set('q', q.trim())
  if (department) params.set('department', department)
  const people = useQuery({
    queryKey: ['people', q.trim(), department],
    queryFn: () => api.get<{ people: Person[]; departments: { name: string; count: number }[] }>(`/people?${params}`),
    placeholderData: (prev) => prev,
  })
  return (
    <div className="content-pad" style={{ maxWidth: 1100 }}>
      <h1 style={{ marginTop: 0 }} className="row"><Users size={22} /> People</h1>
      <div className="row" style={{ flexWrap: 'wrap', gap: 8, marginBottom: 14 }}>
        <div className="search-input" style={{ flex: '1 1 260px', maxWidth: 420 }}>
          <Search size={15} />
          <input className="input" placeholder="Name, job, department or location" aria-label="Search people" value={q} onChange={(e) => setQ(e.target.value)} />
        </div>
      </div>
      {!!people.data?.departments.length && (
        <div className="row people-departments" style={{ flexWrap: 'wrap', gap: 6, marginBottom: 16 }}>
          <button className={`chip${department === null ? ' active' : ''}`} onClick={() => setDepartment(null)}>Everybody</button>
          {people.data.departments.map((d) => (
            <button key={d.name} className={`chip${department?.toLowerCase() === d.name.toLowerCase() ? ' active' : ''}`} onClick={() => setDepartment(d.name)}>
              {d.name} <span className="faint">{d.count}</span>
            </button>
          ))}
        </div>
      )}
      {people.isLoading && <Spinner />}
      {people.data && !people.data.people.length && <div className="empty">Nobody matches that.</div>}
      <div className="grid-cards people-grid">
        {people.data?.people.map((p) => (
          <Link key={p.id} to={`/people/${p.id}`} className="person-card">
            <Avatar name={p.name} id={p.id} size={40} />
            <div style={{ minWidth: 0 }}>
              <div className="person-name">{p.name}</div>
              {(p.jobTitle || p.department) && <div className="faint small">{[p.jobTitle, p.department].filter(Boolean).join(' · ')}</div>}
              {p.location && <div className="faint small row" style={{ gap: 4 }}><MapPin size={12} /> {p.location}</div>}
            </div>
          </Link>
        ))}
      </div>
    </div>
  )
}

/** Someone's profile: what they do, where and when they are, what they worked on lately. */
export function ProfilePage() {
  const { id } = useParams()
  const me = useMe()
  const now = useMinute()
  const profile = useQuery({ queryKey: ['person', id], queryFn: () => api.get<Profile>(`/people/${id}`), enabled: !!id })
  if (profile.isLoading) return <div className="content-pad"><Spinner /></div>
  if (!profile.data) return <div className="empty">This person isn't in the directory.</div>
  const p = profile.data
  const time = localTime(p.timezone, now)
  const isMe = me.data?.user?.id === p.id
  return (
    <div className="content-pad" style={{ maxWidth: 860 }}>
      <div className="profile-head">
        <Avatar name={p.name} id={p.id} size={72} />
        <div style={{ minWidth: 0 }}>
          <h1 style={{ margin: 0 }}>{p.name}</h1>
          {(p.jobTitle || p.department) && (
            <div className="muted row" style={{ gap: 6 }}>
              <Briefcase size={14} /> {[p.jobTitle, p.department].filter(Boolean).join(' · ')}
            </div>
          )}
          <div className="faint small row" style={{ gap: 12, flexWrap: 'wrap', marginTop: 4 }}>
            {p.location && <span className="row" style={{ gap: 4 }}><MapPin size={13} /> {p.location}</span>}
            {time && <span className="row" style={{ gap: 4 }} title={p.timezone ?? undefined}><Clock size={13} /> {time} local time</span>}
            {p.email && <a className="row" style={{ gap: 4 }} href={`mailto:${p.email}`}><Mail size={13} /> {p.email}</a>}
            {p.deactivated && <span className="badge">Deactivated</span>}
          </div>
        </div>
        <span className="spacer" />
        {isMe && <Link className="btn btn-sm" to="/settings">Edit your profile</Link>}
      </div>
      {p.bio && <p className="profile-bio">{p.bio}</p>}
      {p.personalSpaceKey && (
        <p><Link to={`/spaces/${p.personalSpaceKey}`}>Personal space</Link></p>
      )}
      <h3>Recently worked on</h3>
      {p.recentPages.length === 0 ? (
        <div className="empty">Nothing you can see yet.</div>
      ) : (
        <div className="list-plain">
          {p.recentPages.map((r) => (
            <Link key={r.id} to={`/pages/${r.id}`} className="menu-item">
              <FileText size={15} />
              <div style={{ minWidth: 0 }}>
                <div>{r.title}</div>
                <div className="mi-desc">{r.spaceName} · {formatDate(r.editedAt)}</div>
              </div>
            </Link>
          ))}
        </div>
      )}
    </div>
  )
}

/** Settings → About you: the profile fields others see in the directory. */
export function ProfileFields({ userId }: { userId: string }) {
  const qc = useQueryClient()
  const toast = useToast()
  const profile = useQuery({ queryKey: ['person', userId], queryFn: () => api.get<Profile>(`/people/${userId}`) })
  const departments = useQuery({ queryKey: ['people', '', null], queryFn: () => api.get<{ people: Person[]; departments: { name: string; count: number }[] }>('/people') })
  const zones = useMemo(() => {
    try {
      return (Intl as unknown as { supportedValuesOf?: (k: string) => string[] }).supportedValuesOf?.('timeZone') ?? []
    } catch {
      return []
    }
  }, [])
  const [form, setForm] = useState<{ jobTitle: string; department: string; location: string; timezone: string; bio: string } | null>(null)
  useEffect(() => {
    if (profile.data && !form) {
      const p = profile.data
      setForm({ jobTitle: p.jobTitle ?? '', department: p.department ?? '', location: p.location ?? '', timezone: p.timezone ?? '', bio: p.bio ?? '' })
    }
  }, [profile.data, form])
  if (!form) return null
  const set = (k: keyof typeof form) => (e: { target: { value: string } }) => setForm({ ...form, [k]: e.target.value })
  const browserZone = Intl.DateTimeFormat().resolvedOptions().timeZone
  return (
    <div className="card" style={{ marginBottom: 16 }}>
      <h3 style={{ marginTop: 0 }}>About you</h3>
      <p className="muted small" style={{ marginTop: 0 }}>Shown on your profile and in the <Link to="/people">people directory</Link>.</p>
      <div className="profile-fields">
        <div className="field"><label htmlFor="pf-title">Job title</label><input id="pf-title" className="input" maxLength={100} value={form.jobTitle} onChange={set('jobTitle')} /></div>
        <div className="field">
          <label htmlFor="pf-dept">Department</label>
          <input id="pf-dept" className="input" maxLength={100} list="pf-departments" value={form.department} onChange={set('department')} />
          <datalist id="pf-departments">{departments.data?.departments.map((d) => <option key={d.name} value={d.name} />)}</datalist>
        </div>
        <div className="field"><label htmlFor="pf-location">Location</label><input id="pf-location" className="input" maxLength={100} value={form.location} onChange={set('location')} /></div>
        <div className="field">
          <label htmlFor="pf-tz">Time zone</label>
          <select id="pf-tz" className="input" value={form.timezone} onChange={set('timezone')}>
            <option value="">Not set</option>
            {!zones.includes(browserZone) && browserZone && <option value={browserZone}>{browserZone}</option>}
            {zones.map((z) => <option key={z} value={z}>{z}</option>)}
          </select>
        </div>
      </div>
      <div className="field"><label htmlFor="pf-bio">About</label><textarea id="pf-bio" className="input" rows={3} maxLength={2000} value={form.bio} onChange={set('bio')} /></div>
      <div className="row" style={{ gap: 8 }}>
        <button
          className="btn btn-primary"
          onClick={async () => {
            try {
              const saved = await api.put<Profile>('/me/profile', form)
              qc.setQueryData(['person', userId], saved)
              qc.invalidateQueries({ queryKey: ['people'] })
              toast('Profile saved')
            } catch (e) {
              toast(e instanceof Error ? e.message : 'Could not save')
            }
          }}
        >
          Save
        </button>
        {!form.timezone && browserZone && (
          <button className="btn" onClick={() => setForm({ ...form, timezone: browserZone })}>Use {browserZone}</button>
        )}
        <span className="spacer" />
        <Link to={`/people/${userId}`}>View your profile</Link>
      </div>
    </div>
  )
}
