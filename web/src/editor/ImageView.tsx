import { t } from '../i18n'
import { Suspense, lazy, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { NodeViewWrapper, type ReactNodeViewProps } from '@tiptap/react'
import { AlignCenter, AlignLeft, AlignRight, Captions, Download, ExternalLink, Maximize2, PenTool, Trash2, Type, WrapText, X, ZoomIn, Square } from 'lucide-react'
import { api, type Attachment } from '../api'

const DrawingEditor = lazy(() => import('./DrawingEditor'))

/** Uploaded images (relative or absolute URL on this server) can be annotated. */
function sameOriginAttachment(src: string): boolean {
  try {
    const u = new URL(src, location.href)
    return u.origin === location.origin && u.pathname.startsWith('/api/attachments/')
  } catch {
    return false
  }
}

/** Full-screen view of an image (click in read mode, double-click while editing). */
export function Lightbox({ src, alt, caption, onClose }: { src: string; alt: string | null; caption: string | null; onClose: () => void }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation()
        onClose()
      }
    }
    window.addEventListener('keydown', onKey, true)
    const overflow = document.body.style.overflow
    document.body.style.overflow = 'hidden'
    return () => {
      window.removeEventListener('keydown', onKey, true)
      document.body.style.overflow = overflow
    }
  }, [onClose])
  return createPortal(
    <div className="lightbox" role="dialog" aria-modal aria-label={alt || caption || t('Image')} onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="lightbox-bar">
        <a className="icon-btn" href={src} target="_blank" rel="noopener noreferrer" title={t('Open original')}><ExternalLink size={17} /></a>
        <a className="icon-btn" href={src} download title={t('Download')}><Download size={17} /></a>
        <button className="icon-btn" title={t('Close (Esc)')} onClick={onClose}><X size={18} /></button>
      </div>
      <figure className="lightbox-figure" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
        <img src={src} alt={alt ?? ''} />
        {(caption || alt) && <figcaption>{caption || alt}</figcaption>}
      </figure>
    </div>,
    document.body,
  )
}

/** Widths the server makes smaller copies of images in (`?w=`, see the server's renditions.rs). */
const RENDITION_WIDTHS = [640, 960, 1280, 1920]

/**
 * The address of the smallest copy of an attached image that still fills `space` CSS pixels on
 * this screen (the server sends the original when it isn't wider), or the original. A plain
 * `src` rather than `srcset`: with width descriptors, browsers would blow small images up to the
 * descriptor's width.
 */
export function sizedSrc(src: string, space: number): string {
  if (!/^\/api\/attachments\/[0-9a-f-]{36}\/content$/.test(src)) return src
  const need = Math.ceil(space * (window.devicePixelRatio || 1))
  const w = RENDITION_WIDTHS.find((x) => x >= need)
  return w ? `${src}?w=${w}` : src
}

