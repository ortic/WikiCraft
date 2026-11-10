import { useEffect, useState } from 'react'
import { Link, NavLink, Outlet, useLocation, useMatch, useNavigate } from 'react-router-dom'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Search, Home as HomeIcon, LayoutGrid, Star, Settings, Plus, LogOut, Moon, Shield, Menu, Trash2, UserCog, Newspaper, ListChecks, Download, Info, TriangleAlert, CircleCheck, X } from 'lucide-react'
import ShortcutsDialog from './ShortcutsDialog'
import { api, type Page, type Space } from '../api'
import { Avatar, Logo, SpaceIcon, useFeatures, useMe } from '../ui'
import SpaceTree from './SpaceTree'
import SearchPalette from './SearchPalette'
import CreateSpaceDialog from './CreateSpaceDialog'
import NotificationsMenu from './NotificationsMenu'
import TemplatePicker from './TemplatePicker'

export function useCurrentSpaceKey(): string | undefined {
  const pageMatch = useMatch('/pages/:id/*')
  const spaceMatch = useMatch('/spaces/:key/*')
  const pageId = pageMatch?.params.id
  const page = useQuery({ queryKey: ['page', pageId], queryFn: () => api.get<Page>(`/pages/${pageId}`), enabled: !!pageId, retry: false })
  return spaceMatch?.params.key ?? page.data?.spaceKey
}

