import type { ClashCfOpt } from '../../types'

/** kbit/s as Mbit/s text: one decimal below 10. */
export const mbps = (kbps: number | null | undefined) =>
  kbps == null ? null : kbps >= 10_000 ? (kbps / 1000).toFixed(0) : (kbps / 1000).toFixed(1)

/** What the card leads with. */
export function cfState(o: ClashCfOpt): 'off' | 'running' | 'clash_off' | 'no_node' | 'error' | 'waiting' | 'active' {
  if (!o.enabled) return 'off'
  if (o.running) return 'running'
  if (!o.clash_running) return 'clash_off'
  if (o.error?.startsWith('No node')) return 'no_node'
  if (o.error) return 'error'
  return o.applied ? 'active' : 'waiting'
}

/** Mobile data automatic runs use per day, in MB: the last run's, or an estimate before one. */
export function dailyMb(o: ClashCfOpt): number | null {
  if (!o.interval_hours) return null
  const perRun = o.data_kb > 0 ? o.data_kb / 1000 : o.speed_test ? 6.5 : 0.5
  return Math.round((perRun * 24) / o.interval_hours)
}

/** How much faster the TLS handshake is than at the domain's own DNS answer, in ms. */
export function gainMs(o: ClashCfOpt): number | null {
  const a = o.applied?.tls_ms, b = o.baseline?.tls_ms
  return a != null && b != null && o.applied?.ip !== o.baseline?.ip ? b - a : null
}
