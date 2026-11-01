import { useCallback, useEffect, useState } from 'react'
import { api } from '../../data/api'
import { t } from '../../i18n'
import type { FirmwareLock } from '../../types'
import { Toggle } from '../../ui/controls'
import { confirm, toast, toastError } from '../../ui/feedback'
import { Card, Row } from '../../ui/primitives'

export default function FirmwareLockSection() {
  const [lock, setLock] = useState<FirmwareLock | null>(null)
  const [busy, setBusy] = useState(false)

  const refresh = useCallback(async () => {
    try {
      setLock(await api.firmwareLock())
    } catch {
      setLock(null)
    }
  }, [])

  useEffect(() => {
    void refresh()
    const poll = window.setInterval(() => { if (document.visibilityState === 'visible') void refresh() }, 10_000)
    return () => window.clearInterval(poll)
  }, [refresh])

  async function change(enabled: boolean) {
    if (!lock) return
    if (!enabled && !(await confirm({
      title: t('Turn off the firmware update lock?'),
      body: t('The stock web page, the ZTE app and carrier management can then install firmware again. An update can remove the boot hooks and SSH keys this dashboard depends on.'),
      confirmLabel: t('Turn off'),
      danger: true,
    }))) return
    setBusy(true)
    try {
      setLock(await api.firmwareLockSet({ revision: lock.revision, enabled }))
      toast(enabled ? t('Firmware updates are locked.') : t('Firmware update lock is off.'))
    } catch (e) {
      toastError(e, t('Failed to change the firmware update lock'))
      void refresh()
    } finally {
      setBusy(false)
    }
  }

  const on = !!lock?.enabled
  return <Card title={t('Firmware update lock')}>
    <div className="flex min-h-12 items-center justify-between gap-4">
      <p className="min-w-0 text-[12px] text-ink2">
        {t('Stops ZTE firmware updates from the stock web page, the ZTE app, carrier management and automatic checks. Turn it off before updating on purpose.')}
      </p>
      {lock
        ? <Toggle checked={on} disabled={busy} label={t('Firmware update lock')} onChange={(next) => void change(next)} />
        : <span className="shrink-0 text-[12px] text-ink3">{t('Unavailable')}</span>}
    </div>
    {lock && on && <div className="mt-2 border-t border-line/8 pt-2">
      <Row label={t('Update servers')} value={lock.servers_blocked ? t('Blocked') : t('Not blocked')} />
      <Row label={t('Automatic updates')} value={lock.auto_update_off == null ? '—' : lock.auto_update_off ? t('Off') : t('On')} />
      <Row label={t('Packages removed')} value={lock.blocked > 0 && lock.last_blocked
        ? t('{count} (last {time})', { count: lock.blocked, time: new Date(lock.last_blocked * 1000).toLocaleString() })
        : '0'} />
    </div>}
    {lock?.installing && <p className="mt-2 text-[12px] text-warn">
      {t('A firmware install is already running. The lock never interrupts one; keep the device powered until it finishes.')}
    </p>}
    {lock?.error && <p className="mt-2 text-[12px] text-warn">{lock.error}</p>}
  </Card>
}
