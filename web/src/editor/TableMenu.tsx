import { BubbleMenu } from '@tiptap/react/menus'
import { useEditorState, type Editor } from '@tiptap/react'
import { ArrowDownAZ, ArrowDownToLine, ArrowDownZA, ArrowLeftToLine, ArrowRightToLine, ArrowUpToLine, Columns3, Combine, PaintBucket, Rows3, Split, Trash2 } from 'lucide-react'
import { useState } from 'react'
import { Fragment, type Node as PMNode } from '@tiptap/pm/model'

/** The table around the cursor and the cursor's column, when the table has no merged cells. */
function tableAt(editor: Editor): { node: PMNode; pos: number; col: number } | null {
  const { $from } = editor.state.selection
  for (let d = $from.depth; d > 0; d--) {
    const node = $from.node(d)
    if (node.type.name !== 'table') continue
    if (d + 2 > $from.depth) return null
    let merged = false
    node.forEach((row) => row.forEach((cell) => { merged ||= cell.attrs.colspan > 1 || cell.attrs.rowspan > 1 }))
    return merged ? null : { node, pos: $from.before(d), col: $from.index(d + 1) }
  }
  return null
}

/** A number for values like "1,200", "12.5%", "$ 40", "CHF 1'000"; null for text. */
function numeric(s: string): number | null {
  const t = s.replace(/[\s,'’%$€£¥]|CHF|EUR|USD/g, '')
  return t && /^[-+]?\d*\.?\d+(e[-+]?\d+)?$/i.test(t) ? Number(t) : null
}

/** Sorts the rows below the header rows by the cursor's column (numbers numerically, empty cells last). */
export function sortTable(editor: Editor, dir: 1 | -1): boolean {
  const t = tableAt(editor)
  if (!t) return false
  const rows: PMNode[] = []
  t.node.forEach((r) => rows.push(r))
  let head = 0
  while (head < rows.length) {
    let allHeader = rows[head].childCount > 0
    rows[head].forEach((c) => { allHeader &&= c.type.name === 'tableHeader' })
    if (!allHeader) break
    head++
  }
  const key = (r: PMNode) => (t.col < r.childCount ? r.child(t.col).textContent.trim() : '')
  const sorted = rows.slice(head).sort((a, b) => {
    const [x, y] = [key(a), key(b)]
    if (!x || !y) return x ? -1 : y ? 1 : 0
    const [nx, ny] = [numeric(x), numeric(y)]
    const c = nx !== null && ny !== null ? nx - ny : x.localeCompare(y, undefined, { numeric: true, sensitivity: 'base' })
    return c * dir
  })
  const tr = editor.state.tr.replaceWith(t.pos + 1, t.pos + t.node.nodeSize - 1, Fragment.from([...rows.slice(0, head), ...sorted]))
  editor.view.dispatch(tr.scrollIntoView())
  editor.commands.focus()
  return true
}

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
  // re-render when the table's width mode changes (the menu doesn't otherwise follow attribute updates)
  const layout = useEditorState({ editor, selector: (ctx) => (ctx.editor.getAttributes('table').layout as string | null) ?? 'default' })
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
        {b('Sort rows by this column, A to Z / smallest first', () => sortTable(editor, 1), <ArrowDownAZ size={15} />, false, !tableAt(editor))}
        {b('Sort rows by this column, Z to A / largest first', () => sortTable(editor, -1), <ArrowDownZA size={15} />, false, !tableAt(editor))}
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
        {(() => {
          const next = layout === 'default' ? 'wide' : layout === 'wide' ? 'full-width' : 'default'
          const label = layout === 'wide' ? 'Wide' : layout === 'full-width' ? 'Full width' : 'Normal'
          return (
            <button type="button" className="icon-btn text-btn" title={`Table width: ${label} (click for ${next === 'default' ? 'normal' : next.replace('-', ' ')})`} onMouseDown={(e) => e.preventDefault()} onClick={run((c) => c.updateAttributes('table', { layout: next === 'default' ? null : next }))}>
              {label}
            </button>
          )
        })()}
        <span className="sep" />
        {b('Delete table', run((c) => c.deleteTable()), <Trash2 size={15} />, true)}
      </div>
    </BubbleMenu>
  )
}
