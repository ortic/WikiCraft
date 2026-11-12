# Project website

The static page published at <https://ortic.github.io/WikiCraft/> by
`.github/workflows/pages.yml` on every push to `master` that touches `site/`. Plain HTML, CSS and a
little JavaScript; no build step. Preview it with `python3 -m http.server -d site`.

- `img/`: screenshots, taken by `scripts/site-screenshots.mjs` from a freshly seeded WikiCraft
  (see the comment at the top of that script). Retake them after visible UI changes.
- `fonts/`: Inter (SIL Open Font License, `Inter-LICENSE.txt`), served from here so visitors'
  browsers don't contact a font CDN.
