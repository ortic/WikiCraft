// The draw.io editor (diagrams.net in embed mode, or a self-hosted draw.io), in a full-screen
// frame. It talks to the page only through postMessage (draw.io's JSON embed protocol): the
// diagram is handed over and comes back as XML plus a PNG preview; nothing is uploaded by draw.io.

import { t } from '../i18n'
import { useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'

/** A `data:` URL as a Blob (fetching it would need a looser content security policy). */
function dataUrlToBlob(url: string): Blob {
  const [head, data] = url.split(',', 2)
  const type = /data:([^;]+)/.exec(head)?.[1] ?? 'image/png'
  const bin = atob(data ?? '')
  const bytes = new Uint8Array(bin.length)
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i)
  return new Blob([bytes], { type })
}

export default function DrawioEditor({
  url,
  initial,
  source,
  title,
  dark,
  onSave,
  onClose,
}: {
  /** the draw.io to use, e.g. https://embed.diagrams.net */
  url: string
  /** the diagram's XML (empty: a new diagram) */
  initial: string | null
  /** or where to read it from (an attachment of an imported diagram) */
  source?: string | null
  title?: string | null
  dark: boolean
  onSave: (xml: string, png: Blob) => Promise<void>
  onClose: () => void
}) {
  const frame = useRef<HTMLIFrameElement>(null)
  const [loaded, setLoaded] = useState(false)
  const pending = useRef<{ xml: string; exit: boolean } | null>(null)
  const origin = new URL(url).origin
  useEffect(() => {
    ;(document.activeElement as HTMLElement | null)?.blur?.()
    const post = (msg: object) => frame.current?.contentWindow?.postMessage(JSON.stringify(msg), origin)
    const onMessage = async (e: MessageEvent) => {
      // only the draw.io frame we opened
      if (e.origin !== origin || e.source !== frame.current?.contentWindow || typeof e.data !== 'string') return
      let msg: { event?: string; xml?: string; exit?: boolean; data?: string }
      try {
        msg = JSON.parse(e.data)
      } catch {
        return
      }
      if (msg.event === 'init') {
        let xml = initial ?? ''
        if (!xml && source) {
          try {
            xml = await (await fetch(source)).text()
          } catch {
            xml = ''
          }
        }
        post({ action: 'load', xml, autosave: 0, title: title ?? undefined })
        setLoaded(true)
      } else if (msg.event === 'save' && typeof msg.xml === 'string') {
        pending.current = { xml: msg.xml, exit: !!msg.exit }
        post({ action: 'export', format: 'png', xml: msg.xml, spinKey: 'saving', border: 8 })
      } else if (msg.event === 'export' && typeof msg.data === 'string') {
        const p = pending.current
        pending.current = null
        if (!p) return
        try {
          await onSave(p.xml, dataUrlToBlob(msg.data))
          post({ action: 'status', message: t('Saved'), modified: false })
          if (p.exit) onClose()
        } catch (err) {
          post({ action: 'dialog', title: t('Not saved'), message: err instanceof Error ? err.message : t('The diagram could not be saved.'), button: 'OK' })
        }
      } else if (msg.event === 'exit') {
        onClose()
      }
    }
    window.addEventListener('message', onMessage)
    return () => window.removeEventListener('message', onMessage)
  }, [origin, initial, source, title, onSave, onClose])
  const params = new URLSearchParams({ embed: '1', proto: 'json', spin: '1', saveAndExit: '1', noExitBtn: '0', libraries: '1', ui: dark ? 'dark' : 'kennedy', lang: (navigator.language || 'en').slice(0, 2) })
  return createPortal(
    <div className="drawing-modal drawio-modal" role="dialog" aria-label={title ? t('Diagram: {title}', { title }) : t('Diagram')}>
      {!loaded && <div className="drawio-loading"><div className="spinner" />{' '}{t('Loading draw.io…')}</div>}
      <iframe
        ref={frame}
        title={t('draw.io diagram editor')}
        src={`${url}/?${params}`}
        sandbox="allow-scripts allow-same-origin allow-popups allow-popups-to-escape-sandbox allow-downloads allow-modals allow-forms"
      />
    </div>,
    document.body,
  )
}
