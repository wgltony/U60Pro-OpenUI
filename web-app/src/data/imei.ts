// One shared backend, cached reads only. This dependency-free session never retries a POST.
export const imeiPhases = ['idle', 'checking', 'checked', 'preview', 'preparing', 'reboot_to_enable', 'ready_to_write', 'writing', 'reboot_to_verify', 'complete', 'recovery_required', 'unsupported', 'error'] as const
export const imeiActions = ['check', 'preview', 'restore_default', 'begin', 'continue', 'verify', 'restore', 'reboot', 'cancel'] as const
export type ImeiActionName = typeof imeiActions[number]
export type ImeiPhase = typeof imeiPhases[number]
export interface ImeiStatus {
  protocol: 1; revision: number; stamp: number; busy: boolean; phase: ImeiPhase; code: string
  imei1: string | null; imei2: string | null; secondary: 'active' | 'inactive' | 'unknown'
  operation_id: string | null; preview_id: string | null
  target1_masked: string | null; target2_masked: string | null
  allowed: ImeiActionName[]; preview_expires_in: number
}
export type ImeiAction = { action: 'check' | 'restore_default'; revision: number }
  | { action: 'preview'; revision: number; targets: [string | null, string | null] }
  | { action: 'begin'; revision: number; preview_id: string; acknowledge_risk: true; confirm_text: string }
  | { action: 'continue' | 'restore' | 'cancel'; revision: number; operation_id: string; acknowledge_risk: true; confirm_text: string }
  | { action: 'verify'; revision: number; operation_id: string }
  | { action: 'reboot'; revision: number; operation_id: string; acknowledge_reboot: true }
