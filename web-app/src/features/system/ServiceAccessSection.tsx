import { useEffect, useRef, useState } from 'react'
import { api } from '../../data/api'
import { AdminServiceSession, adminValue } from '../../data/adminServices'
import type { AdminServiceView } from '../../data/adminServices'
import type { AdminService, AdminServiceAction, AdminServicesStatus } from '../../types'
import { t } from '../../i18n'
import { Button } from '../../ui/controls'
import { confirm, toast } from '../../ui/feedback'
import { Card } from '../../ui/primitives'

export default function ServiceAccessSection() {
  const session = useRef<AdminServiceSession | null>(null)
  const [view, setView] = useState<AdminServiceView>({ status: null, receivedAt: 0, loading: true, working: false, failed: false })
  const [clock, setClock] = useState(() => performance.now())
  useEffect(() => {
    const active = new AdminServiceSession({ read: api.adminServices, write: api.adminServiceSet }, next => {
      setView(next); setClock(performance.now())
    })
    session.current = active
    void active.refresh()
    const poll = window.setInterval(() => { if (document.visibilityState === 'visible') void active.refresh() }, 5000)
    const age = window.setInterval(() => setClock(performance.now()), 1000)
    const visible = () => { if (document.visibilityState === 'visible') void active.refresh() }
    document.addEventListener('visibilitychange', visible)
    return () => {
      active.dispose(); session.current = null
      window.clearInterval(poll); window.clearInterval(age)
      document.removeEventListener('visibilitychange', visible)
    }
  }, [])

  function warning(s: AdminServicesStatus, service: AdminService, action: AdminServiceAction, enabled: boolean): string {
    if (action === 'boot') return enabled
      ? t('Enables privileged access after future boots. It does not start the service now. Only enable on a trusted device.')
      : t('Future boots will not start this access service. It does not stop it now. Keep web or physical DevUI access available.')
    if (enabled) return t('Enables privileged access now. Use only on trusted networks and USB hosts. Boot policy is unchanged.')
    const other = service === 'adb' ? s.ssh_running : s.adb_running && s.adb_exposed
    return other
      ? t('May disconnect current sessions immediately. Boot policy is unchanged.')
      : t('May disconnect current sessions and turn off the last ADB/SSH channel. Keep web or physical DevUI access available. Boot policy is unchanged.')
  }
  async function change(service: AdminService, action: AdminServiceAction, enabled: boolean) {
    const active = session.current
    if (!active) return
    const result = await active.change(service, action, enabled, s => confirm({
      title: action === 'boot' ? t('{service} at boot', { service: service.toUpperCase() }) : t('{service} runtime', { service: service.toUpperCase() }),
      body: warning(s, service, action, enabled),
      confirmLabel: enabled ? t('Enable') : t('Disable'),
      danger: !enabled,
    }))
    if (session.current !== active) return
    if (result === 'applied') toast(t('Service state verified.'))
    else if (result === 'stale') toast(t('Service status changed or expired. Review it and confirm again.'), 'err')
    else if (result === 'failed') toast(t('Service change failed. Refresh status; do not retry blindly.'), 'err')
  }

  const s = view.status
  const fresh = !!s && !view.failed && clock >= view.receivedAt && clock - view.receivedAt <= 10_000
  const busy = view.working || !!s?.busy
  function control(service: AdminService, action: AdminServiceAction) {
    const title = action === 'runtime' ? t('{service} now', { service: service.toUpperCase() }) : t('{service} at boot', { service: service.toUpperCase() })
    const available = s && (service === 'adb' ? s.adb_available : s.ssh_available)
    const known = fresh && !!s && available
    const on = known ? adminValue(s!, service, action) : false
    const enabled = known && !busy && !view.loading && !(service === 'adb' && action === 'runtime' && !on && !s!.adb_exposed)
    return <div key={`${service}-${action}`} className="flex min-h-16 items-center justify-between gap-4 border-t border-line/8 py-2 first:border-t-0">
      <div className="min-w-0">
        <p className="text-[13px] font-semibold text-ink">{title}</p>
        <p className="text-[12px] text-ink2">{action === 'runtime' ? t('Runtime only; boot policy is unchanged.') : t('Next boot only; does not start or stop it now.')}</p>
      </div>
      {known ? <button type="button" role="switch" aria-label={title} aria-checked={on} disabled={!enabled}
        onClick={() => void change(service, action, !on)}
        className="inline-flex min-h-11 min-w-20 shrink-0 items-center justify-end gap-2 rounded-lg px-1 text-[12px] font-semibold disabled:opacity-40">
        {on ? t('On') : t('Off')}
        <span aria-hidden="true" className={`relative inline-flex h-6 w-10 items-center rounded-full ${on ? 'bg-accent' : 'bg-line/20'}`}>
          <span className={`h-[18px] w-[18px] rounded-full bg-white shadow-sm ${on ? 'translate-x-[21px]' : 'translate-x-[3px]'}`} />
        </span>
      </button> : <span className="shrink-0 text-[12px] text-ink3">{!fresh ? t('Unknown / stale') : t('Unavailable')}</span>}
    </div>
  }
  return <Card title={t('ADB and SSH access')}>
    <p className="mb-2 text-[12px] text-warn">{t('Privileged access: use only trusted networks and USB hosts. Runtime and boot switches are independent.')}</p>
    {control('adb', 'runtime')}{control('adb', 'boot')}{control('ssh', 'runtime')}{control('ssh', 'boot')}
    <p className="mt-2 text-[12px] text-ink2">{t('These controls never change USB composition. ADB needs an existing USB function.')}</p>
    {fresh && s && <p className={`mt-1 text-[12px] ${!s.adb_exposed ? 'text-warn' : 'text-ink2'}`}>
      {t('ADB daemon: {state}; USB exposure: {exposure}.', {
        state: s.adb_running ? t('On') : t('Off'), exposure: s.adb_exposed ? t('Exposed') : t('Not exposed'),
      })}
    </p>}
    <div className="mt-3 flex flex-wrap items-center justify-between gap-2 border-t border-line/8 pt-3">
      <p className={`text-[12px] ${view.failed || !fresh ? 'text-warn' : 'text-ink2'}`} role="status" aria-live="polite">
        {busy ? t('Applying service change…') : view.loading ? t('Reading service status…') : !fresh
          ? t('Service status unavailable / stale. Refresh before changing it.') : t('Service state verified.')}
      </p>
      <Button size="sm" variant="outline" disabled={busy || view.loading} onClick={() => void session.current?.refresh()}>{t('Refresh')}</Button>
    </div>
  </Card>
}
