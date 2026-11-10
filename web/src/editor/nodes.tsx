// Custom Tiptap nodes/marks. Names and attributes mirror crates/doc-model (server schema),
// so documents imported from Confluence or written via the API load without loss.

import { useState } from 'react'
import { Mark, Node, mergeAttributes } from '@tiptap/core'
import { NodeViewContent, NodeViewWrapper, ReactNodeViewRenderer, type ReactNodeViewProps } from '@tiptap/react'
import { useQuery } from '@tanstack/react-query'
import { Link } from 'react-router-dom'
import { FileText, Paperclip, ListTree, Hash, Puzzle, Link2, Download } from 'lucide-react'
import { api, type Page, type PmNode } from '../api'
import { Emoticon as EmoticonIcon, LineIcon, SYMBOL_COLORS, SymbolPicker, parseSymbol } from '../icons'
import { LIVE_MACROS, LiveMacro } from './MacroView'
import { useEditable } from './useEditable'

const CALLOUT_TYPES = ['info', 'note', 'success', 'warning', 'error']

function CalloutView({ node, updateAttributes, editor }: ReactNodeViewProps) {
  const type = CALLOUT_TYPES.includes(node.attrs.type as string) ? (node.attrs.type as string) : 'info'
  const custom = parseSymbol(node.attrs.icon as string | null)
  const [open, setOpen] = useState(false)
  const editable = useEditable(editor)
  return (
    <NodeViewWrapper className="callout" data-callout={type}>
      <span
        className="callout-icon"
        contentEditable={false}
        onClick={() => editable && setOpen((o) => !o)}
        title={editable ? 'Panel type and icon' : undefined}
        style={{ cursor: editable ? 'pointer' : 'default', ...(custom ? { color: SYMBOL_COLORS[custom.color] } : {}) }}
      >
        {custom ? <LineIcon group="symbols" name={custom.name} size={20} /> : <LineIcon group="callout" name={type} size={20} />}
      </span>
      {open && editable && (
        <div className="callout-menu" contentEditable={false} onMouseLeave={() => setOpen(false)}>
          <div className="callout-types">
            {CALLOUT_TYPES.map((t) => (
              <button key={t} type="button" className={`callout-type ${t === type ? 'active' : ''}`} data-callout={t} title={t} onClick={() => updateAttributes({ type: t })}>
                <LineIcon group="callout" name={t} size={16} /> {t[0].toUpperCase() + t.slice(1)}
              </button>
            ))}
          </div>
          <div className="faint small" style={{ margin: '8px 0 4px' }}>Icon</div>
          <SymbolPicker value={(node.attrs.icon as string | null) ?? null} onChange={(v) => updateAttributes({ icon: v, emoji: null })} />
          {custom && (
            <button type="button" className="btn btn-sm" style={{ marginTop: 6 }} onClick={() => updateAttributes({ icon: null })}>
              Use the panel's own icon
            </button>
          )}
        </div>
      )}
      <NodeViewContent className="callout-body" />
    </NodeViewWrapper>
  )
}

export const Callout = Node.create({
  name: 'callout',
  group: 'block',
  content: 'block+',
  defining: true,
  addAttributes() {
    return { type: { default: 'info' }, emoji: { default: null }, icon: { default: null } }
  },
  parseHTML() {
    return [{ tag: 'div[data-type="callout"]' }]
  },
  renderHTML({ HTMLAttributes }) {
    return ['div', mergeAttributes(HTMLAttributes, { 'data-type': 'callout', class: 'callout', 'data-callout': HTMLAttributes.type }), 0]
  },
  addNodeView() {
    return ReactNodeViewRenderer(CalloutView)
  },
})

function StatusView({ node, updateAttributes, editor }: ReactNodeViewProps) {
  const edit = () => {
    if (!editor.isEditable) return
    const text = window.prompt('Status text', node.attrs.text as string)
    if (text === null) return
    const color = window.prompt('Color (grey, blue, green, yellow, red, purple)', node.attrs.color as string) ?? node.attrs.color
    updateAttributes({ text, color })
  }
  return (
    <NodeViewWrapper as="span" className="status-lozenge" data-color={node.attrs.color} onClick={edit}>
      {(node.attrs.text as string) || 'STATUS'}
    </NodeViewWrapper>
  )
}

