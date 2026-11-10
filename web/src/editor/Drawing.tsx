// Drawing block: an Excalidraw sketch stored in the page (its scene as JSON) with a PNG preview
// attachment that readers, shared pages and exports show.

import { Suspense, lazy, useState } from 'react'
import { createPortal } from 'react-dom'
import { Node, mergeAttributes } from '@tiptap/core'
import { NodeViewWrapper, ReactNodeViewRenderer, type ReactNodeViewProps } from '@tiptap/react'
import { PenTool } from 'lucide-react'
import { api, type Attachment } from '../api'
import { Lightbox } from './ImageView'
import { useEditable } from './useEditable'

// Excalidraw reads where its fonts live when its module loads: set it before the lazy import
// (they're self-hosted, see scripts/copy-excalidraw-assets.mjs; a CDN would be blocked anyway)
;(window as unknown as { EXCALIDRAW_ASSET_PATH: string }).EXCALIDRAW_ASSET_PATH = '/excalidraw-assets/'
const DrawingEditor = lazy(() => import('./DrawingEditor'))

/** Scenes larger than this are refused (usually big pasted images). */
const MAX_SCENE = 3_000_000

function isDark() {
  const t = document.documentElement.dataset.theme
  return t === 'dark' || (!t && matchMedia('(prefers-color-scheme: dark)').matches)
}

function DrawingView({ node, updateAttributes, editor, extension, selected }: ReactNodeViewProps) {
  const editable = useEditable(editor)
  const [open, setOpen] = useState(false)
  const [zoom, setZoom] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const preview = node.attrs.preview as string | null
  const pageId = (extension.options as { pageId: string }).pageId
  const save = async (scene: string, png: Blob | null) => {
    if (scene.length > MAX_SCENE) throw new Error('This drawing is too large to save (pasted images take a lot of space).')
    let src: string | null = null
    if (png) {
      // one attachment per drawing; saving again adds a version of it
      const key = (node.attrs.key as string | null) ?? crypto.randomUUID().slice(0, 8)
      const form = new FormData()
      form.append('file', new File([png], `drawing-${key}.png`, { type: 'image/png' }))
      const rows = await api.upload<Attachment[]>(`/pages/${pageId}/attachments`, form)
      src = `/api/attachments/${rows[0].id}/content?v=${Date.now()}`
      updateAttributes({ key })
    }
    updateAttributes({ scene, preview: src })
    setOpen(false)
    setError(null)
  }
  return (
    <NodeViewWrapper className={`drawing-block${selected ? ' is-selected' : ''}`} contentEditable={false}>
      {preview ? (
        <img src={preview} alt="Drawing" className="drawing-preview" draggable={false} onClick={() => (editable ? setOpen(true) : setZoom(true))} />
      ) : (
        <button type="button" className="drawing-empty" onClick={() => editable && setOpen(true)} disabled={!editable}>
          <PenTool size={20} /> {editable ? 'Empty drawing: click to draw' : 'Empty drawing'}
        </button>
      )}
      {editable && preview && (
        <button type="button" className="btn btn-sm drawing-edit" onClick={() => setOpen(true)}>
          <PenTool size={13} /> Edit drawing
        </button>
      )}
      {error && <div className="error-text small">{error}</div>}
      {zoom && preview && <Lightbox src={preview} alt="Drawing" caption={null} onClose={() => setZoom(false)} />}
      {open && (
        <Suspense fallback={createPortal(<div className="drawing-modal drawing-loading"><div className="spinner" /></div>, document.body)}>
          <DrawingEditor initial={(node.attrs.scene as string | null) ?? null} dark={isDark()} onSave={save} onClose={() => setOpen(false)} />
        </Suspense>
      )}
    </NodeViewWrapper>
  )
}

export const Drawing = Node.create<{ pageId: string }>({
  name: 'drawing',
  group: 'block',
  atom: true,
  draggable: true,
  addOptions() {
    return { pageId: '' }
  },
  addAttributes() {
    return {
      scene: { default: null, rendered: false },
      preview: { default: null },
      key: { default: null, rendered: false },
    }
  },
  parseHTML() {
    return [{ tag: 'div[data-type="drawing"]' }]
  },
  renderHTML({ HTMLAttributes }) {
    return ['div', mergeAttributes(HTMLAttributes, { 'data-type': 'drawing' })]
  },
  addNodeView() {
    return ReactNodeViewRenderer(DrawingView)
  },
})
