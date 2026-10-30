import { useEffect, useState } from 'react'
import { api } from '../../data/api'
import { ApiError } from '../../data/client'
import { formatDuration } from '../../format'
import type { CfOptPhase, ClashCfOpt, ClashCfOptUpdate } from '../../types'
import { Button, Field, Select, Toggle } from '../../ui/controls'
import { confirm, toast, toastError } from '../../ui/feedback'
import { Card, Chip } from '../../ui/primitives'
import { N_, t, tm } from '../../i18n'
import { cfState, dailyMb, gainMs, mbps } from './cfoptModel'

// Cloudflare IP optimization (agent/src/clash_cfopt.rs): the agent tests Cloudflare edge
// addresses on a schedule or on request and points Cloudflare-fronted nodes at the best.

const PHASES: Record<CfOptPhase, string> = {
  nodes: N_('Finding nodes that run through Cloudflare…'),
  probing: N_('Testing Cloudflare edge addresses…'),
  verifying: N_('Checking the fastest with a TLS request…'),
  speed: N_('Speed-testing the best addresses…'),
  applying: N_('Applying the pick to Clash…'),
}
const POLL_RUNNING_MS = 2000
const POLL_IDLE_MS = 30_000
const OLD_AGENT = N_('This agent cannot optimize Cloudflare addresses; deploy the latest agent.')

const agoText = (s: number) => (s < 60 ? t('just now') : t('{d} ago', { d: formatDuration(s) }))

