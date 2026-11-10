import { useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { NodeViewWrapper, type ReactNodeViewProps } from '@tiptap/react'
import { AlignCenter, AlignLeft, AlignRight, Captions, Download, ExternalLink, Maximize2, Trash2, Type, WrapText, X, ZoomIn } from 'lucide-react'

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
    <div className="lightbox" role="dialog" aria-modal aria-label={alt || caption || 'Image'} onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="lightbox-bar">
        <a className="icon-btn" href={src} target="_blank" rel="noopener noreferrer" title="Open original"><ExternalLink size={17} /></a>
        <a className="icon-btn" href={src} download title="Download"><Download size={17} /></a>
        <button className="icon-btn" title="Close (Esc)" onClick={onClose}><X size={18} /></button>
      </div>
      <figure className="lightbox-figure" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
        <img src={src} alt={alt ?? ''} />
        {(caption || alt) && <figcaption>{caption || alt}</figcaption>}
      </figure>
    </div>,
    document.body,
  )
}

/** Image with drag-to-resize, alignment, caption and alt text. */
export function ImageView({ node, updateAttributes, deleteNode, selected, editor, getPos }: ReactNodeViewProps) {
  const { src, alt, width, align, caption } = node.attrs as { src: string; alt: string | null; width: number | null; align: string | null; caption: string | null }
  const img = useRef<HTMLImageElement>(null)
  const [dragWidth, setDragWidth] = useState<number | null>(null)
  const [captionOpen, setCaptionOpen] = useState(false)
  const [zoom, setZoom] = useState(false)
  const editable = editor.isEditable
  // attribute changes go through Yjs and drop the node selection: keep the image selected
  const update = (attrs: Record<string, unknown>) => {
    updateAttributes(attrs)
    const pos = typeof getPos === 'function' ? getPos() : undefined
    if (typeof pos === 'number') requestAnimationFrame(() => editor.commands.setNodeSelection(pos))
  }
  const shown = dragWidth ?? width

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
    const v = window.prompt('Alternative text (describes the image for screen readers and search)', alt ?? '')
    if (v !== null) update({ alt: v.trim() || null })
  }

  const btn = (title: string, active: boolean, onClick: () => void, icon: React.ReactNode) => (
    <button type="button" className={`icon-btn${active ? ' active' : ''}`} title={title} aria-label={title} onMouseDown={(e) => e.preventDefault()} onClick={onClick}>
      {icon}
    </button>
  )

  return (
    <NodeViewWrapper as="span" className={`image-block align-${align || 'none'}${selected ? ' is-selected' : ''}`}>
      <span className="image-frame" style={{ width: shown ? `${shown}px` : undefined }}>
        <img
          ref={img}
          src={src}
          alt={alt ?? ''}
          draggable={false}
          className={editable ? undefined : 'zoomable'}
          style={shown ? { width: '100%' } : undefined}
          onClick={editable ? undefined : () => setZoom(true)}
          onDoubleClick={editable ? () => setZoom(true) : undefined}
        />
        {zoom && <Lightbox src={src} alt={alt} caption={caption} onClose={() => setZoom(false)} />}
        {editable && selected && (
          <>
            <span className="image-toolbar" contentEditable={false}>
              {btn('Align left', align === 'left', () => update({ align: 'left' }), <AlignLeft size={15} />)}
              {btn('Center', align === 'center', () => update({ align: 'center' }), <AlignCenter size={15} />)}
              {btn('Align right', align === 'right', () => update({ align: 'right' }), <AlignRight size={15} />)}
              {btn('Float left, text wraps around', align === 'wrap-left', () => update({ align: 'wrap-left' }), <WrapText size={15} />)}
              {btn('Float right, text wraps around', align === 'wrap-right', () => update({ align: 'wrap-right' }), <WrapText size={15} style={{ transform: 'scaleX(-1)' }} />)}
              <span className="sep" />
              {btn('Caption', !!caption || captionOpen, () => setCaptionOpen(true), <Captions size={15} />)}
              {btn(alt ? `Alt text: ${alt}` : 'Add alt text', !!alt, editAlt, <Type size={15} />)}
              {btn('Original size', false, () => update({ width: null }), <Maximize2 size={15} />)}
              {btn('View larger (double-click)', false, () => setZoom(true), <ZoomIn size={15} />)}
              <span className="sep" />
              {btn('Delete image', false, () => deleteNode(), <Trash2 size={15} />)}
            </span>
            <span className="image-resize" contentEditable={false} onPointerDown={startResize} title="Drag to resize" />
            {shown ? <span className="image-size" contentEditable={false}>{shown}px</span> : null}
          </>
        )}
      </span>
      {editable && (captionOpen || caption) ? (
        <input
          className="image-caption-input"
          value={caption ?? ''}
          placeholder="Write a caption…"
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
