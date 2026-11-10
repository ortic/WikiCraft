// Self-hosts Excalidraw's fonts (it would otherwise load them from a CDN, which our CSP blocks).
// The 13 MB CJK font is left out; drawings fall back to a system font for those characters.
import { cpSync, existsSync, mkdirSync, readdirSync, rmSync } from 'node:fs'
import { join } from 'node:path'

const src = 'node_modules/@excalidraw/excalidraw/dist/prod/fonts'
const dst = 'public/excalidraw-assets/fonts'
if (!existsSync(src)) process.exit(0)
rmSync(dst, { recursive: true, force: true })
mkdirSync(dst, { recursive: true })
for (const family of readdirSync(src)) {
  if (family === 'Xiaolai') continue
  cpSync(join(src, family), join(dst, family), { recursive: true })
}
