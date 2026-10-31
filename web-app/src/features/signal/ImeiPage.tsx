import { useEffect, useState } from 'react'
import { req } from '../../data/client'
import { ImeiSession, imeiConfirmation, validImei, validTargets, type ImeiActionName, type ImeiView } from '../../data/imei'
import { N_, t } from '../../i18n'
import { Card } from '../../ui/primitives'

const initial: ImeiView = { status: null, targets: [null, null], consent: null, working: false, waiting: false, error: null }
const labels: Record<ImeiActionName, string> = { check: N_('CHECK IMEI'), preview: N_('Preview targets'), restore_default: N_('Restore default IMEI'), begin: N_('Begin IMEI change'), continue: N_('Continue IMEI change'), verify: N_('Verify after reboot'), restore: N_('Restore IMEI backup'), reboot: N_('Reboot for IMEI operation'), cancel: N_('Cancel IMEI operation') }
const phases = { idle: N_('Not checked'), checking: N_('Checking native records'), checked: N_('Native records checked'), preview: N_('Preview ready'), preparing: N_('Preparing private backup'), reboot_to_enable: N_('First reboot required'), ready_to_write: N_('Waiting for explicit continue'), writing: N_('Applying native records'), reboot_to_verify: N_('Final reboot required'), complete: N_('Durably verified'), recovery_required: N_('Recovery required'), unsupported: N_('IMEI backend unavailable'), error: N_('IMEI operation error') }
export default function ImeiPage() {
  const [state, setState] = useState<{ view: ImeiView; session: ImeiSession | null }>({ view: initial, session: null })
  useEffect(() => {
    const s = new ImeiSession({
      read: () => req('GET', '/api/imei', undefined, undefined, 15_000, undefined, true, 'bare'),
      write: async body => { await req('POST', '/api/imei', body) },
    }, view => setState({ view, session: s }))
    void s.refresh()
    const timer = window.setInterval(() => { void s.refresh() }, 2000)
    return () => { window.clearInterval(timer); s.dispose() }
  }, [])
  return <ImeiControls view={state.view} session={state.session} />
}

