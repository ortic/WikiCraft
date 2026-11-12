#!/usr/bin/env node
// Fills a fresh WikiCraft with a made-up company's wiki and takes the screenshots for the project
// website (site/img/*.png). Everything goes through the public API, so the pictures show what a
// real installation looks like.
//
//   1. start WikiCraft on an EMPTY database, e.g.
//        WIKICRAFT_DATABASE_URL=postgres://wikicraft:wikicraft@127.0.0.1:55432/wikicraft_site \
//        WIKICRAFT_BIND=127.0.0.1:8796 WIKICRAFT_BASE_URL=http://127.0.0.1:8796 \
//        WIKICRAFT_DATA_DIR=$(mktemp -d) WIKICRAFT_STATIC_DIR=web/dist wikicraft serve
//   2. node scripts/site-screenshots.mjs http://127.0.0.1:8796
//
// Running it again against the same database retakes the pictures (ONLY=page,collab picks some)
// without seeding again, but pages then show "changed since your last visit": use a fresh
// database for the pictures you commit.
// Uses Playwright from e2e/node_modules (cd e2e && npm ci && npx playwright install chromium).
// SITE_FONT=/path/to/inter-latin-wght-normal.woff2 renders the app in Inter, its first-choice font
// (without it, the system's sans-serif). Runs pngquant on the results when it is installed.

