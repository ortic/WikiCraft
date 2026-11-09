import { useEffect, useState } from 'react'
import { Link, useNavigate, useParams } from 'react-router-dom'
import { useQuery } from '@tanstack/react-query'
import { api, formatDate } from '../api'
import { Logo, Spinner } from '../ui'

interface PublicPage {
  id: string
  rootId: string
  title: string
  html: string
  updatedAt: string
  updatedByName: string | null
  spaceName: string
  tree: { id: string; parentId: string | null; title: string }[]
}

/** `/s/:token[/:pageId]` — read-only view of a publicly shared page, no account needed. */
export default function PublicPageView() {
  const { token, pageId } = useParams()
  const navigate = useNavigate()
  const q = useQuery({
    queryKey: ['public', token, pageId],
    queryFn: () => api.get<PublicPage>(pageId ? `/public/${token}/pages/${pageId}` : `/public/${token}`),
    retry: false,
  })
  const [navOpen, setNavOpen] = useState(false)
  useEffect(() => {
    if (q.data) document.title = `${q.data.title} · ${q.data.spaceName}`
  }, [q.data])
  if (q.isLoading) return <div className="public-shell"><Spinner /></div>
  if (q.error || !q.data)
    return (
      <div className="auth">
        <div className="auth-card">
          <Logo />
          <h1>Link not available</h1>
          <p className="muted">This link doesn't exist, has expired or was turned off. Ask the person who shared it for a new one.</p>
        </div>
      </div>
    )
  const p = q.data
  const byParent = new Map<string, PublicPage['tree']>()
  for (const n of p.tree) byParent.set(n.parentId ?? '', [...(byParent.get(n.parentId ?? '') ?? []), n])
  const renderTree = (parent: string, level: number): React.ReactNode => {
    const kids = byParent.get(parent)
    if (!kids?.length || level > 30) return null
    return (
      <ul>
        {kids.map((k) => (
          <li key={k.id}>
            <Link className={k.id === p.id ? 'current' : undefined} to={k.id === p.rootId ? `/s/${token}` : `/s/${token}/${k.id}`} onClick={() => setNavOpen(false)}>
              {k.title}
            </Link>
            {renderTree(k.id, level + 1)}
          </li>
        ))}
      </ul>
    )
  }
  // keep navigation between shared pages inside the app
  const onClick = (e: React.MouseEvent) => {
    const a = (e.target as HTMLElement).closest('a')
    const href = a?.getAttribute('href')
    if (href && href.startsWith(`/s/${token}`) && !e.metaKey && !e.ctrlKey) {
      e.preventDefault()
      navigate(href)
    }
  }
  return (
    <div className="public-shell">
      <header className="public-header">
        <Logo />
        <span className="faint small">{p.spaceName}</span>
        <span className="spacer" />
        {p.tree.length > 1 && <button className="btn btn-ghost small public-nav-toggle" onClick={() => setNavOpen((o) => !o)}>Pages</button>}
        <span className="badge">Shared publicly · read-only</span>
      </header>
      <div className="public-body">
        {p.tree.length > 1 && <nav className={`public-nav${navOpen ? ' open' : ''}`}>{renderTree('', 0)}</nav>}
        <main className="public-main">
          <h1>{p.title}</h1>
          <p className="faint small">Updated {formatDate(p.updatedAt)}{p.updatedByName ? ` by ${p.updatedByName}` : ''}</p>
          <article className="ProseMirror public-article" onClick={onClick} dangerouslySetInnerHTML={{ __html: p.html }} />
        </main>
      </div>
    </div>
  )
}
