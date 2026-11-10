import { useEffect, useMemo, useRef, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useEditor, EditorContent, ReactNodeViewRenderer, type Editor as TEditor } from '@tiptap/react'
import { BubbleMenu } from '@tiptap/react/menus'
import DragHandle from '@tiptap/extension-drag-handle-react'
import StarterKit from '@tiptap/starter-kit'
import Collaboration from '@tiptap/extension-collaboration'
import CollaborationCaret from '@tiptap/extension-collaboration-caret'
import { TableKit, TableCell, TableHeader } from '@tiptap/extension-table'
import { CellSelection } from '@tiptap/pm/tables'
import { TaskList, TaskItem } from '@tiptap/extension-list'
import Image from '@tiptap/extension-image'
import { Placeholder, CharacterCount } from '@tiptap/extensions'
import { TextStyle, Color } from '@tiptap/extension-text-style'
import Highlight from '@tiptap/extension-highlight'
import { Details, DetailsSummary, DetailsContent } from '@tiptap/extension-details'
import { Mathematics } from '@tiptap/extension-mathematics'
import Mention from '@tiptap/extension-mention'
import TextAlign from '@tiptap/extension-text-align'
import Subscript from '@tiptap/extension-subscript'
import Superscript from '@tiptap/extension-superscript'
import { common, createLowlight } from 'lowlight'
import * as Y from 'yjs'
import { WebsocketProvider } from 'y-websocket'
import 'katex/dist/katex.min.css'
import { Bold, Italic, Underline, Strikethrough, Code, Link as LinkIcon, Highlighter, MessageSquarePlus, Heading2, List, GripVertical } from 'lucide-react'
import { api, initials, userColor, type Attachment, type User } from '../api'
import { AttachmentNode, Anchor, Callout, ChildPages, Column, Columns, CommentMark, DateNode, DecisionItem, DecisionList, Embed, Emoticon, Include, Macro, Status, Toc } from './nodes'
import { EmoticonSuggest, PageLinkSuggest, SlashCommand, mentionSuggestion, slashItems } from './suggest'
import { CodeBlock } from './CodeBlock'
import { ImageView } from './ImageView'
import { TableMenu } from './TableMenu'
import { FindBar, FindReplace, findKey } from './FindReplace'

const lowlight = createLowlight(common)

