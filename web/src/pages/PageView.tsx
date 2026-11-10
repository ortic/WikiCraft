import { useCallback, useEffect, useRef, useState } from 'react'
import { Link, useNavigate, useParams, useSearchParams } from 'react-router-dom'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Share2, ThumbsUp, Star, MessageSquare, Paperclip, History as HistoryIcon, Lock, MoreHorizontal, Download, Copy, Trash2, ChevronRight, Maximize2, Tag, X, Eye, EyeOff, Printer, LayoutTemplate, FileText, BadgeCheck, UserCheck, Archive, RotateCcw, UserCog, CircleDot } from 'lucide-react'
import { NoAccess, OwnerDialog, RequestReviewDialog, VerificationBadge, VerifyDialog } from '../components/Verification'
import { ApiError, api, formatDate, type Page, type Space } from '../api'
import { PageDecor } from '../components/PageDecor'
import { ErrorBox, Modal, Spinner, useFeatures, useMe, useToast } from '../ui'
import { PageStatus } from '../components/PageStatus'
import Editor, { PresenceAvatars, type Presence } from '../editor/Editor'
import CommentsPanel from '../components/CommentsPanel'
import AttachmentsPanel from '../components/AttachmentsPanel'
import RestrictionsDialog from '../components/RestrictionsDialog'
import ShareDialog from '../components/ShareDialog'

type Panel = 'comments' | 'attachments' | null

