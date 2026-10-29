import { useState } from 'react'
import type { ProxyBackendId } from '../../types'
import { Segmented } from '../../ui/controls'
import { Tabs } from '../../ui/Tabs'
import { ProxyProvider, useProxy } from './ProxyContext'
import BackendSelector from './BackendSelector'
import { ClashProvider } from './ClashContext'
import OverviewTab from './OverviewTab'
import SubscriptionsTab from './SubscriptionsTab'
import ProxiesTab from './ProxiesTab'
import DevicesTab from './DevicesTab'
import ConnectionsTab from './ConnectionsTab'
import LogsTab from './LogsTab'
import AdvancedTab from './AdvancedTab'
import WireGuardTab from './wireguard/WireGuardTab'
import TailscaleTab from './TailscaleTab'
import { PageHeader } from '../../ui/primitives'
import { t } from '../../i18n'

type ClashTab = 'overview' | 'subscriptions' | 'proxies' | 'devices' | 'connections' | 'logs' | 'advanced'

function ClashView() {
  const [tab, setTab] = useState<ClashTab>('overview')

  return (
    <ClashProvider>
      <Tabs
        tabs={[
          { id: 'overview', label: t('Overview') },
          { id: 'subscriptions', label: t('Subscriptions') },
          { id: 'proxies', label: t('Nodes') },
          { id: 'devices', label: t('Devices') },
          { id: 'connections', label: t('Connections') },
          { id: 'logs', label: t('Logs') },
          { id: 'advanced', label: t('Advanced') },
        ]}
        active={tab}
        onChange={setTab}
      />

      {tab === 'overview' && <OverviewTab onGo={setTab} />}
      {tab === 'subscriptions' && <SubscriptionsTab />}
      {tab === 'proxies' && <ProxiesTab />}
      {tab === 'devices' && <DevicesTab />}
      {tab === 'connections' && <ConnectionsTab />}
      {tab === 'logs' && <LogsTab />}
      {tab === 'advanced' && <AdvancedTab />}
    </ClashProvider>
  )
}

function Content() {
  const { status, error } = useProxy()
  // Which settings are shown: the user's pick, else the active backend's (Clash when off).
  const [view, setView] = useState<ProxyBackendId | null>(null)
  const shown = view ?? (status ? (status.active === 'off' ? 'clash' : status.active) : error ? 'clash' : null)

  return (
    <div className="space-y-4">
      <PageHeader title={t('Proxy')} sub={t('Choose WireGuard, Clash or Tailscale and configure each')} />

      {/* After a switch, show the backend just turned on. */}
      <BackendSelector onSwitched={target => setView(target === 'off' ? shown : target)} />

      {shown && <>
        <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
          <h2 className="text-[13px] font-semibold text-ink">{t('Configure')}</h2>
          <Segmented<ProxyBackendId> value={shown} onChange={setView}
            options={[{ value: 'wireguard', label: 'WireGuard' }, { value: 'clash', label: 'Clash' }, { value: 'tailscale', label: t('Tailscale') }]} />
        </div>
        {shown === 'clash' ? <ClashView /> : shown === 'tailscale' ? <TailscaleTab /> : <WireGuardTab />}
      </>}
    </div>
  )
}

/** Native network backends: the active-backend switch, then each one's settings. */
export default function ProxyPage() {
  return (
    <ProxyProvider>
      <Content />
    </ProxyProvider>
  )
}