/** Plain text that is probably Markdown: block syntax on some line, or inline emphasis/links. */
export function looksLikeMarkdown(text: string): boolean {
  if (!text.trim() || text.length > 1_000_000) return false
  const blocks = /^(#{1,6} \S|[-*+] \S|\d+[.)] \S|> \S|```|\|.+\|\s*$|- \[[ xX]\] )/m.test(text)
  const inline = /\*\*[^*\n]+\*\*|__[^_\n]+__|\[[^\]\n]+\]\([^)\s]+\)|`[^`\n]+`/.test(text)
  return text.includes('\n') ? blocks || inline : inline
}

/** HTML with real structure (Word, Google Docs, web pages); editors like VS Code only send styled divs. */
function hasRichHtml(html: string): boolean {
  return /<(h[1-6]|ul|ol|li|table|p|strong|b|em|i|a|pre|blockquote|img)\b/i.test(html)
}

const withBackground = <T extends typeof TableCell>(ext: T) =>
  ext.extend({
    addAttributes() {
      return {
        ...this.parent?.(),
        background: {
          default: null,
          parseHTML: (el: HTMLElement) => el.style.backgroundColor || el.getAttribute('data-background'),
          renderHTML: (attrs: Record<string, unknown>) => (attrs.background ? { style: `background-color: ${attrs.background}`, 'data-background': attrs.background } : {}),
        },
      }
    },
  })

const ImageExt = Image.extend({
  addAttributes() {
    return {
      ...this.parent?.(),
      width: { default: null, renderHTML: (a: Record<string, unknown>) => (a.width ? { width: a.width } : {}) },
      align: { default: null },
      caption: { default: null },
    }
  },
  addNodeView() {
    return ReactNodeViewRenderer(ImageView)
  },
}).configure({ inline: true, allowBase64: false })

export interface Presence {
  clientId: number
  name: string
  color: string
}

interface Props {
  pageId: string
  user: User | null
  editable: boolean
  onPresence?: (users: Presence[]) => void
  onSynced?: (editor: TEditor) => void
  onCommentClick?: (commentId: string) => void
  onRequestComment?: (text: string, apply: (commentId: string) => void) => void
  onStats?: (words: number) => void
}

async function uploadFiles(pageId: string, files: File[]): Promise<Attachment[]> {
  const form = new FormData()
  files.forEach((f) => form.append('file', f, f.name))
  return api.upload<Attachment[]>(`/pages/${pageId}/attachments`, form)
}

function pickFiles(accept: string): Promise<File[]> {
  return new Promise((resolve) => {
    const input = document.createElement('input')
    input.type = 'file'
    input.accept = accept
    input.multiple = true
    input.onchange = () => resolve(Array.from(input.files ?? []))
    input.click()
  })
}

export default function Editor({ pageId, user, editable, onPresence, onSynced, onCommentClick, onRequestComment, onStats }: Props) {
  const navigate = useNavigate()
  const [synced, setSynced] = useState(false)
  const [status, setStatus] = useState<'connecting' | 'connected' | 'disconnected'>('connecting')
  const ydoc = useMemo(() => new Y.Doc(), [pageId])
  const provider = useMemo(() => {
    const proto = location.protocol === 'https:' ? 'wss' : 'ws'
    return new WebsocketProvider(`${proto}://${location.host}/api/pages/${pageId}`, 'collab', ydoc, { disableBc: true, maxBackoffTime: 5000 })
  }, [pageId, ydoc])
  const me = useMemo(() => ({ name: user?.name ?? 'Guest', color: userColor(user?.id ?? 'guest') }), [user])

  const upload = async (editor: TEditor, files: File[], asImage: boolean) => {
    if (!files.length) return
    const rows = await uploadFiles(pageId, files)
    const nodes = rows.map((a) =>
      asImage && a.mediaType.startsWith('image/')
        ? { type: 'image', attrs: { src: `/api/attachments/${a.id}/content`, alt: a.filename } }
        : { type: 'attachment', attrs: { href: `/api/attachments/${a.id}/content`, name: a.filename } },
    )
    editor.chain().focus().insertContent(nodes).run()
  }

  const editorRef = useRef<TEditor | null>(null)
  const editor = useEditor(
    {
      editable: false,
      extensions: [
        StarterKit.configure({ undoRedo: false, codeBlock: false, link: { openOnClick: false, autolink: true, HTMLAttributes: { rel: 'noopener noreferrer' } } }),
        Collaboration.configure({ document: ydoc, field: 'default' }),
        CollaborationCaret.configure({ provider, user: me }),
        CodeBlock.configure({ lowlight }),
        TableKit.configure({ table: { resizable: true }, tableCell: false, tableHeader: false }),
        withBackground(TableCell),
        withBackground(TableHeader as unknown as typeof TableCell),
        TaskList,
        TaskItem.configure({ nested: true }),
        ImageExt,
        Placeholder.configure({ placeholder: ({ node }) => (node.type.name === 'heading' ? 'Heading' : "Type '/' for commands, '@' to mention someone") }),
        CharacterCount,
        TextStyle,
        Color,
        Highlight.configure({ multicolor: true }),
        TextAlign.configure({ types: ['heading', 'paragraph'] }),
        Subscript,
        Superscript,
        Details.configure({ persist: true, HTMLAttributes: { class: 'details' } }),
        DetailsSummary,
        DetailsContent,
        Mathematics,
        Mention.configure({ HTMLAttributes: { class: 'mention' }, suggestion: mentionSuggestion as never, renderText: ({ node }) => `@${node.attrs.label ?? ''}` }),
        Callout,
        Status,
        Emoticon,
        DateNode,
        AttachmentNode,
        Embed,
        Toc,
        ChildPages.configure({ pageId }),
        Include,
        Anchor,
        Columns,
        Column,
        DecisionList,
        DecisionItem,
        Macro.configure({ pageId }),
        PageLinkSuggest.configure({ pageId }),
        EmoticonSuggest,
        CommentMark,
        FindReplace,
        SlashCommand.configure({ items: slashItems((e) => pickFiles('image/*').then((f) => upload(e, f, true)), (e) => pickFiles('*/*').then((f) => upload(e, f, false))) }),
      ],
      editorProps: {
        handlePaste: (_view, event) => {
          const files = Array.from(event.clipboardData?.files ?? [])
          if (files.length && editorRef.current) {
            upload(editorRef.current, files, true)
            return true
          }
          // Markdown text (from a .md file, a chat, a terminal) becomes formatted content
          const ed = editorRef.current
          const text = event.clipboardData?.getData('text/plain') ?? ''
          const html = event.clipboardData?.getData('text/html') ?? ''
          if (ed && looksLikeMarkdown(text) && !hasRichHtml(html) && !ed.isActive('codeBlock')) {
            event.preventDefault()
            api
              .post<{ content: { content?: unknown[] } }>('/markdown', { markdown: text })
              .then((r) => ed.chain().focus().insertContent((r.content.content ?? []) as never).run())
              .catch(() => ed.chain().focus().insertContent(text).run())
            return true
          }
          return false
        },
        handleDrop: (_view, event) => {
          const files = Array.from((event as DragEvent).dataTransfer?.files ?? [])
          if (files.length && editorRef.current) {
            event.preventDefault()
            upload(editorRef.current, files, true)
            return true
          }
          return false
        },
      },
      onUpdate: ({ editor }) => onStats?.(editor.storage.characterCount.words()),
    },
    [pageId, provider],
  )
  editorRef.current = editor

  // read-only until the initial sync: typing into the empty pre-sync doc would interleave with the server state
  useEffect(() => {
    editor?.setEditable(editable && synced)
  }, [editor, editable, synced])

  useEffect(() => {
    const onSync = (isSynced: boolean) => {
      if (isSynced) {
        setSynced(true)
        if (editor) {
          onSynced?.(editor)
          onStats?.(editor.storage.characterCount.words())
        }
      }
    }
    const onStatus = ({ status }: { status: 'connecting' | 'connected' | 'disconnected' }) => setStatus(status)
    provider.on('sync', onSync)
    provider.on('status', onStatus)
    const aw = provider.awareness
    const onAw = () => {
      const list: Presence[] = []
      aw.getStates().forEach((s, id) => {
        if (id !== aw.clientID && s.user) list.push({ clientId: id, name: s.user.name, color: s.user.color })
      })
      onPresence?.(list)
    }
    aw.on('change', onAw)
    return () => {
      provider.off('sync', onSync)
      provider.off('status', onStatus)
      aw.off('change', onAw)
    }
  }, [provider, editor, onPresence, onSynced, onStats])

  useEffect(
    () => () => {
      provider.destroy()
      ydoc.destroy()
    },
    [provider, ydoc],
  )

  // find (Ctrl+F) and replace (Ctrl+H) while the page or its editor has focus
  const [find, setFind] = useState<{ replace: boolean; initial: string } | null>(null)
  const container = useRef<HTMLDivElement>(null)
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || e.altKey || !editor) return
      const key = e.key.toLowerCase()
      if (key !== 'f' && key !== 'h') return
      const active = document.activeElement
      const inPage = !active || active === document.body || !!container.current?.contains(active)
      if (!inPage || (key === 'h' && !editor.isEditable)) return
      e.preventDefault()
      const { from, to } = editor.state.selection
      const selected = from !== to ? editor.state.doc.textBetween(from, to, ' ').slice(0, 200) : ''
      setFind((f) => ({ replace: key === 'h' || !!f?.replace, initial: selected || '' }))
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [editor])

  // in-app navigation for internal links; inline comment clicks
  const onClick = (e: React.MouseEvent) => {
    const target = e.target as HTMLElement
    const commentEl = target.closest('[data-comment-id]') as HTMLElement | null
    if (commentEl && onCommentClick) onCommentClick(commentEl.dataset.commentId!)
    const a = target.closest('a') as HTMLAnchorElement | null
    if (!a || !a.getAttribute('href')) return
    const href = a.getAttribute('href')!
    if (editor?.isEditable && !(e.metaKey || e.ctrlKey)) return // editing: Ctrl/Cmd+click follows links
    e.preventDefault()
    if (href.startsWith('/') && !href.startsWith('/api/')) navigate(href)
    else if (href.startsWith('#')) document.getElementById(href.slice(1))?.scrollIntoView({ behavior: 'smooth' })
    else window.open(href, '_blank', 'noopener,noreferrer')
  }

  const setLink = () => {
    if (!editor) return
    const prev = editor.getAttributes('link').href as string | undefined
    const url = window.prompt('Link URL', prev ?? 'https://')
    if (url === null) return
    if (url === '') editor.chain().focus().extendMarkRange('link').unsetLink().run()
    else editor.chain().focus().extendMarkRange('link').setLink({ href: url }).run()
  }

  const addComment = () => {
    if (!editor || !onRequestComment) return
    const { from, to } = editor.state.selection
    const text = editor.state.doc.textBetween(from, to, ' ')
    onRequestComment(text, (commentId) => {
      editor.chain().focus().setTextSelection({ from, to }).setMark('comment', { commentId }).run()
    })
  }

  return (
    <div onClick={onClick} ref={container}>
      {find && editor && <FindBar key={`${find.replace}-${find.initial}`} editor={editor} withReplace={find.replace} initial={find.initial} onClose={() => setFind(null)} />}
      {!synced && (
        <div className="row faint" style={{ padding: '12px 0' }}>
          <div className="spinner" /> {status === 'disconnected' ? 'Reconnecting…' : 'Loading document…'}
        </div>
      )}
      {editor && (
        <BubbleMenu editor={editor} shouldShow={({ editor, state }) => !state.selection.empty && !findKey.getState(state)?.query && (editor.isEditable || !!onRequestComment) && !editor.isActive('image') && !(state.selection instanceof CellSelection)}>
          <div className="bubble">
            {editor.isEditable && (
              <>
                <button className={`icon-btn ${editor.isActive('bold') ? 'active' : ''}`} title="Bold (Ctrl+B)" onClick={() => editor.chain().focus().toggleBold().run()}><Bold size={15} /></button>
                <button className={`icon-btn ${editor.isActive('italic') ? 'active' : ''}`} title="Italic (Ctrl+I)" onClick={() => editor.chain().focus().toggleItalic().run()}><Italic size={15} /></button>
                <button className={`icon-btn ${editor.isActive('underline') ? 'active' : ''}`} title="Underline (Ctrl+U)" onClick={() => editor.chain().focus().toggleUnderline().run()}><Underline size={15} /></button>
                <button className={`icon-btn ${editor.isActive('strike') ? 'active' : ''}`} title="Strikethrough" onClick={() => editor.chain().focus().toggleStrike().run()}><Strikethrough size={15} /></button>
                <button className={`icon-btn ${editor.isActive('code') ? 'active' : ''}`} title="Inline code" onClick={() => editor.chain().focus().toggleCode().run()}><Code size={15} /></button>
                <button className={`icon-btn ${editor.isActive('highlight') ? 'active' : ''}`} title="Highlight" onClick={() => editor.chain().focus().toggleHighlight().run()}><Highlighter size={15} /></button>
                <span className="sep" />
                <button className={`icon-btn ${editor.isActive('heading', { level: 2 }) ? 'active' : ''}`} title="Heading" onClick={() => editor.chain().focus().toggleHeading({ level: 2 }).run()}><Heading2 size={15} /></button>
                <button className={`icon-btn ${editor.isActive('bulletList') ? 'active' : ''}`} title="Bullet list" onClick={() => editor.chain().focus().toggleBulletList().run()}><List size={15} /></button>
                <button className={`icon-btn ${editor.isActive('link') ? 'active' : ''}`} title="Link" onClick={setLink}><LinkIcon size={15} /></button>
                <span className="sep" />
              </>
            )}
            {onRequestComment && (
              <button className="icon-btn" title="Comment" onClick={addComment}><MessageSquarePlus size={15} /></button>
            )}
          </div>
        </BubbleMenu>
      )}
      {editor && editor.isEditable && <TableMenu editor={editor} />}
      {/* drag blocks (and list items) to move them; click to select the block */}
      {editor && editor.isEditable && (
        <DragHandle editor={editor} className="drag-handle" nested>
          <GripVertical size={15} />
        </DragHandle>
      )}
      <EditorContent editor={editor} />
      {/* clicking the empty space below the document puts the cursor at the end */}
      <div
        style={{ minHeight: 120, cursor: editor?.isEditable ? 'text' : 'default' }}
        onMouseDown={(e) => {
          if (!editor?.isEditable) return
          e.preventDefault()
          const last = editor.state.doc.lastChild
          if (last && (last.type.name !== 'paragraph' || last.content.size > 0)) {
            editor.chain().focus('end').insertContentAt(editor.state.doc.content.size, { type: 'paragraph' }).focus('end').run()
          } else {
            editor.commands.focus('end')
          }
        }}
      />
    </div>
  )
}

export function PresenceAvatars({ users }: { users: Presence[] }) {
  if (!users.length) return null
  return (
    <div className="presence" title={users.map((u) => u.name).join(', ')}>
      {users.slice(0, 5).map((u) => (
        <span key={u.clientId} className="avatar" style={{ background: u.color }}>
          {initials(u.name)}
        </span>
      ))}
    </div>
  )
}
