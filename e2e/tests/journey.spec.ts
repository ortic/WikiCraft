// One journey through a fresh WikiCraft: setup, spaces, editing, collaboration, comments,
// search, sharing, export, two-factor sign-in and accessibility. Tests run in order and build
// on each other.

import { createHmac } from 'node:crypto'
import AxeBuilder from '@axe-core/playwright'
import { expect, firefox, test, type Browser, type BrowserContext, type Page } from '@playwright/test'

const OWNER = { name: 'Olivia Owner', email: 'olivia@e2e.test', password: 'correct horse battery' }
const MEMBER = { name: 'Max Member', email: 'max@e2e.test', password: 'password-123' }

test.describe.configure({ mode: 'serial' })

let ownerState: Awaited<ReturnType<BrowserContext['storageState']>>
let pageUrl = ''
let pageId = ''

async function login(browser: Browser, who: { email: string; password: string }) {
  // (a browser launched by a test, like Firefox below, doesn't get the configured address)
  const ctx = await browser.newContext({ baseURL: test.info().project.use.baseURL })
  const p = await ctx.newPage()
  await p.goto('/login')
  await p.locator('input[autocomplete=username]').fill(who.email)
  await p.locator('input[type=password]').fill(who.password)
  await p.getByRole('button', { name: 'Log in' }).click()
  await p.waitForURL('/')
  return p
}

function totp(secretB32: string, offset = 0) {
  const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567'
  let bits = 0
  let value = 0
  const bytes: number[] = []
  for (const c of secretB32.replace(/\s|=/g, '').toUpperCase()) {
    value = (value << 5) | alphabet.indexOf(c)
    bits += 5
    if (bits >= 8) {
      bytes.push((value >>> (bits - 8)) & 255)
      bits -= 8
    }
  }
  const counter = Buffer.alloc(8)
  counter.writeBigUInt64BE(BigInt(Math.floor(Date.now() / 1000 / 30) + offset))
  const h = createHmac('sha1', Buffer.from(bytes)).update(counter).digest()
  const o = h[h.length - 1] & 15
  return String((h.readUInt32BE(o) & 0x7fffffff) % 1_000_000).padStart(6, '0')
}

test('first-run setup creates the workspace and its owner', async ({ page }) => {
  await page.goto('/')
  await expect(page.getByRole('heading', { name: 'Welcome to WikiCraft' })).toBeVisible()
  const field = (label: string) => page.locator('.field', { hasText: label }).locator('input')
  await field('Workspace name').fill('E2E Corp')
  await field('Your name').fill(OWNER.name)
  await field('E-mail').fill(OWNER.email)
  await field('Password').fill(OWNER.password)
  await page.getByRole('button', { name: 'Create workspace' }).click()
  // the app shell (sidebar) only renders once signed in
  await expect(page.locator('.sidebar')).toBeVisible({ timeout: 20_000 })
  ownerState = await page.context().storageState()
})

test('owner creates a space and a page with rich content', async ({ browser }) => {
  const ctx = await browser.newContext({ storageState: ownerState })
  const page = await ctx.newPage()
  await page.goto('/spaces')
  await page.getByRole('button', { name: 'Create space' }).first().click()
  await page.getByPlaceholder('e.g. Engineering').fill('Product')
  await page.locator('.modal').getByRole('button', { name: 'Create space' }).click()
  await page.waitForURL(/\/spaces\/PRODUCT/)

  // a page through the API (stable), then content through the editor
  const created = await page.evaluate(async () => {
    const r = await fetch('/api/pages', { method: 'POST', headers: { 'content-type': 'application/json', 'x-wikicraft': '1' }, body: JSON.stringify({ spaceKey: 'PRODUCT', title: 'Roadmap 2027' }) })
    return (await r.json()).id as string
  })
  pageId = created
  pageUrl = `/pages/${created}`
  await page.goto(pageUrl)
  const editor = page.locator('.ProseMirror[contenteditable="true"]')
  await expect(editor).toBeVisible({ timeout: 20_000 })
  // the address bar shows the readable address
  await expect(page).toHaveURL(/\/wiki\/PRODUCT\/roadmap-2027$/)
  await editor.click()
  await page.keyboard.type('Our plan for the year: ship the mobile app.')
  await page.keyboard.press('Enter')
  await page.keyboard.type('/info', { delay: 30 })
  await page.keyboard.press('Enter')
  await page.keyboard.type('Dates are estimates.')
  await expect(page.locator('.callout .callout-icon svg')).toBeVisible()
  await page.keyboard.press('Enter')
  await page.keyboard.press('Enter')
  await page.keyboard.type('/table', { delay: 30 })
  await page.keyboard.press('Enter')
  await expect(page.locator('.table-menu')).toBeVisible()
  await page.keyboard.type('Quarter')
  // the content survives a reload (persisted by the collaboration server)
  await page.waitForTimeout(3000)
  await page.reload()
  await expect(page.locator('.ProseMirror')).toContainText('ship the mobile app', { timeout: 20_000 })
  await expect(page.locator('.ProseMirror table')).toContainText('Quarter')
  await ctx.close()
})

test('a link to a block opens the page at that block', async ({ browser }) => {
  const ctx = await browser.newContext({ storageState: ownerState, permissions: ['clipboard-read', 'clipboard-write'] })
  const page = await ctx.newPage()
  await page.goto(pageUrl)
  const editor = page.locator('.ProseMirror[contenteditable="true"]')
  await expect(editor).toBeVisible({ timeout: 20_000 })
  // every stored block has an id, the callout too
  const callout = await page.evaluate(async (id) => {
    const p = await (await fetch(`/api/pages/${id}`)).json()
    const find = (n: { type: string; attrs?: Record<string, unknown>; content?: unknown[] }): string | null =>
      n.type === 'callout' ? (n.attrs?.blockId as string) : (n.content ?? []).map((c) => find(c as never)).find(Boolean) ?? null
    return find(p.content)
  }, pageId)
  expect(callout).toMatch(/^[a-z][a-z0-9]{7}$/)
  // the drag handle's link button copies the block's link; the handle follows mouse movement,
  // so move again until the editor has placed it (slow CI machines miss a single hover)
  const handle = page.locator('.drag-handle .block-link')
  await expect(async () => {
    await page.mouse.move(0, 0)
    await editor.getByText('Dates are estimates.').hover()
    await expect(handle).toBeVisible({ timeout: 1_000 })
  }).toPass({ timeout: 15_000 })
  await handle.click()
  await expect(page.locator('.toast')).toHaveText('Link to this block copied')
  const copied = await page.evaluate(() => navigator.clipboard.readText())
  expect(copied).toMatch(new RegExp(`/pages/${pageId}#block-[a-z0-9]{8}$`))
  // opening it scrolls to the block and highlights it
  const other = await ctx.newPage()
  await other.goto(`${pageUrl}#block-${callout}`)
  await expect(other.locator('.ProseMirror .block-flash')).toContainText('Dates are estimates.', { timeout: 20_000 })
  await ctx.close()
})

