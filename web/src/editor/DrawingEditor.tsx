// The drawing editor (Excalidraw), loaded only when someone opens a drawing.

import { useState } from 'react'
import { createPortal } from 'react-dom'
import { Excalidraw, exportToBlob, serializeAsJSON } from '@excalidraw/excalidraw'
import type { ExcalidrawImperativeAPI } from '@excalidraw/excalidraw/types'
import '@excalidraw/excalidraw/index.css'


export default function DrawingEditor({ initial, dark, onSave, onClose }: { initial: string | null; dark: boolean; onSave: (scene: string, png: Blob | null) => Promise<void>; onClose: () => void }) {
  const [api, setApi] = useState<ExcalidrawImperativeAPI | null>(null)
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
      setError(e instanceof Error ? e.message : 'Could not save the drawing')
      setBusy(false)
    }
  }
  // rendered outside the page editor, so its keys and focus don't reach the document
  return createPortal(
    <div className="drawing-modal" role="dialog" aria-modal aria-label="Drawing" onKeyDown={(e) => e.stopPropagation()}>
      <div className="drawing-bar">
        <b>Drawing</b>
        {error && <span className="error-text small">{error}</span>}
        <span className="spacer" />
        <button className="btn" onClick={onClose} disabled={busy}>Cancel</button>
        <button className="btn btn-primary" onClick={save} disabled={busy || !api}>{busy ? 'Saving…' : 'Save'}</button>
      </div>
      <div className="drawing-canvas">
        <Excalidraw excalidrawAPI={setApi} initialData={initialData} theme={dark ? 'dark' : 'light'} UIOptions={{ canvasActions: { loadScene: false, saveToActiveFile: false, export: false } }} />
      </div>
    </div>,
    document.body,
  )
}