/** Inline emoticon from our icon set (Confluence's tick, cross, smile, …). */
export const Emoticon = Node.create({
  name: 'emoticon',
  group: 'inline',
  inline: true,
  atom: true,
  selectable: true,
  addAttributes() {
    return { name: { default: 'smile' } }
  },
  parseHTML() {
    return [{ tag: 'span[data-type="emoticon"]', getAttrs: (el) => ({ name: (el as HTMLElement).dataset.name }) }]
  },
  renderHTML({ HTMLAttributes }) {
    return ['span', mergeAttributes({ 'data-type': 'emoticon', 'data-name': HTMLAttributes.name, class: 'emoticon-node' })]
  },
  addNodeView() {
    return ReactNodeViewRenderer(({ node }: ReactNodeViewProps) => (
      <NodeViewWrapper as="span" className="emoticon-node">
        <EmoticonIcon name={node.attrs.name as string} />
      </NodeViewWrapper>
    ))
  },
})

export const Status = Node.create({
  name: 'status',
  group: 'inline',
  inline: true,
  atom: true,
  addAttributes() {
    return { text: { default: 'TODO' }, color: { default: 'grey' } }
  },
  parseHTML() {
    return [{ tag: 'span[data-type="status"]' }]
  },
  renderHTML({ HTMLAttributes }) {
    return ['span', mergeAttributes(HTMLAttributes, { 'data-type': 'status', class: 'status-lozenge', 'data-color': HTMLAttributes.color }), HTMLAttributes.text]
  },
  addNodeView() {
    return ReactNodeViewRenderer(StatusView)
  },
})

/** Instructional text for authors (templates, Confluence's "instructional text"): shown in grey
 *  while editing, hidden from readers, never exported. Typing over it replaces it. */
export const InstructionalText = Node.create({
  name: 'instructionalText',
  group: 'inline',
  inline: true,
  atom: true,
  selectable: true,
  addAttributes() {
    return { text: { default: '', rendered: false } }
  },
  parseHTML() {
    return [{ tag: 'span[data-type="instructional-text"]', getAttrs: (el) => ({ text: (el as HTMLElement).textContent ?? '' }) }]
  },
  renderHTML({ node, HTMLAttributes }) {
    return ['span', mergeAttributes(HTMLAttributes, { 'data-type': 'instructional-text', class: 'instructional-text', title: 'Instructions: click and type to replace' }), node.attrs.text as string]
  },
})

function DateView({ node, updateAttributes, editor }: ReactNodeViewProps) {
  const raw = (node.attrs.date as string) || ''
  const d = new Date(raw)
  const label = isNaN(d.getTime()) ? raw : d.toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' })
  const edit = () => {
    if (!editor.isEditable) return
    const v = window.prompt('Date (YYYY-MM-DD)', raw.slice(0, 10))
    if (v) updateAttributes({ date: v })
  }
  return (
    <NodeViewWrapper as="span" className="date-chip" onClick={edit}>
      {label}
    </NodeViewWrapper>
  )
}

export const DateNode = Node.create({
  name: 'date',
  group: 'inline',
  inline: true,
  atom: true,
  addAttributes() {
    return { date: { default: new Date().toISOString().slice(0, 10) } }
  },
  parseHTML() {
    return [{ tag: 'time[datetime]', getAttrs: (el) => ({ date: (el as HTMLElement).getAttribute('datetime') }) }]
  },
  renderHTML({ HTMLAttributes }) {
    return ['time', { datetime: HTMLAttributes.date, class: 'date-chip' }, HTMLAttributes.date]
  },
  addNodeView() {
    return ReactNodeViewRenderer(DateView)
  },
})

