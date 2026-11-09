import { useEffect, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { CheckCircle2, RotateCcw, Trash2, X, MessageSquare, ThumbsUp } from 'lucide-react'
import { api, formatDate, type Comment, type Page, type PmNode } from '../api'
import { Avatar, useMe } from '../ui'

/** Renders a (small) comment document as React elements — no HTML injection. */
function RenderDoc({ node }: { node: PmNode }) {
  const inline = (n: PmNode, i: number): React.ReactNode => {
    if (n.type === 'text') {
      let el: React.ReactNode = n.text
      for (const m of n.marks ?? []) {
        if (m.type === 'bold') el = <b>{el}</b>
        else if (m.type === 'italic') el = <i>{el}</i>
        else if (m.type === 'code') el = <code>{el}</code>
        else if (m.type === 'strike') el = <s>{el}</s>
        else if (m.type === 'link') {
          const href = String(m.attrs?.href ?? '')
          el = /^(https?:|mailto:|\/)/.test(href) ? <a href={href} target={href.startsWith('/') ? undefined : '_blank'} rel="noreferrer">{el}</a> : el
        }
      }
      return <span key={i}>{el}</span>
    }
    if (n.type === 'mention') return <span key={i} className="mention">@{String(n.attrs?.label ?? '')}</span>
    if (n.type === 'hardBreak') return <br key={i} />
    return <span key={i}>{n.content?.map(inline)}</span>
  }
  const block = (n: PmNode, i: number): React.ReactNode => {
    switch (n.type) {
      case 'paragraph':
        return <p key={i}>{n.content?.map(inline)}</p>
      case 'bulletList':
        return <ul key={i}>{n.content?.map(block)}</ul>
      case 'orderedList':
        return <ol key={i}>{n.content?.map(block)}</ol>
      case 'listItem':
        return <li key={i}>{n.content?.map(block)}</li>
      case 'codeBlock':
        return <pre key={i}><code>{n.content?.map((t) => t.text).join('')}</code></pre>
      case 'heading':
        return <p key={i}><b>{n.content?.map(inline)}</b></p>
      default:
        return n.content ? <div key={i}>{n.content.map(block)}</div> : null
    }
  }
  return <>{node.content?.map(block)}</>
}

function Composer({ placeholder, onSubmit, autoFocus, onCancel }: { placeholder: string; onSubmit: (text: string) => Promise<void>; autoFocus?: boolean; onCancel?: () => void }) {
  const [text, setText] = useState('')
  const [busy, setBusy] = useState(false)
  const submit = async () => {
    if (!text.trim()) return
    setBusy(true)
    try {
      await onSubmit(text)
      setText('')
    } finally {
      setBusy(false)
    }
  }
  return (
    <div>
      <textarea
        className="textarea"
        placeholder={placeholder}
        value={text}
        autoFocus={autoFocus}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) submit()
        }}
        style={{ minHeight: 60 }}
      />
      <div className="row" style={{ marginTop: 6 }}>
        <span className="faint small">Markdown supported · Ctrl+Enter to send</span>
        <span className="spacer" />
        {onCancel && (
          <button className="btn btn-sm" onClick={onCancel}>
            Cancel
          </button>
        )}
        <button className="btn btn-primary btn-sm" disabled={busy || !text.trim()} onClick={submit}>
          Comment
        </button>
      </div>
    </div>
  )
}