test('two people edit the same page live', async ({ browser }) => {
  const ownerCtx = await browser.newContext({ storageState: ownerState })
  const owner = await ownerCtx.newPage()
  await owner.goto('/admin/users')
  await owner.evaluate(async (m) => {
    await fetch('/api/users', { method: 'POST', headers: { 'content-type': 'application/json', 'x-wikicraft': '1' }, body: JSON.stringify({ ...m, role: 'member' }) })
  }, MEMBER)
  const member = await login(browser, MEMBER)
  await owner.goto(pageUrl)
  await member.goto(pageUrl)
  const ownerEditor = owner.locator('.ProseMirror[contenteditable="true"]')
  const memberEditor = member.locator('.ProseMirror[contenteditable="true"]')
  await expect(ownerEditor).toBeVisible({ timeout: 20_000 })
  await expect(memberEditor).toBeVisible({ timeout: 20_000 })
  await ownerEditor.locator('p').first().click()
  await owner.keyboard.press('End')
  await owner.keyboard.type(' Owner was here.')
  await expect(memberEditor).toContainText('Owner was here.', { timeout: 15_000 })
  await memberEditor.locator('p').first().click()
  await member.keyboard.press('End')
  await member.keyboard.type(' Max too.')
  await expect(ownerEditor).toContainText('Max too.', { timeout: 15_000 })
  await ownerCtx.close()
  await member.context().close()
})

test('Chromium and Firefox edit one paragraph at once, and offline edits merge', async ({ browser }) => {
  const ownerCtx = await browser.newContext({ storageState: ownerState })
  const owner = await ownerCtx.newPage()
  await owner.goto('/')
  const colleague = { email: 'fiona@e2e.test', password: 'firefox-pass-1' }
  const id = await owner.evaluate(async (who) => {
    const json = { 'content-type': 'application/json', 'x-wikicraft': '1' }
    await fetch('/api/users', { method: 'POST', headers: json, body: JSON.stringify({ ...who, name: 'Fiona Firefox', role: 'member' }) })
    const r = await fetch('/api/pages', { method: 'POST', headers: json, body: JSON.stringify({ spaceKey: 'PRODUCT', title: 'Standup', markdown: 'Agenda:\n\nNotes:' }) })
    return (await r.json()).id as string
  }, colleague)
  const ff = await firefox.launch()
  const fiona = await login(ff, colleague)
  // Fiona's connection for live editing goes through the test, which can cut it
  let offline = false
  const sockets: { close: () => Promise<void> }[] = []
  await fiona.routeWebSocket(/\/collab$/, (ws) => {
    if (offline) return void ws.close()
    ws.connectToServer()
    sockets.push(ws)
  })
  await owner.goto(`/pages/${id}`)
  await fiona.goto(`/pages/${id}`)
  const a = owner.locator('.ProseMirror[contenteditable="true"]')
  const b = fiona.locator('.ProseMirror[contenteditable="true"]')
  await expect(a).toBeVisible({ timeout: 20_000 })
  await expect(b).toBeVisible({ timeout: 20_000 })
  // the document's text, without the other person's caret and name label
  const text = (e: typeof a) =>
    e.evaluate((el) => {
      const copy = el.cloneNode(true) as HTMLElement
      copy.querySelectorAll('[class*="collaboration-carets"]').forEach((c) => c.remove())
      return (copy.textContent ?? '').replace(/\s+/g, ' ').trim()
    })
  const same = () => expect.poll(async () => (await text(a)) === (await text(b)), { timeout: 15_000 }).toBe(true)

  // both type in the same paragraph at the same time, one at its start, one at its end
  await a.getByText('Agenda:').click()
  await owner.keyboard.press('Home')
  await b.getByText('Agenda:').click()
  await fiona.keyboard.press('End')
  await Promise.all([owner.keyboard.type('Daily ', { delay: 25 }), fiona.keyboard.type(' beta two', { delay: 25 })])
  await same()
  expect(await text(a)).toContain('Daily Agenda: beta two')
  // at the very same spot the letters may interleave, but none is lost and both see the same
  await Promise.all([owner.keyboard.press('End'), fiona.keyboard.press('End')])
  await Promise.all([owner.keyboard.type(' xyz', { delay: 25 }), fiona.keyboard.type(' qrs', { delay: 25 })])
  await same()
  const tail = (await text(a)).split('beta two')[1].split('Notes:')[0]
  expect([...tail.replace(/\s/g, '')].sort().join('')).toBe('qrsxyz')

  // Fiona loses the connection and keeps writing; the owner writes too; both arrive
  offline = true
  await Promise.all(sockets.splice(0).map((s) => s.close()))
  await b.getByText('Notes:').click()
  await fiona.keyboard.press('End')
  await fiona.keyboard.type(' written offline')
  await a.getByText('Notes:').click()
  await owner.keyboard.press('Home')
  await owner.keyboard.type('Owner: ')
  await expect.poll(() => text(a)).toContain('Owner: Notes:')
  expect(await text(a)).not.toContain('written offline')
  offline = false
  await expect.poll(() => text(a), { timeout: 30_000 }).toContain('Notes: written offline')
  await expect.poll(() => text(b), { timeout: 30_000 }).toContain('Owner: Notes:')
  await same()
  // and the server kept the merged page
  await expect
    .poll(async () => owner.evaluate(async (pid) => JSON.stringify((await (await fetch(`/api/pages/${pid}`)).json()).content), id), { timeout: 20_000 })
    .toMatch(/Owner: .*Notes:.*written offline/)
  await fiona.context().close()
  await ff.close()
  await ownerCtx.close()
})

test('comments and search', async ({ browser }) => {
  const ctx = await browser.newContext({ storageState: ownerState })
  const page = await ctx.newPage()
  await page.goto(pageUrl)
  await page.getByTitle('Comments').click()
  await page.locator('.side-panel textarea').first().fill('Can we pull the mobile app into Q2?')
  await page.locator('.side-panel .btn-primary').first().click()
  await expect(page.locator('.side-panel')).toContainText('pull the mobile app into Q2')
  // an image in a comment: picked with the image button, shown in the thread
  const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==', 'base64')
  await page.locator('.side-panel input[type=file]').last().setInputFiles({ name: 'sketch.png', mimeType: 'image/png', buffer: png })
  const box = page.locator('.side-panel textarea').last()
  await expect(box).toHaveValue(/!\[sketch\]\(\/api\/attachments\/[0-9a-f-]+\/content\)/)
  await page.locator('.side-panel .btn-primary').last().click()
  const img = page.locator('.side-panel .comment-image img')
  await expect(img).toHaveAttribute('alt', 'sketch')
  await expect.poll(() => img.evaluate((el) => (el as HTMLImageElement).naturalWidth)).toBe(1)
  // search (index updates asynchronously)
  await expect
    .poll(async () => page.evaluate(async () => (await (await fetch('/api/search?q=mobile')).json()).length), { timeout: 20_000 })
    .toBeGreaterThan(0)
  await page.keyboard.press('Control+k')
  await page.getByPlaceholder(/Search pages/).fill('mobile')
  await expect(page.locator('.palette').getByText('Roadmap 2027').first()).toBeVisible()
  await ctx.close()
})