/** Image with drag-to-resize, alignment, caption and alt text. */
export function ImageView({ node, updateAttributes, deleteNode, selected, editor, getPos, extension }: ReactNodeViewProps) {
  const { src, alt, width, align, caption, border } = node.attrs as { src: string; alt: string | null; width: number | null; align: string | null; caption: string | null; border: boolean | null }
  const img = useRef<HTMLImageElement>(null)
  const [dragWidth, setDragWidth] = useState<number | null>(null)
  const [captionOpen, setCaptionOpen] = useState(false)
  const [zoom, setZoom] = useState(false)
  const [annotate, setAnnotate] = useState(false)
  const pageId = (extension.options as { pageId?: string }).pageId || undefined
  // annotations are drawn over the image and saved as a new image (the original stays attached)
  const saveAnnotated = async (_scene: string, png: Blob | null) => {
    if (!png || !pageId) return setAnnotate(false)
    const base = (alt || 'image').replace(/[^\w.-]+/g, '-').slice(0, 40)
    const form = new FormData()
    form.append('file', new File([png], `annotated-${base}-${Date.now()}.png`, { type: 'image/png' }))
    const rows = await api.upload<Attachment[]>(`/pages/${pageId}/attachments`, form)
    update({ src: `/api/attachments/${rows[0].id}/content` })
    setAnnotate(false)
  }
  const editable = editor.isEditable
  // attribute changes go through Yjs and drop the node selection: keep the image selected
  const update = (attrs: Record<string, unknown>) => {
    updateAttributes(attrs)
    const pos = typeof getPos === 'function' ? getPos() : undefined
    if (typeof pos === 'number') requestAnimationFrame(() => editor.commands.setNodeSelection(pos))
  }
  const shown = dragWidth ?? width
  // the room the image gets (its set width, else the column it's in), measured before the first
  // paint so the right copy loads; it only grows, so a narrower moment doesn't load another one
  const frame = useRef<HTMLSpanElement>(null)
  const [space, setSpace] = useState<number | null>(null)
  useLayoutEffect(() => {
    const column = (frame.current?.closest('td, th, .ProseMirror') as HTMLElement | null)?.clientWidth || 1200
    const room = width ? Math.min(width, column) : column
    setSpace((prev) => (prev !== null && room <= prev ? prev : room))
  }, [width])

  const startResize = (e: React.PointerEvent) => {
    e.preventDefault()
    e.stopPropagation()
    const startX = e.clientX
    const startW = img.current?.getBoundingClientRect().width ?? 320
    const max = (img.current?.closest('.ProseMirror') as HTMLElement | null)?.clientWidth ?? 1200
    // centred images grow on both sides
    const factor = align === 'center' ? 2 : align === 'right' ? -1 : 1
    let latest = startW
    const move = (ev: PointerEvent) => {
      latest = Math.round(Math.max(48, Math.min(max, startW + (ev.clientX - startX) * factor)))
      setDragWidth(latest)
    }
    const up = () => {
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', up)
      update({ width: latest })
      setDragWidth(null)
    }
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', up)
  }

  const editAlt = () => {
    const v = window.prompt(t('Alternative text (describes the image for screen readers and search)'), alt ?? '')
    if (v !== null) update({ alt: v.trim() || null })
  }

  const btn = (title: string, active: boolean, onClick: () => void, icon: React.ReactNode) => (
    <button type="button" className={`icon-btn${active ? ' active' : ''}`} title={title} aria-label={title} onMouseDown={(e) => e.preventDefault()} onClick={onClick}>
      {icon}
    </button>
  )

  return (
    <NodeViewWrapper as="span" className={`image-block align-${align || 'none'}${border ? ' bordered' : ''}${selected ? ' is-selected' : ''}`}>
      <span ref={frame} className="image-frame" style={{ width: shown ? `${shown}px` : undefined }}>
        <img
          ref={img}
          src={space === null ? undefined : sizedSrc(src, space)}
          alt={alt ?? ''}
          draggable={false}
          className={editable ? undefined : 'zoomable'}
          style={shown ? { width: '100%' } : undefined}
          onClick={editable ? undefined : () => setZoom(true)}
          onDoubleClick={editable ? () => setZoom(true) : undefined}
        />
        {zoom && <Lightbox src={src} alt={alt} caption={caption} onClose={() => setZoom(false)} />}
        {annotate && (
          <Suspense fallback={null}>
            <DrawingEditor
              initial={null}
              image={src}
              title={t('Annotate image')}
              dark={document.documentElement.dataset.theme === 'dark'}
              onSave={saveAnnotated}
              onClose={() => setAnnotate(false)}
            />
          </Suspense>
        )}
        {editable && selected && (
          <>
            <span className="image-toolbar" contentEditable={false}>
              {btn(t('Align left'), align === 'left', () => update({ align: 'left' }), <AlignLeft size={15} />)}
              {btn(t('Center'), align === 'center', () => update({ align: 'center' }), <AlignCenter size={15} />)}
              {btn(t('Align right'), align === 'right', () => update({ align: 'right' }), <AlignRight size={15} />)}
              {btn(t('Float left, text wraps around'), align === 'wrap-left', () => update({ align: 'wrap-left' }), <WrapText size={15} />)}
              {btn(t('Float right, text wraps around'), align === 'wrap-right', () => update({ align: 'wrap-right' }), <WrapText size={15} style={{ transform: 'scaleX(-1)' }} />)}
              <span className="sep" />
              {btn(t('Caption'), !!caption || captionOpen, () => setCaptionOpen(true), <Captions size={15} />)}
              {btn(border ? t('Remove border') : t('Border'), !!border, () => update({ border: border ? null : true }), <Square size={15} />)}
              {btn(alt ? t('Alt text: {alt}', { alt }) : t('Add alt text'), !!alt, editAlt, <Type size={15} />)}
              {btn(t('Original size'), false, () => update({ width: null }), <Maximize2 size={15} />)}
              {btn(t('View larger (double-click)'), false, () => setZoom(true), <ZoomIn size={15} />)}
              {pageId && sameOriginAttachment(src) && btn(t('Annotate: draw arrows, boxes and text on the image'), false, () => {
                ;(document.activeElement as HTMLElement | null)?.blur?.()
                setAnnotate(true)
              }, <PenTool size={15} />)}
              <span className="sep" />
              {btn(t('Delete image'), false, () => deleteNode(), <Trash2 size={15} />)}
            </span>
            <span className="image-resize" contentEditable={false} onPointerDown={startResize} title={t('Drag to resize')} />
            {shown ? <span className="image-size" contentEditable={false}>{t('{shown}px', { shown })}</span> : null}
          </>
        )}
      </span>
      {editable && (captionOpen || caption) ? (
        <input
          className="image-caption-input"
          value={caption ?? ''}
          placeholder={t('Write a caption…')}
          autoFocus={captionOpen && !caption}
          onChange={(e) => updateAttributes({ caption: e.target.value || null })}
          onBlur={() => setCaptionOpen(false)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' || e.key === 'Escape') (e.target as HTMLInputElement).blur()
          }}
        />
      ) : caption ? (
        <span className="image-caption">{caption}</span>
      ) : null}
    </NodeViewWrapper>
  )
}
