import { Navigate, useLocation, useParams } from 'react-router-dom'
import { useWikiPath } from '../pagePath'
import { Spinner, useMe } from '../ui'
import PageView from './PageView'
import Missing from './Missing'

/** `/wiki/<KEY>/<slug>`: the page a readable address leads to, or an offer to create it. */
export default function WikiPage() {
  const { key, slug } = useParams()
  const loc = useLocation()
  const me = useMe()
  const q = useWikiPath(key, slug)
  if (q.isLoading) return <div className="page-wrap"><Spinner /></div>
  if (q.data?.pageId) return <PageView pageId={q.data.pageId} />
  // maybe there is a page, just not one anonymous visitors may read
  if (!me.data?.user) return <Navigate to={`/login?next=${encodeURIComponent(loc.pathname + loc.search)}`} replace />
  return <Missing space={key} title={titleOf(slug ?? '')} />
}

/** `release-process` → `Release process`; a typed title (`Release Process`) stays as it is. */
function titleOf(slug: string): string {
  if (slug !== slug.toLowerCase() || !slug.includes('-') || slug.includes(' ')) return slug.trim()
  const words = slug.split('-').filter(Boolean).join(' ')
  return words.charAt(0).toUpperCase() + words.slice(1)
}
