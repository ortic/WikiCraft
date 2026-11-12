// WikiCraft website: header shadow, install tabs, copy buttons, screenshot lightbox.

// a border under the header once the page scrolls
const header = document.getElementById('top')
const onScroll = () => header.classList.toggle('scrolled', window.scrollY > 8)
addEventListener('scroll', onScroll, { passive: true })
onScroll()

// tabs (arrow keys move between them, as in the WAI-ARIA tabs pattern)
const tabs = [...document.querySelectorAll('[role=tab]')]
function select(tab) {
  for (const t of tabs) {
    const on = t === tab
    t.setAttribute('aria-selected', String(on))
    t.tabIndex = on ? 0 : -1
    document.getElementById(t.getAttribute('aria-controls')).hidden = !on
  }
}
for (const tab of tabs) {
  tab.addEventListener('click', () => select(tab))
  tab.addEventListener('keydown', (e) => {
    const i = tabs.indexOf(tab)
    const next = e.key === 'ArrowRight' ? tabs[(i + 1) % tabs.length] : e.key === 'ArrowLeft' ? tabs[(i - 1 + tabs.length) % tabs.length] : null
    if (next) {
      e.preventDefault()
      select(next)
      next.focus()
    }
  })
}

// copy the commands of a code block (comments after "#" stay out)
for (const btn of document.querySelectorAll('.copy')) {
  btn.addEventListener('click', async () => {
    const code = btn.parentElement.querySelector('code').cloneNode(true)
    code.querySelectorAll('.c').forEach((c) => c.remove())
    const text = code.textContent.split('\n').map((l) => l.trimEnd()).filter((l, i, all) => l || (i > 0 && all[i - 1])).join('\n').trim()
    try {
      await navigator.clipboard.writeText(text)
      btn.classList.add('done')
      btn.setAttribute('aria-label', 'Copied')
      setTimeout(() => {
        btn.classList.remove('done')
        btn.setAttribute('aria-label', 'Copy commands')
      }, 1600)
    } catch {
      /* clipboard not available (e.g. plain http) */
    }
  })
}

// screenshots open full size
const box = document.getElementById('lightbox')
const boxImg = box.querySelector('img')
const boxCaption = box.querySelector('p')
for (const btn of document.querySelectorAll('[data-zoom]')) {
  btn.addEventListener('click', () => {
    boxImg.src = btn.dataset.zoom
    boxImg.alt = btn.querySelector('img').alt
    boxCaption.textContent = btn.dataset.caption ?? ''
    box.showModal()
  })
}
box.querySelector('.close').addEventListener('click', () => box.close())
box.addEventListener('click', (e) => {
  if (e.target === box || e.target === boxImg) box.close()
})