test('a public link shows the page without an account', async ({ browser }) => {
  const ctx = await browser.newContext({ storageState: ownerState })
  const page = await ctx.newPage()
  await page.goto(pageUrl)
  await page.getByRole('button', { name: 'Share' }).click()
  await page.getByRole('button', { name: 'Create public link' }).click()
  const url = await page.locator('.share-link input').first().inputValue()
  await ctx.close()
  const anon = await browser.newContext()
  const p = await anon.newPage()
  await p.goto(url)
  await expect(p.locator('.public-main > h1')).toHaveText('Roadmap 2027')
  await expect(p.locator('.public-article')).toContainText('ship the mobile app')
  await expect(p.getByText('Shared publicly')).toBeVisible()
  await anon.close()
})

test('space export produces a downloadable archive', async ({ browser }) => {
  const ctx = await browser.newContext({ storageState: ownerState, acceptDownloads: true })
  const page = await ctx.newPage()
  await page.goto('/spaces/PRODUCT/settings?tab=export')
  await page.getByText('Website (HTML)').click()
  await page.getByRole('button', { name: /^Export / }).click()
  await expect(page.getByText('Export ready')).toBeVisible({ timeout: 30_000 })
  const [download] = await Promise.all([page.waitForEvent('download'), page.locator('a[download]').click()])
  expect(download.suggestedFilename()).toMatch(/^PRODUCT-html-.*\.zip$/)
  await ctx.close()
})

test('two-factor sign-in', async ({ browser }) => {
  const member = await login(browser, MEMBER)
  await member.goto('/settings')
  await member.getByRole('button', { name: 'Set up two-factor authentication' }).click()
  const secret = (await member.locator('.mfa-setup code').textContent())!.replace(/\s/g, '')
  await member.locator('.mfa-setup .otp-input').fill(totp(secret))
  await member.getByRole('button', { name: 'Turn on' }).click()
  await expect(member.locator('.recovery-codes code')).toHaveCount(10)
  await member.context().close()

  const ctx = await browser.newContext()
  const p = await ctx.newPage()
  await p.goto('/login')
  await p.locator('input[autocomplete=username]').fill(MEMBER.email)
  await p.locator('input[type=password]').fill(MEMBER.password)
  await p.getByRole('button', { name: 'Log in' }).click()
  await expect(p.getByRole('heading', { name: 'Two-factor sign-in' })).toBeVisible()
  // the enrolment code is used up; the next step's code is within the accepted window
  await p.locator('.otp-input').fill(totp(secret, 1))
  await p.getByRole('button', { name: 'Verify' }).click()
  await p.waitForURL('/')
  expect(await p.evaluate(async () => (await (await fetch('/api/me')).json()).user?.email)).toBe(MEMBER.email)
  await ctx.close()
  void pageId
})

test('find and replace in the editor', async ({ browser }) => {
  const ctx = await browser.newContext({ storageState: ownerState })
  const page = await ctx.newPage()
  await page.goto(pageUrl)
  const editor = page.locator('.ProseMirror[contenteditable="true"]')
  await expect(editor).toBeVisible({ timeout: 20_000 })
  await editor.locator('p').first().click()
  await page.keyboard.press('Control+h')
  await page.locator('.find-bar input[data-role=find]').fill('mobile app')
  await expect(page.locator('.find-count')).toHaveText('1 of 1')
  await expect(page.locator('.ProseMirror .find-current')).toHaveText('mobile app')
  await page.locator('.find-bar input[data-role=replace]').fill('iOS app')
  await page.locator('.find-bar').getByRole('button', { name: 'All' }).click()
  await expect(page.locator('.find-count')).toHaveText('No results')
  await expect(editor).toContainText('ship the iOS app')
  await page.keyboard.press('Escape')
  await expect(page.locator('.find-bar')).toHaveCount(0)
  await ctx.close()
})

test('phone layout never scrolls sideways', async ({ browser }) => {
  const ctx = await browser.newContext({ storageState: ownerState, viewport: { width: 390, height: 844 } })
  const page = await ctx.newPage()
  for (const url of ['/', pageUrl, '/search?q=mobile', '/spaces', '/people', '/admin/users', '/settings', `${pageUrl}/history`]) {
    await page.goto(url)
    await page.waitForLoadState('networkidle').catch(() => {})
    const { sw, cw } = await page.evaluate(() => ({ sw: document.documentElement.scrollWidth, cw: document.documentElement.clientWidth }))
    expect(sw, `${url} is ${sw}px wide on a ${cw}px screen`).toBeLessThanOrEqual(cw)
  }
  await ctx.close()
})

test('template instructions, decisions and the decision log', async ({ browser }) => {
  const ctx = await browser.newContext({ storageState: ownerState })
  const page = await ctx.newPage()
  await page.goto('/')
  const create = (body: object) =>
    page.evaluate(async (b) => {
      const r = await fetch('/api/pages', { method: 'POST', headers: { 'content-type': 'application/json', 'x-wikicraft': '1' }, body: JSON.stringify(b) })
      return (await r.json()).id as string
    }, body)
  // the decision template carries instructions: visible while editing, replaced by typing
  const decision = await create({ spaceKey: 'PRODUCT', templateId: 'builtin:decision', title: 'Decision: launch region' })
  await page.goto(`/pages/${decision}`)
  const hint = page.locator('.ProseMirror .instructional-text')
  await expect(hint).toHaveCount(3, { timeout: 20_000 })
  await hint.nth(1).click()
  await page.keyboard.press('Backspace')
  await page.keyboard.type('/decision')
  await page.keyboard.press('Enter')
  await page.keyboard.type('Launch in Europe first')
  await expect(page.locator('.ProseMirror li[data-type="decisionItem"]')).toContainText('Launch in Europe first')
  await expect(hint).toHaveCount(2)
  // exports leave the remaining instructions out
  await expect
    .poll(async () => page.evaluate(async (id) => (await fetch(`/api/pages/${id}/export?format=md`)).text(), decision), { timeout: 15_000 })
    .toContain('Launch in Europe first')
  const md = await page.evaluate(async (id) => (await fetch(`/api/pages/${id}/export?format=md`)).text(), decision)
  expect(md).not.toContain('easier or harder')

  // a decision log on another page lists it
  const log = await create({ spaceKey: 'PRODUCT', title: 'Decisions', blank: true })
  await page.goto(`/pages/${log}`)
  const editor = page.locator('.ProseMirror[contenteditable="true"]')
  await expect(editor).toBeVisible({ timeout: 20_000 })
  await editor.click()
  await page.keyboard.type('/decisionrep')
  await page.keyboard.press('Enter')
  await expect(page.locator('.macro-table')).toContainText('Launch in Europe first', { timeout: 15_000 })
  await ctx.close()
})