const normalCodes = new Set(['not_checked', 'ready', 'preview_ready', 'working', 'awaiting_first_reboot', 'awaiting_write_confirmation', 'awaiting_final_reboot', 'verified', 'restored', 'cancelled', 'recovery_required'])
const digits = (x: unknown): x is string => typeof x === 'string' && /^[0-9]{15}$/.test(x)
const token = (x: unknown): x is string => typeof x === 'string' && /^[0-9a-f]{32}$/.test(x)
const safeInt = (x: unknown, min: number, max = Number.MAX_SAFE_INTEGER): x is number => typeof x === 'number' && Number.isSafeInteger(x) && x >= min && x <= max
export function validImei(x: string): boolean {
  if (!digits(x) || x === '000000000000000') return false
  return [...x].reduce((sum, c, i) => { let d = Number(c); if (i % 2) { d *= 2; if (d > 9) d -= 9 }; return sum + d }, 0) % 10 === 0
}
export function parseImeiStatus(value: unknown): ImeiStatus {
  const s = value as Record<string, unknown> | null
  const nullable = (x: unknown, test: (x: unknown) => boolean) => x === null || test(x)
  if (!s || typeof s !== 'object' || Array.isArray(s) || s.protocol !== 1 || !safeInt(s.revision, 1) || !safeInt(s.stamp, 1) || typeof s.busy !== 'boolean' ||
    !imeiPhases.includes(s.phase as ImeiPhase) || typeof s.code !== 'string' || !/^[a-z0-9_]{1,48}$/.test(s.code) ||
    !nullable(s.imei1, digits) || !nullable(s.imei2, digits) || !['active', 'inactive', 'unknown'].includes(s.secondary as string) ||
    !nullable(s.operation_id, token) || !nullable(s.preview_id, token) ||
    !nullable(s.target1_masked, x => typeof x === 'string' && /^\*{11}[0-9]{4}$/.test(x)) ||
    !nullable(s.target2_masked, x => typeof x === 'string' && /^\*{11}[0-9]{4}$/.test(x)) ||
    !Array.isArray(s.allowed) || s.allowed.some(a => !imeiActions.includes(a)) || new Set(s.allowed).size !== s.allowed.length || !safeInt(s.preview_expires_in, 0, 60)) throw new Error('invalid_status')
  // Explicit projection: private/unknown fields never enter component state.
  return { protocol: 1, revision: s.revision, stamp: s.stamp, busy: s.busy, phase: s.phase as ImeiPhase, code: s.code,
    imei1: s.imei1 as string | null, imei2: s.imei2 as string | null, secondary: s.secondary as ImeiStatus['secondary'],
    operation_id: s.operation_id as string | null, preview_id: s.preview_id as string | null,
    target1_masked: s.target1_masked as string | null, target2_masked: s.target2_masked as string | null,
    allowed: [...s.allowed] as ImeiActionName[], preview_expires_in: s.preview_expires_in }
}
export function validTargets(s: ImeiStatus, targets: readonly [string | null, string | null]): boolean {
  const [a, b] = targets
  if (a === null && b === null) return false
  if ((a !== null && !validImei(a)) || (b !== null && !validImei(b)) || (s.secondary !== 'active' && b !== null)) return false
  if ((a === null || a === s.imei1) && (b === null || b === s.imei2)) return false
  // Any interrupted per-record apply must remain a distinct, recoverable snapshot.
  if ((a !== null && a === s.imei2) || (b !== null && b === s.imei1)) return false
  return s.secondary !== 'active' || (a ?? s.imei1) !== (b ?? s.imei2)
}
export const imeiConfirmation = (a: ImeiActionName) => a === 'restore' ? 'RESTORE IMEI' : a === 'cancel' ? 'CANCEL IMEI' : a === 'reboot' ? 'REBOOT' : 'CHANGE IMEI'
export interface ImeiConsent { action: ImeiActionName; revision: number; token: string; step: 1 | 2; risk: boolean; text: string; opened: number }
export interface ImeiView { status: ImeiStatus | null; targets: [string | null, string | null]; consent: ImeiConsent | null; working: boolean; waiting: boolean; error: string | null }
export class ImeiSession {
  view: ImeiView = { status: null, targets: [null, null], consent: null, working: false, waiting: false, error: null }
  private active = true
  private reading = false
  private received = 0
  private requested = 0
  private waitingRevision: number | null = null
  private uncertain = false
  private seenRevision = 0
  private previewBinding: { revision: number; token: string; until: number } | null = null
  private readonly io: { read: () => Promise<unknown>; write: (body: ImeiAction) => Promise<void> }
  private readonly changed: (view: ImeiView) => void
  private readonly now: () => number
  constructor(io: { read: () => Promise<unknown>; write: (body: ImeiAction) => Promise<void> }, changed: (view: ImeiView) => void, now = () => performance.now()) { this.io = io; this.changed = changed; this.now = now }
  private emit() { if (this.active) this.changed({ ...this.view, targets: [...this.view.targets], consent: this.view.consent ? { ...this.view.consent } : null }) }
  fresh(): boolean {
    const s = this.view.status, now = this.now()
    // Router epoch is only structurally validated: clocks need not be synchronized.
    return this.active && !!s && now >= this.received && now - this.requested <= 10_000
  }
  can(action: ImeiActionName): boolean {
    const s = this.view.status
    return this.fresh() && !!s && !this.uncertain && !this.view.working && !this.view.waiting && !s.busy && s.phase !== 'unsupported' &&
      s.allowed.includes(action) && (action === 'check' || action === 'verify' || normalCodes.has(s.code)) &&
      (!['preview', 'restore_default', 'begin', 'continue'].includes(action) || (!s.imei1 || validImei(s.imei1)) && (!s.imei2 || validImei(s.imei2))) &&
      (action !== 'begin' || (token(s.preview_id) && s.preview_expires_in > 0 && this.previewBinding?.revision === s.revision && this.previewBinding.token === s.preview_id && this.now() < this.previewBinding.until)) &&
      (!['continue', 'restore', 'cancel', 'reboot', 'verify'].includes(action) || token(s.operation_id))
  }
  async refresh(manual = false): Promise<void> {
    if (!this.active || this.reading || this.view.working) return
    this.reading = true
    const started = this.now()
    try {
      const s = parseImeiStatus(await this.io.read())
      if (!this.active) return
      const now = this.now()
      if (now < started || now - started > 10_000 || s.revision < this.seenRevision) throw new Error('stale_status')
      if (s.preview_id !== null) {
        // Deduct full request latency and never extend the same revision/token by polling.
        const until = started + s.preview_expires_in * 1000, prior = this.previewBinding
        this.previewBinding = { revision: s.revision, token: s.preview_id, until: prior?.revision === s.revision && prior.token === s.preview_id ? Math.min(prior.until, until) : until }
      }
      this.view.status = s; this.received = now; this.requested = started; this.seenRevision = s.revision
      if (this.waitingRevision !== null && s.revision > this.waitingRevision) { this.waitingRevision = null; this.view.waiting = false }
      if (manual) { this.uncertain = false; this.view.error = null }
      if (s.secondary !== 'active') this.view.targets[1] = null
      if (s.phase === 'complete' || ['verified', 'restored', 'cancelled'].includes(s.code)) { this.view.targets = [null, null]; this.view.consent = null }
      const c = this.view.consent
      if (c && (c.revision !== s.revision || c.token !== (c.action === 'begin' ? s.preview_id : s.operation_id) || !this.can(c.action))) { this.view.consent = null; this.view.error = 'stale_status' }
    } catch {
      if (!this.active) return
      this.view.status = null; this.view.consent = null; this.uncertain = true; this.view.error = 'refresh_required'
    } finally { this.reading = false; this.emit() }
  }
  edit(index: 0 | 1, value: string | null) {
    if (!this.can('preview') || this.view.consent || (index === 1 && this.view.status?.secondary !== 'active') || (value !== null && !/^[0-9]{0,15}$/.test(value))) return
    this.view.targets[index] = value; this.emit()
  }
  cancelConsent() { this.view.consent = null; this.emit() }
  startConsent(action: ImeiActionName) {
    if (!['begin', 'continue', 'restore', 'cancel', 'reboot'].includes(action) || !this.can(action) || this.view.consent) return
    const s = this.view.status!, id = action === 'begin' ? s.preview_id : s.operation_id
    if (!token(id)) return
    this.view.consent = { action, revision: s.revision, token: id, step: 1, risk: false, text: '', opened: this.now() }; this.emit()
  }
  risk(value: boolean) { if (this.view.consent?.step === 1) { this.view.consent.risk = value; this.emit() } }
  next() { const c = this.view.consent; if (c?.step === 1 && c.risk && this.can(c.action)) { c.step = 2; this.emit() } }
  confirmText(value: string) { if (this.view.consent?.step === 2) { this.view.consent.text = value.slice(0, 24); this.emit() } }
  async submit(action: ImeiActionName): Promise<void> {
    if (!this.can(action) || this.reading) return
    const s = this.view.status!, c = this.view.consent
    let body: ImeiAction
    // restore_default carries no targets: the router previews its own recorded default.
    if (action === 'check' || action === 'restore_default') body = { action, revision: s.revision }
    else if (action === 'preview') {
      if (!validTargets(s, this.view.targets)) return
      body = { action, revision: s.revision, targets: [...this.view.targets] }
    } else if (action === 'verify') body = { action, revision: s.revision, operation_id: s.operation_id! }
    else {
      if (!c || c.action !== action || c.step !== 2 || !c.risk || c.text !== imeiConfirmation(action) || c.revision !== s.revision || this.now() < c.opened || this.now() - c.opened > 60_000 || c.token !== (action === 'begin' ? s.preview_id : s.operation_id)) return
      if (action === 'begin') body = { action, revision: c.revision, preview_id: c.token, acknowledge_risk: true, confirm_text: c.text }
      else if (action === 'reboot') body = { action, revision: c.revision, operation_id: c.token, acknowledge_reboot: true }
      else body = { action, revision: c.revision, operation_id: c.token, acknowledge_risk: true, confirm_text: c.text }
    }
    this.view.working = true; this.view.consent = null; this.emit()
    try {
      await this.io.write(body)
      if (!this.active) return
      this.waitingRevision = s.revision; this.view.waiting = true
      // Queue acknowledgement is not a durable write verification.
    } catch {
      if (!this.active) return
      this.uncertain = true; this.view.error = 'outcome_unknown'; this.view.status = null
    } finally { this.view.working = false; this.emit() }
  }
  dispose() { this.active = false; this.view.targets = [null, null]; this.view.consent = null; this.view.status = null }
}
