// The fixed formatting toolbar above the page while editing (the floating one still appears on a
// selection). People who prefer a clean page hide it; the choice stays in this browser.

import { t } from '../i18n'
import { useEditorState, type Editor } from '@tiptap/react'
import {
  Bold, Code, Image as ImageIcon, Italic, Link as LinkIcon, List, ListChecks, ListOrdered, PanelTopClose, Plus, Redo2,
  Strikethrough, Table, Underline, Undo2,
} from 'lucide-react'
import TextStyleMenu from './TextStyleMenu'

const KEY = 'wikicraft.editorToolbar'

export function toolbarHidden(): boolean {
  try {
    return localStorage.getItem(KEY) === 'hidden'
  } catch {
    return false
  }
}

export function setToolbarHidden(hidden: boolean) {
  try {
    if (hidden) localStorage.setItem(KEY, 'hidden')
    else localStorage.removeItem(KEY)
  } catch {
    // private mode: the choice lasts until the page is left
  }
}

type Style = 'paragraph' | 'h1' | 'h2' | 'h3' | 'h4' | 'blockquote' | 'codeBlock'

function currentStyle(e: Editor): Style {
  for (const level of [1, 2, 3, 4] as const) if (e.isActive('heading', { level })) return `h${level}`
  if (e.isActive('codeBlock')) return 'codeBlock'
  if (e.isActive('blockquote')) return 'blockquote'
  return 'paragraph'
}

export default function EditorToolbar({ editor, onImage, onLink, onHide }: { editor: Editor; onImage: () => void; onLink: () => void; onHide: () => void }) {
  const s = useEditorState({
    editor,
    selector: ({ editor: e }) => ({
      style: currentStyle(e),
      bold: e.isActive('bold'),
      italic: e.isActive('italic'),
      underline: e.isActive('underline'),
      strike: e.isActive('strike'),
      code: e.isActive('code'),
      bullet: e.isActive('bulletList'),
      ordered: e.isActive('orderedList'),
      task: e.isActive('taskList'),
      link: e.isActive('link'),
      inTable: e.isActive('table'),
      canUndo: e.can().undo(),
      canRedo: e.can().redo(),
    }),
  })
  // buttons must not take focus: the editor keeps the selection and the keyboard
  const keep = (e: React.MouseEvent) => e.preventDefault()
  const chain = () => editor.chain().focus()
  const setStyle = (v: Style) => {
    const c = chain()
    if (v === 'paragraph') c.setParagraph().run()
    else if (v === 'blockquote') c.setParagraph().toggleBlockquote().run()
    else if (v === 'codeBlock') c.setCodeBlock().run()
    else c.setHeading({ level: Number(v.slice(1)) as 1 | 2 | 3 | 4 }).run()
  }
  const btn = (active: boolean, title: string, icon: React.ReactNode, run: () => void, disabled = false) => (
    <button type="button" className={`icon-btn ${active ? 'active' : ''}`} title={title} aria-label={title} aria-pressed={active || undefined} disabled={disabled} onMouseDown={keep} onClick={run}>
      {icon}
    </button>
  )
  return (
    <div className="editor-toolbar" role="toolbar" aria-label={t('Formatting')}>
      {btn(false, t('Undo (Ctrl+Z)'), <Undo2 size={15} />, () => chain().undo().run(), !s.canUndo)}
      {btn(false, t('Redo (Ctrl+Shift+Z)'), <Redo2 size={15} />, () => chain().redo().run(), !s.canRedo)}
      <span className="sep" />
      <select className="input input-sm toolbar-style" aria-label={t('Paragraph style')} value={s.style} onChange={(e) => setStyle(e.target.value as Style)}>
        <option value="paragraph">{t('Normal text')}</option>
        <option value="h1">{t('Heading 1')}</option>
        <option value="h2">{t('Heading 2')}</option>
        <option value="h3">{t('Heading 3')}</option>
        <option value="h4">{t('Heading 4')}</option>
        <option value="blockquote">{t('Quote')}</option>
        <option value="codeBlock">{t('Code block')}</option>
      </select>
      <span className="sep" />
      {btn(s.bold, t('Bold (Ctrl+B)'), <Bold size={15} />, () => chain().toggleBold().run())}
      {btn(s.italic, t('Italic (Ctrl+I)'), <Italic size={15} />, () => chain().toggleItalic().run())}
      {btn(s.underline, t('Underline (Ctrl+U)'), <Underline size={15} />, () => chain().toggleUnderline().run())}
      {btn(s.strike, t('Strikethrough'), <Strikethrough size={15} />, () => chain().toggleStrike().run())}
      {btn(s.code, t('Inline code'), <Code size={15} />, () => chain().toggleCode().run())}
      <TextStyleMenu editor={editor} />
      <span className="sep" />
      {btn(s.bullet, t('Bullet list'), <List size={15} />, () => chain().toggleBulletList().run())}
      {btn(s.ordered, t('Numbered list'), <ListOrdered size={15} />, () => chain().toggleOrderedList().run())}
      {btn(s.task, t('Task list'), <ListChecks size={15} />, () => chain().toggleTaskList().run())}
      <span className="sep" />
      {btn(s.link, t('Link'), <LinkIcon size={15} />, onLink)}
      {btn(false, t('Table'), <Table size={15} />, () => chain().insertTable({ rows: 3, cols: 3, withHeaderRow: true }).run(), s.inTable)}
      {btn(false, t('Image'), <ImageIcon size={15} />, onImage)}
      {/* everything else: the same menu as typing "/" */}
      <button type="button" className="btn btn-ghost btn-sm toolbar-insert" title={t('Insert (or type /)')} onMouseDown={keep} onClick={() => chain().insertContent(editor.state.selection.$from.parent.textContent ? ' /' : '/').run()}>
        <Plus size={15} /> {t('Insert')}
      </button>
      <span className="spacer" />
      {btn(false, t('Hide the toolbar'), <PanelTopClose size={15} />, onHide)}
    </div>
  )
}