export default function CommentsPanel({ page, focus, pending, onPendingDone, onClose }: { page: Page; focus: string | null; pending: { text: string; apply: (id: string) => void } | null; onPendingDone: () => void; onClose: () => void }) {
  const qc = useQueryClient()
  const me = useMe()
  const [showResolved, setShowResolved] = useState(false)
  const [replyTo, setReplyTo] = useState<string | null>(null)
  const comments = useQuery({ queryKey: ['comments', page.id], queryFn: () => api.get<Comment[]>(`/pages/${page.id}/comments`) })
  const refresh = () => qc.invalidateQueries({ queryKey: ['comments', page.id] })

  useEffect(() => {
    if (focus) document.getElementById(`comment-${focus}`)?.scrollIntoView({ behavior: 'smooth', block: 'center' })
  }, [focus, comments.data])

  const post = async (text: string, extra: Record<string, unknown> = {}) => {
    const c = await api.post<Comment>(`/pages/${page.id}/comments`, { text, ...extra })
    refresh()
    return c
  }
  const roots = (comments.data ?? []).filter((c) => !c.parentId)
  const replies = (id: string) => (comments.data ?? []).filter((c) => c.parentId === id)
  const visible = roots.filter((c) => showResolved || !c.resolvedAt)
  const resolvedCount = roots.filter((c) => c.resolvedAt).length

  const item = (c: Comment, isReply = false) => {
    const mine = c.authorId === me.data?.user?.id
    return (
      <div key={c.id} id={`comment-${c.id}`} className={isReply ? 'reply' : `comment ${focus === c.id ? 'focused' : ''} ${c.resolvedAt ? 'resolved' : ''}`}>
        <div className="c-head">
          <Avatar name={c.authorName ?? '?'} id={c.authorId ?? undefined} size={22} />
          <b style={{ fontSize: 13 }}>{c.authorName ?? 'Unknown'}</b>
          <span className="faint small">{formatDate(c.createdAt)}</span>
          <span className="spacer" />
          {!isReply && page.access.comment && (
            <button
              className="icon-btn"
              style={{ width: 24, height: 24 }}
              title={c.resolvedAt ? 'Reopen' : 'Resolve'}
              onClick={async () => {
                await api.patch(`/comments/${c.id}`, { resolved: !c.resolvedAt })
                refresh()
              }}
            >
              {c.resolvedAt ? <RotateCcw size={13} /> : <CheckCircle2 size={14} />}
            </button>
          )}
          {(mine || page.access.admin) && (
            <button
              className="icon-btn"
              style={{ width: 24, height: 24 }}
              title="Delete"
              onClick={async () => {
                if (confirm('Delete this comment?')) {
                  await api.del(`/comments/${c.id}`)
                  refresh()
                }
              }}
            >
              <Trash2 size={13} />
            </button>
          )}
        </div>
        {!isReply && c.inline?.text && <div className="c-quote">{c.inline.text}</div>}
        <div className="c-body">
          <RenderDoc node={c.content} />
        </div>
        <div className="c-foot">
          {me.data?.user ? (
            <button
              className={`like-btn${c.liked ? ' liked' : ''}`}
              title={c.liked ? 'Unlike' : 'Like'}
              onClick={async () => {
                await (c.liked ? api.del(`/comments/${c.id}/like`) : api.put(`/comments/${c.id}/like`))
                refresh()
              }}
            >
              <ThumbsUp size={12} /> {c.likeCount ? c.likeCount : ''}
            </button>
          ) : c.likeCount ? (
            <span className="faint small"><ThumbsUp size={12} /> {c.likeCount}</span>
          ) : null}
        </div>
        {c.resolvedAt && !isReply && <div className="faint small">Resolved by {c.resolvedByName ?? 'someone'}</div>}
        {!isReply && (
          <>
            {replies(c.id).map((r) => item(r, true))}
            {page.access.comment &&
              (replyTo === c.id ? (
                <div className="reply">
                  <Composer placeholder="Reply…" autoFocus onCancel={() => setReplyTo(null)} onSubmit={async (t) => { await post(t, { parentId: c.id }); setReplyTo(null) }} />
                </div>
              ) : (
                <button className="btn btn-ghost btn-sm" style={{ marginTop: 6 }} onClick={() => setReplyTo(c.id)}>
                  Reply
                </button>
              ))}
          </>
        )}
      </div>
    )
  }

  return (
    <aside className="side-panel">
      <h3>
        <MessageSquare size={16} /> Comments <span className="spacer" />
        <button className="icon-btn" onClick={onClose}>
          <X size={16} />
        </button>
      </h3>
      {pending && (
        <div className="comment focused">
          <div className="c-quote">{pending.text}</div>
          <Composer
            placeholder="Add an inline comment…"
            autoFocus
            onCancel={onPendingDone}
            onSubmit={async (t) => {
              const c = await post(t, { inline: { text: pending.text } })
              pending.apply(c.id)
              onPendingDone()
            }}
          />
        </div>
      )}
      {page.access.comment && !pending && (
        <div style={{ marginBottom: 14 }}>
          <Composer placeholder="Add a comment…" onSubmit={async (t) => { await post(t) }} />
        </div>
      )}
      {visible.map((c) => item(c))}
      {comments.data && visible.length === 0 && <div className="empty">No comments yet.</div>}
      {resolvedCount > 0 && (
        <button className="btn btn-ghost btn-sm" onClick={() => setShowResolved((s) => !s)}>
          {showResolved ? 'Hide' : 'Show'} {resolvedCount} resolved
        </button>
      )}
    </aside>
  )
}