function AttachmentView({ node }: ReactNodeViewProps) {
  const name = (node.attrs.name as string) || 'attachment'
  const href = node.attrs.href as string | null
  const lower = name.toLowerCase()
  const isImage = /\.(png|jpe?g|gif|webp)$/.test(lower)
  const isPdf = lower.endsWith('.pdf')
  return (
    <NodeViewWrapper className="block-card" contentEditable={false}>
      <div className="attachment-card">
        <div className="file-icon">
          <Paperclip size={16} />
        </div>
        <div style={{ flex: 1, minWidth: 0 }}>
          <div style={{ fontWeight: 600, overflow: 'hidden', textOverflow: 'ellipsis' }}>{name}</div>
          <div className="small faint">{node.attrs.missing ? 'File was not included in the import' : 'Attachment'}</div>
        </div>
        {href && (
          <a className="btn btn-sm" href={href} target="_blank" rel="noreferrer">
            <Download size={14} /> Open
          </a>
        )}
      </div>
      {href && isImage && <img src={href} alt={name} style={{ marginTop: 10, maxHeight: 360 }} />}
      {href && isPdf && <iframe src={href} title={name} style={{ width: '100%', height: 420, border: 0, marginTop: 10, borderRadius: 6 }} />}
    </NodeViewWrapper>
  )
}

export const AttachmentNode = Node.create({
  name: 'attachment',
  group: 'block',
  atom: true,
  addAttributes() {
    return { href: { default: null }, name: { default: null }, view: { default: null }, missing: { default: null } }
  },
  parseHTML() {
    return [{ tag: 'div[data-type="attachment"]' }]
  },
  renderHTML({ HTMLAttributes }) {
    return ['div', mergeAttributes(HTMLAttributes, { 'data-type': 'attachment' }), ['a', { href: HTMLAttributes.href }, HTMLAttributes.name ?? 'attachment']]
  },
  addNodeView() {
    return ReactNodeViewRenderer(AttachmentView)
  },
})

/** Converts known provider URLs to embeddable iframe URLs (allowlist). */
export function embedUrl(url: string): string | null {
  try {
    const u = new URL(url)
    const host = u.hostname.replace(/^www\./, '')
    if (host === 'youtube.com' && u.searchParams.get('v')) return `https://www.youtube-nocookie.com/embed/${u.searchParams.get('v')}`
    if (host === 'youtu.be') return `https://www.youtube-nocookie.com/embed${u.pathname}`
    if (host === 'vimeo.com' && /^\/\d+/.test(u.pathname)) return `https://player.vimeo.com/video${u.pathname}`
    if (host === 'loom.com' && u.pathname.startsWith('/share/')) return `https://www.loom.com/embed/${u.pathname.split('/')[2]}`
    if (host === 'figma.com') return `https://www.figma.com/embed?embed_host=wikicraft&url=${encodeURIComponent(url)}`
    if (host === 'docs.google.com') return url.replace(/\/edit.*$/, '/preview')
    if (host === 'miro.com' && u.pathname.startsWith('/app/board/')) return `https://miro.com/app/live-embed/${u.pathname.split('/')[3]}/`
  } catch {
    /* not a URL */
  }
  return null
}

function EmbedView({ node }: ReactNodeViewProps) {
  const url = (node.attrs.url as string) || ''
  const frame = embedUrl(url)
  // other sites get a preview card (title, description, site), fetched by the server
  const preview = useQuery({
    queryKey: ['unfurl', url],
    queryFn: () => api.get<{ title: string | null; description: string | null; siteName: string | null }>(`/unfurl?url=${encodeURIComponent(url)}`),
    enabled: !frame && /^https?:\/\//i.test(url),
    staleTime: 3_600_000,
    retry: false,
  })
  let host = ''
  try {
    host = new URL(url).hostname.replace(/^www\./, '')
  } catch {
    host = ''
  }
  const p = preview.data
  return (
    <NodeViewWrapper className="block-card" contentEditable={false}>
      {frame ? (
        <>
          <div className="block-label">
            <Link2 size={13} /> Embed
          </div>
          <iframe className="embed-frame" src={frame} title={url} sandbox="allow-scripts allow-same-origin allow-popups allow-presentation" allowFullScreen />
          <a href={url} target="_blank" rel="noreferrer noopener">{url}</a>
        </>
      ) : (
        <a className="link-card" href={url} target="_blank" rel="noreferrer noopener">
          <span className="link-card-site faint small">
            <Link2 size={12} /> {p?.siteName || host || 'Link'}
          </span>
          <span className="link-card-title">{p?.title || url}</span>
          {p?.description && <span className="link-card-desc faint small">{p.description}</span>}
          {p?.title && <span className="link-card-url faint small">{url}</span>}
        </a>
      )}
    </NodeViewWrapper>
  )
}

