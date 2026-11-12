// Slash-command menu and @mention suggestions.

import { t } from '../i18n'
import { Extension, type Editor, type Range } from '@tiptap/core'
import { ReactRenderer } from '@tiptap/react'
import Suggestion, { type SuggestionOptions, type SuggestionProps, type SuggestionKeyDownProps } from '@tiptap/suggestion'
import { PluginKey } from '@tiptap/pm/state'
import { EMOTICONS, Emoticon } from '../icons'
import { issueRef } from './nodes'
import { forwardRef, useEffect, useImperativeHandle, useState, type ReactNode } from 'react'
import {
  Heading1, Heading2, Heading3, List, ListOrdered, ListChecks, Table2, Quote, Code2, Minus, Table, Info, AlertTriangle, CheckCircle2, XCircle, ChevronRight,
  UserRound, Hash, ListTree, CalendarDays, Tag, Sigma, Image as ImageIcon, Link2, Columns2, Paperclip, StickyNote, FileText, FilePlus2, PenTool, Workflow, Columns3, PanelLeft, PanelRight, Search,
  MessageSquareDashed, Milestone, FileInput, Scissors, CircleDot,
} from 'lucide-react'
import { api } from '../api'
import { headingIds } from './HeadingIds'

export interface MenuItem {
  title: string
  description?: string
  icon?: ReactNode
  group?: string
  keywords?: string
  command: (editor: Editor, range: Range) => void
}

interface ListHandle {
  onKeyDown: (p: SuggestionKeyDownProps) => boolean
}

const SuggestionList = forwardRef<ListHandle, { items: MenuItem[]; command: (i: MenuItem) => void }>(function SuggestionList({ items, command }, ref) {
  const [sel, setSel] = useState(0)
  useEffect(() => setSel(0), [items])
  useImperativeHandle(ref, () => ({
    onKeyDown: ({ event }) => {
      if (event.key === 'ArrowDown') {
        setSel((s) => (s + 1) % Math.max(items.length, 1))
        return true
      }
      if (event.key === 'ArrowUp') {
        setSel((s) => (s - 1 + items.length) % Math.max(items.length, 1))
        return true
      }
      if (event.key === 'Enter') {
        if (items[sel]) command(items[sel])
        return true
      }
      return false
    },
  }))
  useEffect(() => {
    document.querySelector('.menu .menu-item.selected')?.scrollIntoView({ block: 'nearest' })
  }, [sel])
  if (!items.length) return <div className="menu"><div className="menu-item faint">{t('No results')}</div></div>
  let lastGroup: string | undefined
  return (
    <div className="menu">
      {items.map((it, i) => {
        const header = it.group && it.group !== lastGroup ? <div className="menu-group">{it.group}</div> : null
        lastGroup = it.group
        return (
          <div key={it.title + i}>
            {header}
            <div className={`menu-item ${i === sel ? 'selected' : ''}`} onMouseEnter={() => setSel(i)} onMouseDown={(e) => { e.preventDefault(); command(it) }}>
              {it.icon && <div className="mi-icon">{it.icon}</div>}
              <div>
                <div>{it.title}</div>
                {it.description && <div className="mi-desc">{it.description}</div>}
              </div>
            </div>
          </div>
        )
      })}
    </div>
  )
})

/** Generic popup renderer for suggestion plugins (positions next to the caret). */
export function popupRenderer<I>(hideWhenEmpty = false): SuggestionOptions<I>['render'] {
  return () => {
    let component: ReactRenderer<ListHandle> | null = null
    let host: HTMLDivElement | null = null
    const place = (props: SuggestionProps<I>) => {
      const rect = props.clientRect?.()
      if (!rect || !host) return
      host.style.display = hideWhenEmpty && props.items.length === 0 ? 'none' : ''
      const below = rect.bottom + 6
      const h = Math.min(360, host.offsetHeight || 300)
      host.style.left = `${Math.min(rect.left, window.innerWidth - 270)}px`
      host.style.top = `${below + h > window.innerHeight ? Math.max(8, rect.top - h - 6) : below}px`
    }
    return {
      onStart: (props) => {
        component = new ReactRenderer(SuggestionList, { props, editor: props.editor })
        host = document.createElement('div')
        host.style.position = 'fixed'
        host.style.zIndex = '60'
        host.appendChild(component.element)
        document.body.appendChild(host)
        place(props)
      },
      onUpdate: (props) => {
        component?.updateProps(props)
        place(props)
      },
      onKeyDown: (props) => {
        if (host?.style.display === 'none') return false
        if (props.event.key === 'Escape') {
          host?.remove()
          host = null
          return true
        }
        return component?.ref?.onKeyDown(props) ?? false
      },
      onExit: () => {
        host?.remove()
        component?.destroy()
        host = null
        component = null
      },
    }
  }
}