export default function CloudflareCard() {
  const [opt, setOpt] = useState<ClashCfOpt | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  // Bumped after each change so polling picks up a run it started.
  const [round, setRound] = useState(0)

  useEffect(() => {
    let alive = true
    let timer: number | undefined
    const read = async () => {
      try {
        const o = await api.clashCfOpt()
        if (!alive) return
        setOpt(o); setError('')
        timer = window.setTimeout(() => void read(), o.running ? POLL_RUNNING_MS : POLL_IDLE_MS)
      } catch (e) {
        if (!alive) return
        if (e instanceof ApiError && e.status === 404) { setError(t(OLD_AGENT)); return }
        setError(e instanceof Error ? e.message : t('Unable to load Cloudflare optimization'))
        timer = window.setTimeout(() => void read(), POLL_IDLE_MS)
      }
    }
    timer = window.setTimeout(() => void read(), round > 0 ? 500 : 0)
    return () => { alive = false; window.clearTimeout(timer) }
  }, [round])

  async function save(patch: Omit<ClashCfOptUpdate, 'expected_revision'>) {
    if (!opt || busy) return
    if (patch.enabled === false && !(await confirm({
      title: t('Turn off Cloudflare IP optimization?'),
      body: t('Nodes go back to their domain’s own address. Clash reloads its profile, so open connections restart.'),
      confirmLabel: t('Turn off'),
      danger: true,
    }))) return
    setBusy(true)
    try {
      setOpt(await api.clashCfOptSet({ expected_revision: opt.revision, ...patch }))
    } catch (e) {
      if (e instanceof ApiError && e.status === 409) toast(t('Settings changed elsewhere. Refresh and try again.'), 'err')
      else toastError(e, t('Unable to save Cloudflare optimization'))
    } finally {
      setBusy(false); setRound(n => n + 1)
    }
  }

  async function runNow() {
    if (!opt || busy) return
    setBusy(true)
    try {
      setOpt(await api.clashCfOptRun())
    } catch (e) {
      toastError(e, t('Unable to start the optimization'))
    } finally {
      setBusy(false); setRound(n => n + 1)
    }
  }

  const state = opt ? cfState(opt) : null
  const chip = state === 'running' ? <Chip tone="accent">{t('Optimizing')}</Chip>
    : state === 'active' ? <Chip tone="ok">{t('On')}</Chip>
      : state === 'error' ? <Chip tone="danger">{t('Error')}</Chip>
        : state === 'off' || !state ? <Chip>{t('Off')}</Chip>
          : <Chip tone="warn">{t('Waiting')}</Chip>
  const a = opt?.applied
  const gain = opt ? gainMs(opt) : null
  const daily = opt ? dailyMb(opt) : null

  return (
    <Card title={t('Cloudflare IP optimization')} action={chip}>
      <div className="space-y-4">
        <div className="flex items-start justify-between gap-4">
          <div>
            <p className="text-[13px] font-medium text-ink">{t('Optimize automatically')}</p>
            <p className="mt-1 text-xs leading-relaxed text-ink2">{t('For nodes served through Cloudflare (TLS over WebSocket or gRPC): tests Cloudflare edge addresses from this router and points those nodes at the fastest. The domain stays as a fallback.')}</p>
          </div>
          <Toggle label={t('Optimize automatically')} checked={!!opt?.enabled} disabled={!opt || busy} onChange={v => void save({ enabled: v })} />
        </div>
        {error && <p role="alert" className="rounded-lg bg-danger/10 p-3 text-[13px] text-danger">{error}</p>}
        {opt?.enabled && <>
          <div className="grid gap-4 sm:grid-cols-2">
            <Field label={t('Interval')} hint={daily != null ? t('About {n} MB of mobile data a day', { n: daily }) : undefined}>
              <Select value={opt.interval_hours} disabled={busy} onChange={e => void save({ interval_hours: Number(e.target.value) })}>
                {opt.intervals.map(h => <option key={h} value={h}>{h ? t('Every {n} h', { n: h }) : t('Only when I tap Optimize now')}</option>)}
              </Select>
            </Field>
            <div className="flex items-start justify-between gap-3">
              <div>
                <p className="text-[13px] font-medium text-ink">{t('Speed test')}</p>
                <p className="mt-1 text-xs text-ink2">{t('Downloads 2 MB from each of the 3 best addresses, about 6 MB a run. Off: picks by latency only.')}</p>
              </div>
              <Toggle label={t('Speed test')} checked={opt.speed_test} disabled={busy} onChange={v => void save({ speed_test: v })} />
            </div>
          </div>

          {state === 'running' && <p role="status" className="rounded-lg border border-line/10 bg-surface2/60 p-3 text-[13px] text-ink">{t(PHASES[opt.phase ?? 'nodes'])}</p>}
          {state === 'clash_off' && <p className="text-xs text-warn">{t('Runs while Clash is on.')}</p>}
          {state === 'no_node' && <p className="rounded-lg bg-warn/10 p-3 text-xs text-ink2">{t('No node in the active subscription runs through Cloudflare (TLS over WebSocket or gRPC to a Cloudflare-proxied domain), so there is nothing to optimize.')}</p>}
          {state === 'error' && <p role="alert" className="rounded-lg bg-danger/10 p-3 text-[13px] text-danger">{tm(opt.error)}</p>}

          {a && <div className="rounded-xl bg-ok/10 p-3">
            <p className="text-[11px] font-semibold uppercase tracking-wider text-ink3">{t('Current address')}</p>
            <p className="mt-1 flex flex-wrap items-center gap-2">
              <span className="font-mono text-base font-semibold text-ink">{a.ip}</span>
              <Chip>{a.colo}</Chip>
              {a.tls_ms != null && <span className="text-[13px] text-ink2">{t('TLS {n} ms', { n: a.tls_ms })}</span>}
              {a.speed_kbps != null && <span className="text-[13px] text-ink2">{mbps(a.speed_kbps)} Mbps</span>}
            </p>
            {gain != null && gain > 0 && opt.baseline && <p className="mt-1 text-xs text-ok">{t('{n} ms faster than the domain’s own address ({colo})', { n: gain, colo: opt.baseline.colo })}</p>}
            {opt.nodes.length > 0 && <p className="mt-1 text-xs text-ink2">{t('Used by: {nodes}', { nodes: opt.nodes.join(', ') })}</p>}
          </div>}

          {opt.candidates.length > 0 && <div>
            <p className="mb-1 text-[11px] font-semibold uppercase tracking-wider text-ink3">{t('Best addresses in the last run')}</p>
            <ul className="divide-y divide-line/10 text-[12px]">
              {opt.candidates.map(c => (
                <li key={c.ip} className="flex items-center gap-2 py-1.5">
                  <span className="w-32 font-mono text-ink">{c.ip}</span>
                  <span className="w-10 text-ink2">{c.colo}</span>
                  <span className="w-20 text-ink2">{c.tls_ms != null ? t('TLS {n} ms', { n: c.tls_ms }) : '—'}</span>
                  <span className="text-ink2">{c.speed_kbps != null ? `${mbps(c.speed_kbps)} Mbps` : ''}</span>
                  {c.ip === a?.ip && <span className="ml-auto"><Chip tone="ok">{t('In use')}</Chip></span>}
                </li>
              ))}
            </ul>
          </div>}

          <div className="flex flex-wrap items-center gap-3">
            <Button variant="primary" loading={opt.running || busy} disabled={!opt.clash_running || opt.running} onClick={() => void runNow()}>
              {opt.running ? t('Optimizing…') : t('Optimize now')}
            </Button>
            <p className="text-[11px] text-ink3">
              {[
                opt.last_run_age_s != null && t('Last run {when}', { when: agoText(opt.last_run_age_s) }),
                opt.tested > 0 && t('{n} addresses tested', { n: opt.tested }),
                opt.data_kb > 0 && t('{n} MB used', { n: (opt.data_kb / 1000).toFixed(1) }),
                opt.next_run_in_s != null && opt.clash_running && !opt.running && t('next in {d}', { d: formatDuration(Math.max(60, opt.next_run_in_s)) }),
              ].filter(Boolean).join(' · ')}
            </p>
          </div>
        </>}
      </div>
    </Card>
  )
}
