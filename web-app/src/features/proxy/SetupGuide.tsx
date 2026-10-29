import { useState, type ReactNode } from 'react'
import { ICheck } from '../../icons'
import { Button } from '../../ui/controls'
import { Card, Chip } from '../../ui/primitives'
import { t } from '../../i18n'

// First-run checklist shared by Clash, WireGuard and Tailscale: each step says what it needs
// and links to where it is done. Steps are derived from live status, never stored.

export interface SetupStep {
  title: string
  detail?: ReactNode
  done: boolean
  /** Shown as "Optional"; does not hold the guide open. */
  optional?: boolean
  /** Turning the service on: once only this is left, the guide may be hidden. */
  final?: boolean
  actions?: { label: string; onClick: () => void; disabled?: boolean; loading?: boolean }[]
  /** Set when the step cannot be done from here, instead of an action. */
  blocked?: string
}

const KEY = 'u60.setup-hidden.'

function hiddenFor(id: string) {
  try { return localStorage.getItem(KEY + id) === '1' } catch { return false }
}

/** Shown while a required step is open; when only the final switch is left it can be hidden. */
export default function SetupGuide({ id, title, steps }: { id: string; title: string; steps: SetupStep[] }) {
  const [hidden, setHidden] = useState(() => hiddenFor(id))
  const required = steps.filter(s => !s.optional)
  const done = required.filter(s => s.done).length
  if (done === required.length) return null
  const onlyFinal = required.every(s => s.done || s.final)
  if (onlyFinal && hidden) return null
  const current = steps.findIndex(s => !s.done && !s.optional)

  function hide() {
    try { localStorage.setItem(KEY + id, '1') } catch { /* this page only */ }
    setHidden(true)
  }

  return (
    <Card title={title} action={<Chip tone="accent">{t('{done} of {total} done', { done, total: required.length })}</Chip>}>
      <ol className="space-y-2">
        {steps.map((s, i) => {
          const now = i === current
          return (
            <li key={s.title} className={`flex gap-3 rounded-xl p-3 ${now ? 'bg-accent/8 ring-1 ring-accent/30' : 'bg-surface2/50'}`}>
              <span aria-hidden className={`mt-0.5 flex h-6 w-6 shrink-0 items-center justify-center rounded-full text-xs font-bold ${s.done ? 'bg-ok text-white' : now ? 'bg-accent text-white' : 'bg-line/15 text-ink2'}`}>
                {s.done ? <ICheck size={14} /> : i + 1}
              </span>
              <div className="min-w-0 flex-1 space-y-1">
                <p className={`text-[13px] font-semibold ${s.done ? 'text-ink2' : 'text-ink'}`}>
                  {s.title}
                  {s.optional && <span className="ml-2 text-[11px] font-normal text-ink3">{t('Optional')}</span>}
                  <span className="sr-only">{s.done ? t('Done') : t('Not done')}</span>
                </p>
                {s.detail && !s.done && <div className="text-xs leading-relaxed text-ink2">{s.detail}</div>}
                {!s.done && s.blocked && <p className="text-xs text-warn">{s.blocked}</p>}
                {!s.done && !s.blocked && !!s.actions?.length && <div className="flex flex-wrap gap-2 pt-1">
                  {s.actions.map((a, j) => (
                    <Button key={a.label} size="sm" variant={now && j === 0 ? 'primary' : 'outline'} disabled={a.disabled} loading={a.loading} onClick={a.onClick}>
                      {a.label}
                    </Button>
                  ))}
                </div>}
              </div>
            </li>
          )
        })}
      </ol>
      {onlyFinal && <div className="mt-3 flex justify-end">
        <Button size="sm" variant="ghost" onClick={hide}>{t('Hide guide')}</Button>
      </div>}
    </Card>
  )
}