const ic = (C: typeof Info) => <C size={16} />

export function slashItems(onImage: (editor: Editor) => void, onAttachment: (editor: Editor) => void): MenuItem[] {
  return [
    { group: t('Basic'), title: t('Heading 1'), icon: ic(Heading1), keywords: t('h1 title'), command: (e, r) => e.chain().focus().deleteRange(r).setNode('heading', { level: 1 }).run() },
    { group: t('Basic'), title: t('Heading 2'), icon: ic(Heading2), keywords: t('h2'), command: (e, r) => e.chain().focus().deleteRange(r).setNode('heading', { level: 2 }).run() },
    { group: t('Basic'), title: t('Heading 3'), icon: ic(Heading3), keywords: t('h3'), command: (e, r) => e.chain().focus().deleteRange(r).setNode('heading', { level: 3 }).run() },
    { group: t('Basic'), title: t('Bullet list'), icon: ic(List), keywords: t('ul unordered'), command: (e, r) => e.chain().focus().deleteRange(r).toggleBulletList().run() },
    { group: t('Basic'), title: t('Numbered list'), icon: ic(ListOrdered), keywords: t('ol ordered'), command: (e, r) => e.chain().focus().deleteRange(r).toggleOrderedList().run() },
    { group: t('Basic'), title: t('Task list'), description: t('Checklist with assignees and dates'), icon: ic(ListChecks), keywords: t('todo checkbox action'), command: (e, r) => e.chain().focus().deleteRange(r).toggleTaskList().run() },
    { group: t('Basic'), title: t('Quote'), icon: ic(Quote), keywords: t('blockquote'), command: (e, r) => e.chain().focus().deleteRange(r).toggleBlockquote().run() },
    { group: t('Basic'), title: t('Code block'), icon: ic(Code2), keywords: t('snippet pre'), command: (e, r) => e.chain().focus().deleteRange(r).toggleCodeBlock().run() },
    { group: t('Media'), title: t('Drawing'), description: t('Sketch a diagram or whiteboard (Excalidraw)'), icon: ic(PenTool), keywords: t('excalidraw whiteboard sketch draw diagram'), command: (e, r) => e.chain().focus().deleteRange(r).insertContent({ type: 'drawing' }).run() },
    { group: t('Media'), title: t('Draw.io diagram'), description: t('Flowcharts, architecture and network diagrams (draw.io)'), icon: ic(Workflow), keywords: t('drawio diagrams.net flowchart uml architecture network'), command: (e, r) => e.chain().focus().deleteRange(r).insertContent({ type: 'drawing', attrs: { kind: 'drawio' } }).run() },
    { group: t('Media'), title: t('Diagram (Mermaid)'), description: t('Flowcharts, sequence diagrams, Gantt…'), icon: ic(Columns2), keywords: t('mermaid flowchart sequence chart'), command: (e, r) => e.chain().focus().deleteRange(r).insertContent({ type: 'codeBlock', attrs: { language: 'mermaid' }, content: [{ type: 'text', text: t('flowchart LR\n  A[Idea] --> B{Review}\n  B -->|approved| C[Ship it]\n  B -->|changes| A') }] }).run() },
    { group: t('Basic'), title: t('Divider'), icon: ic(Minus), keywords: t('hr rule'), command: (e, r) => e.chain().focus().deleteRange(r).setHorizontalRule().run() },
    { group: t('Basic'), title: t('Table'), icon: ic(Table), keywords: t('grid'), command: (e, r) => e.chain().focus().deleteRange(r).insertTable({ rows: 3, cols: 3, withHeaderRow: true }).run() },
    { group: t('Panels'), title: t('Info panel'), icon: ic(Info), keywords: t('callout note'), command: (e, r) => e.chain().focus().deleteRange(r).wrapIn('callout', { type: 'info' }).run() },
    { group: t('Panels'), title: t('Note panel'), icon: ic(StickyNote), keywords: t('callout'), command: (e, r) => e.chain().focus().deleteRange(r).wrapIn('callout', { type: 'note' }).run() },
    { group: t('Panels'), title: t('Success panel'), icon: ic(CheckCircle2), keywords: t('callout tip'), command: (e, r) => e.chain().focus().deleteRange(r).wrapIn('callout', { type: 'success' }).run() },
    { group: t('Panels'), title: t('Warning panel'), icon: ic(AlertTriangle), keywords: t('callout caution'), command: (e, r) => e.chain().focus().deleteRange(r).wrapIn('callout', { type: 'warning' }).run() },
    { group: t('Panels'), title: t('Error panel'), icon: ic(XCircle), keywords: t('callout danger'), command: (e, r) => e.chain().focus().deleteRange(r).wrapIn('callout', { type: 'error' }).run() },
    { group: t('Panels'), title: t('Expand'), description: t('Collapsible section'), icon: ic(ChevronRight), keywords: t('details toggle collapse'), command: (e, r) => e.chain().focus().deleteRange(r).setDetails().run() },
    { group: t('Panels'), title: t('Two columns'), icon: ic(Columns2), keywords: t('layout section'), command: (e, r) => e.chain().focus().deleteRange(r).insertContent({ type: 'columns', content: [{ type: 'column', content: [{ type: 'paragraph' }] }, { type: 'column', content: [{ type: 'paragraph' }] }] }).run() },
    { group: t('Panels'), title: t('Three columns'), icon: ic(Columns3), keywords: t('layout section 3'), command: (e, r) => e.chain().focus().deleteRange(r).insertContent({ type: 'columns', attrs: { layout: 'three_equal' }, content: [{ type: 'column', content: [{ type: 'paragraph' }] }, { type: 'column', content: [{ type: 'paragraph' }] }, { type: 'column', content: [{ type: 'paragraph' }] }] }).run() },
    { group: t('Panels'), title: t('Sidebar on the left'), icon: ic(PanelLeft), keywords: t('layout section columns narrow'), command: (e, r) => e.chain().focus().deleteRange(r).insertContent({ type: 'columns', attrs: { layout: 'two_left_sidebar' }, content: [{ type: 'column', content: [{ type: 'paragraph' }] }, { type: 'column', content: [{ type: 'paragraph' }] }] }).run() },
    { group: t('Panels'), title: t('Sidebar on the right'), icon: ic(PanelRight), keywords: t('layout section columns narrow'), command: (e, r) => e.chain().focus().deleteRange(r).insertContent({ type: 'columns', attrs: { layout: 'two_right_sidebar' }, content: [{ type: 'column', content: [{ type: 'paragraph' }] }, { type: 'column', content: [{ type: 'paragraph' }] }] }).run() },
    { group: t('Media'), title: t('Image'), description: t('Upload an image'), icon: ic(ImageIcon), keywords: t('picture photo'), command: (e, r) => { e.chain().focus().deleteRange(r).run(); onImage(e) } },
    {
      group: t('Media'),
      title: t('Image from link'),
      description: t('Show an image by its URL'),
      icon: ic(Link2),
      keywords: t('picture url external'),
      command: (e, r) => {
        const url = window.prompt(t('Image URL (https://…)'))
        e.chain().focus().deleteRange(r).run()
        if (url && /^https?:\/\//i.test(url.trim())) e.chain().focus().setImage({ src: url.trim() }).run()
      },
    },
    { group: t('Media'), title: t('File'), description: t('Upload a file with preview'), icon: ic(Paperclip), keywords: t('attachment pdf upload'), command: (e, r) => { e.chain().focus().deleteRange(r).run(); onAttachment(e) } },
    { group: t('Media'), title: t('Embed or link preview'), description: t('YouTube, Loom, Figma, Miro… or a preview card for any page'), icon: ic(Link2), keywords: t('iframe video smart link card unfurl preview'), command: (e, r) => { const url = window.prompt(t('URL to embed')); if (url) e.chain().focus().deleteRange(r).insertContent({ type: 'embed', attrs: { url } }).run() } },
    // blocks without text of their own get a paragraph after them: typing goes on below the block
    // instead of replacing it
    { group: t('Macros'), title: t('Table of contents'), icon: ic(Hash), keywords: t('toc headings'), command: (e, r) => e.chain().focus().deleteRange(r).insertContent([{ type: 'toc' }, { type: 'paragraph' }]).run() },
    { group: t('Macros'), title: t('Child pages'), icon: ic(ListTree), keywords: t('children tree'), command: (e, r) => e.chain().focus().deleteRange(r).insertContent([{ type: 'childPages', attrs: { depth: 1 } }, { type: 'paragraph' }]).run() },
    { group: t('Macros'), title: t('Include page'), description: t('Another page’s content, always up to date'), icon: ic(FileInput), keywords: t('transclude embed reuse excerpt snippet'), command: (e, r) => e.chain().focus().deleteRange(r).insertContent([{ type: 'include' }, { type: 'paragraph' }]).run() },
    { group: t('Macros'), title: t('Excerpt'), description: t('Mark text other pages can include'), icon: ic(Scissors), keywords: t('summary snippet reuse include'), command: (e, r) => e.chain().focus().deleteRange(r).wrapIn('excerpt').run() },
    {
      group: t('Macros'),
      title: t('Issue'),
      description: t('GitHub, GitLab or Jira, with its status'),
      icon: ic(CircleDot),
      keywords: t('jira github gitlab ticket bug pull request merge request'),
      command: (e, r) => {
        const text = window.prompt(t('Issue address, or a Jira key like OPS-42'))
        e.chain().focus().deleteRange(r).run()
        const ref = text ? issueRef(text) : null
        if (ref) e.chain().focus().insertContent([{ type: 'issue', attrs: ref }, { type: 'text', text: ' ' }]).run()
      },
    },
    {
      group: t('Macros'),
      title: t('Jira issues'),
      description: t('A live table of the issues a JQL query finds'),
      icon: ic(CircleDot),
      keywords: t('jira jql issues table query'),
      command: (e, r) => {
        const jql = window.prompt(t('Jira query (JQL)'), 'project = ABC AND resolution = Unresolved ORDER BY priority DESC')
        e.chain().focus().deleteRange(r).run()
        if (jql?.trim()) e.chain().focus().insertContent([{ type: 'macro', attrs: { name: 'jira', params: { jqlQuery: jql.trim(), columns: 'key,summary,type,assignee,priority,status', maximumIssues: '20' } } }, { type: 'paragraph' }]).run()
      },
    },
    { group: t('Macros'), title: t('Status'), icon: ic(Tag), keywords: t('lozenge label'), command: (e, r) => e.chain().focus().deleteRange(r).insertContent({ type: 'status', attrs: { text: t('IN PROGRESS'), color: 'blue' } }).run() },
    { group: t('Macros'), title: t('Decision'), description: t('Record a decision (shows up in decision logs)'), icon: ic(Milestone), keywords: t('decided adr choice'), command: (e, r) => e.chain().focus().deleteRange(r).insertContent({ type: 'decisionList', content: [{ type: 'decisionItem', attrs: { state: 'DECIDED' }, content: [{ type: 'paragraph' }] }] }).run() },
    { group: t('Macros'), title: t('Decision log'), description: t('Decisions recorded in this space'), icon: ic(Milestone), keywords: t('decisions report decisionreport'), command: (e, r) => e.chain().focus().deleteRange(r).insertContent([{ type: 'macro', attrs: { name: 'decisionreport', params: {} } }, { type: 'paragraph' }]).run() },
    { group: t('Macros'), title: t('Properties report'), description: t('Pages in this space and their properties, as a table'), icon: ic(Table2), keywords: t('page properties report detailssummary query table database'), command: (e, r) => e.chain().focus().deleteRange(r).insertContent([{ type: 'macro', attrs: { name: 'detailssummary', params: { spaces: '@self' } } }, { type: 'paragraph' }]).run() },
    { group: t('Macros'), title: t('Task report'), description: t('Open tasks in this space'), icon: ic(ListChecks), keywords: t('tasks report todo'), command: (e, r) => e.chain().focus().deleteRange(r).insertContent([{ type: 'macro', attrs: { name: 'tasks-report-macro', params: {} } }, { type: 'paragraph' }]).run() },
    { group: t('Macros'), title: t('Search box'), description: t('Search this space (or the wiki) from the page'), icon: ic(Search), keywords: t('livesearch search find'), command: (e, r) => e.chain().focus().deleteRange(r).insertContent([{ type: 'macro', attrs: { name: 'livesearch', params: { spaceKey: '@self' } } }, { type: 'paragraph' }]).run() },
    { group: t('Macros'), title: t('Create-page button'), description: t('A button that creates a page below this one, from a template'), icon: ic(FilePlus2), keywords: t('create from template button new page'), command: (e, r) => e.chain().focus().deleteRange(r).insertContent([{ type: 'macro', attrs: { name: 'create-from-template', params: {} } }, { type: 'paragraph' }]).run() },
    { group: t('Macros'), title: t('Instructions'), description: t('Hint for authors, hidden from readers'), icon: ic(MessageSquareDashed), keywords: t('placeholder instructional text template hint'), command: (e, r) => { const text = window.prompt(t('Instructions for whoever fills in this page (readers never see them)'), t('Describe the goal here')); if (text?.trim()) e.chain().focus().deleteRange(r).insertContent({ type: 'instructionalText', attrs: { text: text.trim() } }).run() } },
    { group: t('Macros'), title: t('Date'), icon: ic(CalendarDays), keywords: t('today time'), command: (e, r) => e.chain().focus().deleteRange(r).insertContent({ type: 'date', attrs: { date: new Date().toISOString().slice(0, 10) } }).run() },
    { group: t('Macros'), title: t('Math'), description: t('LaTeX formula'), icon: ic(Sigma), keywords: t('latex katex equation'), command: (e, r) => { const latex = window.prompt(t('LaTeX'), 'E = mc^2'); if (latex) e.chain().focus().deleteRange(r).insertContent({ type: 'blockMath', attrs: { latex } }).run() } },
  ]
}

export const SlashCommand = Extension.create<{ items: MenuItem[] }>({
  name: 'slashCommand',
  addOptions() {
    return { items: [] }
  },
  addProseMirrorPlugins() {
    const all = this.options.items
    return [
      Suggestion<MenuItem>({
        editor: this.editor,
        char: '/',
        startOfLine: false,
        allowSpaces: false,
        items: ({ query }) => {
          const q = query.toLowerCase()
          return all.filter((i) => !q || i.title.toLowerCase().includes(q) || i.keywords?.includes(q)).slice(0, 30)
        },
        command: ({ editor, range, props }) => props.command(editor, range),
        render: popupRenderer<MenuItem>(),
      }),
    ]
  },
})

interface UserHit {
  id: string
  name: string
}

interface SearchHit {
  pageId: string
  title: string
  spaceKey: string
  spaceName: string
}

/** `@` → people (a mention, which notifies them) and, from two letters on, pages (a link). */
export const mentionSuggestion: Omit<SuggestionOptions<MenuItem>, 'editor'> = {
  char: '@',
  items: async ({ query }) => {
    const q = query.trim()
    const [users, pages] = await Promise.all([
      api.get<UserHit[]>(`/users?q=${encodeURIComponent(q)}`).catch(() => [] as UserHit[]),
      q.length >= 2 ? api.get<SearchHit[]>(`/search?q=${encodeURIComponent(q)}`).catch(() => [] as SearchHit[]) : Promise.resolve([] as SearchHit[]),
    ])
    const people: MenuItem[] = users.slice(0, pages.length ? 5 : 8).map((u) => ({
      title: u.name,
      group: pages.length ? t('People') : undefined,
      icon: <UserRound size={16} />,
      command: (editor: Editor, range: Range) => editor.chain().focus().deleteRange(range).insertContent([{ type: 'mention', attrs: { id: u.id, label: u.name } }, { type: 'text', text: ' ' }]).run(),
    }))
    const linked: MenuItem[] = pages.slice(0, 5).map((h) => ({
      title: h.title,
      description: h.spaceName,
      group: t('Pages'),
      icon: <FileText size={16} />,
      command: (editor: Editor, range: Range) =>
        editor
          .chain()
          .focus()
          .deleteRange(range)
          .insertContent([{ type: 'text', text: h.title, marks: [{ type: 'link', attrs: { href: `/pages/${h.pageId}` } }] }, { type: 'text', text: ' ' }])
          .run(),
    }))
    return [...people, ...linked]
  },
  command: ({ editor, range, props }) => props.command(editor, range),
  render: popupRenderer<MenuItem>(),
}

/** `[[` → search pages and insert a link (or create the page). */
export const PageLinkSuggest = Extension.create<{ pageId: string }>({
  name: 'pageLinkSuggest',
  addOptions() {
    return { pageId: '' }
  },
  addProseMirrorPlugins() {
    const pageId = this.options.pageId
    const insertLink = (editor: Editor, range: Range, id: string, title: string) =>
      editor
        .chain()
        .focus()
        .deleteRange(range)
        .insertContent([{ type: 'text', text: title, marks: [{ type: 'link', attrs: { href: `/pages/${id}` } }] }, { type: 'text', text: ' ' }])
        .run()
    return [
      Suggestion<MenuItem>({
        editor: this.editor,
        char: '[[',
        allowSpaces: true,
        allowedPrefixes: null,
        pluginKey: new PluginKey('pageLinkSuggest'),
        items: async ({ query, editor }) => {
          const q = query.replace(/\]+$/, '').trim()
          const link = (ed: Editor, range: Range, href: string, text: string) =>
            ed.chain().focus().deleteRange(range).insertContent([{ type: 'text', text, marks: [{ type: 'link', attrs: { href } }] }, { type: 'text', text: ' ' }]).run()
          // [[#… → a heading on this page
          if (q.startsWith('#')) {
            const term = q.slice(1).toLowerCase()
            return headingIds(editor.state.doc)
              .filter((h) => h.text.toLowerCase().includes(term))
              .slice(0, 10)
              .map((h) => ({
                title: h.text || t('Untitled heading'),
                description: `#${h.id}`,
                group: t('Headings on this page'),
                icon: <Hash size={16} />,
                command: (ed: Editor, range: Range) => link(ed, range, `#${h.id}`, h.text),
              }))
          }
          let hits: SearchHit[] = []
          if (q) {
            hits = await api.get<SearchHit[]>(`/search?q=${encodeURIComponent(q)}`).catch(() => [])
          } else {
            const r = await api.get<{ viewed: { id: string; title: string; spaceName: string }[] }>('/recent').catch(() => ({ viewed: [] }))
            hits = r.viewed.map((v) => ({ pageId: v.id, title: v.title, spaceKey: '', spaceName: v.spaceName }))
          }
          const items: MenuItem[] = hits
            .filter((h) => h.pageId !== pageId)
            .slice(0, 8)
            .map((h) => ({
              title: h.title,
              description: h.spaceName,
              group: q ? t('Pages') : t('Recently viewed'),
              icon: <FileText size={16} />,
              command: (editor, range) => insertLink(editor, range, h.pageId, h.title),
            }))
          // attachments of this page
          if (q && pageId) {
            const files = await api.get<{ id: string; filename: string; size: number }[]>(`/pages/${pageId}/attachments`).catch(() => [])
            for (const f of files.filter((f) => f.filename.toLowerCase().includes(q.toLowerCase())).slice(0, 4)) {
              items.push({
                title: f.filename,
                description: t('Attachment on this page'),
                group: t('Attachments'),
                icon: <Paperclip size={16} />,
                command: (ed: Editor, range: Range) => link(ed, range, `/api/attachments/${f.id}/content`, f.filename),
              })
            }
          }
          if (!q) {
            items.push({
              title: t('Link to a heading…'),
              description: t('Type # and the heading'),
              group: t('Tip'),
              icon: <Hash size={16} />,
              command: (ed: Editor, range: Range) => ed.chain().focus().insertContentAt(range, '[[#').run(),
            })
          }
          if (q && !hits.some((h) => h.title.toLowerCase() === q.toLowerCase())) {
            items.push({
              title: t('Create “{q}”', { q }),
              description: t('New child page of this page'),
              group: t('New'),
              icon: <FilePlus2 size={16} />,
              command: async (editor, range) => {
                const page = await api.get<{ spaceKey: string }>(`/pages/${pageId}`)
                const created = await api.post<{ id: string }>('/pages', { spaceKey: page.spaceKey, parentId: pageId, title: q })
                insertLink(editor, range, created.id, q)
              },
            })
          }
          return items
        },
        command: ({ editor, range, props }) => props.command(editor, range),
        render: popupRenderer<MenuItem>(),
      }),
    ]
  },
})

/** `:name` → one of our emoticons (tick, cross, smile, …). */
export const EmoticonSuggest = Extension.create({
  name: 'emoticonSuggest',
  addProseMirrorPlugins() {
    return [
      Suggestion<MenuItem>({
        editor: this.editor,
        char: ':',
        allowSpaces: false,
        pluginKey: new PluginKey('emoticonSuggest'),
        items: ({ query }) => {
          // need at least one letter, so times like 10:30 or "Note:" don't open the menu
          if (!query) return []
          const q = query.toLowerCase()
          return EMOTICONS.filter((n) => n.includes(q))
            .slice(0, 10)
            .map((name) => ({
              title: name.replace(/-/g, ' '),
              icon: <Emoticon name={name} size={18} />,
              command: (editor: Editor, range: Range) => editor.chain().focus().deleteRange(range).insertContent([{ type: 'emoticon', attrs: { name } }, { type: 'text', text: ' ' }]).run(),
            }))
        },
        command: ({ editor, range, props }) => props.command(editor, range),
        render: popupRenderer<MenuItem>(true),
      }),
    ]
  },
})
