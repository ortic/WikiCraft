// Readable page addresses: `/wiki/<SPACE KEY>/<slug>` next to the permanent `/pages/<id>`.
// The server makes the slug from the title (`Page.slug`, null when an older page in the space
// already has it) and resolves addresses, including the ones a page had before a rename or move.
import { useMatch } from 'react-router-dom'
import { useQuery } from '@tanstack/react-query'
import { api } from './api'

/** The readable address of a page, or null if it has none. */
export function wikiPath(p: { spaceKey: string; slug?: string | null }): string | null {
  return p.slug ? `/wiki/${encodeURIComponent(p.spaceKey)}/${encodeURIComponent(p.slug)}` : null
}

/** Query key under which a readable address's page id is cached. */
export function wikiPathKey(spaceKey: string, slug: string) {
  return ['wiki-path', spaceKey.toLowerCase(), slug] as const
}

/** The page a readable address leads to (`pageId` null: none, or none the reader may see). */
export function useWikiPath(spaceKey: string | undefined, slug: string | undefined) {
  return useQuery({
    queryKey: wikiPathKey(spaceKey ?? '', slug ?? ''),
    queryFn: () => api.get<{ pageId: string | null }>(`/resolve?space=${encodeURIComponent(spaceKey!)}&slug=${encodeURIComponent(slug!)}`),
    enabled: !!spaceKey && !!slug,
    retry: false,
    staleTime: 60_000,
  })
}

/** The id of the page being shown, at `/pages/<id>` or at a readable address. */
export function useCurrentPageId(): string | undefined {
  const page = useMatch('/pages/:id/*')
  const wiki = useMatch('/wiki/:key/:slug')
  const resolved = useWikiPath(wiki?.params.key, wiki?.params.slug)
  return page?.params.id ?? resolved.data?.pageId ?? undefined
}