export default function PageView() {
  const { id } = useParams()
  const [params, setParams] = useSearchParams()
  const nav = useNavigate()
  const qc = useQueryClient()
  const toast = useToast()
  const me = useMe()
  const features = useFeatures()
  const user = me.data?.user ?? null
  const page = useQuery({ queryKey: ['page', id], queryFn: () => api.get<Page>(`/pages/${id}`), retry: false })
  const space = useQuery({ queryKey: ['space', page.data?.spaceKey], queryFn: () => api.get<Space>(`/spaces/${page.data!.spaceKey}`), enabled: !!page.data?.spaceKey })
  const [panel, setPanel] = useState<Panel>(null)
  const [presence, setPresence] = useState<Presence[]>([])
  const [menu, setMenu] = useState(false)
  const [statusOpen, setStatusOpen] = useState(false)
  const [copyOpen, setCopyOpen] = useState(false)
  const [restrictions, setRestrictions] = useState(false)
  const [share, setShare] = useState(false)
  const [verifyOpen, setVerifyOpen] = useState(false)
  // the first answer for this page says what changed since the last visit; later refetches don't
  const [changed, setChanged] = useState<{ id: string; info: NonNullable<Page['changedSince']> } | null>(null)
  useEffect(() => {
    const c = page.data?.changedSince
    if (c && page.data && changed?.id !== page.data.id) setChanged({ id: page.data.id, info: c })
  }, [page.data, changed?.id])
  const [reviewOpen, setReviewOpen] = useState(false)
  const [ownerOpen, setOwnerOpen] = useState(false)
  const [focusComment, setFocusComment] = useState<string | null>(null)
  const [pendingComment, setPendingComment] = useState<{ text: string; apply: (id: string) => void } | null>(null)
  const [words, setWords] = useState(0)
  const [title, setTitle] = useState('')
  const [labelInput, setLabelInput] = useState<string | null>(null)
  const labelSuggest = useQuery({
    queryKey: ['label-suggest', labelInput?.trim().toLowerCase()],
    queryFn: () => api.get<{ name: string; count: number }[]>(`/labels?q=${encodeURIComponent(labelInput?.trim() ?? '')}`),
    enabled: labelInput !== null,
    staleTime: 30_000,
  })
  const titleRef = useRef<HTMLTextAreaElement>(null)

  useEffect(() => {
    if (page.data) setTitle(page.data.title)
  }, [page.data?.id, page.data?.title])
  useEffect(() => {
    setPanel(null)
    setPresence([])
  }, [id])
  useEffect(() => {
    if (params.get('new') && titleRef.current) {
      titleRef.current.focus()
      titleRef.current.select()
      params.delete('new')
      setParams(params, { replace: true })
    }
  }, [page.data, params, setParams])
  useEffect(() => {
    const el = titleRef.current
    if (el) {
      el.style.height = 'auto'
      el.style.height = `${el.scrollHeight}px`
    }
  }, [title])

  const onPresence = useCallback((p: Presence[]) => setPresence(p), [])
  const onStats = useCallback((w: number) => setWords(w), [])
  const onCommentClick = useCallback((cid: string) => {
    setPanel('comments')
    setFocusComment(cid)
  }, [])
  const onRequestComment = useCallback((text: string, apply: (id: string) => void) => {
    setPanel('comments')
    setPendingComment({ text, apply })
  }, [])

  if (page.isLoading) return <div className="page-wrap"><Spinner /></div>
  if (page.error) {
    const status = page.error instanceof ApiError ? page.error.status : 0
    if (status === 404 || status === 403) return <div className="page-wrap"><NoAccess pageId={id!} signedIn={!!user} /></div>
    return <div className="page-wrap"><ErrorBox error={page.error} /></div>
  }
  const p = page.data!
  const canEdit = p.access.edit

  const saveTitle = async () => {
    const t = title.trim() || 'Untitled'
    if (t === p.title) return
    await api.patch(`/pages/${p.id}`, { title: t })
    qc.invalidateQueries({ queryKey: ['page', p.id] })
    qc.invalidateQueries({ queryKey: ['tree', p.spaceKey] })
  }
  const toggleFavorite = async () => {
    if (p.favorite) await api.del(`/pages/${p.id}/favorite`)
    else await api.put(`/pages/${p.id}/favorite`)
    qc.invalidateQueries({ queryKey: ['page', p.id] })
    qc.invalidateQueries({ queryKey: ['favorites'] })
  }
  const trash = async () => {
    setMenu(false)
    if (!confirm(`Move “${p.title}” and its child pages to the trash?`)) return
    await api.del(`/pages/${p.id}`)
    qc.invalidateQueries({ queryKey: ['tree', p.spaceKey] })
    toast('Moved to trash')
    nav(p.parentId ? `/pages/${p.parentId}` : `/spaces/${p.spaceKey}`)
  }
  const archive = async () => {
    if (!confirm('Archive this page and everything under it? It becomes read-only and leaves the page tree and search.')) return
    try {
      await api.post(`/pages/${p.id}/archive`)
    } catch (e) {
      toast(e instanceof Error ? e.message : 'Could not archive')
      return
    }
    qc.invalidateQueries({ queryKey: ['page', p.id] })
    qc.invalidateQueries({ queryKey: ['tree'] })
    toast('Archived')
  }
  const unarchive = async () => {
    await api.post(`/pages/${p.id}/unarchive`)
    qc.invalidateQueries({ queryKey: ['page', p.id] })
    qc.invalidateQueries({ queryKey: ['tree'] })
    toast('Restored from the archive')
  }
  const copy = () => {
    setMenu(false)
    setCopyOpen(true)
  }
  const toggleWatch = async () => {
    setMenu(false)
    if (p.watching) await api.del(`/pages/${p.id}/watch`)
    else await api.put(`/pages/${p.id}/watch`)
    qc.invalidateQueries({ queryKey: ['page', p.id] })
    toast(p.watching ? 'You will no longer be notified about changes' : 'You will be notified about changes and comments')
  }
  const saveAsTemplate = async () => {
    setMenu(false)
    const name = window.prompt('Template name', p.title)
    if (!name) return
    await api.post('/templates', { name, pageId: p.id, spaceKey: p.spaceKey })
    qc.invalidateQueries({ queryKey: ['templates'] })
    toast(`Template “${name}” saved for this space`)
  }
  const saveLabels = async (labels: string[]) => {
    await api.put(`/pages/${p.id}/labels`, { labels })
    qc.invalidateQueries({ queryKey: ['page', p.id] })
  }

  return (
    <div className={panel ? 'with-panel' : ''}>
      <div>
        <div className="page-header">
          <div className="breadcrumbs">
            <Link to={`/spaces/${p.spaceKey}`}>{p.spaceName}</Link>
            {p.breadcrumbs.slice(1).map((b) => (
              <span key={b.id} className="row" style={{ gap: 4 }}>
                <ChevronRight size={12} />
                <Link to={`/pages/${b.id}`}>{b.title}</Link>
              </span>
            ))}
          </div>
          <span className="spacer" />
          <PresenceAvatars users={presence} />
          {p.restricted && (
            <button className="icon-btn" title="This page has restrictions" onClick={() => setRestrictions(true)}>
              <Lock size={16} />
            </button>
          )}
          {user && (
            <button className={`icon-btn ${p.favorite ? 'active' : ''}`} title={p.favorite ? 'Unstar' : 'Star'} onClick={toggleFavorite}>
              <Star size={16} fill={p.favorite ? 'currentColor' : 'none'} />
            </button>
          )}
          <button className={`icon-btn ${panel === 'comments' ? 'active' : ''}`} title="Comments" onClick={() => setPanel(panel === 'comments' ? null : 'comments')}>
            <MessageSquare size={16} />
          </button>
          <button className={`icon-btn ${panel === 'attachments' ? 'active' : ''}`} title="Attachments" onClick={() => setPanel(panel === 'attachments' ? null : 'attachments')}>
            <Paperclip size={16} />
          </button>
          <Link className="icon-btn" title="Page history" to={`/pages/${p.id}/history`}>
            <HistoryIcon size={16} />
          </Link>
          {user && (
            <button className="btn small share-btn" title="Share" onClick={() => setShare(true)}>
              <Share2 size={14} /> Share
            </button>
          )}
          <div style={{ position: 'relative' }}>
            <button className="icon-btn" title="More" onClick={() => setMenu((m) => !m)}>
              <MoreHorizontal size={16} />
            </button>
            {menu && (
              <div className="dropdown menu page-menu" onMouseLeave={() => setMenu(false)}>
                {p.access.admin && (
                  <div className="menu-item" onClick={() => { setMenu(false); setRestrictions(true) }}>
                    <Lock size={15} /> Restrictions
                  </div>
                )}
                {canEdit && (
                  <div className="menu-item" onClick={async () => { setMenu(false); await api.patch(`/pages/${p.id}`, { fullWidth: !p.fullWidth }); qc.invalidateQueries({ queryKey: ['page', p.id] }) }}>
                    <Maximize2 size={15} /> {p.fullWidth ? 'Fixed width' : 'Full width'}
                  </div>
                )}
                {user && (
                  <div className="menu-item" onClick={toggleWatch}>
                    {p.watching ? <EyeOff size={15} /> : <Eye size={15} />} {p.watching ? 'Stop watching' : 'Watch page'}
                  </div>
                )}
                {user && space.data && (
                  <div
                    className="menu-item"
                    onClick={async () => {
                      setMenu(false)
                      const watching = (space.data as Space & { watching?: boolean }).watching
                      await (watching ? api.del(`/spaces/${p.spaceKey}/watch`) : api.put(`/spaces/${p.spaceKey}/watch`))
                      qc.invalidateQueries({ queryKey: ['space', p.spaceKey] })
                      toast(watching ? 'You stopped watching this space' : 'You get notified about new and changed pages in this space')
                    }}
                  >
                    {(space.data as Space & { watching?: boolean }).watching ? <EyeOff size={15} /> : <Eye size={15} />}{' '}
                    {(space.data as Space & { watching?: boolean }).watching ? 'Stop watching the space' : 'Watch the whole space'}
                  </div>
                )}
                {(canEdit || user) && <div className="menu-sep" />}
                {canEdit && (
                  <div className="menu-item" onClick={() => { setMenu(false); setStatusOpen(true) }}>
                    <CircleDot size={15} /> Set status…
                  </div>
                )}
                {canEdit && (
                  <div className="menu-item" onClick={() => { setMenu(false); setVerifyOpen(true) }}>
                    <BadgeCheck size={15} /> Verify page…
                  </div>
                )}
                {user && (p.access.admin || (p as Page & { ownerId?: string }).ownerId === user.id) && (
                  <div className="menu-item" onClick={() => { setMenu(false); setOwnerOpen(true) }}>
                    <UserCog size={15} /> Change owner…
                  </div>
                )}
                {user && (
                  <div className="menu-item" onClick={() => { setMenu(false); setReviewOpen(true) }}>
                    <UserCheck size={15} /> Request review…
                  </div>
                )}
                {canEdit && (
                  <div className="menu-item" onClick={saveAsTemplate}>
                    <LayoutTemplate size={15} /> Save as template
                  </div>
                )}
                {user && (
                  <div className="menu-item" onClick={copy}>
                    <Copy size={15} /> Copy page
                  </div>
                )}
                {features.exports !== false && (<>
                <div className="menu-sep" />
                <div className="menu-label">Export</div>
                <div className="menu-item" onClick={() => { setMenu(false); setTimeout(() => window.print(), 50) }}>
                  <Printer size={15} /> Print or PDF
                </div>
                <a className="menu-item" href={`/api/pages/${p.id}/export?format=docx`}>
                  <FileText size={15} /> Word
                </a>
                <a className="menu-item" href={`/api/pages/${p.id}/export?format=docx&tree=true`} title="This page and all its child pages in one document">
                  <FileText size={15} /> Word, with child pages
                </a>
                <a className="menu-item" href={`/api/pages/${p.id}/export?format=md`}>
                  <Download size={15} /> Markdown
                </a>
                <a className="menu-item" href={`/api/pages/${p.id}/export?format=html`}>
                  <Download size={15} /> HTML
                </a>
                </>)}
                {(p.access.admin || (canEdit && space.data?.homePageId !== p.id)) && <div className="menu-sep" />}
                {canEdit && space.data?.homePageId !== p.id && (
                  <div className="menu-item" onClick={() => { setMenu(false); archive() }}>
                    <Archive size={15} /> Archive page
                  </div>
                )}
                {p.access.admin && (
                  <div className="menu-item btn-danger" onClick={trash}>
                    <Trash2 size={15} /> Move to trash
                  </div>
                )}
              </div>
            )}
          </div>
        </div>

        <div className={`page-wrap ${p.fullWidth ? 'full' : ''}`}>
          {space.data?.archivedAt && (
            <div className="archived-banner">This space was archived on {formatDate(space.data.archivedAt)} and is read-only.</div>
          )}
          <PageDecor page={p} canEdit={canEdit} onChange={() => qc.invalidateQueries({ queryKey: ['page', p.id] })} />
          <textarea
            ref={titleRef}
            className="page-title"
            rows={1}
            value={title}
            readOnly={!canEdit}
            placeholder="Untitled"
            onChange={(e) => setTitle(e.target.value.replace(/\n/g, ''))}
            onBlur={saveTitle}
            onKeyDown={(e) => {
              if (e.key === 'Enter') {
                e.preventDefault()
                ;(document.querySelector('.ProseMirror') as HTMLElement | null)?.focus()
              }
            }}
          />
          <div className="page-meta">
            <span>
              Updated {formatDate(p.updatedAt)} by {p.updatedByName ?? 'someone'}
            </span>
            {(p as Page & { ownerName?: string }).ownerName && (
              <>
                <span>·</span>
                <span title="Responsible for this page">Owner: {(p as Page & { ownerName?: string }).ownerName}</span>
              </>
            )}
            <span>·</span>
            <span>v{p.version}</span>
            {p.views > 0 && (
              <>
                <span>·</span>
                <span title={`${p.viewers} people`}>{p.views} views</span>
              </>
            )}
            {p.kind === 'blog' && <span className="badge badge-accent">Blog post</span>}
            {words > 0 && (
              <>
                <span>·</span>
                <span>{words} words</span>
              </>
            )}
            <VerificationBadge pageId={p.id} onClick={canEdit ? () => setVerifyOpen(true) : undefined} />
            <PageStatus pageId={p.id} status={p.pageStatus} canEdit={canEdit} open={statusOpen} onOpenChange={setStatusOpen} />
            {!canEdit && <span className="badge">Read only</span>}
          </div>
          {p.access.archivedPage && (
            <div className="archived-banner">
              <Archive size={15} />
              <span>
                {p.access.archivedPage === p.id ? 'This page is archived.' : 'This page is in an archived section.'} It's read-only and hidden from the page tree and search.
              </span>
              {p.access.admin && p.access.archivedPage === p.id && (
                <button className="btn btn-sm" onClick={unarchive}><RotateCcw size={13} /> Restore</button>
              )}
              {p.access.archivedPage !== p.id && <Link to={`/pages/${p.access.archivedPage}`}>Go to the archived page</Link>}
            </div>
          )}
          {changed?.id === p.id && (
            <div className="changed-banner">
              <HistoryIcon size={15} />
              <span>
                Changed since your last visit {formatDate(changed.info.at)}
                {changed.info.editors.length ? ` by ${changed.info.editors.slice(0, 3).join(', ')}${changed.info.editors.length > 3 ? ' and others' : ''}` : ''}
                {' '}({changed.info.versions} {changed.info.versions === 1 ? 'version' : 'versions'}).
              </span>
              <Link to={`/pages/${p.id}/history?since=${changed.info.version}`}>View changes</Link>
              <button className="icon-btn" title="Dismiss" onClick={() => setChanged({ id: '', info: changed.info })}><X size={14} /></button>
            </div>
          )}
          <Editor key={p.id} pageId={p.id} user={user} editable={canEdit} onPresence={onPresence} onStats={onStats} onCommentClick={onCommentClick} onRequestComment={p.access.comment ? onRequestComment : undefined} />

          <div className="page-reactions">
            {user ? (
              <button
                className={`like-btn${p.likes?.liked ? ' liked' : ''}`}
                title={p.likes?.names.length ? `Liked by ${p.likes.names.join(', ')}${(p.likes.count ?? 0) > p.likes.names.length ? ' and others' : ''}` : 'Like this page'}
                onClick={async () => {
                  const next = await (p.likes?.liked ? api.del(`/pages/${p.id}/like`) : api.put(`/pages/${p.id}/like`))
                  qc.setQueryData(['page', p.id], { ...p, likes: next })
                }}
              >
                <ThumbsUp size={14} /> {p.likes?.liked ? 'Liked' : 'Like'}
              </button>
            ) : null}
            {(p.likes?.count ?? 0) > 0 && (
              <span className="faint small">
                {p.likes!.names.slice(0, 3).join(', ')}
                {p.likes!.count > 3 ? ` and ${p.likes!.count - 3} others` : ''} liked this
              </span>
            )}
          </div>

          <div className="labels">
            <Tag size={14} className="faint" />
            {p.labels.map((l) => (
              <span key={l} className="label-chip row" style={{ gap: 4 }}>
                <Link to={`/labels/${encodeURIComponent(l)}`} style={{ color: 'inherit' }}>
                  {l}
                </Link>
                {canEdit && (
                  <X size={11} style={{ cursor: 'pointer' }} onClick={() => saveLabels(p.labels.filter((x) => x !== l))} />
                )}
              </span>
            ))}
            {canEdit &&
              (labelInput === null ? (
                <button className="btn btn-ghost btn-sm" onClick={() => setLabelInput('')}>
                  + Add label
                </button>
              ) : (
                <>
                <input
                  className="input"
                  style={{ width: 160, height: 26 }}
                  autoFocus
                  list="label-suggestions"
                  aria-label="New label"
                  value={labelInput}
                  onChange={async (e) => {
                    const v = e.target.value
                    // picked from the suggestions: add it right away
                    if ((e.nativeEvent as InputEvent).inputType === 'insertReplacementText' || (!(e.nativeEvent as InputEvent).inputType && labelSuggest.data?.some((l) => l.name === v))) {
                      await saveLabels([...p.labels, v])
                      setLabelInput('')
                      return
                    }
                    setLabelInput(v)
                  }}
                  onBlur={() => setLabelInput(null)}
                  onKeyDown={async (e) => {
                    if (e.key === 'Enter' && labelInput.trim()) {
                      await saveLabels([...p.labels, labelInput.trim()])
                      setLabelInput('')
                    }
                    if (e.key === 'Escape') setLabelInput(null)
                  }}
                  placeholder="label name"
                />
                <datalist id="label-suggestions">
                  {labelSuggest.data
                    ?.filter((l) => !p.labels.includes(l.name))
                    .map((l) => (
                      <option key={l.name} value={l.name}>{l.count === 1 ? '1 page' : `${l.count} pages`}</option>
                    ))}
                </datalist>
                </>
              ))}
          </div>
        </div>
      </div>
      {panel === 'comments' && (
        <CommentsPanel
          page={p}
          focus={focusComment}
          pending={pendingComment}
          onPendingDone={() => setPendingComment(null)}
          onClose={() => setPanel(null)}
        />
      )}
      {panel === 'attachments' && <AttachmentsPanel page={p} onClose={() => setPanel(null)} />}
      {restrictions && <RestrictionsDialog page={p} onClose={() => setRestrictions(false)} />}
      {verifyOpen && <VerifyDialog pageId={p.id} onClose={() => setVerifyOpen(false)} />}
      {reviewOpen && <RequestReviewDialog pageId={p.id} onClose={() => setReviewOpen(false)} />}
      {ownerOpen && <OwnerDialog pageId={p.id} current={(p as Page & { ownerName?: string }).ownerName ?? null} onClose={() => setOwnerOpen(false)} />}
      {copyOpen && <CopyPageDialog page={p} onClose={() => setCopyOpen(false)} />}
      {share && <ShareDialog pageId={p.id} title={p.title} restricted={!!p.restricted} onClose={() => setShare(false)} />}
    </div>
  )
}

