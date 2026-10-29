/* eslint-disable react-refresh/only-export-components */
import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from 'react'
import { api } from '../../data/api'
import { ApiError } from '../../data/client'
import type { ProxyBackendId, ProxyChoice, ProxyStatus } from '../../types'
import { confirm, toast, toastError } from '../../ui/feedback'
import { t } from '../../i18n'

// The active proxy backend (agent/src/proxy.rs). The Clash and WireGuard views keep their
// own polls and reload when `epoch` changes.

export const BACKEND_NAMES: Record<ProxyBackendId, string> = { wireguard: 'WireGuard', clash: 'Clash', tailscale: t('Tailscale') }

export const backendOf = (s: ProxyStatus, id: ProxyBackendId) => s.backends.find(b => b.id === id)

interface ProxyState {
  status: ProxyStatus | null
  error: string
  /** Where a running switch goes. */
  pending: ProxyChoice | null
  refresh: () => Promise<void>
  /** Asks, then makes `target` the only backend on; true when it switched. */
  switchTo: (target: ProxyChoice) => Promise<boolean>
  /** Counts finished switches. */
  epoch: number
  settingsBusy: boolean
  beginSettingsUpdate: () => boolean
  endSettingsUpdate: () => void
}

const Ctx = createContext<ProxyState | null>(null)

export function useProxy() {
  const value = useContext(Ctx)
  if (!value) throw new Error('useProxy outside ProxyProvider') // i18n-ignore: developer error
  return value
}

/** Every activation is confirmed; the server, not this dialog, enforces exclusivity. */
function ask(s: ProxyStatus, target: ProxyChoice) {
  const activeNames = s.backends.filter(b => b.enabled).map(b => BACKEND_NAMES[b.id]).join(' + ')
  if (target === 'off') {
    return confirm({ title: t('Turn {name} off?', { name: activeNames || t('Proxy') }), body: t('Settings are kept. Forwarded traffic pauses briefly while rules are removed; then all devices use the normal network.'), confirmLabel: t('Turn off') })
  }
  return confirm({ title: t('Activate {name}?', { name: BACKEND_NAMES[target] }), body: t('Only one network backend can be active. Other backends will be turned off, with their settings kept. Devices may briefly lose internet.'), confirmLabel: t('Activate backend') })
}

export function ProxyProvider({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<ProxyStatus | null>(null)
  const [error, setError] = useState('')
  const [pending, setPending] = useState<ProxyChoice | null>(null)
  const [epoch, setEpoch] = useState(0)
  const [settingsBusy, setSettingsBusy] = useState(false)
  const busy = useRef(false)
  const mounted = useRef(true)
  const generation = useRef(0)
  const latest = useRef<ProxyStatus | null>(null)

  const refresh = useCallback(async () => {
    if (busy.current) return
    const current = generation.current
    try {
      const s = await api.proxy()
      if (!mounted.current || busy.current || current !== generation.current) return
      latest.current = s; setStatus(s); setError('')
    } catch (e) {
      // Agents older than the selector answer 404 "not found".
      if (mounted.current && current === generation.current) setError(e instanceof ApiError && e.status === 404 ? t('This agent cannot switch the proxy; deploy the latest agent.') : e instanceof Error ? e.message : t('Unable to load proxy status'))
    }
  }, [])

  useEffect(() => {
    mounted.current = true
    void refresh()
    const timer = window.setInterval(() => { if (!document.hidden) void refresh() }, 5000)
    return () => { mounted.current = false; window.clearInterval(timer) }
  }, [refresh])

  const switchTo = useCallback(async (target: ProxyChoice) => {
    const s = latest.current
    if (!s || busy.current || (target === s.active && !s.conflict)) return false
    if (!(await ask(s, target)) || busy.current) return false
    const revision = (id: ProxyBackendId) => backendOf(s, id)?.revision ?? 0
    generation.current += 1
    busy.current = true; setPending(target)
    let done = false
    try {
      // With both saved as on, the agent turns off the one not chosen at its own revision.
      const next = await api.proxySet(target === 'off'
        ? { backend: target, expected_revision: revision(s.active as ProxyBackendId), active: s.active as ProxyBackendId }
        : { backend: target, expected_revision: revision(target) })
      if (mounted.current) { latest.current = next; setStatus(next); setError('') }
      done = true
      toast(target === 'off' ? t('Proxy off. All devices use the normal network.') : target === 'wireguard' ? t('WireGuard policy applied. Waiting for peer handshake.') : target === 'tailscale' ? t('Tailscale request applied. Check current status.') : t('Clash is on.'))
    } catch (e) {
      if (e instanceof ApiError && e.status === 409) toast(t('Proxy settings changed elsewhere. The latest state is shown; try again.'), 'err')
      else toastError(e, t('Unable to switch proxy'))
    } finally {
      busy.current = false
      if (mounted.current) { setPending(null); setEpoch(n => n + 1) }
    }
    if (!done) await refresh()
    return done
  }, [refresh])

  const beginSettingsUpdate = useCallback(() => {
    if (busy.current) return false
    busy.current = true
    generation.current += 1
    setSettingsBusy(true)
    return true
  }, [])
  const endSettingsUpdate = useCallback(() => {
    busy.current = false
    if (mounted.current) { setSettingsBusy(false); setEpoch(n => n + 1) }
  }, [])

  return <Ctx.Provider value={{ status, error, pending, refresh, switchTo, epoch, settingsBusy, beginSettingsUpdate, endSettingsUpdate }}>{children}</Ctx.Provider>
}
