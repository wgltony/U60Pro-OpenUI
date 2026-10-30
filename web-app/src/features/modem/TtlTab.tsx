import { useCallback, useEffect, useState } from 'react'
import { api } from '../../data/api'
import { t } from '../../i18n'
import type { TtlStatus } from '../../types'
import { Button, Input } from '../../ui/controls'
import { toast, toastError } from '../../ui/feedback'
import { Card, Chip } from '../../ui/primitives'

/** Common TTL / hop-limit targets for carrier tethering bypass. 65 egresses at
 * 64 after the forward hop, matching a phone's native stack; 128 / 117 suit
 * carriers that profile a Windows-style 128 origin. */
const PRESETS = [65, 64, 128, 117]

function presetNote(value: number): string {
  switch (value) {
    case 65:
      return t('Recommended')
    case 64:
      return t('Standard')
    case 128:
      return t('Windows')
    case 117:
      return t('Some carriers')
    default:
      return ''
  }
}

export default function TtlTab() {
  const [status, setStatus] = useState<TtlStatus | null>(null)
  const [ttlInput, setTtlInput] = useState('65')
  const [busy, setBusy] = useState(false)

  const fetchStatus = useCallback(async () => {
    try {
      const data = await api.ttlStatus()
      setStatus(data)
      if (data.ttl_value && data.ttl_value > 0) setTtlInput(String(data.ttl_value))
    } catch {
      setStatus(null)
    }
  }, [])

  useEffect(() => {
    fetchStatus()
  }, [fetchStatus])

  const active = Boolean(status?.active || status?.ipv6_active)
  const parsed = parseInt(ttlInput, 10)
  const validInput = Number.isInteger(parsed) && parsed >= 1 && parsed <= 255

  async function applyTtl(value?: number) {
    const val = value ?? parsed
    if (!Number.isInteger(val) || val < 1 || val > 255) {
      toast(t('TTL must be 1-255'), 'err')
      return
    }
    setTtlInput(String(val))
    setBusy(true)
    try {
      await api.ttlSet(val)
      toast(t('TTL set to {ttl} (IPv4 + IPv6)', { ttl: val }))
      await fetchStatus()
    } catch (e) {
      toastError(e, t('Failed to set TTL'))
    } finally {
      setBusy(false)
    }
  }

  async function clearTtl() {
    setBusy(true)
    try {
      await api.ttlClear()
      toast(t('TTL clamping disabled'))
      await fetchStatus()
    } catch (e) {
      toastError(e, t('Failed to clear TTL'))
    } finally {
      setBusy(false)
    }
  }

  // A preset is "live" when clamping is active and the device's current value
  // matches it; otherwise it tracks what's typed in the custom field.
  const currentValue = active ? (status?.ttl_value ?? null) : validInput ? parsed : null

  return (
    <Card title={t('TTL clamping')}>
      <div className="space-y-3">
        <p className="text-[12px] text-ink2">
          {t('Overrides the TTL / hop limit on LAN ingress traffic to prevent carrier tethering detection. Applied immediately and persists across reboots.')}
        </p>

        {status == null ? (
          <p className="text-[13px] text-ink3">{t('Checking status…')}</p>
        ) : (
          <>
            {active && (
              <div className="flex items-center gap-2">
                <span className="h-2 w-2 rounded-full bg-ok" />
                <span className="tnum text-[13px] font-semibold text-ok">
                  {t('Active (TTL={ttl})', { ttl: status?.ttl_value ?? '' })}
                </span>
                {status?.ipv6_active && <Chip tone="default">IPv4 + IPv6</Chip>}
              </div>
            )}

            <div>
              <p className="mb-1.5 text-[11px] font-semibold uppercase tracking-wider text-ink3">
                {t('Presets')}
              </p>
              <div className="flex flex-wrap gap-2">
                {PRESETS.map((value) => {
                  const selected = currentValue === value
                  return (
                    <button
                      key={value}
                      type="button"
                      disabled={busy}
                      onClick={() => applyTtl(value)}
                      className={`flex min-w-[72px] flex-col items-center rounded-lg border px-3 py-1.5 transition-colors disabled:pointer-events-none disabled:opacity-45 ${
                        selected
                          ? 'border-accent/40 bg-accent/10 text-accent'
                          : 'border-line/15 bg-surface text-ink hover:bg-surface2'
                      }`}
                    >
                      <span className="tnum text-[15px] font-semibold leading-tight">{value}</span>
                      <span className="text-[10px] text-ink3">{presetNote(value)}</span>
                    </button>
                  )
                })}
              </div>
            </div>

            <div>
              <p className="mb-1.5 text-[11px] font-semibold uppercase tracking-wider text-ink3">
                {t('Custom value')}
              </p>
              <div className="flex flex-wrap items-center gap-2">
                <div className="w-24">
                  <Input
                    type="number"
                    min={1}
                    max={255}
                    value={ttlInput}
                    onChange={(e) => setTtlInput(e.target.value)}
                    placeholder="65"
                  />
                </div>
                <Button
                  variant={active ? 'outline' : 'primary'}
                  onClick={() => applyTtl()}
                  loading={busy}
                  disabled={!validInput}
                >
                  {active ? t('Update') : t('Enable clamping')}
                </Button>
                {active && (
                  <Button variant="ghost" onClick={clearTtl} disabled={busy}>
                    {t('Disable')}
                  </Button>
                )}
              </div>
            </div>
          </>
        )}
      </div>
    </Card>
  )
}
