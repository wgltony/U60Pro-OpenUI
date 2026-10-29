import type { AdminService, AdminServiceAction, AdminServicesStatus, AdminServiceUpdate } from '../types'

const BOOLS = ['busy', 'adb_running', 'adb_exposed', 'adb_boot', 'ssh_running', 'ssh_boot', 'adb_available', 'ssh_available'] as const
export function parseAdminServices(value: unknown): AdminServicesStatus {
  if (!value || typeof value !== 'object') throw new Error('Invalid service status') // i18n-ignore: internal diagnostic, never rendered
  const v = value as Record<string, unknown>
  if (!Number.isSafeInteger(v.revision) || (v.revision as number) < 0 ||
      !Number.isSafeInteger(v.stamp) || (v.stamp as number) <= 0 ||
      BOOLS.some(key => typeof v[key] !== 'boolean')) throw new Error('Invalid service status') // i18n-ignore: internal diagnostic, never rendered
  // Deliberately project only this public schema; never retain arbitrary backend fields.
  return {
    revision: v.revision as number, stamp: v.stamp as number,
    busy: v.busy as boolean, adb_running: v.adb_running as boolean,
    adb_exposed: v.adb_exposed as boolean, adb_boot: v.adb_boot as boolean,
    ssh_running: v.ssh_running as boolean, ssh_boot: v.ssh_boot as boolean,
    adb_available: v.adb_available as boolean, ssh_available: v.ssh_available as boolean,
  }
}

export interface AdminServiceView {
  status: AdminServicesStatus | null
  receivedAt: number
  loading: boolean
  working: boolean
  failed: boolean
}
export type AdminChangeResult = 'applied' | 'cancelled' | 'blocked' | 'stale' | 'failed'
export function adminValue(s: AdminServicesStatus, service: AdminService, action: AdminServiceAction): boolean {
  return service === 'adb' ? (action === 'runtime' ? s.adb_running : s.adb_boot) :
    (action === 'runtime' ? s.ssh_running : s.ssh_boot)
}
function eligible(s: AdminServicesStatus, service: AdminService, action: AdminServiceAction, enabled: boolean): boolean {
  return !s.busy && (service === 'adb' ? s.adb_available : s.ssh_available) &&
    !(service === 'adb' && action === 'runtime' && enabled && !s.adb_exposed) &&
    adminValue(s, service, action) !== enabled
}

/** One mounted card's read/confirm/write session. No optimistic writes, replay or retries.
 * Freshness uses local receipt age, NOT device epoch: device/browser clocks can differ.
 * Before a confirmed write, re-read and compare the captured revision + desired field.
 */
export class AdminServiceSession {
  view: AdminServiceView = { status: null, receivedAt: 0, loading: false, working: false, failed: false }
  private alive = true
  private generation = 0
  private readonly api: { read: () => Promise<unknown>; write: (body: AdminServiceUpdate) => Promise<unknown> }
  private readonly changed: (view: AdminServiceView) => void
  private readonly now: () => number
  constructor(
    api: { read: () => Promise<unknown>; write: (body: AdminServiceUpdate) => Promise<unknown> },
    changed: (view: AdminServiceView) => void,
    now: () => number = () => performance.now(),
  ) { this.api = api; this.changed = changed; this.now = now }
  private publish(patch: Partial<AdminServiceView>) {
    if (!this.alive) return
    this.view = { ...this.view, ...patch }
    this.changed(this.view)
  }
  fresh(): boolean {
    const age = this.now() - this.view.receivedAt
    return !!this.view.status && !this.view.failed && age >= 0 && age <= 10_000
  }
  canChange(service: AdminService, action: AdminServiceAction, enabled: boolean): boolean {
    return this.alive && !this.view.loading && !this.view.working && this.fresh() &&
      eligible(this.view.status!, service, action, enabled)
  }
  async refresh(): Promise<void> {
    if (!this.alive || this.view.loading || this.view.working) return
    const generation = ++this.generation
    this.publish({ loading: true })
    try {
      const status = parseAdminServices(await this.api.read())
      if (this.alive && generation === this.generation) this.publish({ status, receivedAt: this.now(), failed: false })
    } catch {
      if (this.alive && generation === this.generation) this.publish({ status: null, failed: true })
    } finally {
      if (this.alive && generation === this.generation) this.publish({ loading: false })
    }
  }
  async change(service: AdminService, action: AdminServiceAction, enabled: boolean,
    confirm: (status: AdminServicesStatus) => Promise<boolean>): Promise<AdminChangeResult> {
    if (!this.canChange(service, action, enabled)) return 'blocked'
    const initial = this.view.status!
    const body: AdminServiceUpdate = { revision: initial.revision, service, action, enabled }
    const generation = ++this.generation
    this.publish({ working: true }) // Immediate synchronous lock, including the confirmation dialog.
    try {
      if (!await confirm(initial)) return 'cancelled'
      if (!this.alive || generation !== this.generation) return 'blocked'
      if (!this.fresh()) return 'stale'
      const current = parseAdminServices(await this.api.read())
      if (!this.alive || generation !== this.generation) return 'blocked'
      this.publish({ status: current, receivedAt: this.now(), failed: false })
      if (current.revision !== body.revision || !eligible(current, service, action, enabled)) return 'stale'
      const saved = parseAdminServices(await this.api.write(body))
      if (!this.alive || generation !== this.generation) return 'blocked'
      if (saved.revision < body.revision || saved.busy || adminValue(saved, service, action) !== enabled) {
        throw new Error('Unverified service update') // i18n-ignore: internal diagnostic, never rendered
      }
      this.publish({ status: saved, receivedAt: this.now(), failed: false })
      return 'applied'
    } catch {
      // Includes 409: invalidate and require an explicit fresh read + new confirmation.
      // A lost HTTP response can follow a successful device action; never auto-replay it.
      this.publish({ status: null, failed: true })
      return 'failed'
    } finally {
      if (this.alive && generation === this.generation) this.publish({ working: false })
    }
  }
  dispose() { this.alive = false; ++this.generation }
}
