import { useCallback, useEffect, useRef, useState } from 'react'
import { api } from '../../data/api'
import { ApiError } from '../../data/client'
import type { TailscaleStatus, TailscaleUpdate } from '../../types'
import { Button, Field, Input, Segmented, Select } from '../../ui/controls'
import { Card, Chip } from '../../ui/primitives'
import { confirm, toast, toastError } from '../../ui/feedback'
import { t, tm } from '../../i18n'
import { useProxy } from './ProxyContext'
import SetupGuide, { type SetupStep } from './SetupGuide'
import { eligibleExitNodes, remoteAccessState, setupStage, tailscaleUpdate, validAuthKey, validHostname, type SignIn } from './tailscaleModel'

const ADMIN = 'https://login.tailscale.com/admin/machines'

/** Asked before changes that move traffic or end access; sign-in itself changes nothing yet. */
function ask(status: TailscaleStatus, action: TailscaleUpdate['action']) {
  if (action === 'login' || action === 'cancel_login') return Promise.resolve(true)
  if (action === 'logout') return confirm({
    title: t('Sign out of Tailscale?'),
    body: t('This router leaves your tailnet: Tailscale mode and remote management turn off, and you need to sign in again to use them. A page opened through Tailscale disconnects.'),
    confirmLabel: t('Sign out'),
    danger: true,
  })
  const remoteOn = action === 'remote_access' && !status.remote_access
  return confirm(action === 'remote_access' ? {
    title: remoteOn ? t('Turn on remote management?') : t('Turn off remote management?'),
    body: remoteOn
      ? t('Tailscale stays connected and offers this LAN ({lan}) to your tailnet. Clash and WireGuard are not affected. Approve the route once in the Tailscale admin console.', { lan: status.lan_cidr ?? '—' })
      : t('Tailnet devices can no longer reach this router. If Tailscale mode is off, Tailscale stops. A page opened through Tailscale disconnects.'),
    confirmLabel: t('Confirm'),
    danger: !remoteOn,
  } : {
    title: action === 'connect' ? t('Connect Tailscale?') : action === 'disconnect' ? t('Disconnect Tailscale?') : t('Change Tailscale exit node?'),
    body: action === 'connect' ? t('Connecting activates Tailscale mode and turns Clash and WireGuard off. Their settings are kept.')
      : action === 'disconnect' ? (status.remote_access
        ? t('Tailscale mode turns off. Remote management keeps Tailscale connected for access to this router.')
        : t('Tailscale will disconnect. No other network backend is started automatically.'))
        : t('The selected exit changes the Tailscale internet route. None clears the exit selection; it does not activate Clash.'),
    confirmLabel: t('Confirm'),
  })
}

