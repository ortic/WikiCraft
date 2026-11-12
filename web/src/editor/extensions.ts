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
import { Indent } from './Indent'
import { AttachmentNode, Anchor, Callout, ChildPages, Column, Columns, CommentMark, DateNode, DecisionItem, DecisionList, Embed, Emoticon, Excerpt, Include, InstructionalText, Macro, Status, Toc } from './nodes'
import { CodeBlock } from './CodeBlock'
import { ImageView } from './ImageView'
import { Drawing } from './Drawing'

const lowlight = createLowlight(common)

/** Table width (Confluence's `data-layout`): normal, `wide` or `full-width`, shown on the wrapper. */
class LayoutTableView extends TableView {
  constructor(...args: ConstructorParameters<typeof TableView>) {
    super(...args)
    this.sync(args[0])
  }
  /** width and row numbers live on the wrapper (the table element is the view's own) */
  private sync(node: PMNode) {
    const layout = node.attrs.layout as string | null
    if (layout && layout !== 'default') this.dom.dataset.layout = layout
    else delete this.dom.dataset.layout
    if (node.attrs.numbered) this.dom.dataset.numbered = 'true'
    else delete this.dom.dataset.numbered
  }
  update(node: PMNode) {
    if (!super.update(node)) return false
    this.sync(node)
    return true
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
    Columns,
    Column,
    DecisionList,
    DecisionItem,
    Macro.configure({ pageId }),
    Drawing.configure({ pageId }),
    CommentMark,
  ]
}