import { execFileSync } from 'node:child_process'
import { existsSync, readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { fileURLToPath } from 'node:url'

const require = createRequire(new URL('../e2e/package.json', import.meta.url))
const { chromium, request } = require('@playwright/test')

const BASE = (process.argv[2] ?? 'http://127.0.0.1:8796').replace(/\/$/, '')
const OUT = fileURLToPath(new URL('../site/img/', import.meta.url))
const ONLY = process.env.ONLY?.split(',')
const PASSWORD = 'site-screenshots-1'
const SCALE = 2

// ---------- content helpers (ProseMirror JSON, as the editor stores it) ----------

const txt = (text, ...marks) => ({ type: 'text', text, ...(marks.length ? { marks: marks.map((m) => (typeof m === 'string' ? { type: m } : m)) } : {}) })
const inl = (c) => (typeof c === 'string' ? txt(c) : c)
const p = (...c) => ({ type: 'paragraph', ...(c.length ? { content: c.map(inl) } : {}) })
const h = (level, text) => ({ type: 'heading', attrs: { level }, content: [txt(text)] })
const b = (text) => txt(text, 'bold')
const code = (text) => txt(text, 'code')
const link = (text, href) => txt(text, { type: 'link', attrs: { href } })
const callout = (type, ...c) => ({ type: 'callout', attrs: { type }, content: c.map((x) => (typeof x === 'string' ? p(x) : x)) })
const status = (text, color) => ({ type: 'status', attrs: { text, color } })
const mention = (u) => ({ type: 'mention', attrs: { id: u.id, label: u.name } })
const date = (d) => ({ type: 'date', attrs: { date: d } })
const tasks = (...items) => ({ type: 'taskList', content: items.map(([checked, ...c]) => ({ type: 'taskItem', attrs: { checked }, content: [p(...c)] })) })
const ul = (...items) => ({ type: 'bulletList', content: items.map((i) => ({ type: 'listItem', content: [Array.isArray(i) ? p(...i) : p(i)] })) })
const ol = (...items) => ({ type: 'orderedList', content: items.map((i) => ({ type: 'listItem', content: [Array.isArray(i) ? p(...i) : p(i)] })) })
const codeBlock = (language, text) => ({ type: 'codeBlock', attrs: { language }, content: [txt(text)] })
const cell = (kind, c) => ({ type: kind, content: [Array.isArray(c) ? p(...c) : p(c)] })
const table = (head, rows) => ({
  type: 'table',
  content: [
    { type: 'tableRow', content: head.map((x) => cell('tableHeader', x)) },
    ...rows.map((r) => ({ type: 'tableRow', content: r.map((x) => cell('tableCell', x)) })),
  ],
})
const decisions = (...items) => ({ type: 'decisionList', content: items.map((t) => ({ type: 'decisionItem', attrs: { state: 'DECIDED' }, content: [p(t)] })) })
const expand = (title, ...c) => ({ type: 'details', content: [{ type: 'detailsSummary', content: [txt(title)] }, { type: 'detailsContent', content: c }] })
const columns = (...cols) => ({ type: 'columns', content: cols.map((c) => ({ type: 'column', content: c })) })
const macro = (name, params = {}) => ({ type: 'macro', attrs: { name, params } })
const doc = (...content) => ({ type: 'doc', content })

// ---------- API ----------

let api
async function call(method, path, body) {
  const r = await api.fetch(`/api${path}`, { method, data: body, headers: { 'x-wikicraft': '1' } })
  if (!r.ok()) throw new Error(`${method} ${path}: ${r.status()} ${await r.text()}`)
  const t = await r.text()
  return t ? JSON.parse(t) : null
}
const post = (path, body) => call('POST', path, body)
const put = (path, body) => call('PUT', path, body)
const patch = (path, body) => call('PATCH', path, body)

async function page(spaceKey, title, content, extra = {}) {
  const { parentId, kind, status: pageStatus, cover, icon, properties, labels } = extra
  const r = await post('/pages', { spaceKey, title, parentId, kind, ...(content ? { content } : { blank: true }) })
  if (pageStatus || cover || icon) await patch(`/pages/${r.id}`, { pageStatus, cover, icon })
  if (properties) await put(`/pages/${r.id}/properties`, { properties })
  if (labels) await put(`/pages/${r.id}/labels`, { labels })
  return r.id
}

// ---------- the made-up workspace ----------

const OWNER = { name: 'Olivia Novak', email: 'olivia@acme.test' }
const PEOPLE = [
  { name: 'Mia Chen', email: 'mia@acme.test' },
  { name: 'Ben Ortiz', email: 'ben@acme.test' },
  { name: 'Sara Lind', email: 'sara@acme.test' },
  { name: 'Jonas Weber', email: 'jonas@acme.test' },
  { name: 'Priya Raman', email: 'priya@acme.test' },
  { name: 'Tom Dubois', email: 'tom@acme.test' },
]

const ids = {}

async function seed() {
  await post('/setup', { workspaceName: 'Acme', ...OWNER, password: PASSWORD })
  await post('/auth/login', { email: OWNER.email, password: PASSWORD })
  OWNER.id = (await call('GET', '/me')).user.id
  for (const u of PEOPLE) u.id = (await post('/users', { ...u, password: PASSWORD, role: 'member' })).id
  const [mia, ben, sara, jonas, priya, tom] = PEOPLE

  await post('/spaces', { key: 'ENG', name: 'Engineering', description: 'Architecture, runbooks and how we build software.', icon: 'code:blue' })
  await post('/spaces', { key: 'PRODUCT', name: 'Product', description: 'Roadmap, specs and release notes.', icon: 'rocket:violet' })
  await post('/spaces', { key: 'PEOPLE', name: 'People & Culture', description: 'Handbook, onboarding and benefits.', icon: 'people:amber' })
  await post('/spaces', { key: 'SUPPORT', name: 'Customer Support', description: 'Help-desk playbooks and known issues.', icon: 'headset:teal' })

  // Engineering
  const arch = await page('ENG', 'Architecture', doc(
    p('How the platform fits together. Each service owns its data; they talk over the event bus.'),
    codeBlock('mermaid', 'flowchart LR\n  web[Web app] --> api[API gateway]\n  api --> pay[Payments]\n  api --> cat[Catalog]\n  pay --> bus[(Event bus)]\n  cat --> bus\n  bus --> mail[Notifications]\n  pay --> db[(Postgres)]'),
    h(2, 'Services'),
    table(['Service', 'Owner', 'Status'], [
      ['Payments', [mention(mia)], [status('Live', 'green')]],
      ['Catalog', [mention(ben)], [status('Live', 'green')]],
      ['Notifications', [mention(jonas)], [status('Migrating', 'yellow')]],
    ]),
  ), { labels: ['architecture'] })
  const runbooks = await page('ENG', 'Runbooks', null, { kind: 'folder' })
  ids.runbook = await page('ENG', 'Payment service runbook', doc(
    callout('info', 'Paged for failed payments? Start here. The service retries on its own for five minutes before it alerts.'),
    h(2, 'Quick checks'),
    tasks(
      [true, 'Check the provider status page'],
      [false, 'Compare the error rate with last week ', mention(ben), ' ', date('2026-11-19')],
      [false, 'Post an update in ', code('#payments-incidents')],
    ),
    h(2, 'Escalation'),
    table(['Severity', 'Who', 'Respond within'], [
      [[status('SEV1', 'red')], ['On-call and ', mention(mia)], '15 minutes'],
      [[status('SEV2', 'yellow')], 'On-call', '1 hour'],
      [[status('SEV3', 'blue')], 'Team channel', 'Next business day'],
    ]),
    h(2, 'Restart a worker'),
    codeBlock('bash', 'kubectl -n payments rollout restart deployment/payment-worker\nkubectl -n payments rollout status deployment/payment-worker --timeout=120s'),
    callout('warning', 'Never restart all regions at once: queued payments are only replayed by a running worker.'),
  ), {
    parentId: runbooks, status: 'Ready for review', labels: ['runbook', 'payments'],
    properties: [
      { key: 'Owner', type: 'user', value: mia.id },
      { key: 'Service tier', type: 'select', value: 'Tier 1' },
      { key: 'Last drill', type: 'date', value: '2026-10-28' },
    ],
  })
  ids.failover = await page('ENG', 'Database failover', doc(
    p('Promote the replica when the primary is gone for more than two minutes.'),
    ol('Confirm the primary is unreachable from two zones', ['Promote the replica: ', code('pg_ctl promote')], 'Point the connection pooler at the new primary', 'Open an incident and tell the team'),
  ), {
    parentId: runbooks, labels: ['runbook', 'database'],
    properties: [
      { key: 'Owner', type: 'user', value: ben.id },
      { key: 'Service tier', type: 'select', value: 'Tier 1' },
      { key: 'Last drill', type: 'date', value: '2026-09-14' },
    ],
  })
  await page('ENG', 'Incident response', doc(p('Who does what when something breaks.'), ul('Incident lead runs the call', 'Scribe keeps the timeline', 'Comms lead updates the status page')), {
    parentId: runbooks, labels: ['runbook'],
    properties: [
      { key: 'Owner', type: 'user', value: jonas.id },
      { key: 'Service tier', type: 'select', value: 'Tier 2' },
      { key: 'Last drill', type: 'date', value: '2026-11-03' },
    ],
  })
  ids.runbookIndex = await page('ENG', 'Runbook overview', doc(
    p('Every runbook, its owner and when we last practised it. The table fills itself from the pages\' properties.'),
    macro('detailssummary', { spaces: '@self', labels: 'runbook' }),
  ), { parentId: runbooks })
  ids.onboarding = await page('ENG', 'Engineering onboarding', doc(
    p('Welcome! Your first week, step by step.'),
    tasks([true, 'Get laptop and accounts'], [true, 'Read the ', link('architecture overview', `/wiki/ENG/architecture`)], [false, 'Pair with your buddy on a small bug'], [false, 'Ship something to production']),
  ))
  ids.decisions = await page('ENG', 'Decision log', doc(
    p('Architecture decisions, newest first.'),
    decisions('Use Postgres logical replication for the reporting database', 'Retire the legacy SOAP endpoints by March 2027', 'All new services ship with OpenTelemetry tracing'),
  ), { labels: ['decision'] })

  // Product
  const roadmapData = {
    title: 'Release plan 2027',
    timeline: { startDate: '2027-01-04 00:00:00', endDate: '2027-09-30 00:00:00', displayOption: 'MONTH' },
    lanes: [
      { title: 'Web', color: { lane: '#6366f1', bar: '#c7d2fe', text: '#1e1b4b' }, bars: [
        { title: 'New editor toolbar', startDate: '2027-01-11 00:00:00', duration: 2, rowIndex: 0 },
        { title: 'Search filters', startDate: '2027-02-15 00:00:00', duration: 2.5, rowIndex: 1 },
        { title: 'Offline reading', startDate: '2027-05-03 00:00:00', duration: 3, rowIndex: 0 },
      ] },
      { title: 'Mobile', color: { lane: '#0d9488', bar: '#99f6e4', text: '#042f2e' }, bars: [
        { title: 'iOS beta', startDate: '2027-02-01 00:00:00', duration: 3, rowIndex: 0 },
        { title: 'Android beta', startDate: '2027-04-05 00:00:00', duration: 3, rowIndex: 1 },
      ] },
      { title: 'Platform', color: { lane: '#d97706', bar: '#fde68a', text: '#451a03' }, bars: [
        { title: 'EU data region', startDate: '2027-01-18 00:00:00', duration: 4, rowIndex: 0 },
        { title: 'SSO for guests', startDate: '2027-06-01 00:00:00', duration: 2, rowIndex: 1 },
      ] },
    ],
    markers: [{ title: 'Public launch', markerDate: '2027-06-15 00:00:00' }],
  }
  ids.roadmap = await page('PRODUCT', 'Roadmap 2027', doc(
    p('What we plan to ship next year, by team. Dates are targets, not promises.'),
    { type: 'macro', attrs: { name: 'roadmap', params: { timeline: 'true', source: encodeURIComponent(JSON.stringify(roadmapData)) } } },
    h(2, 'Open questions'),
    tasks([false, 'Confirm the EU region launch date with legal ', mention(sara)], [false, 'Decide on the Android beta size ', mention(priya)]),
  ), { labels: ['roadmap'] })
  const pricing = (v2) => doc(
    p('How we price the hosted plans. Sales and support quote from this page.'),
    h(2, 'Plans'),
    p(v2 ? 'Plans start at twelve euros per seat and month. Volume discounts start at 50 seats.' : 'Plans start at ten euros per seat and month. Volume discounts start at 100 seats.'),
    p(v2 ? 'Monthly or annual billing. Customers can cancel at the end of each term.' : 'Annual billing only. Customers can cancel at the end of each term.'),
    ...(v2 ? [p('Non-profits get the first 25 seats free.')] : []),
    h(2, 'Trials'),
    p(v2 ? 'Every plan starts with a 30-day trial. No credit card needed.' : 'Every plan starts with a 14-day trial. No credit card needed.'),
    p('Trials keep their content when they convert to a paid plan.'),
    h(2, 'Discounts'),
    p('Education customers get 40 percent off the list price.'),
    p(v2 ? 'Sales may offer up to 15 percent on three-year contracts; anything more needs approval from finance.' : 'Sales may offer up to 10 percent on two-year contracts.'),
  )
  ids.pricing = await page('PRODUCT', 'Pricing', pricing(false))
  await put(`/pages/${ids.pricing}/content`, { content: pricing(true) })
  ids.offsite = await page('PEOPLE', 'Team offsite 2027', doc(
    p('Three days in the mountains with the whole company. Everything we need to plan, in one place.'),
    h(2, 'Agenda'),
    p(),
  ))
  ids.spec = await page('PRODUCT', 'Spec: guest access', doc(
    callout('note', 'Draft. Comments welcome until Friday.'),
    h(2, 'Problem'),
    p('Partners need to read a handful of pages without getting a full account. Today we export PDFs and e-mail them, which goes out of date within days.'),
    h(2, 'Proposal'),
    p('Invite guests into a single space. Guests see only that space, cannot search the rest of the wiki and lose access automatically after 90 days.'),
    h(2, 'Out of scope'),
    ul('Paid guest seats', 'Guests creating spaces'),
  ), { status: 'In progress', labels: ['spec'] })
  for (const [title, text] of [
    ['Release 4.2: faster search', 'Search results now appear while you type, and filters remember your last choice.'],
    ['Release 4.1: dark mode everywhere', 'Every screen, including exports and e-mails, now follows your theme.'],
  ]) await page('PRODUCT', title, doc(p(text)), { kind: 'blog' })

  // People & Culture
  await page('PEOPLE', 'Team handbook', doc(
    columns(
      [h(3, 'How we work'), ul('Write things down', 'Default to open', 'Decide, then commit')],
      [h(3, 'Time off'), p('30 days per year, plus public holidays. Plan with your team and add it to the calendar.')],
    ),
    expand('Expenses', p('Submit receipts within 30 days. Anything over 500 euros needs your lead\'s approval first.')),
  ), { cover: 'gradient:sunrise' })

  // Customer Support
  await page('SUPPORT', 'Refund requests', doc(p('Refunds within 14 days need no approval. After that, ask ', mention(tom), '.')), { labels: ['playbook'] })

  // comments, an inline one included
  const c1 = await post(`/pages/${ids.spec}/comments`, { text: `Should guests see page history? @[${mia.name}](user:${mia.id}) what do you think?` })
  ids.inline = await post(`/pages/${ids.spec}/comments`, { text: 'Can we make the 90 days configurable per space?', inline: { text: 'lose access automatically after 90 days' } })
  const specContent = (await call('GET', `/pages/${ids.spec}`)).content
  for (const n of specContent.content) {
    if (n.type !== 'paragraph' || !n.content) continue
    n.content = n.content.flatMap((t) => {
      const target = 'lose access automatically after 90 days'
      if (t.type !== 'text' || !t.text.includes(target)) return [t]
      const [before, after] = t.text.split(target)
      return [txt(before), txt(target, { type: 'comment', attrs: { commentId: ids.inline.id } }), txt(after)]
    })
  }
  await put(`/pages/${ids.spec}/content`, { content: specContent })
  for (const [who, text] of [[ben, 'Yes, but only versions after they were invited.'], [mia, 'Agreed. Older versions may mention other customers.']]) {
    const ctx = await request.newContext({ baseURL: BASE })
    await ctx.fetch('/api/auth/login', { method: 'POST', data: { email: who.email, password: PASSWORD }, headers: { 'x-wikicraft': '1' } })
    await ctx.fetch(`/api/pages/${ids.spec}/comments`, { method: 'POST', data: { text, parentId: c1.id }, headers: { 'x-wikicraft': '1' } })
    await ctx.dispose()
  }
  await post(`/pages/${ids.runbook}/comments`, { text: 'Added the restart steps from last week\'s drill.' })

}

// a second run (to retake pictures) finds the pages of the first one
async function lookup() {
  await post('/auth/login', { email: OWNER.email, password: PASSWORD })
  const find = async (space, title) => (await call('GET', `/resolve?space=${space}&title=${encodeURIComponent(title)}`)).pageId
  ids.runbook = await find('ENG', 'Payment service runbook')
  ids.runbookIndex = await find('ENG', 'Runbook overview')
  ids.onboarding = await find('ENG', 'Engineering onboarding')
  ids.offsite = await find('PEOPLE', 'Team offsite 2027')
  ids.roadmap = await find('PRODUCT', 'Roadmap 2027')
  ids.pricing = await find('PRODUCT', 'Pricing')
  ids.spec = await find('PRODUCT', 'Spec: guest access')
  ids.inline = (await call('GET', `/pages/${ids.spec}/comments`)).find((c) => c.inline)
  // search needs the index to catch up
  for (let i = 0; i < 40; i++) {
    const hits = await call('GET', '/search?q=failover')
    if (hits.length) break
    await new Promise((r) => setTimeout(r, 500))
  }
}

// ---------- screenshots ----------

const font = process.env.SITE_FONT && existsSync(process.env.SITE_FONT) ? readFileSync(process.env.SITE_FONT) : null

async function context(browser, { user = OWNER, theme = 'light', width = 1440, height = 900, mobile = false } = {}) {
  const ctx = await browser.newContext({
    baseURL: BASE,
    viewport: { width, height },
    deviceScaleFactor: SCALE,
    isMobile: mobile,
    hasTouch: mobile,
    colorScheme: theme === 'dark' ? 'dark' : 'light',
    reducedMotion: 'reduce',
  })
  await ctx.addInitScript((t) => {
    try {
      localStorage.setItem('wc-theme', t)
    } catch {}
  }, theme)
  if (font) {
    await ctx.route('**/__site-font/inter.woff2', (r) => r.fulfill({ body: font, contentType: 'font/woff2' }))
    await ctx.addInitScript(() => {
      const add = () => {
        const s = document.createElement('style')
        s.textContent = "@font-face{font-family:'Inter';src:url('/__site-font/inter.woff2') format('woff2');font-weight:100 900;font-display:block}"
        document.head.append(s)
      }
      if (document.head) add()
      else document.addEventListener('DOMContentLoaded', add)
    })
  }
  const r = await ctx.request.fetch('/api/auth/login', { method: 'POST', data: { email: user.email, password: PASSWORD }, headers: { 'x-wikicraft': '1' } })
  if (!r.ok()) throw new Error(`login ${user.email}: ${r.status()}`)
  return ctx
}

async function settle(pg) {
  await pg.waitForLoadState('networkidle').catch(() => {})
  // the hover-only block handles stay where the last click was
  await pg.addStyleTag({ content: '.drag-handle, .block-link-float { visibility: hidden !important; }' })
  await pg.evaluate(() => document.fonts.ready)
  await pg.mouse.move(1, 1)
  await pg.waitForTimeout(600)
}

const saved = []
async function shot(pg, name, opts = {}) {
  const file = `${OUT}${name}.png`
  saved.push(file)
  await pg.screenshot({ path: file, ...opts })
  console.log('saved', file)
}

const SHOTS = {
  // the hero: a runbook with properties, a callout, tasks, a table and a code block
  async page(browser) {
    const ctx = await context(browser)
    const pg = await ctx.newPage()
    await pg.goto(`/pages/${ids.runbook}`)
    await pg.locator('.ProseMirror').first().waitFor()
    await settle(pg)
    await shot(pg, 'page')
    await ctx.close()
  },
  async dark(browser) {
    const ctx = await context(browser, { theme: 'dark' })
    const pg = await ctx.newPage()
    await pg.goto(`/pages/${ids.roadmap}`)
    await pg.locator('.roadmap-bar').first().waitFor({ timeout: 20_000 })
    await settle(pg)
    await shot(pg, 'roadmap-dark', { clip: { x: 420, y: 130, width: 880, height: 700 } })
    await ctx.close()
  },
  // two people in one page: Ben's caret and name show up in Olivia's editor
  async collab(browser) {
    const a = await context(browser)
    const bctx = await context(browser, { user: PEOPLE[1] })
    const pa = await a.newPage()
    const pb = await bctx.newPage()
    await pa.goto(`/pages/${ids.onboarding}`)
    await pb.goto(`/pages/${ids.onboarding}`)
    const eb = pb.locator('.ProseMirror[contenteditable="true"]')
    await eb.waitFor({ timeout: 20_000 })
    await pa.locator('.ProseMirror[contenteditable="true"]').waitFor({ timeout: 20_000 })
    await eb.locator('p').first().click()
    await pb.keyboard.press('End')
    const typed = ' Ask anything in #eng-help'
    await pb.keyboard.type(typed, { delay: 30 })
    await pa.locator('[class*="collaboration-carets"]').first().waitFor({ timeout: 20_000 })
    await settle(pa)
    await shot(pa, 'collab', { clip: { x: 420, y: 48, width: 1020, height: 600 } })
    // leave the page as it was, for the next run
    for (const _ of typed) await pb.keyboard.press('Backspace')
    await pb.waitForTimeout(1500)
    await a.close()
    await bctx.close()
  },
  async comments(browser) {
    const ctx = await context(browser)
    const pg = await ctx.newPage()
    await pg.goto(`/pages/${ids.spec}`)
    await pg.locator('.ProseMirror').first().waitFor()
    await pg.locator(`[data-comment-id="${ids.inline.id}"]`).first().click()
    await pg.waitForTimeout(500)
    await pg.evaluate(() => document.querySelectorAll('*').forEach((el) => { if (el.scrollTop > 0 && !el.closest('.comments-panel, aside')) el.scrollTop = 0 }))
    await settle(pg)
    await shot(pg, 'comments', { clip: { x: 290, y: 48, width: 1150, height: 800 } })
    await ctx.close()
  },
  async history(browser) {
    const ctx = await context(browser)
    const pg = await ctx.newPage()
    await pg.goto(`/pages/${ids.pricing}/history`)
    await pg.locator('.diff-line.changed').first().waitFor({ timeout: 20_000 })
    await settle(pg)
    await shot(pg, 'history', { clip: { x: 330, y: 50, width: 1110, height: 560 } })
    await ctx.close()
  },
  async search(browser) {
    const ctx = await context(browser)
    const pg = await ctx.newPage()
    await pg.goto('/search?q=runbook')
    await pg.locator('main').getByText('Payment service runbook').first().waitFor({ timeout: 20_000 })
    await settle(pg)
    await shot(pg, 'search', { clip: { x: 330, y: 50, width: 1110, height: 600 } })
    await ctx.close()
  },
  async report(browser) {
    const ctx = await context(browser)
    const pg = await ctx.newPage()
    await pg.goto(`/pages/${ids.runbookIndex}`)
    await pg.locator('.live-macro').first().getByText('Database failover').waitFor({ timeout: 20_000 })
    await settle(pg)
    await shot(pg, 'report', { clip: { x: 420, y: 140, width: 880, height: 500 } })
    await ctx.close()
  },
  async slash(browser) {
    const ctx = await context(browser)
    const pg = await ctx.newPage()
    await pg.goto(`/pages/${ids.offsite}`)
    const ed = pg.locator('.ProseMirror[contenteditable="true"]')
    await ed.waitFor({ timeout: 20_000 })
    await ed.locator('p').last().click()
    await pg.keyboard.type(process.env.SLASH ?? '/', { delay: 40 })
    await pg.waitForTimeout(400)
    await settle(pg)
    await shot(pg, 'editor', { clip: { x: 420, y: 130, width: 880, height: 650 } })
    // leave the page as it was, for the next run
    await pg.keyboard.press('Escape')
    for (const _ of process.env.SLASH ?? '/') await pg.keyboard.press('Backspace')
    await ctx.close()
  },
  async mobile(browser) {
    const ctx = await context(browser, { width: 390, height: 844, mobile: true })
    const pg = await ctx.newPage()
    await pg.goto(`/pages/${ids.runbook}`)
    await pg.locator('.ProseMirror').first().waitFor()
    await settle(pg)
    await shot(pg, 'mobile')
    await ctx.close()
  },
}

async function main() {
  api = await request.newContext({ baseURL: BASE })
  if ((await (await api.fetch('/api/setup')).json()).needsSetup) await seed()
  await lookup()
  const browser = await chromium.launch()
  for (const [name, take] of Object.entries(SHOTS)) {
    if (ONLY && !ONLY.includes(name)) continue
    await take(browser)
  }
  await browser.close()
  await api.dispose()
  try {
    execFileSync('pngquant', ['--force', '--ext', '.png', '--quality', '75-95', '--skip-if-larger', ...saved])
  } catch (e) {
    if (e.code !== 'ENOENT') console.warn('pngquant:', e.message)
  }
}

main().catch((e) => {
  console.error(e)
  process.exit(1)
})