export default function TailscaleTab() {
  const { status: proxy, pending, settingsBusy, epoch, refresh: refreshProxy, beginSettingsUpdate, endSettingsUpdate } = useProxy()
  const [status, setStatus] = useState<TailscaleStatus | null>(null)
  const [error, setError] = useState('')
  const [selected, setSelected] = useState('')
  const [working, setWorking] = useState<TailscaleUpdate['action'] | null>(null)
  const [method, setMethod] = useState<'browser' | 'key'>('browser')
  const [signIn, setSignIn] = useState<SignIn>({ hostname: '', authKey: '' })
  const mounted = useRef(true)
  const generation = useRef(0)
  const busy = useRef(false)
  const dirty = useRef(false)

  const refresh = useCallback(async () => {
    if (busy.current) return
    const current = ++generation.current
    try {
      const next = await api.tailscale()
      if (!mounted.current || busy.current || current !== generation.current) return
      setStatus(next); setError('')
      if (!dirty.current) setSelected(next.selected_exit_node)
    } catch (e) {
      if (!mounted.current || current !== generation.current) return
      setError(e instanceof ApiError && e.status === 404 ? t('This agent does not support Tailscale.') : e instanceof Error ? e.message : t('Unable to load Tailscale'))
    }
  }, [])

  useEffect(() => {
    mounted.current = true
    void refresh()
    const timer = window.setInterval(() => { if (!document.hidden) void refresh() }, 5000)
    return () => { mounted.current = false; generation.current += 1; window.clearInterval(timer) }
  }, [refresh, epoch])

  // While a sign-in runs, follow it closely: the link appears and completes within seconds.
  const signingIn = !!status?.login_active
  useEffect(() => {
    if (!signingIn) return
    const timer = window.setInterval(() => { if (!document.hidden) void refresh() }, 2000)
    return () => window.clearInterval(timer)
  }, [signingIn, refresh])

  const mutate = async (action: TailscaleUpdate['action'], login?: SignIn) => {
    if (!status || busy.current || pending || settingsBusy || error) return
    // Hold a local guard while the standard confirmation dialog is open, too.
    busy.current = true
    const accepted = await ask(status, action)
    if (!accepted || !mounted.current || !beginSettingsUpdate()) { busy.current = false; return }
    generation.current += 1
    setWorking(action)
    try {
      const next = await api.tailscaleSet(tailscaleUpdate(status, action, selected, login))
      if (mounted.current) {
        dirty.current = false; setStatus(next); setSelected(next.selected_exit_node); setError('')
        // The key is used once; never keep it in the form.
        if (action === 'login') setSignIn(v => ({ ...v, authKey: '' }))
      }
      toast(action === 'login' ? t('Sign-in started.') : action === 'logout' ? t('Signed out of Tailscale.') : t('Tailscale request applied. Check current status.'))
    } catch (e) {
      if (e instanceof ApiError && e.status === 409) {
        dirty.current = false
        toast(t('Tailscale settings changed elsewhere. Refresh and try again.'), 'err')
      } else toastError(e, t('Unable to update Tailscale'))
    } finally {
      busy.current = false
      endSettingsUpdate()
      if (mounted.current) { setWorking(null); await refresh() }
      await refreshProxy()
    }
  }

  const nodes = status ? eligibleExitNodes(status) : []
  const blocked = !!working || !!pending || settingsBusy || !!error || !status
  const running = status?.enabled && status.backend_state === 'Running'
  const canSelect = !blocked && running && proxy?.active === 'tailscale' && !status?.error
  const selectedMissing = selected !== '' && !nodes.some(node => node.id === selected)
  const remote = status ? remoteAccessState(status) : 'unsupported'
  const remoteChip = remote === 'active' ? <Chip tone="ok">{t('Working')}</Chip>
    : remote === 'pending' ? <Chip tone="warn">{t('Waiting for approval')}</Chip>
      : remote === 'starting' ? <Chip tone="warn">{t('Starting')}</Chip>
        : remote === 'error' ? <Chip tone="danger">{t('Error')}</Chip> : <Chip>{t('Off')}</Chip>
  const lanIp = status?.lan_ip || '192.168.0.1'
  const stage = status ? setupStage(status) : 'unsupported'
  const signedIn = stage === 'ready' || stage === 'unsupported'
  const formOk = validHostname(signIn.hostname.trim()) && (method === 'browser' ? true : signIn.authKey.trim() !== '' && validAuthKey(signIn.authKey.trim()))

  const steps: SetupStep[] = status && stage !== 'unsupported' ? [
    {
      title: t('Tailscale runs on the router'),
      done: stage !== 'unavailable',
      blocked: t('Tailscale is not installed here, or its service is stopped. Install it with scripts/prepare-tailscale.py and deploy-components.py --tailscale.'),
    },
    {
      title: t('Sign in to your Tailscale account'),
      done: stage === 'ready',
      detail: stage === 'approval'
        ? t('Signed in. Approve this router in the admin console under Machines to finish.')
        : t('Use the sign-in card below: open a link on any phone or computer, or paste an auth key. No Tailscale account yet? The link lets you create one for free.'),
    },
    {
      title: t('Choose how to use it'),
      done: !!status.remote_access || status.enabled,
      final: true,
      detail: t('Remote management lets your tailnet reach this router and its LAN, alongside Clash or WireGuard. Tailscale mode instead sends devices’ internet through an exit node and turns Clash and WireGuard off.'),
      actions: [
        { label: t('Turn on remote management'), onClick: () => void mutate('remote_access'), disabled: blocked || stage !== 'ready', loading: working === 'remote_access' },
        { label: t('Use Tailscale mode'), onClick: () => void mutate('connect'), disabled: blocked || stage !== 'ready', loading: working === 'connect' },
      ],
    },
    ...(status.remote_access ? [{
      title: t('Approve the subnet route'),
      done: !!status.route_approved,
      detail: <>{t('Open the Tailscale admin console → Machines → {name} → Edit route settings, and tick {lan}.', { name: status.self?.name || '—', lan: status.lan_cidr ?? '' })} <a className="font-semibold text-accent underline" href={ADMIN} target="_blank" rel="noreferrer">{t('Open admin console')}</a></>,
    }] : []),
  ] : []

  return <div className="space-y-4">
    {steps.length > 0 && <SetupGuide id="tailscale" title={t('Set up Tailscale')} steps={steps} />}
    {status && (stage === 'signin' || stage === 'waiting' || stage === 'approval') && <Card title={t('Sign in to Tailscale')}>
      <div className="space-y-3">
        {stage === 'approval' && <div className="rounded-xl bg-warn/10 p-3 text-[13px] text-ink2">
          <p className="font-semibold text-ink">{t('Waiting for approval')}</p>
          <p className="mt-1">{t('Your tailnet requires new devices to be approved. Approve this router in the admin console under Machines; this page updates by itself.')}</p>
          <a className="mt-1 inline-block font-semibold text-accent underline" href={ADMIN} target="_blank" rel="noreferrer">{t('Open admin console')}</a>
        </div>}
        {stage === 'waiting' && <>
          {status.auth_url ? <div className="rounded-xl bg-accent/8 p-3 text-[13px] text-ink2">
            <p className="font-semibold text-ink">{t('Open this link on any phone or computer and sign in')}</p>
            <a className="mt-1 block break-all font-mono text-accent underline" href={status.auth_url} target="_blank" rel="noreferrer">{status.auth_url}</a>
            <p className="mt-2 text-xs text-ink3">{t('This page updates by itself when you are done. The link stays valid for 10 minutes.')}</p>
          </div> : <p role="status" className="text-sm text-ink2">{t('Signing in…')}</p>}
          <Button variant="ghost" disabled={blocked} loading={working === 'cancel_login'} onClick={() => void mutate('cancel_login')}>{t('Cancel sign-in')}</Button>
        </>}
        {stage === 'signin' && <>
          <Segmented<'browser' | 'key'> value={method} onChange={setMethod} disabled={blocked}
            options={[{ value: 'browser', label: t('Sign-in link') }, { value: 'key', label: t('Auth key') }]} />
          <p className="text-xs text-ink2">{method === 'browser'
            ? t('The router gets a sign-in link. Open it on your phone or computer, sign in to Tailscale (or create an account), and the router joins your tailnet.')
            : t('For signing in without a browser. Create a key in the admin console under Settings → Keys and paste it here. It is used once and not saved.')}</p>
          <div className="grid gap-3 sm:grid-cols-2">
            <Field label={t('Device name')} hint={t('How this router appears in your tailnet. Leave empty to use its system name.')}>
              <Input aria-label={t('Device name')} value={signIn.hostname} maxLength={63} disabled={blocked} autoComplete="off" spellCheck={false} placeholder="u60-pro"
                onChange={e => setSignIn({ ...signIn, hostname: e.target.value })} />
            </Field>
            {method === 'key' && <Field label={t('Auth key')} hint={t('Starts with tskey-.')}>
              {/* i18n-ignore: the key's literal prefix */}
              <Input aria-label={t('Auth key')} type="password" value={signIn.authKey} disabled={blocked} autoComplete="off" spellCheck={false} placeholder="tskey-auth-…"
                onChange={e => setSignIn({ ...signIn, authKey: e.target.value })} />
            </Field>}
          </div>
          {!validHostname(signIn.hostname.trim()) && <p className="text-xs text-danger">{t('Device name may use only letters, digits and hyphens (up to 63)')}</p>}
          {status.login_error && <p role="alert" className="rounded-lg bg-danger/10 p-3 text-[13px] text-danger">{tm(status.login_error)}</p>}
          <Button variant="primary" disabled={blocked || !formOk} loading={working === 'login'}
            onClick={() => void mutate('login', method === 'browser' ? { hostname: signIn.hostname, authKey: '' } : signIn)}>
            {method === 'browser' ? t('Get sign-in link') : t('Sign in')}
          </Button>
          <p className="text-xs text-ink3">{t('Signing in changes nothing for your devices: Tailscale DNS and other devices’ routes stay off until you choose a use above.')}</p>
        </>}
      </div>
    </Card>}
    <Card title={t('Tailscale')} action={status ? <Chip tone={status.error ? 'danger' : running ? 'ok' : 'default'}>{status.enabled ? t('On') : status.remote_access && status.backend_state === 'Running' ? t('Remote management only') : t('Off')}</Chip> : undefined}>
      <div className="space-y-3">
        {!status && !error && <p role="status" className="text-sm text-ink2">{t('Loading…')}</p>}
        {error && <p role="alert" className="text-sm text-danger">{tm(error)}</p>}
        {status && <>
          <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-[13px]">
            <dt className="text-ink3">{t('Daemon state')}</dt><dd className="break-all">{status.backend_state}</dd>
            {status.account && <><dt className="text-ink3">{t('Account')}</dt><dd className="break-all">{status.account}</dd></>}
            <dt className="text-ink3">{t('Device name')}</dt><dd className="break-all">{status.self?.name || '—'}</dd>
            <dt className="text-ink3">{t('Tailscale IPs')}</dt><dd className="break-all">{status.self?.ips.join(', ') || '—'}</dd>
            <dt className="text-ink3">{t('Peer connection')}</dt><dd>{running && status.self?.online ? t('Online') : t('Offline')}</dd>
          </dl>
          {status.error && <p role="alert" className="text-sm text-danger">{tm(status.error)}</p>}
        </>}
        <p className="text-xs text-ink3">{t('Daemon status does not verify LAN internet forwarding.')}</p>
        <div className="flex flex-wrap gap-2">
          <Button variant="primary" disabled={blocked || !!running || !signedIn} loading={working === 'connect'} onClick={() => void mutate('connect')}>{t('Connect')}</Button>
          <Button variant="danger" disabled={blocked || !status?.enabled} loading={working === 'disconnect'} onClick={() => void mutate('disconnect')}>{t('Disconnect')}</Button>
          <Button disabled={!!working || !!pending || settingsBusy} onClick={() => void refresh()}>{t('Refresh')}</Button>
          {stage === 'ready' && <Button variant="ghost" className="ml-auto text-danger" disabled={blocked} loading={working === 'logout'} onClick={() => void mutate('logout')}>{t('Sign out')}</Button>}
        </div>
        {working && <p role="status" className="text-xs text-ink2">{t('Applying. This can take up to two minutes.')}</p>}
      </div>
    </Card>
    {remote !== 'unsupported' && status && signedIn && <Card title={t('Remote management (subnet route)')} action={remoteChip}>
      <div className="space-y-3">
        <p className="text-[13px] text-ink2">{t('Reach this router from any device on your tailnet — SSH, OpenUI and the LAN — wherever you are. Works alongside Clash and WireGuard.')}</p>
        <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-[13px]">
          <dt className="text-ink3">{t('LAN subnet')}</dt><dd className="break-all tnum">{status.lan_cidr || '—'}</dd>
          <dt className="text-ink3">{t('Advertised')}</dt><dd className="break-all tnum">{(status.advertised_routes ?? []).join(', ') || t('No')}</dd>
          <dt className="text-ink3">{t('Approved in admin console')}</dt><dd>{status.route_approved ? t('Yes') : t('No')}</dd>
        </dl>
        {status.remote_error && <p role="alert" className="text-sm text-danger">{tm(status.remote_error)}</p>}
        {remote === 'pending' && <div className="rounded-xl bg-warn/10 p-3 text-[13px] text-ink2">
          <p className="font-semibold text-ink">{t('One more step: approve the route')}</p>
          <p className="mt-1">{t('Open the Tailscale admin console → Machines → {name} → Edit route settings, and tick {lan}.', { name: status.self?.name || '—', lan: status.lan_cidr ?? '' })}</p>
          <a className="mt-1 inline-block font-semibold text-accent underline" href={ADMIN} target="_blank" rel="noreferrer">login.tailscale.com/admin/machines</a>
        </div>}
        {remote === 'active' && <div className="rounded-xl bg-ok/10 p-3 text-[13px] text-ink2">
          <p className="font-semibold text-ink">{t('From any device on your tailnet:')}</p>
          {/* i18n-ignore: a shell command */}
          <p className="mt-1 font-mono">ssh root@{lanIp}</p>
          <p className="font-mono">http://{lanIp}:8080</p>
          <p className="mt-1 text-xs text-ink3">{t('That device must accept subnet routes (on by default in the macOS, iOS and Android apps; tailscale up --accept-routes on Linux).')}</p>
        </div>}
        <div className="flex flex-wrap gap-2">
          <Button variant={status.remote_access ? 'danger' : 'primary'} disabled={blocked} loading={working === 'remote_access'} onClick={() => void mutate('remote_access')}>
            {status.remote_access ? t('Turn off remote management') : t('Turn on remote management')}
          </Button>
        </div>
      </div>
    </Card>}
    {signedIn && <Card title={t('Tailscale exit node')}>
      <div className="space-y-3">
        <Field label={t('Exit node')} hint={t('Only online eligible exit nodes are offered. The agent checks eligibility again before applying.')}>
          <Select value={selected} disabled={!canSelect} onChange={event => { dirty.current = true; setSelected(event.target.value) }}>
            <option value="">{t('None (no exit node)')}</option>
            {selectedMissing && <option value={selected} disabled>{t('Selected exit is unavailable')}</option>}
            {nodes.map(node => <option key={node.id} value={node.id}>{node.name || node.ip} — {node.ip}{node.hostname && node.hostname !== node.name ? ` · ${node.hostname}` : ''}</option>)}
          </Select>
        </Field>
        {!canSelect && <p className="text-xs text-ink2">{t('Connect Tailscale mode before changing the exit node.')}</p>}
        {status && nodes.length === 0 && <p className="text-xs text-ink2">{t('No online eligible exit nodes.')}</p>}
        <Button variant="primary" disabled={!canSelect || selected === status?.selected_exit_node || selectedMissing} loading={working === 'select_exit'} onClick={() => void mutate('select_exit')}>{t('Apply exit node')}</Button>
      </div>
    </Card>}
  </div>
}
