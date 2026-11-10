// Text colour, highlight colour and alignment for the selection (from the bubble menu). The
// colours are mid-tones and translucent highlights that read well in light and dark themes.

import { useState } from 'react'
import type { Editor } from '@tiptap/react'
import { AlignCenter, AlignLeft, AlignRight, Baseline, Ban } from 'lucide-react'

export const TEXT_COLORS: [string, string][] = [
  ['Grey', '#6b7280'],
  ['Red', '#d14343'],
  ['Orange', '#c2620a'],
  ['Green', '#2e8540'],
  ['Teal', '#0f8a8a'],
  ['Blue', '#2f6fdb'],
  ['Purple', '#8b4fd6'],
]

export const HIGHLIGHTS: [string, string][] = [
  ['Yellow', 'rgba(250, 204, 21, 0.35)'],
  ['Green', 'rgba(34, 197, 94, 0.28)'],
  ['Blue', 'rgba(59, 130, 246, 0.25)'],
  ['Purple', 'rgba(168, 85, 247, 0.25)'],
  ['Red', 'rgba(239, 68, 68, 0.25)'],
  ['Grey', 'rgba(120, 120, 135, 0.22)'],
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
        title="Text colour, highlight and alignment"
        aria-label="Text colour, highlight and alignment"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
      >
        <Baseline size={15} style={current ? { color: current } : undefined} />
      </button>
      {open && (
        <div className="text-style-pop card" role="group" aria-label="Text style">
          <div className="text-style-row">
            <span className="faint small">Text</span>
            <button onMouseDown={keep} className="swatch" title="Default colour" aria-label="Default text colour" onClick={() => run((c) => c.unsetColor())}>
              <Ban size={13} />
            </button>
            {TEXT_COLORS.map(([name, color]) => (
              <button key={name} onMouseDown={keep} className={`swatch${current === color ? ' on' : ''}`} title={name} aria-label={`Text colour ${name}`} onClick={() => run((c) => c.setColor(color))}>
                <span style={{ color, fontWeight: 700 }}>A</span>
              </button>
            ))}
          </div>
          <div className="text-style-row">
            <span className="faint small">Highlight</span>
            <button onMouseDown={keep} className="swatch" title="No highlight" aria-label="No highlight" onClick={() => run((c) => c.unsetHighlight())}>
              <Ban size={13} />
            </button>
            {HIGHLIGHTS.map(([name, color]) => (
              <button
                key={name}
                onMouseDown={keep}
                className={`swatch${editor.isActive('highlight', { color }) ? ' on' : ''}`}
                title={`${name} highlight`}
                aria-label={`Highlight ${name}`}
                style={{ background: color }}
                onClick={() => run((c) => c.setHighlight({ color }))}
              />
            ))}
          </div>
          <div className="text-style-row">
            <span className="faint small">Align</span>
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
                title={`Align ${align}`}
                aria-label={`Align ${align}`}
                onClick={() => run((c) => c.setTextAlign(align))}
              >
                <Icon size={14} />
              </button>
            ))}
          </div>
        </div>
      )}
    </span>
  )
}
