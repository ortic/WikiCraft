import { t as tr } from '../i18n'
import { useCallback, useEffect, useRef, useState } from 'react'
import { Link, useNavigate, useParams, useSearchParams } from 'react-router-dom'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Share2, ThumbsUp, Star, MessageSquare, Paperclip, History as HistoryIcon, Lock, MoreHorizontal, Download, Copy, Trash2, ChevronRight, Maximize2, Tag, X, Eye, EyeOff, Printer, LayoutTemplate, FileText, BadgeCheck, UserCheck, Archive, RotateCcw, UserCog, CircleDot, FolderInput, ListPlus, Pin } from 'lucide-react'
import { NoAccess, OwnerDialog, RequestReviewDialog, VerificationBadge, VerifyDialog } from '../components/Verification'
import { ApiError, api, can, formatDate, type Page, type Space } from '../api'
import { PageDecor } from '../components/PageDecor'
import { ErrorBox, Modal, Spinner, useFeatures, useMe, useToast } from '../ui'
import { SaveTemplateDialog } from '../components/TemplateManager'
import { PageStatus } from '../components/PageStatus'
import { FolderView } from '../components/FolderView'
import { PageViews } from '../components/Viewers'
import Editor, { PresenceAvatars, type Presence } from '../editor/Editor'
import CommentsPanel from '../components/CommentsPanel'
import AttachmentsPanel from '../components/AttachmentsPanel'
import RestrictionsDialog from '../components/RestrictionsDialog'
import ShareDialog from '../components/ShareDialog'
import MinorEditToggle from '../components/MinorEdit'
import DraftBanner from '../components/DraftBanner'
import PlacePicker from '../components/PlacePicker'
import { PageProperties } from '../components/PageProperties'

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
  const [addingProp, setAddingProp] = useState(false)
  const [childTplOpen, setChildTplOpen] = useState(false)
  const [saveTplOpen, setSaveTplOpen] = useState(false)
  const [copyOpen, setCopyOpen] = useState(false)
  const [moveOpen, setMoveOpen] = useState(false)
  const [pdfOpen, setPdfOpen] = useState(false)
  const [restrictions, setRestrictions] = useState(false)
  const [share, setShare] = useState(false)
  const [verifyOpen, setVerifyOpen] = useState(false)
  // the first answer for this page says what changed since the last visit; later refetches don't
  const [changed, setChanged] = useState<{ id: string; info: NonNullable<Page['changedSince']> } | null>(null)
  const [showChanges, setShowChanges] = useState(true)
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
  // "delete own" also needs every page below to be yours (the server checks those)
  const canTrash = p.access.admin && (can(p.access, p.kind === 'blog' ? 'blog.delete' : 'page.delete') || (!!p.access.own && can(p.access, 'delete.own')))

  const saveTitle = async () => {
    const t = title.trim() || tr('Untitled')
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
    if (!confirm(tr('Move “{title}” and its child pages to the trash?', { title: p.title }))) return
    await api.del(`/pages/${p.id}`)
    qc.invalidateQueries({ queryKey: ['tree', p.spaceKey] })
    toast(tr('Moved to trash'))
    nav(p.parentId ? `/pages/${p.parentId}` : `/spaces/${p.spaceKey}`)
  }
  const archive = async () => {
    if (!confirm(tr('Archive this page and everything under it? It becomes read-only and leaves the page tree and search.'))) return
    try {
      await api.post(`/pages/${p.id}/archive`)
    } catch (e) {
      toast(e instanceof Error ? e.message : tr('Could not archive'))
      return
    }
    qc.invalidateQueries({ queryKey: ['page', p.id] })
    qc.invalidateQueries({ queryKey: ['tree'] })
    toast(tr('Archived'))
  }
  const unarchive = async () => {
    await api.post(`/pages/${p.id}/unarchive`)
    qc.invalidateQueries({ queryKey: ['page', p.id] })
    qc.invalidateQueries({ queryKey: ['tree'] })
    toast(tr('Restored from the archive'))
  }
  const copy = () => {
    setMenu(false)
    setCopyOpen(true)
  }
  const publish = async () => {
    try {
      await api.post(`/pages/${p.id}/publish`)
      qc.invalidateQueries({ queryKey: ['page', p.id] })
      qc.invalidateQueries({ queryKey: ['tree', p.spaceKey] })
      qc.invalidateQueries({ queryKey: ['drafts'] })
      toast(tr('Published: everyone who can see this space can read it now'))
    } catch (e) {
      toast(e instanceof Error ? e.message : tr('Could not publish'))
    }
  }
  const toggleWatch = async () => {
    setMenu(false)
    if (p.watching) await api.del(`/pages/${p.id}/watch`)
    else await api.put(`/pages/${p.id}/watch`)
    qc.invalidateQueries({ queryKey: ['page', p.id] })
    toast(p.watching ? tr('You will no longer be notified about changes') : tr('You will be notified about changes and comments'))
  }
  const watchWithChildren = async () => {
    setMenu(false)
    await api.put(`/pages/${p.id}/watch`, { includeChildren: true })
    qc.invalidateQueries({ queryKey: ['page', p.id] })
    toast(tr('You will be notified about this page and the pages below it, including new ones'))
  }
  const saveAsTemplate = () => {
    setMenu(false)
    setSaveTplOpen(true)
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
            <button className="icon-btn" title={tr('This page has restrictions')} onClick={() => setRestrictions(true)}>
              <Lock size={16} />
            </button>
          )}
          {user && (
            <button className={`icon-btn ${p.favorite ? 'active' : ''}`} title={p.favorite ? tr('Unstar') : tr('Star')} onClick={toggleFavorite}>
              <Star size={16} fill={p.favorite ? 'currentColor' : 'none'} />
            </button>
          )}
          <button className={`icon-btn ${panel === 'comments' ? 'active' : ''}`} title={tr('Comments')} onClick={() => setPanel(panel === 'comments' ? null : 'comments')}>
            <MessageSquare size={16} />
          </button>
          <button className={`icon-btn ${panel === 'attachments' ? 'active' : ''}`} title={tr('Attachments')} onClick={() => setPanel(panel === 'attachments' ? null : 'attachments')}>
            <Paperclip size={16} />
          </button>
          <Link className="icon-btn" title={tr('Page history')} to={`/pages/${p.id}/history`}>
            <HistoryIcon size={16} />
          </Link>
          {user && (
            <button className="btn small share-btn" title={tr('Share')} onClick={() => setShare(true)}>
              <Share2 size={14} />{' '}{tr('Share')}</button>
          )}
          <div style={{ position: 'relative' }}>
            <button className="icon-btn" title={tr('More')} onClick={() => setMenu((m) => !m)}>
              <MoreHorizontal size={16} />
            </button>
            {menu && (
              <div className="dropdown menu page-menu" onMouseLeave={() => setMenu(false)}>
                {p.access.admin && can(p.access, 'restrict') && (
                  <div className="menu-item" onClick={() => { setMenu(false); setRestrictions(true) }}>
                    <Lock size={15} />{' '}{tr('Restrictions')}</div>
                )}
                {canEdit && (
                  <div className="menu-item" onClick={async () => { setMenu(false); await api.patch(`/pages/${p.id}`, { fullWidth: !p.fullWidth }); qc.invalidateQueries({ queryKey: ['page', p.id] }) }}>
                    <Maximize2 size={15} /> {p.fullWidth ? tr('Fixed width') : tr('Full width')}
                  </div>
                )}
                {user && (
                  <div className="menu-item" onClick={toggleWatch}>
                    {p.watching ? <EyeOff size={15} /> : <Eye size={15} />} {p.watching ? (p.watchingChildren ? tr('Stop watching this and the pages below') : tr('Stop watching')) : tr('Watch page')}
                  </div>
                )}
                {user && !p.watchingChildren && (
                  <div className="menu-item" onClick={watchWithChildren}>
                    <Eye size={15} /> {p.watching ? tr('Also watch the pages below') : tr('Watch page and the pages below')}
                  </div>
                )}
                {user && !p.watching && p.watchedThrough && (
                  <div className="menu-note faint small">{tr('You get updates through “{title}”', { title: p.watchedThrough.title })}</div>
                )}
                {user && space.data && (
                  <div
                    className="menu-item"
                    onClick={async () => {
                      setMenu(false)
                      const watching = (space.data as Space & { watching?: boolean }).watching
                      await (watching ? api.del(`/spaces/${p.spaceKey}/watch`) : api.put(`/spaces/${p.spaceKey}/watch`))
                      qc.invalidateQueries({ queryKey: ['space', p.spaceKey] })
                      toast(watching ? tr('You stopped watching this space') : tr('You get notified about new and changed pages in this space'))
                    }}
                  >
                    {(space.data as Space & { watching?: boolean }).watching ? <EyeOff size={15} /> : <Eye size={15} />}{' '}
                    {(space.data as Space & { watching?: boolean }).watching ? tr('Stop watching the space') : tr('Watch the whole space')}
                  </div>
                )}
                {(canEdit || user) && <div className="menu-sep" />}
                {canEdit && (
                  <div className="menu-item" onClick={() => { setMenu(false); setStatusOpen(true) }}>
                    <CircleDot size={15} />{' '}{tr('Set status…')}</div>
                )}
                {canEdit && (
                  <div className="menu-item" onClick={() => { setMenu(false); setAddingProp(true) }}>
                    <ListPlus size={15} />{' '}{tr('Add property')}</div>
                )}
                {canEdit && (
                  <div className="menu-item" onClick={() => { setMenu(false); setVerifyOpen(true) }}>
                    <BadgeCheck size={15} />{' '}{tr('Verify page…')}</div>
                )}
                {user && (p.access.admin || (p as Page & { ownerId?: string }).ownerId === user.id) && (
                  <div className="menu-item" onClick={() => { setMenu(false); setOwnerOpen(true) }}>
                    <UserCog size={15} />{' '}{tr('Change owner…')}</div>
                )}
                {user && (
                  <div className="menu-item" onClick={() => { setMenu(false); setReviewOpen(true) }}>
                    <UserCheck size={15} />{' '}{tr('Request review…')}</div>
                )}
                {canEdit && (
                  <div className="menu-item" onClick={saveAsTemplate}>
                    <LayoutTemplate size={15} />{' '}{tr('Save as template')}</div>
                )}
                {space.data?.role === 'admin' && !(space.data.shortcuts ?? []).some((s) => s.url === `/pages/${p.id}`) && (space.data.shortcuts?.length ?? 0) < 20 && (
                  <div
                    className="menu-item"
                    onClick={async () => {
                      setMenu(false)
                      try {
                        await api.patch(`/spaces/${p.spaceKey}`, { shortcuts: [...(space.data?.shortcuts ?? []), { title: p.title.slice(0, 80) || tr('Untitled'), url: `/pages/${p.id}` }] })
                        qc.invalidateQueries({ queryKey: ['space'] })
                        qc.invalidateQueries({ queryKey: ['spaces'] })
                        toast(tr('Added to the space shortcuts in the sidebar'))
                      } catch (e) {
                        toast(e instanceof Error ? e.message : tr('Could not add the shortcut'))
                      }
                    }}
                  >
                    <Pin size={15} />{' '}{tr('Add to space shortcuts')}</div>
                )}
                {canEdit && p.kind !== 'blog' && (
                  <div className="menu-item" onClick={() => { setMenu(false); setChildTplOpen(true) }}>
                    <LayoutTemplate size={15} />{' '}{tr('Template for pages below…')}</div>
                )}
                {user && (
                  <div className="menu-item" onClick={copy}>
                    <Copy size={15} />{' '}{tr('Copy page')}</div>
                )}
                {p.access.admin && (
                  <div className="menu-item" onClick={() => { setMenu(false); setMoveOpen(true) }}>
                    <FolderInput size={15} />{' '}{tr('Move…')}</div>
                )}
                {features.exports !== false && can(p.access, 'export') && (<>
                <div className="menu-sep" />
                <div className="menu-label">{tr('Export')}</div>
                <div className="menu-item" onClick={() => { setMenu(false); setPdfOpen(true) }}>
                  <FileText size={15} />{' '}{tr('PDF…')}</div>
                <div className="menu-item" onClick={() => { setMenu(false); setTimeout(() => window.print(), 50) }}>
                  <Printer size={15} />{' '}{tr('Print')}</div>
                <a className="menu-item" href={`/api/pages/${p.id}/export?format=docx`}>
                  <FileText size={15} />{' '}{tr('Word')}</a>
                <a className="menu-item" href={`/api/pages/${p.id}/export?format=docx&tree=true`} title={tr('This page and all its child pages in one document')}>
                  <FileText size={15} />{' '}{tr('Word, with child pages')}</a>
                <a className="menu-item" href={`/api/pages/${p.id}/export?format=md`}>
                  <Download size={15} />{' '}{tr('Markdown')}</a>
                <a className="menu-item" href={`/api/pages/${p.id}/export?format=html`}>
                  <Download size={15} />{' '}{tr('HTML')}</a>
                </>)}
                {(canTrash || (canEdit && space.data?.homePageId !== p.id)) && <div className="menu-sep" />}
                {canEdit && can(p.access, 'archive') && space.data?.homePageId !== p.id && (
                  <div className="menu-item" onClick={() => { setMenu(false); archive() }}>
                    <Archive size={15} />{' '}{tr('Archive page')}</div>
                )}
                {canTrash && (
                  <div className="menu-item btn-danger" onClick={trash}>
                    <Trash2 size={15} />{' '}{tr('Move to trash')}</div>
                )}
              </div>
            )}
          </div>
        </div>

        <div className={`page-wrap ${p.fullWidth ? 'full' : ''}`}>
          {space.data?.archivedAt && (
            <div className="archived-banner">{tr('This space was archived on {date} and is read-only.', { date: formatDate(space.data.archivedAt) })}</div>
          )}
          {p.draft && <DraftBanner page={p} onPublish={publish} />}
          <PageDecor page={p} canEdit={canEdit} onChange={() => qc.invalidateQueries({ queryKey: ['page', p.id] })} />
          <textarea
            ref={titleRef}
            className="page-title"
            rows={1}
            value={title}
            readOnly={!canEdit}
            placeholder={tr('Untitled')}
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
            <span>{tr('Updated {date} by', { date: formatDate(p.updatedAt) })}{' '}{p.updatedBy && p.updatedByName ? <Link to={`/people/${p.updatedBy}`}>{p.updatedByName}</Link> : (p.updatedByName ?? tr('someone'))}
            </span>
            {(p as Page & { ownerName?: string }).ownerName && (
              <>
                <span>·</span>
                <span title={tr('Responsible for this page')}>{tr('Owner:')}{' '}{p.ownerId ? <Link to={`/people/${p.ownerId}`}>{(p as Page & { ownerName?: string }).ownerName}</Link> : (p as Page & { ownerName?: string }).ownerName}
                </span>
              </>
            )}
            <span>·</span>
            <span>{tr('v{version}', { version: p.version })}</span>
            {p.views > 0 && (
              <>
                <span>·</span>
                <PageViews pageId={p.id} views={p.views} viewers={p.viewers} canSeeWho={!!features.viewerList && canEdit} />
              </>
            )}
            {p.kind === 'blog' && <span className="badge badge-accent">{tr('Blog post')}</span>}
            {words > 0 && (
              <>
                <span>·</span>
                <span>{tr('{words} words', { words })}</span>
              </>
            )}
            <VerificationBadge pageId={p.id} onClick={canEdit ? () => setVerifyOpen(true) : undefined} />
            <PageStatus pageId={p.id} status={p.pageStatus} canEdit={canEdit} open={statusOpen} onOpenChange={setStatusOpen} />
            {canEdit && !p.draft && p.kind !== 'folder' && <MinorEditToggle pageId={p.id} />}
            {!canEdit && <span className="badge">{tr('Read only')}</span>}
          </div>
          <PageProperties page={p} canEdit={canEdit} adding={addingProp} onAddingDone={() => setAddingProp(false)} />
          {p.access.archivedPage && (
            <div className="archived-banner">
              <Archive size={15} />
              <span>
                {p.access.archivedPage === p.id ? tr('This page is archived.') : tr('This page is in an archived section.')}{' '}{tr('It\'s read-only and hidden from the page tree and search.')}</span>
              {p.access.admin && can(p.access, 'archive') && p.access.archivedPage === p.id && (
                <button className="btn btn-sm" onClick={unarchive}><RotateCcw size={13} />{' '}{tr('Restore')}</button>
              )}
              {p.access.archivedPage !== p.id && <Link to={`/pages/${p.access.archivedPage}`}>{tr('Go to the archived page')}</Link>}
            </div>
          )}
          {changed?.id === p.id && (
            <div className="changed-banner">
              <HistoryIcon size={15} />
              <span>{tr('Changed since your last visit {date}', { date: formatDate(changed.info.at) })}
                {changed.info.editors.length
                  ? ' ' + (changed.info.editors.length > 3 ? tr('by {names} and others', { names: changed.info.editors.slice(0, 3).join(', ') }) : tr('by {names}', { names: changed.info.editors.join(', ') }))
                  : ''}
                {' '}{tr('({count} versions).', { count: changed.info.versions })}
              </span>
              <Link to={`/pages/${p.id}/history?since=${changed.info.version}`}>{tr('View changes')}</Link>
              {!!changed.info.blocks?.length && p.kind !== 'folder' && (
                <button className="btn btn-ghost btn-sm" aria-pressed={showChanges} onClick={() => setShowChanges(!showChanges)}>
                  {showChanges ? tr('Hide highlights') : changed.info.blocks.length === 1 ? tr('Highlight the change') : tr('Highlight changes')}
                </button>
              )}
              <button className="icon-btn" title={tr('Dismiss')} onClick={() => setChanged({ id: '', info: changed.info })}><X size={14} /></button>
            </div>
          )}
          {p.kind === 'folder' ? (
            <FolderView page={p} canEdit={canEdit} />
          ) : (
          <>
          <Editor
            key={p.id}
            pageId={p.id}
            user={user}
            editable={canEdit}
            onPresence={onPresence}
            onStats={onStats}
            onCommentClick={onCommentClick}
            onRequestComment={p.access.comment ? onRequestComment : undefined}
            highlightBlocks={changed?.id === p.id && showChanges ? changed.info.blocks : undefined}
          />

          <div className="page-reactions">
            {user ? (
              <button
                className={`like-btn${p.likes?.liked ? ' liked' : ''}`}
                title={p.likes?.names.length ? ((p.likes.count ?? 0) > p.likes.names.length ? tr('Liked by {names} and others', { names: p.likes.names.join(', ') }) : tr('Liked by {names}', { names: p.likes.names.join(', ') })) : tr('Like this page')}
                onClick={async () => {
                  const next = await (p.likes?.liked ? api.del(`/pages/${p.id}/like`) : api.put(`/pages/${p.id}/like`))
                  qc.setQueryData(['page', p.id], { ...p, likes: next })
                }}
              >
                <ThumbsUp size={14} /> {p.likes?.liked ? tr('Liked') : tr('Like')}
              </button>
            ) : null}
            {(p.likes?.count ?? 0) > 0 && (
              <span className="faint small">
                {p.likes!.count > 3
                  ? tr('{names} and {count} others liked this', { names: p.likes!.names.slice(0, 3).join(', '), count: p.likes!.count - 3 })
                  : tr('{names} liked this', { names: p.likes!.names.slice(0, 3).join(', '), count: p.likes!.names.length })}</span>
            )}
          </div>
          </>
          )}

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
                <button className="btn btn-ghost btn-sm" onClick={() => setLabelInput('')}>{tr('+ Add label')}</button>
              ) : (
                <>
                <input
                  className="input"
                  style={{ width: 160, height: 26 }}
                  autoFocus
                  list="label-suggestions"
                  aria-label={tr('New label')}
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
                  placeholder={tr('label name')}
                />
                <datalist id="label-suggestions">
                  {labelSuggest.data
                    ?.filter((l) => !p.labels.includes(l.name))
                    .map((l) => (
                      <option key={l.name} value={l.name}>{tr('{count} pages', { count: l.count })}</option>
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
      {childTplOpen && <ChildTemplateDialog page={p} onClose={() => setChildTplOpen(false)} />}
      {saveTplOpen && <SaveTemplateDialog page={p} onClose={() => setSaveTplOpen(false)} />}
      {moveOpen && <MovePageDialog page={p} onClose={() => setMoveOpen(false)} />}
      {pdfOpen && <PdfExportDialog page={p} onClose={() => setPdfOpen(false)} />}
      {share && <ShareDialog pageId={p.id} title={p.title} restricted={!!p.restricted} onClose={() => setShare(false)} />}
    </div>
  )
}

/** Copy a page, optionally with the pages below it (restricted ones you can't see stay behind). */
/** Which template new pages below this one start from (for them and the pages below them). */
/** PDF of the page (or with the pages below it), typeset on the server. */
function PdfExportDialog({ page, onClose }: { page: Page; onClose: () => void }) {
  const [tree, setTree] = useState(false)
  const [toc, setToc] = useState(false)
  const [cover, setCover] = useState(false)
  const href = `/api/pages/${page.id}/export?format=pdf&tree=${tree}&toc=${toc}&cover=${cover}`
  return (
    <Modal
      title={tr('Export as PDF')}
      onClose={onClose}
      width={420}
      footer={
        <>
          <button className="btn" onClick={onClose}>{tr('Cancel')}</button>
          <a className="btn btn-primary" href={href} onClick={() => setTimeout(onClose, 100)}>{tr('Download PDF')}</a>
        </>
      }
    >
      <p className="muted" style={{ marginTop: 0 }}>{tr('A4 pages with the space and title in the header and page numbers in the footer.')}</p>
      <label className="row" style={{ marginBottom: 8 }}>
        <input type="checkbox" checked={tree} onChange={(e) => { setTree(e.target.checked); if (e.target.checked) setToc(true) }} />{' '}{tr('Include the pages below this one')}</label>
      <label className="row" style={{ marginBottom: 8 }}>
        <input type="checkbox" checked={toc} onChange={(e) => setToc(e.target.checked)} />{' '}{tr('Table of contents')}</label>
      <label className="row">
        <input type="checkbox" checked={cover} onChange={(e) => setCover(e.target.checked)} />{' '}{tr('Cover page')}</label>
    </Modal>
  )
}

function ChildTemplateDialog({ page, onClose }: { page: Page; onClose: () => void }) {
  const qc = useQueryClient()
  const toast = useToast()
  const templates = useQuery({ queryKey: ['templates', page.spaceKey], queryFn: () => api.get<{ id: string; name: string }[]>(`/templates?spaceKey=${encodeURIComponent(page.spaceKey)}`) })
  const [value, setValue] = useState(page.childTemplate ?? '')
  const save = async () => {
    try {
      await api.patch(`/pages/${page.id}`, { childTemplate: value })
      qc.invalidateQueries({ queryKey: ['page', page.id] })
      qc.invalidateQueries({ queryKey: ['default-template'] })
      toast(value ? tr('New pages below start from that template') : tr('New pages below start like the rest of the space'))
      onClose()
    } catch (e) {
      toast(e instanceof Error ? e.message : tr('Could not save'))
    }
  }
  return (
    <Modal
      title={tr('Template for pages below')}
      onClose={onClose}
      width={460}
      footer={
        <>
          <button className="btn" onClick={onClose}>{tr('Cancel')}</button>
          <button className="btn btn-primary" onClick={save}>{tr('Save')}</button>
        </>
      }
    >
      <p className="muted" style={{ marginTop: 0 }}>{tr('New pages created below “{title}” (and below those) start from this template. People can still pick another one or a blank page.', { title: page.title })}</p>
      <label className="field">
        <span className="sr-only">{tr('Template')}</span>
        <select className="select" aria-label={tr('Template')} value={value} onChange={(e) => setValue(e.target.value)}>
          <option value="">{tr('Same as the rest of the space')}</option>
          {templates.data?.map((t) => <option key={t.id} value={t.id}>{t.name}</option>)}
        </select>
      </label>
    </Modal>
  )
}

function CopyPageDialog({ page, onClose }: { page: Page; onClose: () => void }) {
  const qc = useQueryClient()
  const nav = useNavigate()
  const toast = useToast()
  const [title, setTitle] = useState(tr('Copy of {title}', { title: page.title }))
  const [children, setChildren] = useState(false)
  const [attachments, setAttachments] = useState(true)
  const [place, setPlace] = useState<{ spaceKey: string; parentId: string | null }>({ spaceKey: page.spaceKey, parentId: page.parentId })
  const [busy, setBusy] = useState(false)
  const kids = useQuery({ queryKey: ['children', page.id, 1], queryFn: () => api.get<unknown[]>(`/pages/${page.id}/children?depth=1`) })
  const submit = async () => {
    setBusy(true)
    try {
      const r = await api.post<{ id: string }>(`/pages/${page.id}/copy`, {
        title: title.trim() || undefined,
        includeChildren: children,
        includeAttachments: attachments,
        spaceKey: place.spaceKey,
        parentId: place.parentId ?? undefined,
      })
      qc.invalidateQueries({ queryKey: ['tree', page.spaceKey] })
      qc.invalidateQueries({ queryKey: ['tree', place.spaceKey] })
      onClose()
      nav(`/pages/${r.id}`)
    } catch (e) {
      toast(e instanceof Error ? e.message : tr('Copy failed'))
      setBusy(false)
    }
  }
  return (
    <Modal
      title={tr('Copy page')}
      onClose={onClose}
      footer={
        <>
          <button className="btn" onClick={onClose}>{tr('Cancel')}</button>
          <button className="btn btn-primary" disabled={busy} onClick={submit}>{busy ? tr('Copying…') : tr('Copy')}</button>
        </>
      }
    >
      <div className="field">
        <label htmlFor="copy-title">{tr('Title')}</label>
        <input id="copy-title" className="input" autoFocus value={title} maxLength={255} onChange={(e) => setTitle(e.target.value)} onKeyDown={(e) => e.key === 'Enter' && submit()} />
      </div>
      <PlacePicker spaceKey={place.spaceKey} parentId={place.parentId} idPrefix="copy" onChange={(spaceKey, parentId) => setPlace({ spaceKey, parentId })} />
      {(kids.data?.length ?? 0) > 0 && (
        <label className="row small">
          <input type="checkbox" checked={children} onChange={(e) => setChildren(e.target.checked)} />{' '}{tr('Include the {count} child pages and everything below', { count: kids.data!.length })}</label>
      )}
      <label className="row small">
        <input type="checkbox" checked={attachments} onChange={(e) => setAttachments(e.target.checked)} />{' '}{tr('Copy the attachments too')}</label>
      <p className="faint small" style={{ marginBottom: 0 }}>{tr('The copy keeps labels and restrictions.')}</p>
    </Modal>
  )
}

/** Moves the page (with everything below it) elsewhere in this space or to another one. */
function MovePageDialog({ page, onClose }: { page: Page; onClose: () => void }) {
  const qc = useQueryClient()
  const toast = useToast()
  const [place, setPlace] = useState<{ spaceKey: string; parentId: string | null }>({ spaceKey: page.spaceKey, parentId: page.parentId })
  const [busy, setBusy] = useState(false)
  const unchanged = place.spaceKey === page.spaceKey && place.parentId === page.parentId
  const submit = async () => {
    setBusy(true)
    try {
      await api.post(`/pages/${page.id}/move`, { spaceKey: place.spaceKey, parentId: place.parentId })
      qc.invalidateQueries({ queryKey: ['tree', page.spaceKey] })
      qc.invalidateQueries({ queryKey: ['tree', place.spaceKey] })
      qc.invalidateQueries({ queryKey: ['page', page.id] })
      toast(place.spaceKey === page.spaceKey ? tr('Moved') : tr('Moved to {spaceKey}', { spaceKey: place.spaceKey }))
      onClose()
    } catch (e) {
      toast(e instanceof Error ? e.message : tr('Move failed'))
      setBusy(false)
    }
  }
  return (
    <Modal
      title={tr('Move page')}
      onClose={onClose}
      footer={
        <>
          <button className="btn" onClick={onClose}>{tr('Cancel')}</button>
          <button className="btn btn-primary" disabled={busy || unchanged} onClick={submit}>{busy ? tr('Moving…') : tr('Move')}</button>
        </>
      }
    >
      <PlacePicker spaceKey={place.spaceKey} parentId={place.parentId} exclude={page.id} idPrefix="move" onChange={(spaceKey, parentId) => setPlace({ spaceKey, parentId })} />
      <p className="faint small" style={{ marginBottom: 0 }}>{tr('The pages below it move too. Links to it keep working. Moving to another space needs admin rights here.')}</p>
    </Modal>
  )
}
