// The node and mark types of page content: the page editor adds collaboration, suggestions and
// menus on top; read-only copies (an included page) use them as they are.

import type { Extensions } from '@tiptap/core'
import { ReactNodeViewRenderer } from '@tiptap/react'
import StarterKit from '@tiptap/starter-kit'
import { TableKit, TableCell, TableHeader, Table, TableView } from '@tiptap/extension-table'
import { TaskList, TaskItem } from '@tiptap/extension-list'
import Image, { type ImageOptions } from '@tiptap/extension-image'
import { TextStyle, Color } from '@tiptap/extension-text-style'
import Highlight from '@tiptap/extension-highlight'
import { Details, DetailsSummary, DetailsContent } from '@tiptap/extension-details'
import { Mathematics } from '@tiptap/extension-mathematics'
import Mention, { type MentionOptions } from '@tiptap/extension-mention'
import TextAlign from '@tiptap/extension-text-align'
import Subscript from '@tiptap/extension-subscript'
import Superscript from '@tiptap/extension-superscript'
import { common, createLowlight } from 'lowlight'
import type { Node as PMNode } from '@tiptap/pm/model'
import type { EditorView } from '@tiptap/pm/view'
import { Indent } from './Indent'
import { AttachmentNode, Anchor, Callout, ChildPages, Column, Columns, CommentMark, DateNode, DecisionItem, DecisionList, Embed, Emoticon, Excerpt, Include, InstructionalText, Issue, Macro, Status, Toc } from './nodes'
import { CodeBlock } from './CodeBlock'
import { ImageView } from './ImageView'
import { Drawing } from './Drawing'

const lowlight = createLowlight(common)

/** Table width (Confluence's `data-layout`): normal, `wide` or `full-width`, shown on the wrapper. */
const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' })