export default function Layout() {
  const me = useMe()
  const features = useFeatures()
  const user = me.data?.user
  const nav = useNavigate()
  const qc = useQueryClient()
  const loc = useLocation()
  const [search, setSearch] = useState(false)
  const [createSpace, setCreateSpace] = useState(false)
  const [createPage, setCreatePage] = useState(false)
  const [userMenu, setUserMenu] = useState(false)
  const [mobileOpen, setMobileOpen] = useState(false)
  const spaceKey = useCurrentSpaceKey()
  const spaces = useQuery({ queryKey: ['spaces'], queryFn: () => api.get<Space[]>('/spaces') })
  const favorites = useQuery({ queryKey: ['favorites'], queryFn: () => api.get<{ id: string; title: string }[]>('/favorites'), enabled: !!user })
  const current = spaces.data?.find((s) => s.key.toLowerCase() === spaceKey?.toLowerCase())

  useEffect(() => setMobileOpen(false), [loc.pathname])
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
        e.preventDefault()
        setSearch(true)
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  const logout = async () => {
    await api.post('/auth/logout')
    qc.clear()
    location.href = '/login'
  }
  const toggleTheme = () => {
    const isDark = document.documentElement.dataset.theme === 'dark' || (!document.documentElement.dataset.theme && matchMedia('(prefers-color-scheme: dark)').matches)
    const next = isDark ? 'light' : 'dark'
    document.documentElement.dataset.theme = next
    try {
      localStorage.setItem('wc-theme', next)
    } catch {
      /* ignore */
    }
  }

  return (
    <div className={`shell ${mobileOpen ? 'sidebar-open' : ''}`}>
      <header className="topbar">
        <button className="icon-btn mobile-only" onClick={() => setMobileOpen((o) => !o)} aria-label="Menu">
          <Menu size={18} />
        </button>
        <Link to="/" className="logo-link" style={{ textDecoration: 'none' }}>
          <Logo />
        </Link>
        <button className="search-trigger" onClick={() => setSearch(true)}>
          <Search size={15} /> <span className="search-label">Search pages…</span> <span className="spacer" /> <span className="kbd">Ctrl K</span>
        </button>
        <span className="spacer" />
        {user && current && current.role !== 'viewer' && current.role !== 'commenter' && (
          <button className="btn btn-primary create-btn" onClick={() => setCreatePage(true)} aria-label="Create">
            <Plus size={15} /> <span className="create-label">Create</span>
          </button>
        )}
        {user && <NotificationsMenu />}
        <button className="icon-btn" onClick={toggleTheme} title="Toggle dark mode">
          <Moon size={16} />
        </button>
        {user ? (
          <div style={{ position: 'relative' }}>
            <button className="icon-btn" onClick={() => setUserMenu((o) => !o)} style={{ width: 'auto', padding: '0 4px' }}>
              <Avatar name={user.name} id={user.id} />
            </button>
            {userMenu && (
              <div className="dropdown menu" onMouseLeave={() => setUserMenu(false)}>
                <div className="menu-item" style={{ cursor: 'default' }}>
                  <div>
                    <div style={{ fontWeight: 600 }}>{user.name}</div>
                    <div className="mi-desc">{user.email}</div>
                  </div>
                </div>
                {user.role !== 'guest' && features.personalSpaces !== false && (
                  <div
                    className="menu-item"
                    onClick={async () => {
                      setUserMenu(false)
                      const s = await api.post<{ key: string }>('/me/space')
                      nav(`/spaces/${encodeURIComponent(s.key)}`)
                    }}
                  >
                    <Star size={15} /> Personal space
                  </div>
                )}
                <div className="menu-item" onClick={() => { setUserMenu(false); nav('/settings') }}>
                  <UserCog size={15} /> Profile, API tokens & AI
                </div>
                {(user.role === 'owner' || user.role === 'admin') && (
                  <div className="menu-item" onClick={() => { setUserMenu(false); nav('/admin') }}>
                    <Shield size={15} /> Administration
                  </div>
                )}
                <div className="menu-item" onClick={logout}>
                  <LogOut size={15} /> Log out
                </div>
              </div>
            )}
          </div>
        ) : (
          <Link className="btn" to="/login">
            Log in
          </Link>
        )}
      </header>

      <aside className="sidebar">
        <NavLink to="/" end className="nav-item">
          <HomeIcon size={16} /> Home
        </NavLink>
        <NavLink to="/spaces" end className="nav-item">
          <LayoutGrid size={16} /> Spaces
        </NavLink>
        {user && (
          <NavLink to="/tasks" className="nav-item">
            <ListChecks size={16} /> My tasks
          </NavLink>
        )}
        {user && (user.role === 'owner' || user.role === 'admin') && (
          <NavLink to="/admin" className="nav-item">
            <Shield size={16} /> Admin
          </NavLink>
        )}

        {current ? (
          <>
            <div className="sidebar-section" style={{ marginTop: 18 }}>
              Space
            </div>
            <Link to={`/spaces/${current.key}`} className="space-switch" style={{ color: 'var(--text)', textDecoration: 'none' }}>
              <SpaceIcon space={current} />
              <div style={{ minWidth: 0 }}>
                <div style={{ fontWeight: 600, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{current.name}</div>
                <div className="faint small">{current.pageCount} pages</div>
              </div>
            </Link>
            <NavLink to={`/spaces/${current.key}/blog`} className="nav-item" style={{ marginTop: 4 }}>
              <Newspaper size={15} /> Blog
            </NavLink>
            <SpaceTree space={current} />
            {current.role === 'admin' && (
              <NavLink to={`/spaces/${current.key}/settings`} className={({ isActive }) => `nav-item${isActive && !['trash', 'export'].includes(new URLSearchParams(loc.search).get('tab') ?? '') ? ' active' : ''}`} style={{ marginTop: 8 }}>
                <Settings size={15} /> Space settings
              </NavLink>
            )}
            {(current.role === 'admin' || current.role === 'editor') && (
              <NavLink to={`/spaces/${current.key}/settings?tab=trash`} className={({ isActive }) => `nav-item${isActive && (new URLSearchParams(loc.search).get('tab') ?? '') === 'trash' ? ' active' : ''}`}>
                <Trash2 size={15} /> Trash
              </NavLink>
            )}
            {current.role !== 'admin' && (
              <NavLink to={`/spaces/${current.key}/settings?tab=export`} className={({ isActive }) => `nav-item${isActive && (new URLSearchParams(loc.search).get('tab') ?? '') === 'export' ? ' active' : ''}`}>
                <Download size={15} /> Export
              </NavLink>
            )}
          </>
        ) : (
          <>
            {!!favorites.data?.length && (
              <>
                <div className="sidebar-section">
                  <Star size={12} style={{ marginRight: 4 }} /> Starred
                </div>
                {favorites.data.slice(0, 10).map((f) => (
                  <NavLink key={f.id} to={`/pages/${f.id}`} className="nav-item">
                    <Star size={14} /> {f.title}
                  </NavLink>
                ))}
              </>
            )}
            <div className="sidebar-section">
              Spaces <span className="spacer" />
              {user && user.role !== 'guest' && (
                <button className="icon-btn" style={{ width: 22, height: 22 }} title="Create space" onClick={() => setCreateSpace(true)}>
                  <Plus size={14} />
                </button>
              )}
            </div>
            {spaces.data?.map((s) => (
              <NavLink key={s.id} to={`/spaces/${s.key}`} className="nav-item">
                <SpaceIcon space={s} size={20} /> {s.name}
              </NavLink>
            ))}
          </>
        )}
      </aside>

      <main className="main">
        <AnnouncementBanner />
        <Outlet />
          <ShortcutsDialog />
      </main>
      {search && <SearchPalette onClose={() => setSearch(false)} />}
      {createSpace && <CreateSpaceDialog onClose={() => setCreateSpace(false)} />}
      {createPage && current && <TemplatePicker spaceKey={current.key} onClose={() => setCreatePage(false)} />}
    </div>
  )
}

const DISMISSED = 'wikicraft-announcement-dismissed'

/** The workspace announcement (Admin → Overview); dismissed until it changes. */
function AnnouncementBanner() {
  const q = useQuery({
    queryKey: ['announcement'],
    queryFn: () => api.get<{ text: string; level: 'info' | 'warning' | 'success'; id: string } | null>('/announcement'),
    staleTime: 5 * 60_000,
  })
  const [dismissed, setDismissed] = useState(() => {
    try {
      return localStorage.getItem(DISMISSED)
    } catch {
      return null
    }
  })
  const a = q.data
  if (!a || a.id === dismissed) return null
  const Icon = a.level === 'warning' ? TriangleAlert : a.level === 'success' ? CircleCheck : Info
  return (
    <div className={`announcement announcement-${a.level}`} role="status">
      <Icon size={16} className="announcement-icon" />
      <span className="announcement-text">
        {a.text.split(/(https?:\/\/[^\s]+)/).map((part, i) =>
          i % 2 ? <a key={i} href={part} target="_blank" rel="noreferrer noopener">{part}</a> : part,
        )}
      </span>
      <button
        className="icon-btn"
        title="Dismiss"
        aria-label="Dismiss announcement"
        onClick={() => {
          try {
            localStorage.setItem(DISMISSED, a.id)
          } catch {
            /* private mode: hide for this visit only */
          }
          setDismissed(a.id)
        }}
      >
        <X size={15} />
      </button>
    </div>
  )
}
