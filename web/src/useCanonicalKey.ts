import { useEffect } from 'react'
import { useLocation, useNavigate } from 'react-router-dom'

/** A renamed space still answers to its old key: move the URL to the current one. */
export function useCanonicalSpaceKey(urlKey: string | undefined, currentKey: string | undefined) {
  const nav = useNavigate()
  const loc = useLocation()
  useEffect(() => {
    if (!urlKey || !currentKey || urlKey.toUpperCase() === currentKey.toUpperCase()) return
    const prefix = `/spaces/${urlKey}`
    if (loc.pathname.startsWith(prefix)) nav(`/spaces/${currentKey}${loc.pathname.slice(prefix.length)}${loc.search}`, { replace: true })
  }, [urlKey, currentKey, loc.pathname, loc.search, nav])
}