/** A cell's value for sorting: numbers (also `1,250.50`, `12 %`, `-3`) as numbers, else text. */
function sortKey(cell: Element | undefined): number | string {
  const text = (cell?.textContent ?? '').trim()
  const num = text.replace(/[\s,'’]/g, '').replace(/%$/, '')
  return num !== '' && /^[-+]?\d*\.?\d+$/.test(num) ? Number(num) : text
}

function compare(a: number | string, b: number | string): number {
  if (typeof a === 'number' && typeof b === 'number') return a - b
  if (a === '' || b === '') return a === '' ? (b === '' ? 0 : 1) : -1 // empty cells last
  return collator.compare(String(a), String(b))
}

/**
 * Table width (Confluence's `data-layout`) and row numbers on the wrapper; for readers, sorting by
 * a column (a sorted copy is shown, the page stays as it is); a header row that stays in view
 * while scrolling through a long table, when the table fits the page.
 */
class LayoutTableView extends TableView {
  private view: EditorView
  private sorted: { col: number; dir: 1 | -1 } | null = null
  private copy: HTMLTableElement | null = null
  private fits: ResizeObserver | null = null

  constructor(...args: ConstructorParameters<typeof TableView>) {
    super(...args)
    this.view = args[2] as EditorView
    this.sync(args[0])
    // sticky headers need the wrapper not to scroll sideways: only when the table fits
    if (typeof ResizeObserver !== 'undefined') {
      this.fits = new ResizeObserver(() => {
        const fit = this.table.scrollWidth <= this.dom.clientWidth + 1
        if (fit) this.dom.dataset.fits = 'true'
        else delete this.dom.dataset.fits
      })
      this.fits.observe(this.dom)
      this.fits.observe(this.table)
    }
    this.table.addEventListener('click', (e) => this.onHeaderClick(e))
    // the rows are rendered after the view is made
    requestAnimationFrame(() => this.markHeader())
  }

  /** width and row numbers live on the wrapper (the table element is the view's own) */
  private sync(node: PMNode) {
    const layout = node.attrs.layout as string | null
    if (layout && layout !== 'default') this.dom.dataset.layout = layout
    else delete this.dom.dataset.layout
    if (node.attrs.numbered) this.dom.dataset.numbered = 'true'
    else delete this.dom.dataset.numbered
  }

  /** Readers sort by a column of the header row: ascending, descending, then as written. */
  private sortable(): boolean {
    return !this.view.editable && this.hasSortableShape()
  }

  /** A header row on top, two or more rows below it, no merged cells. */
  private hasSortableShape(): boolean {
    const rows = Array.from(this.contentDOM.children) as HTMLTableRowElement[]
    const head = rows[0]
    if (!head || !Array.from(head.cells).every((c) => c.tagName === 'TH')) return false
    const body = rows.slice(1)
    // merged cells would fall apart when rows move
    return body.length >= 2 && body.every((r) => Array.from(r.cells).every((c) => c.colSpan === 1 && c.rowSpan === 1))
  }

  private onHeaderClick(e: MouseEvent) {
    const th = (e.target as HTMLElement).closest('th')
    if (!th || (e.target as HTMLElement).closest('a, button, input') || !this.sortable()) return
    const row = th.parentElement as HTMLTableRowElement
    const head = this.copy ? this.copy.tBodies[0]?.rows[0] : this.contentDOM.children[0]
    if (row !== head) return
    const col = th.cellIndex
    this.sorted = this.sorted?.col !== col ? { col, dir: 1 } : this.sorted.dir === 1 ? { col, dir: -1 } : null
    this.render()
  }

  private render() {
    this.copy?.remove()
    this.copy = null
    if (!this.sorted || !this.sortable()) {
      this.sorted = null
      delete this.dom.dataset.sorted
      this.markHeader()
      return
    }
    const { col, dir } = this.sorted
    const copy = this.table.cloneNode(true) as HTMLTableElement
    copy.classList.add('table-sorted-copy')
    copy.removeAttribute('contenteditable')
    const tbody = copy.tBodies[0]
    const rows = Array.from(tbody.rows)
    const body = rows.slice(1)
    body.sort((a, b) => dir * compare(sortKey(a.cells[col]), sortKey(b.cells[col])))
    for (const r of body) tbody.appendChild(r)
    copy.addEventListener('click', (e) => this.onHeaderClick(e))
    this.markHeader(copy)
    this.copy = copy
    this.dom.appendChild(copy)
    this.dom.dataset.sorted = 'true'
  }

  /**
   * The sort cue on the wrapper (shown in read-only editors only, see styles.css) and
   * `aria-sort` on the sorted copy's column. The table's own cells are the editor's: changing
   * them would count as an edit.
   */
  private markHeader(copy?: HTMLTableElement) {
    if (this.hasSortableShape()) this.dom.dataset.sortable = 'true'
    else delete this.dom.dataset.sortable
    const head = copy?.tBodies[0]?.rows[0]
    if (head && this.sorted) head.cells[this.sorted.col]?.setAttribute('aria-sort', this.sorted.dir === 1 ? 'ascending' : 'descending')
  }

  update(node: PMNode) {
    if (!super.update(node)) return false
    this.sync(node)
    // the rows change after this returns: sort the new ones then (only while sorted: editors
    // never are, and their tables shouldn't do work on every keystroke)
    if (this.sorted) requestAnimationFrame(() => this.render())
    else requestAnimationFrame(() => this.markHeader())
    return true
  }

  destroy() {
    this.fits?.disconnect()
  }
}

const LayoutTable = Table.extend({
  addAttributes() {
    return {
      ...this.parent?.(),
      layout: {
        default: null,
        parseHTML: (el: HTMLElement) => el.getAttribute('data-layout'),
        renderHTML: (attrs: Record<string, unknown>) => (attrs.layout ? { 'data-layout': attrs.layout } : {}),
      },
      /** a number in front of every row with data cells */
      numbered: {
        default: null,
        parseHTML: (el: HTMLElement) => (el.getAttribute('data-numbered') === 'true' || el.getAttribute('data-number-column') === 'true' ? true : null),
        renderHTML: (attrs: Record<string, unknown>) => (attrs.numbered ? { 'data-numbered': 'true' } : {}),
      },
    }
  },
}).configure({ resizable: true, View: LayoutTableView })

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

const ImageExt = Image.extend<ImageOptions & { pageId: string }>({
  addOptions() {
    // the page, for the annotation tool's uploads
    return { ...this.parent!(), pageId: '' }
  },
  addAttributes() {
    return {
      ...this.parent?.(),
      width: { default: null, renderHTML: (a: Record<string, unknown>) => (a.width ? { width: a.width } : {}) },
      align: { default: null },
      caption: { default: null },
      // a thin frame around screenshots on white backgrounds
      border: { default: null, renderHTML: (a: Record<string, unknown>) => (a.border ? { 'data-border': 'true' } : {}) },
    }
  },
  addNodeView() {
    return ReactNodeViewRenderer(ImageView)
  },
})

/**
 * Everything page content may contain. `pageId`: the page it belongs to (child pages, report
 * macros, drawings and images refer to it). Options: `mention`, the @-suggestion while editing;
 * `chain`, the pages shown around this content, outermost first (so includes stop before a
 * loop); `readOnly`, a copy nobody types into (no empty paragraph kept at the end).
 */
export function contentExtensions(pageId: string, opts: { mention?: Partial<MentionOptions>; chain?: string[]; readOnly?: boolean } = {}): Extensions {
  const { mention, chain = [pageId], readOnly = false } = opts
  return [
    StarterKit.configure({ undoRedo: false, codeBlock: false, trailingNode: readOnly ? false : undefined, link: { openOnClick: false, autolink: true, HTMLAttributes: { rel: 'noopener noreferrer' } } }),
    CodeBlock.configure({ lowlight }),
    TableKit.configure({ table: false, tableCell: false, tableHeader: false }),
    LayoutTable,
    withBackground(TableCell),
    withBackground(TableHeader as unknown as typeof TableCell),
    TaskList,
    TaskItem.configure({ nested: true }),
    ImageExt.configure({ inline: true, allowBase64: false, pageId }),
    TextStyle,
    Color,
    Highlight.configure({ multicolor: true }),
    TextAlign.configure({ types: ['heading', 'paragraph'] }),
    Indent,
    Subscript,
    Superscript,
    Details.configure({ persist: true, HTMLAttributes: { class: 'details' } }),
    DetailsSummary,
    DetailsContent,
    Mathematics,
    Mention.configure({ HTMLAttributes: { class: 'mention' }, renderText: ({ node }) => `@${node.attrs.label ?? ''}`, ...mention }),
    Callout,
    Status,
    InstructionalText,
    Emoticon,
    DateNode,
    AttachmentNode,
    Embed,
    Toc,
    ChildPages.configure({ pageId }),
    Include.configure({ chain }),
    Excerpt,
    Anchor,
    Issue,
    Columns,
    Column,
    DecisionList,
    DecisionItem,
    Macro.configure({ pageId }),
    Drawing.configure({ pageId }),
    CommentMark,
  ]
}
