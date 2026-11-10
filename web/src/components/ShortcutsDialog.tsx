import { useEffect, useState } from 'react'
import { Modal } from '../ui'

const mod = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform) ? '⌘' : 'Ctrl'

const GROUPS: [string, [string, string][]][] = [
  ['General', [[`${mod} K`, 'Search pages'], [`${mod} F`, 'Find in page'], [`${mod} H`, 'Find and replace (editing)'], [`${mod} /`, 'Keyboard shortcuts'], ['Esc', 'Close dialogs and menus']]],
  ['Formatting', [[`${mod} B`, 'Bold'], [`${mod} I`, 'Italic'], [`${mod} U`, 'Underline'], [`${mod} Shift X`, 'Strikethrough'], [`${mod} E`, 'Inline code'], [`${mod} Shift H`, 'Highlight']]],
  ['Blocks', [['/', 'Insert anything (slash menu)'], ['# ', 'Heading (## and ### for smaller)'], ['- or * ', 'Bullet list'], ['1. ', 'Numbered list'], ['[] ', 'Task list'], ['> ', 'Quote'], ['```', 'Code block'], ['---', 'Divider'], [`Tab at the start / ${mod} ]`, 'Indent a paragraph'], [`Shift Tab / ${mod} [`, 'Outdent']]],
  ['Linking', [['[[', 'Link to a page (or create one)'], ['@', 'Mention someone or link a page'], [':tick', 'Insert an emoticon'], [`${mod} Z / ${mod} Shift Z`, 'Undo / redo']]],
  ['Tables', [['Tab / Shift Tab', 'Next / previous cell'], ['Toolbar above the table', 'Rows, columns, headers, merge, colour']]],
]

/** Opens with Ctrl/⌘ + / anywhere in the app. */
export default function ShortcutsDialog() {
  const [open, setOpen] = useState(false)
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key === '/') {
        e.preventDefault()
        setOpen((o) => !o)
      }
    }
    const onOpen = () => setOpen(true)
    window.addEventListener('keydown', onKey)
    window.addEventListener('wikicraft:shortcuts', onOpen)
    return () => {
      window.removeEventListener('keydown', onKey)
      window.removeEventListener('wikicraft:shortcuts', onOpen)
    }
  }, [])
  if (!open) return null
  return (
    <Modal title="Keyboard shortcuts" onClose={() => setOpen(false)} width={720}>
      <div className="shortcuts">
        {GROUPS.map(([group, rows]) => (
          <section key={group}>
            <h4>{group}</h4>
            {rows.map(([keys, what]) => (
              <div key={what} className="shortcut-row">
                <span>{what}</span>
                <span className="keys">{keys.split(' ').map((k, i) => <kbd key={i}>{k}</kbd>)}</span>
              </div>
            ))}
          </section>
        ))}
      </div>
    </Modal>
  )
}
