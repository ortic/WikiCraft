import { BubbleMenu } from '@tiptap/react/menus'
import type { Editor } from '@tiptap/react'
import { ArrowDownToLine, ArrowLeftToLine, ArrowRightToLine, ArrowUpToLine, Columns3, Combine, PaintBucket, Rows3, Split, Trash2 } from 'lucide-react'
import { useState } from 'react'

const CELL_COLORS = [
  ['None', null],
  ['Blue', '#dbeafe'],
  ['Green', '#dcfce7'],
  ['Yellow', '#fef9c3'],
  ['Orange', '#ffedd5'],
  ['Red', '#fee2e2'],
  ['Purple', '#f3e8ff'],
  ['Grey', '#f1f5f9'],
] as const

/** Floating toolbar above the table the cursor is in: rows, columns, headers, merge, colour. */
export function TableMenu({ editor }: { editor: Editor }) {
  const [colors, setColors] = useState(false)
  const tableRect = () => {
    const { from } = editor.state.selection
    const dom = editor.view.domAtPos(from).node as HTMLElement | null
    const el = (dom?.nodeType === 1 ? dom : dom?.parentElement)?.closest('table')
    return el ?? null
  }
  const run = (fn: (c: ReturnType<Editor['chain']>) => ReturnType<Editor['chain']>) => () => fn(editor.chain().focus()).run()
  const b = (title: string, onClick: () => void, icon: React.ReactNode, danger = false, disabled = false) => (
    <button type="button" className={`icon-btn${danger ? ' danger' : ''}`} title={title} aria-label={title} disabled={disabled} onMouseDown={(e) => e.preventDefault()} onClick={onClick}>
      {icon}
    </button>
  )
  return (
    <BubbleMenu
      editor={editor}
      pluginKey="tableMenu"
      shouldShow={({ editor }) => editor.isEditable && editor.isActive('table')}
      getReferencedVirtualElement={() => {
        const t = tableRect()
        return t ? { getBoundingClientRect: () => t.getBoundingClientRect(), getClientRects: () => t.getClientRects() } : null
      }}
      options={{ placement: 'top-start', offset: 8, flip: false }}
    >
      <div className="bubble table-menu" onMouseLeave={() => setColors(false)}>
        {b('Insert row above', run((c) => c.addRowBefore()), <ArrowUpToLine size={15} />)}
        {b('Insert row below', run((c) => c.addRowAfter()), <ArrowDownToLine size={15} />)}
        {b('Delete row', run((c) => c.deleteRow()), <Rows3 size={15} />, true)}
        <span className="sep" />
        {b('Insert column left', run((c) => c.addColumnBefore()), <ArrowLeftToLine size={15} />)}
        {b('Insert column right', run((c) => c.addColumnAfter()), <ArrowRightToLine size={15} />)}
        {b('Delete column', run((c) => c.deleteColumn()), <Columns3 size={15} />, true)}
        <span className="sep" />
        <button type="button" className={`icon-btn text-btn${editor.isActive('tableHeader') ? ' active' : ''}`} title="Header row" onMouseDown={(e) => e.preventDefault()} onClick={run((c) => c.toggleHeaderRow())}>Header row</button>
        <button type="button" className="icon-btn text-btn" title="Header column" onMouseDown={(e) => e.preventDefault()} onClick={run((c) => c.toggleHeaderColumn())}>Header col</button>
        <span className="sep" />
        {b('Merge cells', run((c) => c.mergeCells()), <Combine size={15} />, false, !editor.can().mergeCells())}
        {b('Split cell', run((c) => c.splitCell()), <Split size={15} />, false, !editor.can().splitCell())}
        <span className="table-color">
          {b('Cell colour', () => setColors((o) => !o), <PaintBucket size={15} />)}
          {colors && (
            <span className="color-pop">
              {CELL_COLORS.map(([name, c]) => (
                <button
                  key={name}
                  type="button"
                  className="swatch"
                  title={name}
                  aria-label={`Cell colour ${name}`}
                  style={{ background: c ?? 'var(--bg)', outline: c ? undefined : '1px dashed var(--border)' }}
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => {
                    editor.chain().focus().setCellAttribute('background', c).run()
                    setColors(false)
                  }}
                />
              ))}
            </span>
          )}
        </span>
        <span className="sep" />
        {b('Delete table', run((c) => c.deleteTable()), <Trash2 size={15} />, true)}
      </div>
    </BubbleMenu>
  )
}
