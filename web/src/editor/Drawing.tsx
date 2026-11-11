// Drawing block: an Excalidraw sketch (its scene as JSON) or a draw.io diagram (kind "drawio",
// its XML) stored in the page, with a PNG preview attachment that readers, shared pages and
// exports show. Diagrams imported from Confluence point at their diagram file (`source`) until
// first saved here.

import { t as tr } from '../i18n'
import { Suspense, lazy, useState } from 'react'
import { createPortal } from 'react-dom'
import { Node, mergeAttributes } from '@tiptap/core'
import { NodeViewWrapper, ReactNodeViewRenderer, type ReactNodeViewProps } from '@tiptap/react'
import { PenTool, Workflow } from 'lucide-react'
import { api, type Attachment } from '../api'
import { useFeatures } from '../ui'
import { Lightbox } from './ImageView'
import { useEditable } from './useEditable'

// Excalidraw reads where its fonts live when its module loads: set it before the lazy import
// (they're self-hosted, see scripts/copy-excalidraw-assets.mjs; a CDN would be blocked anyway)
;(window as unknown as { EXCALIDRAW_ASSET_PATH: string }).EXCALIDRAW_ASSET_PATH = '/excalidraw-assets/'
const DrawingEditor = lazy(() => import('./DrawingEditor'))
const DrawioEditor = lazy(() => import('./DrawioEditor'))

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
  const drawio = node.attrs.kind === 'drawio'
  const drawioUrl = useFeatures().drawioUrl ?? null
  // draw.io turned off by an admin: diagrams stay readable, not editable
  const canEdit = editable && (!drawio || !!drawioUrl)
  const title = (node.attrs.title as string | null) ?? null
  const save = async (scene: string, png: Blob | null) => {
    if (scene.length > MAX_SCENE) throw new Error(tr('This drawing is too large to save (pasted images take a lot of space).'))
    let src: string | null = null
    if (png) {
      // one attachment per drawing; saving again adds a version of it
      const key = (node.attrs.key as string | null) ?? crypto.randomUUID().slice(0, 8)
      const form = new FormData()
      form.append('file', new File([png], `${drawio ? 'diagram' : 'drawing'}-${key}.png`, { type: 'image/png' }))
      const rows = await api.upload<Attachment[]>(`/pages/${pageId}/attachments`, form)
      src = `/api/attachments/${rows[0].id}/content?v=${Date.now()}`
      updateAttributes({ key })
    }
    // saved here: the diagram lives in the page from now on
    updateAttributes({ scene, preview: src, source: null })
    if (!drawio) setOpen(false)
    setError(null)
  }
  const label = drawio ? 'diagram' : 'drawing'
  const Icon = drawio ? Workflow : PenTool
  return (
    <NodeViewWrapper className={`drawing-block${selected ? ' is-selected' : ''}`} contentEditable={false}>
      {preview ? (
        <img src={preview} alt={title ?? (drawio ? tr('Diagram') : tr('Drawing'))} className="drawing-preview" draggable={false} onClick={() => (canEdit ? setOpen(true) : setZoom(true))} />
      ) : (
        <button type="button" className="drawing-empty" onClick={() => canEdit && setOpen(true)} disabled={!canEdit}>
          <Icon size={20} /> {canEdit ? (drawio ? tr('Empty {label}: click to edit in draw.io', { label }) : tr('Empty {label}: click to draw', { label })) : drawio && editable ? tr('draw.io diagrams are turned off on this wiki') : tr('Empty {label}', { label })}
        </button>
      )}
      {canEdit && preview && (
        <button type="button" className="btn btn-sm drawing-edit" onClick={() => setOpen(true)}>
          <Icon size={13} />{' '}{tr('Edit {label}', { label })}
        </button>
      )}
      {error && <div className="error-text small">{error}</div>}
      {zoom && preview && <Lightbox src={preview} alt={title ?? tr('Drawing')} caption={null} onClose={() => setZoom(false)} />}
      {open && (
        <Suspense fallback={createPortal(<div className="drawing-modal drawing-loading"><div className="spinner" /></div>, document.body)}>
          {drawio && drawioUrl ? (
            <DrawioEditor
              url={drawioUrl}
              initial={(node.attrs.scene as string | null) ?? null}
              source={(node.attrs.source as string | null) ?? null}
              title={title}
              dark={isDark()}
              onSave={save}
              onClose={() => setOpen(false)}
            />
          ) : (
            <DrawingEditor initial={(node.attrs.scene as string | null) ?? null} dark={isDark()} onSave={save} onClose={() => setOpen(false)} />
          )}
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
      // "drawio" for draw.io diagrams (default: Excalidraw)
      kind: { default: null, rendered: false },
      // an imported diagram's file, until it's first saved here
      source: { default: null, rendered: false },
      title: { default: null, rendered: false },
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
