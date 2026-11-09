import { useRef, useState } from 'react'
import { NodeViewWrapper, type ReactNodeViewProps } from '@tiptap/react'
import { AlignCenter, AlignLeft, AlignRight, Captions, ExternalLink, Maximize2, Trash2, Type } from 'lucide-react'

/** Image with drag-to-resize, alignment, caption and alt text. */
export function ImageView({ node, updateAttributes, deleteNode, selected, editor, getPos }: ReactNodeViewProps) {
  const { src, alt, width, align, caption } = node.attrs as { src: string; alt: string | null; width: number | null; align: string | null; caption: string | null }
  const img = useRef<HTMLImageElement>(null)
  const [dragWidth, setDragWidth] = useState<number | null>(null)
  const [captionOpen, setCaptionOpen] = useState(false)
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
        <img ref={img} src={src} alt={alt ?? ''} draggable={false} style={shown ? { width: '100%' } : undefined} />
        {editable && selected && (
          <>
            <span className="image-toolbar" contentEditable={false}>
              {btn('Align left', align === 'left', () => update({ align: 'left' }), <AlignLeft size={15} />)}
              {btn('Center', align === 'center', () => update({ align: 'center' }), <AlignCenter size={15} />)}
              {btn('Align right', align === 'right', () => update({ align: 'right' }), <AlignRight size={15} />)}
              <span className="sep" />
              {btn('Caption', !!caption || captionOpen, () => setCaptionOpen(true), <Captions size={15} />)}
              {btn(alt ? `Alt text: ${alt}` : 'Add alt text', !!alt, editAlt, <Type size={15} />)}
              {btn('Original size', false, () => update({ width: null }), <Maximize2 size={15} />)}
              {btn('Open image', false, () => window.open(src, '_blank', 'noopener'), <ExternalLink size={15} />)}
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
