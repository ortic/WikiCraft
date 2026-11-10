import { useEffect } from 'react'
import { Navigate, Route, Routes, useLocation } from 'react-router-dom'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import SecuritySettings from './components/SecuritySettings'
import { api, setDateConventions } from './api'
import { applyAccent, applyFavicon } from './look'
import { ToastProvider, useMe, Spinner } from './ui'
import Layout from './components/Layout'
import { AcceptInvite, ForgotPassword, Login, ResetPassword, Setup, Signup } from './pages/Auth'
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
import { PeoplePage, ProfilePage } from './pages/People'
import Blog from './pages/Blog'
import TasksPage from './pages/Tasks'
import PublicPageView from './pages/PublicPage'

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
  // the workspace's look and date conventions (idempotent; before anything renders a date)
  setDateConventions(me.data?.workspace?.dateFormat, me.data?.timeZone || me.data?.workspace?.timeZone)
  applyAccent(me.data?.workspace?.accentColor)
  applyFavicon(me.data?.workspace?.logoVersion)
  const user = me.data?.user
  // anonymous visitors may read public spaces; everything else needs a login
  const publicPath = loc.pathname.startsWith('/pages/') || loc.pathname.startsWith('/spaces/')
  // public share links: no account, no app chrome
  if (loc.pathname.startsWith('/s/'))
    return (
      <Routes>
        <Route path="/s/:token" element={<PublicPageView />} />
        <Route path="/s/:token/:pageId" element={<PublicPageView />} />
      </Routes>
    )
  // account links from e-mails work whether or not someone is signed in in this browser
  if (loc.pathname === '/accept-invite') return <AcceptInvite />
  if (loc.pathname === '/reset-password') return <ResetPassword />
  if (!user && loc.pathname === '/forgot-password') return <ForgotPassword />
  if (!user && loc.pathname === '/signup') return <Signup />
  if (!user && loc.pathname === '/login') return <Login />
  if (!user && !publicPath) return <Navigate to={`/login?next=${encodeURIComponent(loc.pathname + loc.search)}`} replace />
  // the workspace requires two-factor authentication: set it up before anything else
  if (user && me.data?.mustSetUpTwoFactor)
    return (
      <ToastProvider>
        <TwoFactorRequired />
      </ToastProvider>
    )
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
          <Route path="/people" element={<PeoplePage />} />
          <Route path="/people/:id" element={<ProfilePage />} />
          <Route path="/tasks" element={<TasksPage />} />
          <Route path="*" element={<div className="empty">Not found</div>} />
        </Route>
      </Routes>
    </ToastProvider>
  )
}

/** Shown instead of the app until the person sets up two-factor authentication. */
function TwoFactorRequired() {
  const qc = useQueryClient()
  const me = useMe()
  const mfa = useQuery({ queryKey: ['mfa'], queryFn: () => api.get<{ enabled: boolean }>('/me/mfa') })
  useEffect(() => {
    if (mfa.data?.enabled) qc.invalidateQueries({ queryKey: ['me'] })
  }, [mfa.data?.enabled, qc])
  return (
    <div className="auth">
      <div className="auth-card" style={{ maxWidth: 640 }}>
        <h1 style={{ marginTop: 0 }}>Set up two-factor authentication</h1>
        <p className="muted">
          {me.data?.workspace?.name ?? 'This wiki'} asks everyone in your role to protect their account with a second step: a code from an authenticator app
          (for example 1Password, Bitwarden, Google or Microsoft Authenticator). It takes a minute.
        </p>
        <SecuritySettings />
        <p className="small">
          <a
            href="/login"
            onClick={async (e) => {
              e.preventDefault()
              await api.post('/auth/logout')
              location.href = '/login'
            }}
          >
            Sign out
          </a>
        </p>
      </div>
    </div>
  )
}