// Pure view for offline bilingual fixtures; only ImeiPage owns polling/lifecycle.
export function ImeiControls({ view, session: s }: { view: ImeiView; session: ImeiSession | null }) {
  const status = view.status, consent = view.consent
  const can = (a: ImeiActionName) => s?.can(a) ?? false
  const invalidOriginal = status && (status.code === 'original_validation_failed' || (status.imei1 !== null && !validImei(status.imei1)) || (status.imei2 !== null && !validImei(status.imei2)))
  const buttonClass = 'min-h-11 rounded-lg border border-line/20 px-3 py-2 text-sm font-semibold disabled:cursor-not-allowed disabled:opacity-40'
  return <Card>
    <div className="space-y-3" aria-label={t('Guarded IMEI controls')}>
      <h2 className="text-base font-bold">{t('Guarded IMEI controls')}</h2>
      <p className="text-sm text-ink2">{t('Authorized device only. Two reboots and network interruption are required. Retain the verified private backup. Never blindly repeat a request.')}</p>
      <p className="text-xs text-ink3">{t('Native NV550 records are not physical SIM slot numbers. Opening this page never probes or writes the modem.')}</p>
      <div role="status" aria-live="polite" className="break-words text-sm">
        {status ? t(phases[status.phase]) : t('IMEI backend unavailable')}
        {status && <span className="ml-2 text-xs text-ink3">{status.code}</span>}
        {(view.working || view.waiting || status?.busy) && <p>{t('Queued or busy. A queue acknowledgement is not verified success.')}</p>}
      </div>
      {view.error && <p role="alert" className="text-sm text-red-500">{view.error === 'outcome_unknown' ? t('Outcome unknown or request refused. Refresh status; no request was replayed.') : t('Status invalid, stale or offline. Refresh before another action; no request was replayed.')}</p>}
      {invalidOriginal && <p role="alert" className="text-sm text-red-500">{t('Observed original is invalid or unverified. It is not corrected; new writes are blocked.')}</p>}
      <div className="space-y-1 break-all font-mono text-sm">
        <p>NV0: {status?.imei1 ?? t('Not checked')}</p>
        {status?.secondary === 'active' ? <p>NV1: {status.imei2 ?? t('Not checked')}</p> : <p>{t('NV1 inactive or unknown: keep exact original record.')}</p>}
      </div>
      <div className="flex flex-wrap gap-2">
        <button type="button" className={buttonClass} disabled={!can('check') || !!consent} onClick={() => { void s?.submit('check') }}>{t('CHECK IMEI')}</button>
        <button type="button" className={buttonClass} disabled={view.working} onClick={() => { s?.cancelConsent(); void s?.refresh(true) }}>{t('Refresh status')}</button>
      </div>
      {!consent && <>
        {([0, 1] as const).filter(i => i === 0 || status?.secondary === 'active').map(i => <fieldset key={i} className="space-y-2 rounded-lg border border-line/10 p-3" disabled={!can('preview') || !!invalidOriginal}>
          <legend className="px-1 text-sm">{t('Native record {index}', { index: i })}</legend>
          <label className="flex min-h-11 items-center gap-2 text-sm"><input type="checkbox" checked={view.targets[i] === null} onChange={e => s?.edit(i, e.target.checked ? null : '')} />{t('Keep exact original record')}</label>
          {view.targets[i] !== null && <label className="block text-sm">{t('New IMEI: 15 ASCII digits, valid Luhn')}<input className="mt-1 min-h-11 w-full rounded-lg border border-line/20 bg-transparent p-2 font-mono" type="text" inputMode="numeric" autoComplete="off" autoCorrect="off" spellCheck={false} maxLength={15} value={view.targets[i]} onChange={e => s?.edit(i, e.target.value)} onKeyDown={e => { if (e.key === 'Enter') e.preventDefault() }} /></label>}
        </fieldset>)}
        <div className="flex flex-wrap gap-2">
          <button type="button" className={buttonClass} disabled={!can('preview') || !status || !validTargets(status, view.targets) || !!invalidOriginal} onClick={() => { void s?.submit('preview') }}>{t('Preview targets')}</button>
          {status?.allowed.includes('restore_default') && <button type="button" className={buttonClass} disabled={!can('restore_default') || !!invalidOriginal} onClick={() => { void s?.submit('restore_default') }}>{t('Restore default IMEI')}</button>}
        </div>
        {status?.allowed.includes('restore_default') && <p className="text-xs text-ink3">{t('Default IMEI is the value recorded at the first CHECK, before any change. Restoring it builds a preview; the change itself still needs both confirmations and two reboots.')}</p>}
        {status?.phase === 'preview' && <div className="break-all font-mono text-sm"><p>NV0: {status.target1_masked ?? t('Keep exact original record')}</p>{status.secondary === 'active' && <p>NV1: {status.target2_masked ?? t('Keep exact original record')}</p>}<p>{t('Preview expires in {seconds}s', { seconds: status.preview_expires_in })}</p></div>}
        <div className="flex flex-wrap gap-2">{(['begin', 'continue', 'verify', 'reboot', 'restore', 'cancel'] as const).filter(a => status?.allowed.includes(a)).map(a => <button key={a} type="button" className={buttonClass} disabled={!can(a)} onClick={() => { if (a === 'verify') void s?.submit(a); else s?.startConsent(a) }}>{t(labels[a])}</button>)}</div>
        {(status?.phase === 'reboot_to_enable' || status?.phase === 'reboot_to_verify') && <p className="text-sm text-ink2">{t('You may reboot manually. After reconnecting, refresh status and use only the offered Continue or Verify action.')}</p>}
      </>}
      {consent && <div role="group" aria-label={t('IMEI confirmation')} className="space-y-3 rounded-lg border border-red-500/40 p-3">
        <h3 className="font-semibold">{t(labels[consent.action])}</h3>
        <p className="break-all font-mono text-sm">NV0: {status?.target1_masked ?? t('Keep exact original record')}<br />{status?.secondary === 'active' && <>NV1: {status.target2_masked ?? t('Keep exact original record')}</>}</p>
        {consent.step === 1 ? <>
          <label className="flex min-h-11 items-center gap-2 text-sm"><input type="checkbox" checked={consent.risk} onChange={e => s?.risk(e.target.checked)} />{t('I am authorized and accept the two-reboot, network-loss and recovery risks.')}</label>
          <button type="button" className={buttonClass} disabled={!consent.risk || !can(consent.action)} onClick={() => s?.next()}>{t('Next confirmation')}</button>
        </> : <>
          <label className="block text-sm">{t('Second confirmation: type {text}', { text: imeiConfirmation(consent.action) })}<input className="mt-1 min-h-11 w-full rounded-lg border border-line/20 bg-transparent p-2" type="text" autoComplete="off" spellCheck={false} value={consent.text} onChange={e => s?.confirmText(e.target.value)} onKeyDown={e => { if (e.key === 'Enter') e.preventDefault() }} /></label>
          <button type="button" className={buttonClass} disabled={!can(consent.action) || consent.text !== imeiConfirmation(consent.action)} onClick={() => { void s?.submit(consent.action) }}>{t('Confirm selected IMEI action')}</button>
        </>}
        <button type="button" className={`${buttonClass} ml-2`} onClick={() => s?.cancelConsent()}>{t('Cancel confirmation')}</button>
      </div>}
    </div>
  </Card>
}