test('a template asks for its variables before creating the page', async ({ browser }) => {
  const ctx = await browser.newContext({ storageState: ownerState })
  const page = await ctx.newPage()
  await page.goto('/spaces/PRODUCT')
  await page.getByRole('button', { name: 'Create', exact: true }).click()
  const dialog = page.locator('.modal')
  await dialog.getByRole('button', { name: /^Incident postmortem/ }).click()
  await expect(dialog).toContainText('New page: Incident postmortem')
  await dialog.getByLabel('Incident').fill('Checkout outage')
  await dialog.getByRole('button', { name: 'Create page' }).click()
  await expect(page).toHaveURL(/\/(pages|wiki)\//)
  await expect(page.locator('textarea.page-title')).toHaveValue('Postmortem: Checkout outage', { timeout: 15_000 })
  await ctx.close()
})

test('a draw.io diagram is edited in the draw.io frame and saved with a preview', async ({ browser }) => {
  const ctx = await browser.newContext({ storageState: ownerState })
  // a stand-in for diagrams.net that speaks its embed protocol: it asks for the diagram, then
  // "saves" an edited one and returns a PNG for the export
  const png = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg=='
  await ctx.route('https://embed.diagrams.net/**', (route) =>
    route.fulfill({
      contentType: 'text/html',
      body: `<!doctype html><script>
        const send = (m) => parent.postMessage(JSON.stringify(m), '*')
        const edited = '<mxfile><diagram><mxGraphModel><root><mxCell id="2" value="Checkout service" vertex="1"/></root></mxGraphModel></diagram></mxfile>'
        addEventListener('message', (e) => {
          const m = JSON.parse(e.data)
          if (m.action === 'load') send({ event: 'save', xml: edited, exit: true })
          if (m.action === 'export') send({ event: 'export', format: 'png', xml: m.xml, data: 'data:image/png;base64,${png}' })
        })
        send({ event: 'init' })
      </script>`,
    }),
  )
  const page = await ctx.newPage()
  await page.goto('/')
  const id = await page.evaluate(async () => {
    const r = await fetch('/api/pages', { method: 'POST', headers: { 'content-type': 'application/json', 'x-wikicraft': '1' }, body: JSON.stringify({ spaceKey: 'PRODUCT', title: 'Architecture diagram', blank: true }) })
    return (await r.json()).id as string
  })
  await page.goto(`/pages/${id}`)
  const editor = page.locator('.ProseMirror[contenteditable="true"]')
  await expect(editor).toBeVisible({ timeout: 20_000 })
  await editor.click()
  await page.keyboard.type('/draw.io')
  await page.keyboard.press('Enter')
  await page.getByRole('button', { name: /click to edit in draw.io/ }).click()
  // saved and closed: the preview shows, the diagram is in the page (and searchable)
  await expect(page.locator('.drawing-block img.drawing-preview')).toBeVisible({ timeout: 15_000 })
  await expect(page.locator('.drawio-modal')).toHaveCount(0)
  await expect
    .poll(async () => page.evaluate(async (pid) => JSON.stringify((await (await fetch(`/api/pages/${pid}`)).json()).content), id), { timeout: 15_000 })
    .toContain('Checkout service')
  await ctx.close()
})

test('text colour from the selection menu and a three-column layout', async ({ browser }) => {
  const ctx = await browser.newContext({ storageState: ownerState })
  const page = await ctx.newPage()
  await page.goto('/')
  const id = await page.evaluate(async () => {
    const r = await fetch('/api/pages', { method: 'POST', headers: { 'content-type': 'application/json', 'x-wikicraft': '1' }, body: JSON.stringify({ spaceKey: 'PRODUCT', title: 'Colours', markdown: 'Ship the release today' }) })
    return (await r.json()).id as string
  })
  await page.goto(`/pages/${id}`)
  const editor = page.locator('.ProseMirror[contenteditable="true"]')
  await expect(editor).toContainText('Ship the release today', { timeout: 20_000 })
  await editor.getByText('Ship the release today').click()
  await page.keyboard.press('Home')
  await page.keyboard.press('Shift+End')
  await expect.poll(() => page.evaluate(() => String(window.getSelection()))).toBe('Ship the release today')
  await page.locator('.bubble').getByRole('button', { name: 'Text colour, highlight, alignment and indentation' }).click()
  await page.locator('.bubble').getByRole('button', { name: 'Text colour Red' }).click()
  await expect(editor.locator('span[style*="color"]')).toHaveText('Ship the release today')
  await page.keyboard.press('End')
  // the editor has taken in the collapsed selection once the selection menu is gone
  await expect(page.locator('.bubble')).toBeHidden()
  await page.keyboard.press('Enter')
  await page.keyboard.type('/three')
  await page.keyboard.press('Enter')
  await expect(page.locator('.ProseMirror .columns[data-layout="three_equal"] > .column')).toHaveCount(3)
  await expect
    .poll(async () => page.evaluate(async (pid) => JSON.stringify((await (await fetch(`/api/pages/${pid}`)).json()).content), id), { timeout: 15_000 })
    .toMatch(/#d14343[\s\S]*three_equal|three_equal[\s\S]*#d14343/)
  // Tab at the start of a paragraph indents it (even right after Home)
  await editor.getByText('Ship the release today').click()
  await page.keyboard.press('Home')
  await page.keyboard.press('Tab')
  await expect(editor.locator('p[data-indent="1"]')).toContainText('Ship the release today')
  await expect
    .poll(async () => page.evaluate(async (pid) => JSON.stringify((await (await fetch(`/api/pages/${pid}`)).json()).content), id), { timeout: 15_000 })
    .toMatch(/"indent":1\},"content":\[\{"marks":\[[^\]]*\],"text":"Ship the release today"/)
  await ctx.close()
})

test('the formatting toolbar sets styles, undoes, and can be hidden', async ({ browser }) => {
  const ctx = await browser.newContext({ storageState: ownerState })
  const page = await ctx.newPage()
  await page.goto('/')
  const id = await page.evaluate(async () => {
    const r = await fetch('/api/pages', { method: 'POST', headers: { 'content-type': 'application/json', 'x-wikicraft': '1' }, body: JSON.stringify({ spaceKey: 'PRODUCT', title: 'Toolbar', markdown: 'Launch plan' }) })
    return (await r.json()).id as string
  })
  await page.goto(`/pages/${id}`)
  const editor = page.locator('.ProseMirror[contenteditable="true"]')
  await expect(editor).toContainText('Launch plan', { timeout: 20_000 })
  const toolbar = page.getByRole('toolbar', { name: 'Formatting' })
  await editor.getByText('Launch plan').click()
  await toolbar.getByLabel('Paragraph style').selectOption('h2')
  await expect(editor.locator('h2')).toHaveText('Launch plan')
  await page.keyboard.press('End')
  await page.keyboard.press('Shift+Home')
  await toolbar.getByRole('button', { name: 'Bold (Ctrl+B)' }).click()
  await expect(editor.locator('h2 strong')).toHaveText('Launch plan')
  await expect(toolbar.getByRole('button', { name: 'Bold (Ctrl+B)' })).toHaveAttribute('aria-pressed', 'true')
  await toolbar.getByRole('button', { name: 'Undo (Ctrl+Z)' }).click()
  await expect(editor.locator('h2, strong')).toHaveCount(0)
  // hidden for this browser until shown again
  await toolbar.getByRole('button', { name: 'Hide the toolbar' }).click()
  await page.reload()
  await expect(page.getByRole('button', { name: 'Show the formatting toolbar' })).toBeVisible({ timeout: 20_000 })
  await expect(toolbar).toHaveCount(0)
  await page.getByRole('button', { name: 'Show the formatting toolbar' }).click()
  await expect(toolbar).toBeVisible()
  await ctx.close()
})

test('a search box and a create-page button inside a page', async ({ browser }) => {
  const ctx = await browser.newContext({ storageState: ownerState })
  const page = await ctx.newPage()
  await page.goto('/')
  const id = await page.evaluate(async () => {
    const r = await fetch('/api/pages', { method: 'POST', headers: { 'content-type': 'application/json', 'x-wikicraft': '1' }, body: JSON.stringify({ spaceKey: 'PRODUCT', title: 'Team hub', blank: true }) })
    return (await r.json()).id as string
  })
  await page.goto(`/pages/${id}`)
  const editor = page.locator('.ProseMirror[contenteditable="true"]')
  await expect(editor).toBeVisible({ timeout: 20_000 })
  await editor.click()
  await page.keyboard.type('/create-page')
  await page.keyboard.press('Enter')
  await page.keyboard.type('/search')
  await page.keyboard.press('Enter')
  // the button offers the templates, for a page below this one
  await page.locator('.macro-create button').click()
  await expect(page.locator('.modal')).toContainText('Create a page')
  await page.keyboard.press('Escape')
  // the search box searches this space
  const box = page.locator('.macro-search input')
  await box.fill('roadmap')
  await box.press('Enter')
  await expect(page).toHaveURL(/\/search\?q=roadmap&space=PRODUCT/)
  await ctx.close()
})

test('a page includes another one, or only its excerpt, and shows its changes', async ({ browser }) => {
  const ctx = await browser.newContext({ storageState: ownerState })
  const page = await ctx.newPage()
  await page.goto('/')
  const para = (text: string) => ({ type: 'paragraph', content: [{ type: 'text', text }] })
  const create = (title: string, content: unknown) =>
    page.evaluate(async ([title, content]) => {
      const r = await fetch('/api/pages', { method: 'POST', headers: { 'content-type': 'application/json', 'x-wikicraft': '1' }, body: JSON.stringify({ spaceKey: 'PRODUCT', title, ...(content ? { content } : { blank: true }) }) })
      return (await r.json()).id as string
    }, [title, content] as const)
  const checklist = (step: string) => ({ type: 'doc', content: [para('Before anything else:'), { type: 'excerpt', content: [para(step)] }, para('Then tag the release.')] })
  const source = await create('Release checklist', checklist('Freeze the main branch.'))
  const host = await create('Release day', null)
  await expect.poll(() => page.evaluate(async () => (await (await fetch('/api/search?q=release%20checklist', { headers: { 'x-wikicraft': '1' } })).json()).length)).toBeGreaterThan(0)

  await page.goto(`/pages/${host}`)
  const editor = page.locator('.ProseMirror[contenteditable="true"]')
  await expect(editor).toBeVisible({ timeout: 20_000 })
  await editor.click()
  await page.keyboard.type('/include')
  await page.keyboard.press('Enter')
  await page.getByLabel('Find the page to include').fill('release checklist')
  await page.locator('.include-hits button', { hasText: 'Release checklist' }).click()
  const included = page.locator('.include-block .included-content')
  await expect(included).toContainText('Then tag the release.')
  await expect(included).toContainText('Freeze the main branch.')
  // only the excerpt
  await page.getByLabel('What to include').selectOption('excerpt')
  await expect(included).toHaveText('Freeze the main branch.')
  // the included page changes: the next visit shows the new text
  await page.evaluate(async ([id, content]) => {
    await fetch(`/api/pages/${id}/content`, { method: 'PUT', headers: { 'content-type': 'application/json', 'x-wikicraft': '1' }, body: JSON.stringify({ content }) })
  }, [source, checklist('Freeze main and announce it.')] as const)
  await page.reload()
  await expect(page.locator('.include-block .included-content')).toHaveText('Freeze main and announce it.', { timeout: 20_000 })
  await ctx.close()
})

test('history compares two versions word by word, inline or side by side', async ({ browser }) => {
  const ctx = await browser.newContext({ storageState: ownerState })
  const page = await ctx.newPage()
  await page.goto('/')
  const id = await page.evaluate(async () => {
    const h = { 'content-type': 'application/json', 'x-wikicraft': '1' }
    const r = await fetch('/api/pages', { method: 'POST', headers: h, body: JSON.stringify({ spaceKey: 'PRODUCT', title: 'Pricing', markdown: 'Plans start at ten euros per seat.\n\nAnnual billing only.' }) })
    const pid = (await r.json()).id as string
    await fetch(`/api/pages/${pid}/content`, { method: 'PUT', headers: h, body: JSON.stringify({ markdown: 'Plans start at twelve euros per seat.\n\nAnnual billing only.' }) })
    return pid
  })
  await page.goto(`/pages/${id}/history`)
  await expect(page.locator('.diff-line.changed del')).toHaveText('ten')
  await expect(page.locator('.diff-line.changed ins')).toHaveText('twelve')
  await expect(page.getByText('1 change', { exact: true })).toBeVisible()
  await page.getByRole('button', { name: 'Side by side' }).click()
  await expect(page.locator('.diff-split del')).toHaveText('ten')
  await expect(page.locator('.diff-split ins')).toHaveText('twelve')
  await page.getByLabel('Show unchanged').uncheck()
  await expect(page.locator('.diff-split')).not.toContainText('Annual billing')
  await ctx.close()
})

test('a page moves to another space from the page menu', async ({ browser }) => {
  const ctx = await browser.newContext({ storageState: ownerState })
  const page = await ctx.newPage()
  await page.goto('/')
  const id = await page.evaluate(async () => {
    const h = { 'content-type': 'application/json', 'x-wikicraft': '1' }
    await fetch('/api/spaces', { method: 'POST', headers: h, body: JSON.stringify({ key: 'OPS', name: 'Operations' }) })
    const r = await fetch('/api/pages', { method: 'POST', headers: h, body: JSON.stringify({ spaceKey: 'PRODUCT', title: 'On-call guide', markdown: 'Who to call' }) })
    return (await r.json()).id as string
  })
  await page.goto(`/pages/${id}`)
  await page.getByRole('button', { name: 'More' }).click()
  await page.getByText('Move…').click()
  await page.getByLabel('Space').selectOption('OPS')
  await page.locator('.modal').getByRole('button', { name: 'Move' }).click()
  await expect
    .poll(async () => page.evaluate(async (pid) => (await (await fetch(`/api/pages/${pid}`)).json()).spaceKey, id), { timeout: 15_000 })
    .toBe('OPS')
  await ctx.close()
})

test('a table gets numbered rows from its toolbar', async ({ browser }) => {
  const ctx = await browser.newContext({ storageState: ownerState })
  const page = await ctx.newPage()
  await page.goto('/')
  const id = await page.evaluate(async () => {
    const cell = (t: string, kind = 'tableCell') => ({ type: kind, content: [{ type: 'paragraph', content: [{ type: 'text', text: t }] }] })
    const content = { type: 'doc', content: [{ type: 'table', content: [
      { type: 'tableRow', content: [cell('Step', 'tableHeader')] },
      { type: 'tableRow', content: [cell('Unpack')] },
      { type: 'tableRow', content: [cell('Plug in')] },
    ] }] }
    const r = await fetch('/api/pages', { method: 'POST', headers: { 'content-type': 'application/json', 'x-wikicraft': '1' }, body: JSON.stringify({ spaceKey: 'PRODUCT', title: 'Setup steps', content }) })
    return (await r.json()).id as string
  })
  await page.goto(`/pages/${id}`)
  const editor = page.locator('.ProseMirror[contenteditable="true"]')
  await editor.getByText('Unpack').click()
  await page.getByRole('button', { name: 'Numbered rows' }).click()
  const wrapper = page.locator('.tableWrapper')
  await expect(wrapper).toHaveAttribute('data-numbered', 'true')
  const label = await page.evaluate(() => getComputedStyle(document.querySelectorAll('.tableWrapper td')[1], '::before').content)
  expect(label).toContain('counter')
  await expect
    .poll(async () => page.evaluate(async (pid) => JSON.stringify((await (await fetch(`/api/pages/${pid}`)).json()).content), id), { timeout: 15_000 })
    .toContain('"numbered":true')
  await page.getByRole('button', { name: 'Numbered rows' }).click()
  await expect(wrapper).not.toHaveAttribute('data-numbered', 'true')
  await ctx.close()
})

test('a reader sorts a table by a column; the header stays in view', async ({ browser }) => {
  const ctx = await browser.newContext({ storageState: ownerState })
  const page = await ctx.newPage()
  await page.goto('/')
  const reader = { email: 'rosa@e2e.test', password: 'reader-pass-1' }
  const id = await page.evaluate(async (reader) => {
    const json = { 'content-type': 'application/json', 'x-wikicraft': '1' }
    await fetch('/api/users', { method: 'POST', headers: json, body: JSON.stringify({ ...reader, name: 'Rosa Reader', role: 'member' }) })
    const cell = (t: string, kind = 'tableCell') => ({ type: kind, content: [{ type: 'paragraph', content: [{ type: 'text', text: t }] }] })
    const rows = [{ type: 'tableRow', content: [cell('Office', 'tableHeader'), cell('People', 'tableHeader')] }]
    const data: [string, string][] = [['Bern', '40'], ['Aarau', '9'], ['Chur', '120'], ...Array.from({ length: 30 }, (_, i) => [`Zone ${i + 10}`, String(200 + i)] as [string, string])]
    for (const [o, n] of data) rows.push({ type: 'tableRow', content: [cell(o), cell(n)] })
    const r = await fetch('/api/pages', { method: 'POST', headers: json, body: JSON.stringify({ spaceKey: 'PRODUCT', title: 'Offices', content: { type: 'doc', content: [{ type: 'table', content: rows }] } }) })
    const pid = (await r.json()).id as string
    // only the owner edits this page: everyone else reads it
    const me = await (await fetch('/api/me')).json()
    await fetch(`/api/pages/${pid}/restrictions`, { method: 'PUT', headers: json, body: JSON.stringify({ restrictions: [{ kind: 'edit', subjectType: 'user', subjectId: me.user.id }] }) })
    return pid
  }, reader)
  await ctx.close()

  const p = await login(browser, reader)
  await p.goto(`/pages/${id}`)
  await expect(p.locator('.ProseMirror[contenteditable="false"] .tableWrapper[data-sortable]')).toBeVisible({ timeout: 20_000 })
  const column = (i: number) => p.locator('.tableWrapper > table:visible tbody tr td:nth-child(' + (i + 1) + ')')
  await p.locator('th', { hasText: 'People' }).click()
  await expect(column(1).first()).toHaveText('9')
  await expect(p.locator('.table-sorted-copy th', { hasText: 'People' })).toHaveAttribute('aria-sort', 'ascending')
  await p.locator('.table-sorted-copy th', { hasText: 'People' }).click()
  await expect(column(1).first()).toHaveText('229')
  await p.locator('.table-sorted-copy th', { hasText: 'Office' }).click()
  await expect(column(0).first()).toHaveText('Aarau')
  // the page itself didn't change
  const stored = await p.evaluate(async (pid) => JSON.stringify((await (await fetch(`/api/pages/${pid}`)).json()).content), id)
  expect(stored.indexOf('Bern')).toBeLessThan(stored.indexOf('Aarau'))
  // scrolled to the end of the table, its header row is still in view below the page header
  await column(0).last().scrollIntoViewIfNeeded()
  const gap = await p.evaluate(() => document.querySelector('.table-sorted-copy th')!.getBoundingClientRect().top - document.querySelector('.page-header')!.getBoundingClientRect().bottom)
  expect(Math.abs(gap)).toBeLessThan(2)
  await p.context().close()
})

test('an imported Roadmap Planner shows its lanes, bars and milestones', async ({ browser }) => {
  const ctx = await browser.newContext({ storageState: ownerState })
  const page = await ctx.newPage()
  await page.goto('/')
  const id = await page.evaluate(async () => {
    const data = {
      title: 'Release plan',
      timeline: { startDate: '2027-01-04 00:00:00', endDate: '2027-07-01 00:00:00', displayOption: 'MONTH' },
      lanes: [
        { title: 'Web', color: { lane: '#f6c342', bar: '#fadb8e', text: '#594300' }, bars: [{ title: 'Search', startDate: '2027-01-01 00:00:00', duration: 2, rowIndex: 0 }, { title: 'Export', startDate: '2027-02-01 00:00:00', duration: 1.5, rowIndex: 1 }] },
        { title: 'Mobile', color: { lane: '#3b7fc4', bar: '#6c9fd3', text: '#ffffff' }, bars: [{ title: 'iOS', startDate: '2027-03-01 00:00:00', duration: 3, rowIndex: 0 }] },
      ],
      markers: [{ title: 'Launch', markerDate: '2027-05-15 00:00:00' }],
    }
    // as Confluence stores it: URL-encoded JSON in `source`
    const macro = { type: 'macro', attrs: { name: 'roadmap', params: { timeline: 'true', source: encodeURIComponent(JSON.stringify(data)) } } }
    const r = await fetch('/api/pages', { method: 'POST', headers: { 'content-type': 'application/json', 'x-wikicraft': '1' }, body: JSON.stringify({ spaceKey: 'PRODUCT', title: 'Release roadmap', content: { type: 'doc', content: [macro] } }) })
    return (await r.json()).id as string
  })
  await page.goto(`/pages/${id}`)
  const roadmap = page.locator('.live-macro[data-macro="roadmap"]')
  await expect(roadmap.locator('.roadmap-bar')).toHaveCount(3, { timeout: 20_000 })
  await expect(roadmap).toContainText('Release plan')
  await expect(roadmap.locator('.roadmap-lane-title')).toHaveText(['Web', 'Mobile'])
  await expect(roadmap.locator('.roadmap-marker')).toContainText('Launch')
  // bars sit where their dates are: "Export" starts a month after "Search", one row lower
  const [search, exp] = [await roadmap.locator('.roadmap-bar', { hasText: 'Search' }).boundingBox(), await roadmap.locator('.roadmap-bar', { hasText: 'Export' }).boundingBox()]
  expect(exp!.x - search!.x).toBeGreaterThan(80)
  expect(exp!.y).toBeGreaterThan(search!.y)
  await ctx.close()
})

test('blocks changed since your last visit are highlighted', async ({ browser }) => {
  const ctx = await browser.newContext({ storageState: ownerState })
  const page = await ctx.newPage()
  await page.goto('/')
  const erin = { email: 'erin@e2e.test', password: 'password-456' }
  const id = await page.evaluate(async (who) => {
    const h = { 'content-type': 'application/json', 'x-wikicraft': '1' }
    await fetch('/api/users', { method: 'POST', headers: h, body: JSON.stringify({ ...who, name: 'Erin Editor', role: 'member' }) })
    const r = await fetch('/api/pages', { method: 'POST', headers: h, body: JSON.stringify({ spaceKey: 'PRODUCT', title: 'House rules', markdown: 'Rule one\n\nRule two' }) })
    const pid = (await r.json()).id as string
    // the owner has seen this version
    await fetch(`/api/pages/${pid}`)
    return pid
  }, erin)
  const other = await login(browser, erin)
  const st = await other.evaluate(async (pid) => {
    const r = await fetch(`/api/pages/${pid}/content`, { method: 'PUT', headers: { 'content-type': 'application/json', 'x-wikicraft': '1' }, body: JSON.stringify({ markdown: 'Rule one\n\nRule two, revised' }) })
    return r.status
  }, id)
  expect(st).toBe(200)
  await other.context().close()

  await page.goto(`/pages/${id}`)
  await expect(page.locator('.changed-banner')).toContainText('Changed since your last visit')
  await expect(page.locator('.ProseMirror .changed-block')).toHaveText('Rule two, revised', { timeout: 20_000 })
  await page.getByRole('button', { name: 'Hide highlights' }).click()
  await expect(page.locator('.ProseMirror .changed-block')).toHaveCount(0)
  await ctx.close()
})

test('page properties: typed values in a panel, and a properties report that filters by them', async ({ browser }) => {
  const ctx = await browser.newContext({ storageState: ownerState })
  const page = await ctx.newPage()
  await page.goto('/')
  const [launch, overview] = await page.evaluate(async () => {
    const h = { 'content-type': 'application/json', 'x-wikicraft': '1' }
    const create = async (title: string) => (await (await fetch('/api/pages', { method: 'POST', headers: h, body: JSON.stringify({ spaceKey: 'PRODUCT', title, markdown: title }) })).json()).id as string
    const hiring = await create('Hiring plan')
    await fetch(`/api/pages/${hiring}/properties`, { method: 'PUT', headers: h, body: JSON.stringify({ properties: [{ key: 'Status', type: 'select', value: 'Open' }, { key: 'Effort', type: 'number', value: 2 }] }) })
    return [await create('Launch plan'), await create('Plans overview')]
  })
  await page.goto(`/pages/${launch}`)
  await page.getByRole('button', { name: 'More' }).click()
  await page.getByText('Add property').click()
  // a name already used in the space brings its type and values along
  await page.getByLabel('Property name').fill('Status')
  await expect(page.getByLabel('Property type')).toHaveValue('select')
  await page.getByLabel('Value').fill('Done')
  await page.getByRole('button', { name: 'Save' }).click()
  const props = page.getByRole('table', { name: 'Page properties' })
  await expect(props.locator('.prop-chip')).toHaveText('Done')
  await props.getByRole('button', { name: 'Add property' }).click()
  await page.getByLabel('Property name').fill('Effort')
  await page.getByLabel('Property type').selectOption('number')
  await page.getByLabel('Value').fill('5')
  await page.getByLabel('Value').press('Enter')
  await expect(props.getByRole('row')).toHaveCount(2)
  await expect(props).toContainText(/Effort\s*5/)

  await page.goto(`/pages/${overview}`)
  const editor = page.locator('.ProseMirror[contenteditable="true"]')
  await expect(editor).toBeVisible({ timeout: 20_000 })
  await editor.click()
  await page.keyboard.press('End')
  await page.keyboard.press('Enter')
  await page.keyboard.type('/properties')
  await page.keyboard.press('Enter')
  const report = page.locator('[data-macro="detailssummary"]')
  await expect(report.locator('tbody')).toContainText('Hiring plan')
  await expect(report.locator('tbody')).toContainText('Launch plan')
  await report.getByRole('button', { name: 'Filter and columns' }).click()
  await report.getByPlaceholder('Status = Done').fill('Effort >= 3')
  await report.getByRole('button', { name: 'Apply' }).click()
  await expect(report.locator('tbody tr')).toHaveCount(1)
  await expect(report.locator('tbody')).toContainText('Launch plan')
  await ctx.close()
})

test('workspace accent colour and date format, and a space with its own colour', async ({ browser }) => {
  const ctx = await browser.newContext({ storageState: ownerState })
  const page = await ctx.newPage()
  const accent = () => page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue('--accent').trim())
  await page.goto('/admin')
  await page.locator('#ws-accent').fill('#0e7c66')
  await page.locator('.card', { hasText: 'Look and dates' }).getByRole('button', { name: 'Save' }).click()
  await expect.poll(accent).toBe('#0e7c66')
  await page.locator('#ws-datefmt').selectOption('dd.mm.yyyy')
  await expect(page.locator('.card', { hasText: 'Look and dates' })).toContainText(/For example 05\.03\.\d{4}/)

  // a space's own colour wins inside it; its sidebar can leave out the blog
  const h = { 'content-type': 'application/json', 'x-wikicraft': '1' }
  await page.evaluate(async (headers) => {
    await fetch('/api/spaces/PRODUCT', { method: 'PATCH', headers, body: JSON.stringify({ look: { accentColor: '#aa3300', hideBlog: true } }) })
  }, h)
  await page.goto(pageUrl)
  await expect.poll(accent).toBe('#aa3300')
  await expect(page.locator('.sidebar').getByRole('link', { name: 'Blog' })).toHaveCount(0)
  await page.goto('/')
  await expect.poll(accent).toBe('#0e7c66')

  // a pale colour is darkened until text keeps AA contrast
  await page.evaluate(async (headers) => {
    await fetch('/api/admin/workspace', { method: 'PUT', headers, body: JSON.stringify({ accentColor: '#ffd400' }) })
  }, h)
  await page.goto('/')
  await expect.poll(accent).not.toBe('#0e7c66')
  const { violations } = await new AxeBuilder({ page }).withRules(['color-contrast']).analyze()
  expect(violations.map((v) => v.nodes.map((n) => n.target.join(' ')).join(', '))).toEqual([])

  await page.evaluate(async (headers) => {
    await fetch('/api/admin/workspace', { method: 'PUT', headers, body: JSON.stringify({ accentColor: '', dateFormat: 'auto' }) })
    await fetch('/api/spaces/PRODUCT', { method: 'PATCH', headers, body: JSON.stringify({ look: {} }) })
  }, h)
  await ctx.close()
})

test('a page saved as a template for every space, managed in Administration → Templates', async ({ browser }) => {
  const ctx = await browser.newContext({ storageState: ownerState })
  const page = await ctx.newPage()
  await page.goto(pageUrl)
  await page.getByRole('button', { name: 'More' }).click()
  await page.getByText('Save as template').click()
  const dialog = page.getByRole('dialog')
  await dialog.getByLabel('Name').fill('Release checklist')
  await dialog.getByLabel('Description').fill('Before every release')
  await dialog.getByLabel('Every space').check()
  await dialog.getByRole('button', { name: 'Save template' }).click()
  await expect(dialog).toHaveCount(0)
  await page.goto('/admin/templates')
  const row = page.locator('li', { hasText: 'Release checklist' })
  await expect(row).toContainText('Before every release')
  await row.getByRole('button', { name: 'Edit Release checklist' }).click()
  await page.getByLabel('Name').fill('Release checklist v2')
  await page.getByRole('dialog').getByRole('button', { name: 'Save' }).click()
  await expect(page.locator('li', { hasText: 'Release checklist v2' })).toBeVisible()
  page.once('dialog', (d) => d.accept())
  await page.getByRole('button', { name: 'Delete Release checklist v2' }).click()
  await expect(page.getByText('None yet.')).toBeVisible()
  await ctx.close()
})

test('the high-contrast theme meets WCAG AAA contrast (7:1) on the main screens', async ({ browser }) => {
  const ctx = await browser.newContext({ storageState: ownerState })
  await ctx.addInitScript(() => localStorage.setItem('wc-theme', 'contrast'))
  const page = await ctx.newPage()
  const problems: string[] = []
  for (const url of ['/', pageUrl, '/spaces', '/people', '/settings']) {
    await page.goto(url)
    if (url === pageUrl) await expect(page.locator('.ProseMirror')).toContainText('iOS app', { timeout: 20_000 })
    else await expect(page.locator('.sidebar')).toBeVisible()
    await page.waitForLoadState('networkidle').catch(() => {})
    expect(await page.evaluate(() => document.documentElement.dataset.theme)).toBe('contrast')
    const { violations } = await new AxeBuilder({ page }).withRules(['color-contrast-enhanced']).analyze()
    for (const v of violations) {
      problems.push(`${url}: ${v.nodes.length} × ${v.id}\n${v.nodes.slice(0, 6).map((n) => `    ${n.target.join(' ')}: ${(n.failureSummary ?? '').split('\n').slice(1, 2).join(' ')}`).join('\n')}`)
    }
  }
  await ctx.close()
  expect(problems.length, `contrast below 7:1:\n\n${problems.join('\n\n')}\n`).toBe(0)
})

test('main screens have no serious accessibility violations (axe, WCAG 2.2 AA)', async ({ browser }) => {
  const problems: string[] = []
  const audit = async (p: Page, name: string) => {
    await p.waitForLoadState('networkidle').catch(() => {})
    const { violations } = await new AxeBuilder({ page: p })
      .withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa'])
      .analyze()
    for (const v of violations.filter((v) => v.impact === 'critical' || v.impact === 'serious')) {
      const nodes = v.nodes.map((n) => `    ${n.target.join(' ')}\n      ${(n.failureSummary ?? '').replace(/\n/g, '\n      ')}`)
      problems.push(`${name}: [${v.impact}] ${v.id}: ${v.help}\n  ${v.helpUrl}\n${nodes.join('\n')}`)
    }
  }
  const ctx = await browser.newContext({ storageState: ownerState })
  const page = await ctx.newPage()
  for (const url of ['/', pageUrl, '/spaces', '/people', '/search?q=mobile', '/settings', '/admin']) {
    await page.goto(url)
    if (url === pageUrl) await expect(page.locator('.ProseMirror')).toContainText('iOS app', { timeout: 20_000 })
    else await expect(page.locator('.sidebar')).toBeVisible()
    await audit(page, url)
  }
  await ctx.close()
  const anon = await browser.newContext()
  const login = await anon.newPage()
  await login.goto('/login')
  await expect(login.getByRole('button', { name: 'Log in' })).toBeVisible()
  await audit(login, '/login')
  await anon.close()
  expect(problems.length, `accessibility violations:\n\n${problems.join('\n\n')}\n`).toBe(0)
})