export const Embed = Node.create({
  name: 'embed',
  group: 'block',
  atom: true,
  addAttributes() {
    return { url: { default: '' } }
  },
  parseHTML() {
    return [{ tag: 'div[data-type="embed"]' }]
  },
  renderHTML({ HTMLAttributes }) {
    return ['div', mergeAttributes(HTMLAttributes, { 'data-type': 'embed' }), ['a', { href: HTMLAttributes.url }, HTMLAttributes.url]]
  },
  addNodeView() {
    return ReactNodeViewRenderer(EmbedView)
  },
})

export function slugify(s: string): string {
  const out = s
    .toLowerCase()
    .replace(/[^\p{L}\p{N}]+/gu, '-')
    .replace(/^-+|-+$/g, '')
  return out || 'section'
}

function TocView({ editor, node }: ReactNodeViewProps) {
  const min = (node.attrs.minLevel as number) || 1
  const max = (node.attrs.maxLevel as number) || 6
  const items: { level: number; text: string; pos: number }[] = []
  editor.state.doc.descendants((n, pos) => {
    if (n.type.name === 'heading') {
      const level = n.attrs.level as number
      if (level >= min && level <= max) items.push({ level, text: n.textContent, pos })
    }
  })
  const jump = (pos: number) => {
    const dom = editor.view.nodeDOM(pos) as HTMLElement | null
    dom?.scrollIntoView({ behavior: 'smooth', block: 'start' })
  }
  return (
    <NodeViewWrapper className="block-card toc-list" contentEditable={false}>
      <div className="block-label">
        <Hash size={13} /> Table of contents
      </div>
      {items.length === 0 ? (
        <div className="faint small">Add headings to populate the table of contents.</div>
      ) : (
        items.map((i, idx) => (
          <div key={idx} className={`toc-l${i.level}`}>
            <a onClick={() => jump(i.pos)}>{i.text || 'Untitled heading'}</a>
          </div>
        ))
      )}
    </NodeViewWrapper>
  )
}

export const Toc = Node.create({
  name: 'toc',
  group: 'block',
  atom: true,
  addAttributes() {
    return { minLevel: { default: null }, maxLevel: { default: null } }
  },
  parseHTML() {
    return [{ tag: 'nav[data-type="toc"]' }]
  },
  renderHTML({ HTMLAttributes }) {
    return ['nav', mergeAttributes(HTMLAttributes, { 'data-type': 'toc' })]
  },
  addNodeView() {
    return ReactNodeViewRenderer(TocView)
  },
})

interface ChildRow {
  id: string
  title: string
  parentId: string | null
  excerpt: string
}

function ChildPagesView({ extension, node }: ReactNodeViewProps) {
  const pageId = extension.options.pageId as string
  const depth = (node.attrs.depth as number | null) ?? 1
  const q = useQuery({ queryKey: ['children', pageId, depth], queryFn: () => api.get<ChildRow[]>(`/pages/${pageId}/children?depth=${depth}`), enabled: !!pageId })
  let rows = q.data ?? []
  if (node.attrs.reverse) rows = [...rows].reverse()
  if (node.attrs.limit) rows = rows.slice(0, node.attrs.limit as number)
  const byParent = new Map<string, ChildRow[]>()
  for (const r of rows) byParent.set(r.parentId ?? '', [...(byParent.get(r.parentId ?? '') ?? []), r])
  const render = (parent: string, level: number): React.ReactNode => {
    const kids = byParent.get(parent)
    if (!kids?.length || level > 50) return null
    return (
      <ul>
        {kids.map((k) => (
          <li key={k.id}>
            <Link to={`/pages/${k.id}`}>{k.title}</Link>
            {render(k.id, level + 1)}
          </li>
        ))}
      </ul>
    )
  }
  return (
    <NodeViewWrapper className="block-card" contentEditable={false}>
      <div className="block-label">
        <ListTree size={13} /> Child pages
      </div>
      {q.isLoading ? <div className="spinner" /> : rows.length === 0 ? <div className="faint small">No child pages yet.</div> : render(pageId, 0)}
    </NodeViewWrapper>
  )
}

