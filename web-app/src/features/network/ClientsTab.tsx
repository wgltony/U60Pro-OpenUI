import { useState } from 'react'
import { api } from '../../data/api'
import { usePoll } from '../../data/poll'
import type { Client, UsbStatus, WifiBlocklist } from '../../types'
import { ICable, ILaptop, IRefresh, IUsb, IWifi } from '../../icons'
import { Button } from '../../ui/controls'
import { confirm, toast, toastError } from '../../ui/feedback'
import { Card, Chip, Empty, Skeleton } from '../../ui/primitives'
import { t } from '../../i18n'

function formatLinkMbps(value?: number) {
  if (value == null || value <= 0) return '\u2014'
  return `${Math.round(value)} Mbps`
}

function formatBitrate(mbps?: number) {
  if (mbps == null || mbps <= 0) return null
  return mbps >= 1000 ? `${mbps / 1000} Gbit/s` : `${mbps} Mbit/s`
}

function formatWifiLink(client: Client) {
  const parts: string[] = []
  if (client.tx_bitrate_mbps != null) parts.push(`TX ${client.tx_bitrate_mbps.toFixed(0)}`)
  if (client.rx_bitrate_mbps != null) parts.push(`RX ${client.rx_bitrate_mbps.toFixed(0)}`)
  return parts.length > 0 ? `${parts.join(' / ')} Mbps` : '\u2014'
}

function groupClients(clients: Client[]) {
  return {
    wifi: clients.filter((c) => c.medium === 'wifi'),
    usb: clients.filter((c) => c.medium === 'usb-c'),
    ethernet: clients.filter((c) => c.medium === 'ethernet'),
    other: clients.filter((c) => !c.medium || c.medium === 'wired'),
  }
}

const TH_CLS = 'pb-1.5 pr-4 font-semibold'
const TD_CLS = 'py-2 pr-4'

