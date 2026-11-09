// WikiCraft's own SVG icon set. The drawings live in crates/doc-model/assets/icons.json so the
// server (exports, e-mails) renders exactly the same icons. Emoji are never used as icons.

import set from '../../crates/doc-model/assets/icons.json'

type Group = 'callout' | 'symbols' | 'ui'
const icons = set as unknown as Record<Group | 'emoticons', Record<string, string>> & { symbolColors: Record<string, string> }

export const SYMBOLS = Object.keys(icons.symbols)
export const SYMBOL_COLORS = icons.symbolColors
export const EMOTICONS = Object.keys(icons.emoticons)

interface IconProps {
  size?: number
  className?: string
  title?: string
  style?: React.CSSProperties
}

/** A line icon from the set (stroke = currentColor). */
export function LineIcon({ group, name, size = 16, className, title, style }: IconProps & { group: Group; name: string }) {
  const inner = icons[group]?.[name]
  if (!inner) return null
  return (
    <svg
      className={className}
      style={style}
      viewBox="0 0 24 24"
      width={size}
      height={size}
      fill="none"
      stroke="currentColor"
      strokeWidth={1.75}
      strokeLinecap="round"
      strokeLinejoin="round"
      role={title ? 'img' : undefined}
      aria-label={title}
      aria-hidden={title ? undefined : true}
      // static markup from our own icon set
      dangerouslySetInnerHTML={{ __html: (title ? `<title>${title}</title>` : '') + inner }}
    />
  )
}

/** A coloured emoticon (Confluence's set: tick, cross, smile, …). */
export function Emoticon({ name, size, className }: { name: string; size?: number | string; className?: string }) {
  const inner = icons.emoticons[name]
  if (!inner) return null
  return <svg className={className ?? 'emoticon'} viewBox="0 0 24 24" width={size ?? '1.2em'} height={size ?? '1.2em'} role="img" aria-label={name} dangerouslySetInnerHTML={{ __html: inner }} />
}

/** Space / template icons are stored as `symbol` or `symbol:color`. */
export function parseSymbol(value: string | null | undefined): { name: string; color: string } | null {
  if (!value) return null
  const [name, color] = value.split(':')
  if (!icons.symbols[name]) return null
  return { name, color: SYMBOL_COLORS[color] ? color : 'blue' }
}

/** A white symbol on a coloured rounded tile. */
export function SymbolTile({ value, size = 32, fallback }: { value: string | null | undefined; size?: number; fallback?: React.ReactNode }) {
  const s = parseSymbol(value)
  if (!s) return <>{fallback ?? null}</>
  return (
    <span className="symbol-tile" style={{ width: size, height: size, borderRadius: Math.round(size * 0.26), background: SYMBOL_COLORS[s.color] }}>
      <LineIcon group="symbols" name={s.name} size={Math.round(size * 0.62)} />
    </span>
  )
}

/** Grid to pick a symbol and a colour. */
export function SymbolPicker({ value, onChange }: { value: string | null; onChange: (v: string | null) => void }) {
  const cur = parseSymbol(value)
  const color = cur?.color ?? 'blue'
  return (
    <div className="symbol-picker">
      <div className="symbol-colors">
        {Object.entries(SYMBOL_COLORS).map(([k, c]) => (
          <button key={k} type="button" className={`swatch${k === color ? ' selected' : ''}`} style={{ background: c }} aria-label={k} onClick={() => onChange(`${cur?.name ?? 'book'}:${k}`)} />
        ))}
        <button type="button" className="btn btn-ghost small" onClick={() => onChange(null)}>No icon</button>
      </div>
      <div className="symbol-grid">
        {SYMBOLS.map((name) => (
          <button key={name} type="button" title={name} className={`symbol-option${cur?.name === name ? ' selected' : ''}`} onClick={() => onChange(`${name}:${color}`)}>
            <SymbolTile value={`${name}:${color}`} size={30} />
          </button>
        ))}
      </div>
    </div>
  )
}
