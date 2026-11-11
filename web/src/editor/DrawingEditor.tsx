// The drawing editor (Excalidraw), loaded only when someone opens a drawing.

import { t } from '../i18n'
import { useEffect, useState } from 'react'
import { createPortal } from 'react-dom'
import { Excalidraw, convertToExcalidrawElements, exportToBlob, serializeAsJSON } from '@excalidraw/excalidraw'
import type { BinaryFileData, DataURL, ExcalidrawImperativeAPI } from '@excalidraw/excalidraw/types'
import type { FileId } from '@excalidraw/excalidraw/element/types'
import '@excalidraw/excalidraw/index.css'


export default function DrawingEditor({
  initial,
  image,
  title = t('Drawing'),
  dark,
  onSave,
  onClose,
}: {
  initial: string | null
  /** annotate this image: it becomes the (locked) background of the drawing */
  image?: string
  title?: string
  dark: boolean
  onSave: (scene: string, png: Blob | null) => Promise<void>
  onClose: () => void
}) {
  const [api, setApi] = useState<ExcalidrawImperativeAPI | null>(null)
  // keys must not reach the page editor underneath (it may have a block selected)
  useEffect(() => {
    ;(document.activeElement as HTMLElement | null)?.blur?.()
  }, [])
  useEffect(() => {
    if (!api || !image) return
    let cancelled = false
    ;(async () => {
      const blob = await (await fetch(image)).blob()
      const dataURL = await new Promise<string>((resolve) => {
        const r = new FileReader()
        r.onload = () => resolve(r.result as string)
        r.readAsDataURL(blob)
      })
      const size = await new Promise<{ w: number; h: number }>((resolve) => {
        const img = new Image()
        img.onload = () => resolve({ w: img.naturalWidth, h: img.naturalHeight })
        img.src = dataURL
      })
      if (cancelled) return
      const fileId = 'background' as FileId
      const file: BinaryFileData = { id: fileId, dataURL: dataURL as DataURL, mimeType: (blob.type || 'image/png') as BinaryFileData['mimeType'], created: Date.now() }
      api.addFiles([file])
      const scale = Math.min(1, 1200 / size.w)
      const [el] = convertToExcalidrawElements([{ type: 'image', fileId, x: 0, y: 0, width: size.w * scale, height: size.h * scale }])
      api.updateScene({ elements: [{ ...el, locked: true }] })
      api.scrollToContent(undefined, { fitToContent: true })
    })()
    return () => {
      cancelled = true
    }
  }, [api, image])
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  let initialData = undefined
  try {
    initialData = initial ? JSON.parse(initial) : undefined
  } catch {
    initialData = undefined
  }
  const save = async () => {
    if (!api) return
    setBusy(true)
    setError(null)
    try {
      const elements = api.getSceneElements()
      const appState = api.getAppState()
      const files = api.getFiles()
      const scene = serializeAsJSON(elements, appState, files, 'local')
      const png = elements.length
        ? await exportToBlob({
            elements,
            appState: { ...appState, exportBackground: true, exportWithDarkMode: false, exportPadding: 16 },
            files,
            mimeType: 'image/png',
            getDimensions: (w: number, h: number) => ({ width: w * 2, height: h * 2, scale: 2 }),
          })
        : null
      await onSave(scene, png)
    } catch (e) {
      setError(e instanceof Error ? e.message : t('Could not save the drawing'))
      setBusy(false)
    }
  }
  // rendered outside the page editor, so its keys and focus don't reach the document
  return createPortal(
    <div className="drawing-modal" role="dialog" aria-modal aria-label={t('Drawing')} onKeyDown={(e) => e.stopPropagation()}>
      <div className="drawing-bar">
        <b>{title}</b>
        {error && <span className="error-text small">{error}</span>}
        <span className="spacer" />
        <button className="btn" onClick={onClose} disabled={busy}>{t('Cancel')}</button>
        <button className="btn btn-primary" onClick={save} disabled={busy || !api}>{busy ? t('Saving…') : t('Save')}</button>
      </div>
      <div className="drawing-canvas">
        <Excalidraw autoFocus excalidrawAPI={setApi} initialData={initialData} theme={dark ? 'dark' : 'light'} UIOptions={{ canvasActions: { loadScene: false, saveToActiveFile: false, export: false } }} />
      </div>
    </div>,
    document.body,
  )
}
