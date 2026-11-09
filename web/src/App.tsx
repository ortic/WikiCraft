import { Navigate, Route, Routes, useLocation } from 'react-router-dom'
import { useQuery } from '@tanstack/react-query'
import { api } from './api'
import { ToastProvider, useMe, Spinner } from './ui'
import Layout from './components/Layout'
import { Login, Setup } from './pages/Auth'
import Home from './pages/Home'
import SpacesPage, { SpaceHome } from './pages/Spaces'
import PageView from './pages/PageView'
import History from './pages/History'
import Missing from './pages/Missing'
import SearchPage from './pages/Search'
import Admin from './pages/Admin'
import SpaceSettings from './pages/SpaceSettings'
import LabelPage from './pages/LabelPage'
import Settings from './pages/Settings'
import Blog from './pages/Blog'

export default function App() {
  const setup = useQuery({ queryKey: ['setup'], queryFn: () => api.get<{ needsSetup: boolean }>('/setup') })
  const me = useMe()
  const loc = useLocation()
  if (setup.isLoading || me.isLoading) {
    return (
      <div className="auth">
        <Spinner />
      </div>
    )
  }
  if (setup.data?.needsSetup) return <Setup />
  const user = me.data?.user
  // anonymous visitors may read public spaces; everything else needs a login
  const publicPath = loc.pathname.startsWith('/pages/') || loc.pathname.startsWith('/spaces/')
  if (!user && loc.pathname === '/login') return <Login />
  if (!user && !publicPath) return <Navigate to={`/login?next=${encodeURIComponent(loc.pathname + loc.search)}`} replace />
  return (
    <ToastProvider>
      <Routes>
        <Route path="/login" element={<Navigate to="/" replace />} />
        <Route element={<Layout />}>
          <Route path="/" element={<Home />} />
          <Route path="/spaces" element={<SpacesPage />} />
          <Route path="/spaces/:key" element={<SpaceHome />} />
          <Route path="/spaces/:key/settings" element={<SpaceSettings />} />
          <Route path="/spaces/:key/blog" element={<Blog />} />
          <Route path="/pages/:id" element={<PageView />} />
          <Route path="/pages/:id/history" element={<History />} />
          <Route path="/missing" element={<Missing />} />
          <Route path="/search" element={<SearchPage />} />
          <Route path="/labels/:name" element={<LabelPage />} />
          <Route path="/admin/*" element={<Admin />} />
          <Route path="/settings" element={<Settings />} />
          <Route path="*" element={<div className="empty">Not found</div>} />
        </Route>
      </Routes>
    </ToastProvider>
  )
}