export const ChildPages = Node.create<{ pageId: string }>({
  name: 'childPages',
  group: 'block',
  atom: true,
  addOptions() {
    return { pageId: '' }
  },
  addAttributes() {
    return { depth: { default: null }, sort: { default: null }, reverse: { default: null }, style: { default: null }, limit: { default: null }, search: { default: null } }
  },
  parseHTML() {
    return [{ tag: 'div[data-type="childPages"]' }]
  },
  renderHTML({ HTMLAttributes }) {
    return ['div', mergeAttributes(HTMLAttributes, { 'data-type': 'childPages' })]
  },
  addNodeView() {
    return ReactNodeViewRenderer(ChildPagesView)
  },
})

function plainExcerpt(doc: PmNode, max = 600): string {
  const parts: string[] = []
  const walk = (n: PmNode) => {
    if (n.text) parts.push(n.text)
    n.content?.forEach(walk)
    if (['paragraph', 'heading', 'listItem'].includes(n.type)) parts.push('\n')
  }
  walk(doc)
  const s = parts.join('').replace(/\n{2,}/g, '\n').trim()
  return s.length > max ? s.slice(0, max) + '…' : s
}

function IncludeView({ node }: ReactNodeViewProps) {
  const pageId = node.attrs.pageId as string | null
  const q = useQuery({ queryKey: ['page', pageId], queryFn: () => api.get<Page>(`/pages/${pageId}`), enabled: !!pageId, retry: false })
  return (
    <NodeViewWrapper className="block-card" contentEditable={false}>
      <div className="block-label">
        <FileText size={13} /> {node.attrs.excerpt ? 'Excerpt from' : 'Included page'}{' '}
        {pageId ? <Link to={`/pages/${pageId}`}>{q.data?.title ?? (node.attrs.title as string)}</Link> : <span>{node.attrs.title as string}</span>}
      </div>
      {q.data ? <div style={{ whiteSpace: 'pre-wrap' }}>{plainExcerpt(q.data.content)}</div> : q.isError ? <div className="faint small">Page not available.</div> : null}
    </NodeViewWrapper>
  )
}

export const Include = Node.create({
  name: 'include',
  group: 'block',
  atom: true,
  addAttributes() {
    return { pageId: { default: null }, href: { default: null }, title: { default: '' }, excerpt: { default: null } }
  },
  parseHTML() {
    return [{ tag: 'div[data-type="include"]' }]
  },
  renderHTML({ HTMLAttributes }) {
    return ['div', mergeAttributes(HTMLAttributes, { 'data-type': 'include' })]
  },
  addNodeView() {
    return ReactNodeViewRenderer(IncludeView)
  },
})

export const Anchor = Node.create({
  name: 'anchor',
  group: 'inline',
  inline: true,
  atom: true,
  addAttributes() {
    return { name: { default: '' } }
  },
  parseHTML() {
    return [{ tag: 'a[data-type="anchor"]' }]
  },
  renderHTML({ HTMLAttributes }) {
    return ['a', { 'data-type': 'anchor', id: slugify(HTMLAttributes.name || ''), class: 'anchor-dot', title: `Anchor: ${HTMLAttributes.name}` }]
  },
})

export const Columns = Node.create({
  name: 'columns',
  group: 'block',
  content: 'column+',
  addAttributes() {
    return { layout: { default: null } }
  },
  parseHTML() {
    return [{ tag: 'div[data-type="columns"]' }]
  },
  renderHTML({ HTMLAttributes }) {
    return ['div', mergeAttributes(HTMLAttributes, { 'data-type': 'columns', class: 'columns' }), 0]
  },
})

