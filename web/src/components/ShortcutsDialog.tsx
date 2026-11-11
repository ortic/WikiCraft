import { t } from '../i18n'
import { useEffect, useState } from 'react'
import { Modal } from '../ui'

const mod = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform) ? '⌘' : 'Ctrl'

const GROUPS: [string, [string, string][]][] = [
  [t('General'), [[`${mod} K`, t('Search pages')], [`${mod} F`, t('Find in page')], [`${mod} H`, t('Find and replace (editing)')], [`${mod} /`, t('Keyboard shortcuts')], ['Esc', t('Close dialogs and menus')]]],
  [t('Formatting'), [[`${mod} B`, t('Bold')], [`${mod} I`, t('Italic')], [`${mod} U`, t('Underline')], [`${mod} Shift X`, t('Strikethrough')], [`${mod} E`, t('Inline code')], [`${mod} Shift H`, t('Highlight')]]],
  [t('Blocks'), [['/', t('Insert anything (slash menu)')], ['# ', t('Heading (## and ### for smaller)')], ['- or * ', t('Bullet list')], ['1. ', t('Numbered list')], ['[] ', t('Task list')], ['> ', t('Quote')], ['```', t('Code block')], ['---', t('Divider')], [`Tab at the start / ${mod} ]`, t('Indent a paragraph')], [`Shift Tab / ${mod} [`, t('Outdent')]]],
  [t('Linking'), [['[[', t('Link to a page (or create one)')], ['@', t('Mention someone or link a page')], [':tick', t('Insert an emoticon')], [`${mod} Z / ${mod} Shift Z`, t('Undo / redo')]]],
  [t('Tables'), [['Tab / Shift Tab', t('Next / previous cell')], [t('Toolbar above the table'), t('Rows, columns, headers, merge, colour')]]],
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
    <Modal title={t('Keyboard shortcuts')} onClose={() => setOpen(false)} width={720}>
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
