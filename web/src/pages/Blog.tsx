import { t } from '../i18n'
import { Link, useNavigate, useParams } from 'react-router-dom'
import { useCanonicalSpaceKey } from '../useCanonicalKey'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { MessageSquare, PenSquare, Rss } from 'lucide-react'
import { api, can, formatDate, type Space } from '../api'
import { Avatar, Spinner } from '../ui'

interface BlogItem {
  id: string
  title: string
  excerpt: string
  createdAt: string
  createdByName: string | null
  commentCount: number
}

export default function Blog() {
  const { key } = useParams()
  const nav = useNavigate()
  const qc = useQueryClient()
  const space = useQuery({ queryKey: ['space', key], queryFn: () => api.get<Space>(`/spaces/${key}`) })
  useCanonicalSpaceKey(key, space.data?.key)
  const posts = useQuery({ queryKey: ['blog', key], queryFn: () => api.get<BlogItem[]>(`/spaces/${key}/blog`) })
  const canWrite = space.data && ['editor', 'admin'].includes(space.data.role) && can(space.data, 'blog.add')
  const write = async () => {
    const r = await api.post<{ id: string }>('/pages', { spaceKey: key, kind: 'blog', title: t('Untitled post') })
    qc.invalidateQueries({ queryKey: ['blog', key] })
    nav(`/pages/${r.id}?new=1`)
  }
  return (
    <div className="content-pad" style={{ maxWidth: 820 }}>
      <div className="row">
        <h1 style={{ margin: 0 }}>{t('{name} blog', { name: space.data?.name })}</h1>
        <span className="spacer" />
        {space.data && (
          <a className="btn btn-ghost" href={`/api/spaces/${encodeURIComponent(space.data.key)}/blog.atom`} title={t('Follow this blog in a feed reader (Atom)')}>
            <Rss size={15} />{' '}{t('Feed')}</a>
        )}
        {canWrite && (
          <button className="btn btn-primary" onClick={write}>
            <PenSquare size={15} />{' '}{t('Write a post')}</button>
        )}
      </div>
      {posts.isLoading && <Spinner />}
      {posts.data?.length === 0 && <div className="empty">{t('No blog posts yet. Share news, release notes or announcements with your team.')}</div>}
      <div style={{ marginTop: 20 }}>
        {posts.data?.map((p) => (
          <article key={p.id} className="card" style={{ marginBottom: 14 }}>
            <Link to={`/pages/${p.id}`} style={{ color: 'var(--text)' }}>
              <h2 style={{ margin: '0 0 6px' }}>{p.title}</h2>
            </Link>
            <div className="row faint small" style={{ marginBottom: 8 }}>
              <Avatar name={p.createdByName ?? '?'} size={20} /> {p.createdByName ?? t('Unknown')} · {formatDate(p.createdAt)}
              {p.commentCount > 0 && (
                <>
                  · <MessageSquare size={12} /> {p.commentCount}
                </>
              )}
            </div>
            <p className="muted" style={{ margin: 0 }}>{p.excerpt}{p.excerpt.length >= 300 ? '…' : ''}</p>
          </article>
        ))}
      </div>
    </div>
  )
}
