// Table of contents entries for a `toc` block: the same rules as the server
// (crates/doc-model/src/toc.rs) so the editor, shared pages and exports list the same headings.

import type { Node as PMNode } from '@tiptap/pm/model'
import { headingIds } from './HeadingIds'

export interface TocAttrs {
  minLevel?: number | null
  maxLevel?: number | null
  include?: string | null
  exclude?: string | null
  numbered?: boolean | null
  style?: string | null
}

export interface TocEntry {
  pos: number
  level: number
  depth: number
  text: string
  id: string
  number: string
}

// Patterns come from page authors and run in every reader's browser. JavaScript's regex engine
// backtracks, so nested quantifiers like `(a+)+` could hang the tab: those are matched as plain
// text instead (the server uses a linear-time engine either way).
const NESTED_QUANTIFIER = /\((?:[^()\\]|\\.)*[+*}](?:[^()\\]|\\.)*\)\s*[+*{]/

function pattern(p: string | null | undefined): RegExp | null {
  const s = (p ?? '').trim()
  if (!s || s.length > 200) return null
  try {
    return NESTED_QUANTIFIER.test(s) ? new RegExp(s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')) : new RegExp(s)
  } catch {
    return null
  }
}

export function tocEntries(doc: PMNode, attrs: TocAttrs): TocEntry[] {
  const clamp = (v: number | null | undefined, d: number) => Math.min(6, Math.max(1, Number(v) || d))
  const min = clamp(attrs.minLevel, 1)
  const max = clamp(attrs.maxLevel, 6)
  const include = pattern(attrs.include)
  const exclude = pattern(attrs.exclude)
  const listed = headingIds(doc).filter((h) => {
    const t = h.text.slice(0, 500)
    return h.level >= min && h.level <= max && (!include || include.test(t)) && !(exclude && exclude.test(t))
  })
  const base = Math.min(...listed.map((h) => h.level), 6)
  const counters = [0, 0, 0, 0, 0, 0]
  return listed.map((h) => {
    const depth = h.level - base
    counters[depth] += 1
    for (let i = depth + 1; i < 6; i++) counters[i] = 0
    for (let i = 0; i < depth; i++) if (counters[i] === 0) counters[i] = 1
    const number = attrs.numbered ? counters.slice(0, depth + 1).join('.') : ''
    return { pos: h.pos, level: h.level, depth, text: h.text, id: h.id, number }
  })
}
