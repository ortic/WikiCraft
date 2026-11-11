import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { BrowserRouter } from 'react-router-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import './styles.css'
import { chooseLanguage, loadLanguage, rememberedLanguage } from './i18n'

const queryClient = new QueryClient({
  defaultOptions: { queries: { staleTime: 15_000, refetchOnWindowFocus: false, retry: (n, e: any) => n < 2 && !(e?.status >= 400 && e?.status < 500) } },
})

try {
  const theme = localStorage.getItem('wc-theme')
  if (theme) document.documentElement.dataset.theme = theme
} catch {
  /* storage unavailable */
}

// the language first, then the app: labels defined when modules load are translated too
async function start() {
  await loadLanguage(rememberedLanguage() ?? chooseLanguage())
  const { default: App } = await import('./App')
  createRoot(document.getElementById('root')!).render(
    <StrictMode>
      <QueryClientProvider client={queryClient}>
        <BrowserRouter>
          <App />
        </BrowserRouter>
      </QueryClientProvider>
    </StrictMode>,
  )
}
void start()