/** Copy a page, optionally with the pages below it (restricted ones you can't see stay behind). */
function CopyPageDialog({ page, onClose }: { page: Page; onClose: () => void }) {
  const qc = useQueryClient()
  const nav = useNavigate()
  const toast = useToast()
  const [title, setTitle] = useState(`Copy of ${page.title}`)
  const [children, setChildren] = useState(false)
  const [busy, setBusy] = useState(false)
  const kids = useQuery({ queryKey: ['children', page.id, 1], queryFn: () => api.get<unknown[]>(`/pages/${page.id}/children?depth=1`) })
  const submit = async () => {
    setBusy(true)
    try {
      const r = await api.post<{ id: string }>(`/pages/${page.id}/copy`, { title: title.trim() || undefined, includeChildren: children })
      qc.invalidateQueries({ queryKey: ['tree', page.spaceKey] })
      onClose()
      nav(`/pages/${r.id}`)
    } catch (e) {
      toast(e instanceof Error ? e.message : 'Copy failed')
      setBusy(false)
    }
  }
  return (
    <Modal
      title="Copy page"
      onClose={onClose}
      footer={
        <>
          <button className="btn" onClick={onClose}>Cancel</button>
          <button className="btn btn-primary" disabled={busy} onClick={submit}>{busy ? 'Copying…' : 'Copy'}</button>
        </>
      }
    >
      <div className="field">
        <label htmlFor="copy-title">Title</label>
        <input id="copy-title" className="input" autoFocus value={title} maxLength={255} onChange={(e) => setTitle(e.target.value)} onKeyDown={(e) => e.key === 'Enter' && submit()} />
      </div>
      {(kids.data?.length ?? 0) > 0 && (
        <label className="row small">
          <input type="checkbox" checked={children} onChange={(e) => setChildren(e.target.checked)} /> Include the {kids.data!.length === 1 ? 'child page' : `${kids.data!.length} child pages`} and everything below
        </label>
      )}
      <p className="faint small" style={{ marginBottom: 0 }}>The copy keeps labels and restrictions and lands next to this page.</p>
    </Modal>
  )
}