export const Column = Node.create({
  name: 'column',
  content: 'block+',
  isolating: true,
  parseHTML() {
    return [{ tag: 'div[data-type="column"]' }]
  },
  renderHTML({ HTMLAttributes }) {
    return ['div', mergeAttributes(HTMLAttributes, { 'data-type': 'column', class: 'column' }), 0]
  },
})

export const DecisionList = Node.create({
  name: 'decisionList',
  group: 'block',
  content: 'decisionItem+',
  parseHTML() {
    return [{ tag: 'ul[data-type="decisionList"]' }]
  },
  renderHTML({ HTMLAttributes }) {
    return ['ul', mergeAttributes(HTMLAttributes, { 'data-type': 'decisionList', class: 'decision-list' }), 0]
  },
})

export const DecisionItem = Node.create({
  name: 'decisionItem',
  content: 'block+',
  defining: true,
  addAttributes() {
    return { state: { default: 'DECIDED' } }
  },
  parseHTML() {
    return [{ tag: 'li[data-type="decisionItem"]' }]
  },
  renderHTML({ HTMLAttributes }) {
    return ['li', mergeAttributes(HTMLAttributes, { 'data-type': 'decisionItem' }), ['div', 0]]
  },
})

function MacroView({ node, extension }: ReactNodeViewProps) {
  const params = (node.attrs.params as Record<string, string> | null) ?? {}
  const name = node.attrs.name as string
  if (LIVE_MACROS.has(name)) {
    return (
      <NodeViewWrapper className="block-card live-macro" contentEditable={false} data-macro={name}>
        <LiveMacro name={name} params={params} pageId={extension.options.pageId as string} />
      </NodeViewWrapper>
    )
  }
  const entries = Object.entries(params).filter(([, v]) => v)
  return (
    <NodeViewWrapper className="macro-box">
      <div className="block-label" contentEditable={false}>
        <Puzzle size={13} /> Confluence macro: {node.attrs.name as string}
        {entries.length > 0 && (
          <span className="faint" style={{ textTransform: 'none', fontWeight: 400 }}>
            {' '}
            ({entries.slice(0, 4).map(([k, v]) => `${k}=${String(v).slice(0, 40)}`).join(', ')})
          </span>
        )}
      </div>
      {node.attrs.bodyText ? (
        <pre contentEditable={false} style={{ fontSize: 12, maxHeight: 200, overflow: 'auto' }}>
          {node.attrs.bodyText as string}
        </pre>
      ) : null}
      <NodeViewContent />
    </NodeViewWrapper>
  )
}

export const Macro = Node.create<{ pageId: string }>({
  name: 'macro',
  group: 'block',
  content: 'block*',
  addOptions() {
    return { pageId: '' }
  },
  addAttributes() {
    return { name: { default: 'macro' }, params: { default: null }, bodyText: { default: null } }
  },
  parseHTML() {
    return [{ tag: 'div[data-type="macro"]' }]
  },
  renderHTML({ HTMLAttributes }) {
    return ['div', mergeAttributes({ 'data-type': 'macro', 'data-name': HTMLAttributes.name }), 0]
  },
  addNodeView() {
    return ReactNodeViewRenderer(MacroView)
  },
})

/** Inline comment anchor. */
export const CommentMark = Mark.create<{ resolved: Set<string> }>({
  name: 'comment',
  inclusive: false,
  excludes: '',
  addOptions() {
    return { resolved: new Set() }
  },
  addAttributes() {
    return { commentId: { default: null } }
  },
  parseHTML() {
    return [{ tag: 'span[data-comment-id]', getAttrs: (el) => ({ commentId: (el as HTMLElement).getAttribute('data-comment-id') }) }]
  },
  renderHTML({ HTMLAttributes }) {
    return ['span', { 'data-comment-id': HTMLAttributes.commentId, class: 'inline-comment' }, 0]
  },
})
