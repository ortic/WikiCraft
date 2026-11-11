// Text colour, highlight colour and alignment for the selection (from the bubble menu). The
// colours are mid-tones and translucent highlights that read well in light and dark themes.

import { t } from '../i18n'
import { useState } from 'react'
import type { Editor } from '@tiptap/react'
import { AlignCenter, AlignLeft, AlignRight, Baseline, Ban, IndentDecrease, IndentIncrease } from 'lucide-react'

export const TEXT_COLORS: [string, string][] = [
  [t('Grey'), '#6b7280'],
  [t('Red'), '#d14343'],
  [t('Orange'), '#c2620a'],
  [t('Green'), '#2e8540'],
  [t('Teal'), '#0f8a8a'],
  [t('Blue'), '#2f6fdb'],
  [t('Purple'), '#8b4fd6'],
]

export const HIGHLIGHTS: [string, string][] = [
  [t('Yellow'), 'rgba(250, 204, 21, 0.35)'],
  [t('Green'), 'rgba(34, 197, 94, 0.28)'],
  [t('Blue'), 'rgba(59, 130, 246, 0.25)'],
  [t('Purple'), 'rgba(168, 85, 247, 0.25)'],
  [t('Red'), 'rgba(239, 68, 68, 0.25)'],
  [t('Grey'), 'rgba(120, 120, 135, 0.22)'],
]

export default function TextStyleMenu({ editor }: { editor: Editor }) {
  const [open, setOpen] = useState(false)
  const current = editor.getAttributes('textStyle').color as string | undefined
  const run = (f: (c: ReturnType<Editor['chain']>) => ReturnType<Editor['chain']>) => {
    f(editor.chain().focus()).run()
    setOpen(false)
  }
  // buttons must not take focus: the editor keeps the selection and the keyboard
  const keep = (e: React.MouseEvent) => e.preventDefault()
  return (
    <span className="text-style">
      <button
        onMouseDown={keep}
        className={`icon-btn ${open || current || editor.isActive('highlight') ? 'active' : ''}`}
        title={t('Text colour, highlight, alignment and indentation')}
        aria-label={t('Text colour, highlight, alignment and indentation')}
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
      >
        <Baseline size={15} style={current ? { color: current } : undefined} />
      </button>
      {open && (
        <div className="text-style-pop card" role="group" aria-label={t('Text style')}>
          <div className="text-style-row">
            <span className="faint small">{t('Text')}</span>
            <button onMouseDown={keep} className="swatch" title={t('Default colour')} aria-label={t('Default text colour')} onClick={() => run((c) => c.unsetColor())}>
              <Ban size={13} />
            </button>
            {TEXT_COLORS.map(([name, color]) => (
              <button key={name} onMouseDown={keep} className={`swatch${current === color ? ' on' : ''}`} title={name} aria-label={t('Text colour {name}', { name })} onClick={() => run((c) => c.setColor(color))}>
                <span style={{ color, fontWeight: 700 }}>{t('A')}</span>
              </button>
            ))}
          </div>
          <div className="text-style-row">
            <span className="faint small">{t('Highlight')}</span>
            <button onMouseDown={keep} className="swatch" title={t('No highlight')} aria-label={t('No highlight')} onClick={() => run((c) => c.unsetHighlight())}>
              <Ban size={13} />
            </button>
            {HIGHLIGHTS.map(([name, color]) => (
              <button
                key={name}
                onMouseDown={keep}
                className={`swatch${editor.isActive('highlight', { color }) ? ' on' : ''}`}
                title={t('{name} highlight', { name })}
                aria-label={t('Highlight {name}', { name })}
                style={{ background: color }}
                onClick={() => run((c) => c.setHighlight({ color }))}
              />
            ))}
          </div>
          <div className="text-style-row">
            <span className="faint small">{t('Align')}</span>
            {(
              [
                ['left', AlignLeft],
                ['center', AlignCenter],
                ['right', AlignRight],
              ] as const
            ).map(([align, Icon]) => (
              <button
                key={align}
                onMouseDown={keep}
                className={`swatch${editor.isActive({ textAlign: align }) ? ' on' : ''}`}
                title={align === 'left' ? t('Align left') : align === 'center' ? t('Center') : t('Align right')}
                aria-label={align === 'left' ? t('Align left') : align === 'center' ? t('Center') : t('Align right')}
                onClick={() => run((c) => c.setTextAlign(align))}
              >
                <Icon size={14} />
              </button>
            ))}
            <button onMouseDown={keep} className="swatch" title={t('Indent (Ctrl+])')} aria-label={t('Indent')} onClick={() => editor.chain().focus().indent().run()}>
              <IndentIncrease size={14} />
            </button>
            <button onMouseDown={keep} className="swatch" title={t('Outdent (Ctrl+[)')} aria-label={t('Outdent')} onClick={() => editor.chain().focus().outdent().run()}>
              <IndentDecrease size={14} />
            </button>
          </div>
        </div>
      )}
    </span>
  )
}