export default function ClientsTab() {
  // Clients is an expensive endpoint (iw station dump + bridge fdb + arp) —
  // poll slowly and offer a manual refresh instead.
  const { data, refreshing, refresh } = usePoll(
    'clients',
    async () => {
      // Agents without the blocklist API answer 404: the block controls stay hidden.
      const [clients, usb, blocklist] = await Promise.all([api.clients(), api.usbStatus().catch(() => null), api.wifiBlocked().catch(() => null)])
      return { clients: clients ?? [], usb, blocklist }
    },
    15000,
  )
  const [busy, setBusy] = useState('')

  async function setBlocked(mac: string, blocked: boolean, name?: string) {
    if (busy) return
    if (blocked && !(await confirm({
      title: t('Block {name}?', { name: name || mac }),
      body: t('It is disconnected from the hotspot and cannot rejoin until you unblock it. If it comes back with a new random MAC under the same name, it is blocked again.'),
      confirmLabel: t('Block'),
      danger: true,
    }))) return
    setBusy(mac)
    try {
      await api.wifiBlockSet(mac, blocked, name)
      toast(blocked ? t('{name} blocked.', { name: name || mac }) : t('{name} unblocked.', { name: name || mac }))
      await refresh()
    } catch (e) {
      toastError(e, blocked ? t('Unable to block the device') : t('Unable to unblock the device'))
    } finally {
      setBusy('')
    }
  }

  if (!data) {
    return (
      <div className="space-y-3">
        <Skeleton className="h-20" />
        <Skeleton className="h-56" />
      </div>
    )
  }

  const { clients, usb, blocklist } = data as { clients: Client[]; usb: UsbStatus | null; blocklist: WifiBlocklist | null }
  const canBlock = !!blocklist && !blocklist.allowlist
  const grouped = groupClients(clients)
  const usbLink = usb?.link
  const usbNegotiatedRate = formatBitrate(usbLink?.negotiated_mbps)
  const usbMaxRate = formatBitrate(usbLink?.max_mbps)

  return (
    <div className="space-y-3">
      <Card
        title={t('Connected clients ({count})', { count: clients.length })}
        action={
          <Button size="sm" variant="ghost" onClick={refresh} loading={refreshing}>
            <IRefresh size={13} /> {t('Refresh')}
          </Button>
        }
      >
        {clients.length === 0 ? (
          <Empty icon={<ILaptop size={28} />} title={t('No clients connected')} />
        ) : (
          <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
            {[
              { label: 'Wi-Fi', count: grouped.wifi.length, icon: <IWifi size={15} /> },
              { label: 'USB-C', count: grouped.usb.length, icon: <IUsb size={15} /> },
              { label: t('Ethernet'), count: grouped.ethernet.length, icon: <ICable size={15} /> },
              { label: t('Other'), count: grouped.other.length, icon: <ILaptop size={15} /> },
            ].map((g) => (
              <div key={g.label} className="rounded-lg bg-surface2/70 px-3 py-2.5">
                <div className="flex items-center gap-1.5 text-ink3">
                  {g.icon}
                  <p className="text-[10px] font-semibold uppercase tracking-wider">{g.label}</p>
                </div>
                <p className="tnum mt-1 text-2xl font-bold text-ink">{g.count}</p>
              </div>
            ))}
          </div>
        )}
      </Card>

      {grouped.wifi.length > 0 && (
        <Card title={`Wi-Fi (${grouped.wifi.length})`} pad={false}>
          <div className="overflow-x-auto px-4 pb-3">
            <table className="w-full text-[13px]">
              <thead>
                <tr className="border-b border-line/8 text-left text-[11px] uppercase tracking-wider text-ink3">
                  <th className={TH_CLS}>{t('Hostname')}</th>
                  <th className={TH_CLS}>IP</th>
                  <th className={TH_CLS}>{t('Radio')}</th>
                  <th className={TH_CLS}>{t('Signal')}</th>
                  <th className={TH_CLS}>{t('Link')}</th>
                  <th className="pb-1.5 font-semibold">MAC</th>
                  {canBlock && <th className="pb-1.5" />}
                </tr>
              </thead>
              <tbody>
                {grouped.wifi.map((c) => (
                  <tr key={c.mac} className="border-b border-line/6 last:border-0">
                    <td className={`${TD_CLS} font-medium text-ink`}>{c.hostname || '\u2014'}</td>
                    <td className={`${TD_CLS} tnum font-mono text-[12px] text-ink2`}>{c.ip ?? '\u2014'}</td>
                    <td className={TD_CLS}>
                      <Chip tone={c.wifi_band === '5 GHz' ? 'accent' : 'ok'}>{c.wifi_band ?? 'Wi-Fi'}</Chip>
                    </td>
                    <td className={`${TD_CLS} tnum text-ink2`}>
                      {c.signal_dbm != null ? `${c.signal_dbm} dBm` : '\u2014'}
                    </td>
                    <td className={`${TD_CLS} tnum text-ink2`}>{formatWifiLink(c)}</td>
                    <td className="tnum py-2 font-mono text-[11px] text-ink3">{c.mac}</td>
                    {canBlock && <td className="py-2 pl-4 text-right">
                      <Button size="sm" variant="ghost" className="whitespace-nowrap text-danger" loading={busy === c.mac} disabled={!!busy}
                        onClick={() => void setBlocked(c.mac, true, c.hostname)}>{t('Block')}</Button>
                    </td>}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </Card>
      )}

      {blocklist && (blocklist.blocked.length > 0 || blocklist.allowlist) && (
        <Card title={t('Blocked devices ({count})', { count: blocklist.blocked.length })}>
          {blocklist.allowlist ? <p className="text-[13px] text-ink2">{t('Wi-Fi allowlist mode is on; blocking is unavailable')}</p> : <>
            <div className="divide-y divide-line/8">
              {blocklist.blocked.map((b) => (
                <div key={b.mac} className="flex items-center gap-3 py-2">
                  <div className="min-w-0 flex-1">
                    <p className="truncate text-[13px] font-medium text-ink">{b.name || '—'}</p>
                    <p className="tnum font-mono text-[11px] text-ink3">{b.mac}</p>
                  </div>
                  <Button size="sm" variant="outline" loading={busy === b.mac} disabled={!!busy} onClick={() => void setBlocked(b.mac, false, b.name ?? undefined)}>{t('Unblock')}</Button>
                </div>
              ))}
            </div>
            <p className="mt-2 text-xs text-ink3">{t('Unblocking one entry also unblocks the other MACs saved under the same name. Blocking is shared with the device screen.')}</p>
          </>}
        </Card>
      )}

      {(grouped.usb.length > 0 || usbLink) && (
        <Card title={`USB-C (${grouped.usb.length})`} pad={false}>
          <div className="px-4 pb-3">
            {usbLink && (
              <div className="mb-3 flex flex-wrap items-center gap-2 rounded-lg bg-surface2/70 px-3 py-2">
                <span className="text-[10px] font-semibold uppercase tracking-wider text-ink3">{t('Tether link')}</span>
                <span className="text-[13px] font-bold text-ink">
                  {usbLink.negotiated_label ?? usbLink.negotiated ?? t('Unknown')}
                  {usbNegotiatedRate && <span className="font-medium text-ink2"> · {usbNegotiatedRate}</span>}
                </span>
                {usbLink.at_full_speed === false && usbMaxRate && (
                  <Chip tone="warn">
                    {usbLink.max_label != null ? t('{max} capable · {rate} — cable/port limiting', { max: usbLink.max_label, rate: usbMaxRate }) : t('Higher speed capable · {rate} — cable/port limiting', { rate: usbMaxRate })}
                  </Chip>
                )}
                {usbLink.at_full_speed === true && <Chip tone="ok">{t('Full speed')}</Chip>}
              </div>
            )}
            {grouped.usb.length > 0 ? (
              <div className="overflow-x-auto">
                <table className="w-full text-[13px]">
                  <thead>
                    <tr className="border-b border-line/8 text-left text-[11px] uppercase tracking-wider text-ink3">
                      <th className={TH_CLS}>{t('Hostname')}</th>
                      <th className={TH_CLS}>IP</th>
                      <th className={TH_CLS}>{t('Interface')}</th>
                      <th className="pb-1.5 font-semibold">MAC</th>
                    </tr>
                  </thead>
                  <tbody>
                    {grouped.usb.map((c) => (
                      <tr key={c.mac} className="border-b border-line/6 last:border-0">
                        <td className={`${TD_CLS} font-medium text-ink`}>{c.hostname || '\u2014'}</td>
                        <td className={`${TD_CLS} tnum font-mono text-[12px] text-ink2`}>{c.ip ?? '\u2014'}</td>
                        <td className={`${TD_CLS} tnum font-mono text-[12px] text-ink2`}>{c.interface ?? '\u2014'}</td>
                        <td className="tnum py-2 font-mono text-[11px] text-ink3">{c.mac}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            ) : (
              <p className="text-[13px] text-ink3">{t('No USB-C clients connected')}</p>
            )}
          </div>
        </Card>
      )}

      {grouped.ethernet.length > 0 && (
        <Card title={t('Ethernet ({count})', { count: grouped.ethernet.length })} pad={false}>
          <div className="overflow-x-auto px-4 pb-3">
            <table className="w-full text-[13px]">
              <thead>
                <tr className="border-b border-line/8 text-left text-[11px] uppercase tracking-wider text-ink3">
                  <th className={TH_CLS}>{t('Hostname')}</th>
                  <th className={TH_CLS}>IP</th>
                  <th className={TH_CLS}>{t('Speed')}</th>
                  <th className="pb-1.5 font-semibold">MAC</th>
                </tr>
              </thead>
              <tbody>
                {grouped.ethernet.map((c) => (
                  <tr key={c.mac} className="border-b border-line/6 last:border-0">
                    <td className={`${TD_CLS} font-medium text-ink`}>{c.hostname || '\u2014'}</td>
                    <td className={`${TD_CLS} tnum font-mono text-[12px] text-ink2`}>{c.ip ?? '\u2014'}</td>
                    <td className={`${TD_CLS} tnum text-ink2`}>{formatLinkMbps(c.wired_link_mbps)}</td>
                    <td className="tnum py-2 font-mono text-[11px] text-ink3">{c.mac}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </Card>
      )}

      {grouped.other.length > 0 && (
        <Card title={t('Other ({count})', { count: grouped.other.length })} pad={false}>
          <div className="overflow-x-auto px-4 pb-3">
            <table className="w-full text-[13px]">
              <thead>
                <tr className="border-b border-line/8 text-left text-[11px] uppercase tracking-wider text-ink3">
                  <th className={TH_CLS}>{t('Hostname')}</th>
                  <th className={TH_CLS}>IP</th>
                  <th className="pb-1.5 font-semibold">MAC</th>
                </tr>
              </thead>
              <tbody>
                {grouped.other.map((c) => (
                  <tr key={c.mac} className="border-b border-line/6 last:border-0">
                    <td className={`${TD_CLS} font-medium text-ink`}>{c.hostname || '\u2014'}</td>
                    <td className={`${TD_CLS} tnum font-mono text-[12px] text-ink2`}>{c.ip ?? '\u2014'}</td>
                    <td className="tnum py-2 font-mono text-[11px] text-ink3">{c.mac}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </Card>
      )}
    </div>
  )
}
