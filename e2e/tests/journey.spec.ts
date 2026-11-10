// One journey through a fresh WikiCraft: setup, spaces, editing, collaboration, comments,
// search, sharing, export, two-factor sign-in and accessibility. Tests run in order and build
// on each other.

import { createHmac } from 'node:crypto'
import AxeBuilder from '@axe-core/playwright'
import { expect, test, type Browser, type BrowserContext, type Page } from '@playwright/test'

const OWNER = { name: 'Olivia Owner', email: 'olivia@e2e.test', password: 'correct horse battery' }
const MEMBER = { name: 'Max Member', email: 'max@e2e.test', password: 'password-123' }

test.describe.configure({ mode: 'serial' })

let ownerState: Awaited<ReturnType<BrowserContext['storageState']>>
let pageUrl = ''
let pageId = ''

async function login(browser: Browser, who: { email: string; password: string }) {
  const ctx = await browser.newContext()
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
  // the drag handle's link button copies the block's link
  await editor.getByText('Dates are estimates.').hover()
  await page.locator('.drag-handle .block-link').click()
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

test('comments and search', async ({ browser }) => {
  const ctx = await browser.newContext({ storageState: ownerState })
  const page = await ctx.newPage()
  await page.goto(pageUrl)
  await page.getByTitle('Comments').click()
  await page.locator('.side-panel textarea').first().fill('Can we pull the mobile app into Q2?')
  await page.locator('.side-panel .btn-primary').first().click()
  await expect(page.locator('.side-panel')).toContainText('pull the mobile app into Q2')
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
  for (const url of ['/', pageUrl, '/search?q=mobile', '/spaces', '/admin/users', '/settings', `${pageUrl}/history`]) {
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
  await expect(page).toHaveURL(/\/pages\//)
  await expect(page.locator('textarea.page-title')).toHaveValue('Postmortem: Checkout outage', { timeout: 15_000 })
  await ctx.close()
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
  for (const url of ['/', pageUrl, '/spaces', '/search?q=mobile', '/settings', '/admin']) {
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
