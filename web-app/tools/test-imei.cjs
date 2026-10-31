const { test } = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')
const vm = require('node:vm')
const ts = require('typescript')
const React = require('react')
const { renderToStaticMarkup } = require('react-dom/server')
const plain = x => JSON.parse(JSON.stringify(x))
const fixed = ['111111111111119', '222222222222228', '333333333333337', '444444444444446'] // Synthetic repeated-digit fixtures, NOT device values.
const op = 'a'.repeat(32), preview = 'b'.repeat(32)
const fixture = () => ({ protocol: 1, revision: 7, stamp: 1000, busy: false, phase: 'checked', code: 'ready', imei1: fixed[0], imei2: fixed[1], secondary: 'active', operation_id: op, preview_id: preview, target1_masked: '***********3337', target2_masked: null, allowed: ['check', 'preview'], preview_expires_in: 60 })
const deferred = () => { let resolve, reject; const promise = new Promise((a, b) => { resolve = a; reject = b }); return { promise, resolve, reject } }
function loader(lang = 'en', globals = {}) {
  const cache = new Map(), storage = new Map([['u60.lang', lang]])
  const context = { localStorage: { getItem: k => storage.get(k) ?? null, setItem: (k, v) => storage.set(k, v) }, document: { documentElement: {} }, ...globals }
  function load(file) {
    if (cache.has(file)) return cache.get(file).exports
    const module = { exports: {} }; cache.set(file, module)
    const code = ts.transpileModule(fs.readFileSync(file, 'utf8'), { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX } }).outputText
    const localRequire = spec => {
      if (!spec.startsWith('.')) return require(spec)
      if (spec.endsWith('/data/client')) return { req: () => { throw new Error('Fixtures must not call HTTP') } }
      const base = path.resolve(path.dirname(file), spec)
      return load([base + '.ts', base + '.tsx', path.join(base, 'index.ts')].find(fs.existsSync))
    }
    vm.runInNewContext(`(function(module,exports,require){${code}\n})`, context)(module, module.exports, localRequire)
    return module.exports
  }
  return { load: name => load(path.join(__dirname, '../src', name)), storage }
}
function setup() {
  const { ImeiSession } = loader().load('data/imei.ts')
  let clock = 1_000_000
  const state = { remote: fixture(), writes: [], reads: 0, views: [], failRead: false, failWrite: false, writeGate: null }
  const session = new ImeiSession({
    read: async () => { state.reads++; if (state.failRead) throw new Error('offline'); return plain(state.remote) },
    write: async body => { state.writes.push(plain(body)); if (state.writeGate) await state.writeGate.promise; if (state.failWrite) throw new Error('409 or timeout') },
  }, v => state.views.push(plain(v)), () => clock)
  return { session, state, tick: ms => { clock += ms }, sync: () => { state.remote.stamp = Math.floor(clock / 1000) } }
}
const authorize = (session, action) => { session.startConsent(action); session.risk(true); session.next(); session.confirmText(action === 'restore' ? 'RESTORE IMEI' : action === 'cancel' ? 'CANCEL IMEI' : action === 'reboot' ? 'REBOOT' : 'CHANGE IMEI') }

test('bare status schema: strict types, exact fields, safe integers, code/phase/token/masks and projection', () => {
  const { parseImeiStatus } = loader().load('data/imei.ts')
  assert.deepEqual(plain(parseImeiStatus(fixture())), fixture())
  for (const key of Object.keys(fixture())) { const missing = fixture(); delete missing[key]; assert.throws(() => parseImeiStatus(missing), /invalid_status/) }
  for (const patch of [{ protocol: '1' }, { revision: 0 }, { revision: Number.MAX_SAFE_INTEGER + 1 }, { stamp: 0 }, { stamp: 1.5 }, { busy: 1 }, { phase: 'unknown' }, { code: '<b>' }, { code: 'A'.repeat(49) }, { imei1: 111111111111119 }, { imei2: '１２３４５６７８９０１２３４５' }, { secondary: 'yes' }, { operation_id: 'A'.repeat(32) }, { preview_id: '' }, { target1_masked: '**********33337' }, { target2_masked: '***********abcd' }, { allowed: ['write'] }, { allowed: ['check', 'check'] }, { preview_expires_in: 61 }, { preview_expires_in: '0' }]) assert.throws(() => parseImeiStatus({ ...fixture(), ...patch }))
  assert.deepEqual(plain(parseImeiStatus({ ...fixture(), private_backup: 'never retained' })), fixture())
  assert.equal(parseImeiStatus({ ...fixture(), imei1: '111111111111118' }).imei1, '111111111111118') // Observed invalid original is not silently corrected.
})

test('strict Luhn and targets: no all-zero, Unicode, unchanged, duplicate or inactive secondary', () => {
  const { validImei, validTargets } = loader().load('data/imei.ts'), s = fixture()
  for (const x of fixed) assert.equal(validImei(x), true)
  for (const x of ['', '0'.repeat(15), '111111111111118', '１２３４５６７８９０１２３４５', '111111111111119\n', '11111111111111 ', '1111111111111190']) assert.equal(validImei(x), false)
  for (const pair of [[null, null], [fixed[0], null], [fixed[0], fixed[1]], [fixed[1], null], [fixed[2], fixed[2]], ['111111111111118', null]]) assert.equal(validTargets(s, pair), false)
  assert.equal(validTargets(s, [fixed[2], null]), true)
  assert.equal(validTargets({ ...s, secondary: 'inactive' }, [fixed[2], fixed[3]]), false)
})

test('cross-slot originals reject swap and interrupted duplicate hybrids without submitting', async () => {
  const { validTargets } = loader().load('data/imei.ts'), s = fixture()
  for (const pair of [[fixed[1], fixed[0]], [fixed[1], fixed[2]], [fixed[2], fixed[0]], [null, fixed[0]]]) {
    assert.equal(validTargets(s, pair), false)
    const { session, state } = setup(); await session.refresh(); session.edit(0, pair[0]); session.edit(1, pair[1]); await session.submit('preview'); assert.equal(state.writes.length, 0)
  }
  // Keeping or explicitly re-entering one record's own original is still allowed.
  for (const pair of [[fixed[0], fixed[2]], [fixed[2], fixed[1]], [null, fixed[2]], [fixed[2], null]]) assert.equal(validTargets(s, pair), true)
})

test('router +/- epoch offsets never gate CHECK, recovery or fresh consent; default clock is monotonic', async () => {
  let clock = 123
  const { ImeiSession } = loader('en', { performance: { now: () => clock }, Date: { now: () => { throw new Error('Wall clock must not be used') } } }).load('data/imei.ts')
  for (const stamp of [1, 999, 1001, 86_401, Number.MAX_SAFE_INTEGER]) {
    let remote = { ...fixture(), stamp, phase: 'recovery_required', code: 'recovery_required', allowed: ['check', 'verify', 'restore', 'begin'] }, writes = 0
    const s = new ImeiSession({ read: async () => plain(remote), write: async () => { writes++ } }, () => {})
    await s.refresh(); assert.equal(s.can('check'), true); assert.equal(s.can('verify'), true); assert.equal(s.can('restore'), true)
    authorize(s, 'begin'); remote.stamp = stamp === 1 ? Number.MAX_SAFE_INTEGER : 1; await s.refresh(); assert.equal(s.view.consent.step, 2)
    assert.equal(writes, 0); clock += 10_001; assert.equal(s.can('check'), false); await s.submit('begin'); assert.equal(writes, 0); s.dispose()
  }
})

test('delayed responses and real local age fail closed; request latency deducts preview lifetime', async () => {
  const { ImeiSession } = loader().load('data/imei.ts')
  for (const delay of [10_001, -1]) {
    let clock = 500; const gate = deferred(); let writes = 0
    const s = new ImeiSession({ read: () => gate.promise, write: async () => { writes++ } }, () => {}, () => clock)
    const read = s.refresh(); clock += delay; gate.resolve(fixture()); await read
    assert.equal(s.view.status, null); assert.equal(s.can('check'), false); await s.submit('check'); assert.equal(writes, 0)
  }
  let clock = 500, writes = 0; const gate = deferred()
  const s = new ImeiSession({ read: () => gate.promise, write: async () => { writes++ } }, () => {}, () => clock)
  const read = s.refresh(); clock += 1500; gate.resolve({ ...fixture(), allowed: ['begin', 'check'], preview_expires_in: 1 }); await read
  assert.equal(s.can('check'), true); assert.equal(s.can('begin'), false); authorize(s, 'begin'); await s.submit('begin'); assert.equal(writes, 0)
  clock += 8501; assert.equal(s.can('check'), false); await s.submit('check'); assert.equal(writes, 0) // Request latency also consumes the 10s status-age budget.
})

test('polling cannot renew an expired revision/token deadline even across a failed read', async () => {
  const { session, state, tick } = setup(); state.remote.allowed = ['begin']; state.remote.preview_expires_in = 1
  await session.refresh(); authorize(session, 'begin'); tick(500); await session.refresh(); assert.equal(session.can('begin'), true)
  tick(501); await session.refresh(); assert.equal(session.can('begin'), false); assert.equal(session.view.consent, null)
  state.failRead = true; await session.refresh(); state.failRead = false; state.remote.preview_expires_in = 60; await session.refresh(true)
  assert.equal(session.can('begin'), false); await session.submit('begin'); assert.equal(state.writes.length, 0)
  state.remote.revision++; state.remote.preview_id = 'c'.repeat(32); await session.refresh(true); assert.equal(session.can('begin'), true); assert.equal(state.writes.length, 0)
})

test('entry/poll/edit/keyboard completion and cancelled confirmations never submit', async () => {
  const { session, state } = setup(); assert.equal(session.can('check'), false)
  await session.refresh(); await session.refresh(); assert.equal(state.writes.length, 0)
  session.edit(0, ''); assert.equal(session.view.targets[0], '')
  session.edit(0, fixed[2]); session.edit(1, null); assert.deepEqual(plain(session.view.targets), [fixed[2], null]); assert.equal(state.writes.length, 0)
  session.edit(0, '１２３'); assert.equal(session.view.targets[0], fixed[2])
  state.remote = { ...state.remote, phase: 'preview', code: 'preview_ready', allowed: ['begin'] }; await session.refresh()
  session.startConsent('begin'); assert.equal(session.view.consent.risk, false); session.next(); assert.equal(session.view.consent.step, 1)
  session.risk(true); session.next(); assert.equal(session.view.consent.step, 2); session.cancelConsent(); await session.submit('begin'); assert.equal(state.writes.length, 0)
})

test('preview sends exactly two keep/input targets and queued response is not success', async () => {
  const { session, state } = setup(); await session.refresh(); session.edit(0, fixed[2]); await session.submit('preview')
  assert.deepEqual(state.writes, [{ action: 'preview', revision: 7, targets: [fixed[2], null] }]); assert.equal(session.view.waiting, true); assert.equal(session.view.status.phase, 'checked')
  await session.submit('preview'); await session.refresh(); assert.equal(state.writes.length, 1); assert.equal(session.view.waiting, true)
  state.remote = { ...state.remote, revision: 8, phase: 'preview', code: 'preview_ready', allowed: ['begin'] }; await session.refresh(); assert.equal(session.view.waiting, false)
})

test('restore default sends only the revision, needs valid originals and renders only when offered', async () => {
  const { session, state } = setup(); state.remote.allowed = ['check', 'preview', 'restore_default']; await session.refresh()
  session.edit(0, fixed[2]); await session.submit('restore_default')
  assert.deepEqual(state.writes, [{ action: 'restore_default', revision: 7 }]); assert.equal(session.view.waiting, true)
  const bad = setup(); bad.state.remote = { ...bad.state.remote, imei1: '111111111111118', allowed: ['restore_default'] }; await bad.session.refresh()
  assert.equal(bad.session.can('restore_default'), false); await bad.session.submit('restore_default'); assert.equal(bad.state.writes.length, 0)
  const none = setup(); await none.session.refresh(); assert.equal(none.session.can('restore_default'), false)
  for (const lang of ['zh', 'en']) {
    const l = loader(lang), { ImeiControls } = l.load('features/signal/ImeiPage.tsx'), { ImeiSession } = l.load('data/imei.ts'), label = lang === 'zh' ? '恢复默认 IMEI' : 'Restore default IMEI'
    for (const allowed of [['check', 'preview'], ['check', 'preview', 'restore_default']]) {
      const s = new ImeiSession({ read: async () => ({ ...fixture(), allowed }), write: async () => { throw new Error('must not write during rendering') } }, () => {}, () => 1_000_000)
      await s.refresh()
      assert.equal(renderToStaticMarkup(React.createElement(ImeiControls, { view: plain(s.view), session: s })).includes(label), allowed.includes('restore_default'))
    }
  }
})

test('all typed action bodies and explicit two-stage consent with exact confirmation text', async () => {
  for (const action of ['check', 'begin', 'continue', 'verify', 'restore', 'reboot', 'cancel']) {
    const { session, state } = setup(); state.remote.allowed = [action]; await session.refresh()
    if (!['check', 'verify'].includes(action)) {
      session.startConsent(action); await session.submit(action); assert.equal(state.writes.length, 0)
      session.risk(true); session.next(); session.confirmText('wrong'); await session.submit(action); assert.equal(state.writes.length, 0)
      session.confirmText(action === 'restore' ? 'RESTORE IMEI' : action === 'cancel' ? 'CANCEL IMEI' : action === 'reboot' ? 'REBOOT' : 'CHANGE IMEI')
    }
    await session.submit(action)
    const expected = { action, revision: 7 }
    if (action === 'begin') Object.assign(expected, { preview_id: preview, acknowledge_risk: true, confirm_text: 'CHANGE IMEI' })
    else if (action !== 'check') Object.assign(expected, { operation_id: op })
    if (['continue', 'restore', 'cancel'].includes(action)) Object.assign(expected, { acknowledge_risk: true, confirm_text: action === 'restore' ? 'RESTORE IMEI' : action === 'cancel' ? 'CANCEL IMEI' : 'CHANGE IMEI' })
    if (action === 'reboot') expected.acknowledge_reboot = true
    assert.deepEqual(state.writes, [expected])
  }
})

test('revision, token, busy and expiry fail closed during consent independently of epoch', async () => {
  for (const patch of [{ revision: 8 }, { preview_id: 'c'.repeat(32) }, { busy: true }, { allowed: [] }, { preview_expires_in: 0 }, { code: 'unknown_code' }]) {
    const { session, state } = setup(); state.remote.allowed = ['begin']; await session.refresh(); authorize(session, 'begin')
    state.remote = { ...state.remote, ...patch }; await session.refresh(); await session.submit('begin'); assert.equal(state.writes.length, 0); assert.equal(session.view.consent, null)
  }
  const { session, state, tick } = setup(); state.remote.allowed = ['begin']; state.remote.preview_expires_in = 1; await session.refresh(); authorize(session, 'begin'); tick(1001); await session.submit('begin'); assert.equal(state.writes.length, 0)
})

test('unknown codes allow only explicitly allowed check/verify; unavailable and invalid original block writes', async () => {
  const { session, state } = setup(); state.remote.allowed = ['check', 'preview', 'begin', 'continue', 'verify', 'restore', 'reboot', 'cancel']; state.remote.code = 'unknown_safe_code'; await session.refresh()
  assert.equal(session.can('check'), true); assert.equal(session.can('verify'), true)
  for (const a of ['preview', 'begin', 'continue', 'restore', 'reboot', 'cancel']) assert.equal(session.can(a), false)
  state.remote.phase = 'unsupported'; await session.refresh(); assert.equal(session.can('check'), false)
  state.remote.phase = 'checked'; state.remote.code = 'ready'; state.remote.imei1 = '111111111111118'; await session.refresh()
  for (const a of ['preview', 'begin', 'continue']) assert.equal(session.can(a), false)
})

test('double clicks and outstanding reads reserve synchronous exclusion', async () => {
  const { session, state } = setup(); await session.refresh(); state.writeGate = deferred()
  const first = session.submit('check'); await session.submit('check'); await session.refresh(); assert.equal(state.writes.length, 1); assert.equal(state.reads, 1)
  state.writeGate.resolve(); await first
  const { ImeiSession } = loader().load('data/imei.ts'), gate = deferred(); let reads = 0, writes = 0
  const s = new ImeiSession({ read: async () => { reads++; return gate.promise }, write: async () => { writes++ } }, () => {}, () => 1_000_000)
  const read = s.refresh(); await s.refresh(); await s.submit('check'); assert.equal(reads, 1); assert.equal(writes, 0); gate.resolve(fixture()); await read
})

test('timeout/offline/409 are not replayed; automatic reads cannot clear uncertainty', async () => {
  const { session, state } = setup(); await session.refresh(); state.failWrite = true; await session.submit('check')
  assert.equal(state.writes.length, 1); assert.equal(session.view.status, null); assert.equal(session.view.error, 'outcome_unknown')
  state.failWrite = false; await session.refresh(); await session.submit('check'); assert.equal(state.writes.length, 1)
  await session.refresh(true); assert.equal(session.view.error, null); assert.equal(state.writes.length, 1)
  state.failRead = true; await session.refresh(); assert.equal(session.view.status, null); await session.submit('check'); assert.equal(state.writes.length, 1)
})

test('unmount ignores late replies, wipes inputs and does not replay on reopen or complete', async () => {
  const { session, state } = setup(); await session.refresh(); session.edit(0, fixed[2]); state.writeGate = deferred()
  const write = session.submit('check'); session.dispose(); const views = state.views.length; state.writeGate.resolve(); await write
  assert.equal(state.views.length, views); assert.deepEqual(plain(session.view.targets), [null, null]); assert.equal(session.view.status, null)
  const { ImeiSession } = loader().load('data/imei.ts'), gate = deferred(); let emitted = 0
  const s = new ImeiSession({ read: () => gate.promise, write: async () => { throw new Error('must not write') } }, () => { emitted++ }, () => 1_000_000)
  const read = s.refresh(); s.dispose(); gate.resolve(fixture()); await read; assert.equal(emitted, 0)
  const reopened = setup(); await reopened.session.refresh(); assert.deepEqual(plain(reopened.session.view.targets), [null, null]); assert.equal(reopened.state.writes.length, 0)
  reopened.session.edit(0, fixed[2]); reopened.state.remote.phase = 'complete'; await reopened.session.refresh(); assert.deepEqual(plain(reopened.session.view.targets), [null, null])
})

test('offline bilingual compact-width view fixtures render all phases and both confirmations', async () => {
  for (const lang of ['zh', 'en']) {
    const l = loader(lang), { ImeiControls } = l.load('features/signal/ImeiPage.tsx'), { ImeiSession, imeiPhases } = l.load('data/imei.ts')
    const s = new ImeiSession({ read: async () => fixture(), write: async () => { throw new Error('must not write during rendering') } }, () => {}, () => 1_000_000)
    await s.refresh()
    for (const phase of imeiPhases) {
      const view = plain(s.view); view.status.phase = phase
      const html = renderToStaticMarkup(React.createElement('div', { style: { width: 320, minHeight: 480 } }, React.createElement(ImeiControls, { view, session: s })))
      assert.match(html, /width:320px/); assert.match(html, /min-height:480px/)
      assert.ok(html.includes(lang === 'zh' ? '受保护的 IMEI 管理' : 'Guarded IMEI controls')); assert.ok(!html.includes('localStorage') && !html.includes('http://'))
    }
    for (const step of [1, 2]) {
      const view = plain(s.view); view.consent = { action: 'begin', revision: 7, token: preview, risk: false, step, text: '', opened: 1_000_000 }
      const html = renderToStaticMarkup(React.createElement(ImeiControls, { view, session: s }))
      assert.ok(html.includes(lang === 'zh' ? '取消本次确认' : 'Cancel confirmation')); assert.ok(html.includes('***********3337'))
      assert.ok(html.includes(step === 1 ? (lang === 'zh' ? '进入第二次确认' : 'Next confirmation') : (lang === 'zh' ? '第二次确认' : 'Second confirmation')))
    }
  }
})

test('revision high-water survives failed reads; missing tokens and expired consent cannot submit', async () => {
  const { session, state, tick, sync } = setup(); await session.refresh(); state.failRead = true; await session.refresh()
  state.failRead = false; state.remote.revision = 6; await session.refresh(true); assert.equal(session.view.status, null); await session.submit('check'); assert.equal(state.writes.length, 0)
  state.remote.revision = 8; state.remote.allowed = ['begin', 'verify']; state.remote.preview_id = null; state.remote.operation_id = null; await session.refresh(true)
  assert.equal(session.can('begin'), false); assert.equal(session.can('verify'), false)
  state.remote.preview_id = preview; await session.refresh(true); authorize(session, 'begin'); tick(60_001); sync(); await session.refresh(); await session.submit('begin'); assert.equal(state.writes.length, 0)
})

test('cancelled/restored/verified success clears inputs without an optimistic UI change', async () => {
  for (const code of ['cancelled', 'restored', 'verified']) {
    const { session, state } = setup(); await session.refresh(); session.edit(0, fixed[2]); state.remote.code = code; await session.refresh(); assert.deepEqual(plain(session.view.targets), [null, null]); assert.equal(state.writes.length, 0)
  }
})

test('shared HTTP client authenticates bare IMEI GET/typed POST and does not retry 401/409', async () => {
  const requests = [], events = []
  let status = 200, payload = fixture()
  const context = { exports: {}, AbortController, setTimeout, clearTimeout,
    Event: class { constructor(type) { this.type = type } },
    window: { location: { hostname: '192.0.2.1' }, dispatchEvent: e => events.push(e.type) },
    sessionStorage: { getItem: () => 'synthetic-session-token', removeItem: () => {}, setItem: () => {} },
    fetch: async (url, options) => { requests.push({ url, options }); return { ok: status === 200, status, json: async () => payload } },
  }
  const source = fs.readFileSync(path.join(__dirname, '../src/data/client.ts'), 'utf8').replaceAll('import.meta.env.MODE', JSON.stringify('production'))
  vm.runInNewContext(ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText, context)
  const { req } = context.exports
  assert.deepEqual(plain(await req('GET', '/api/imei', undefined, undefined, 15_000, undefined, true, 'bare')), fixture())
  assert.equal(requests[0].url, 'http://192.0.2.1:9090/api/imei'); assert.equal(requests[0].options.headers.Authorization, 'Bearer synthetic-session-token'); assert.equal(requests[0].options.body, undefined)
  payload = { ok: true }; await req('POST', '/api/imei', { action: 'check', revision: 7 })
  assert.deepEqual(JSON.parse(requests[1].options.body), { action: 'check', revision: 7 })
  status = 409; payload = { ok: false, error: 'stale_revision' }; await assert.rejects(req('POST', '/api/imei', { action: 'check', revision: 7 }), e => e.status === 409); assert.equal(requests.length, 3)
  status = 401; payload = { ok: false, error: 'unauthorized' }; await assert.rejects(req('GET', '/api/imei', undefined, undefined, 15_000, undefined, true, 'bare'), e => e.status === 401); assert.equal(requests.length, 4); assert.deepEqual(events, ['zte-auth-expired'])
})

test('signal entry and real page wire shared authenticated bare GET + POST, no storage or form submit', () => {
  const source = fs.readFileSync(path.join(__dirname, '../src/features/signal/ImeiPage.tsx'), 'utf8')
  const group = fs.readFileSync(path.join(__dirname, '../src/features/signal/SignalGroup.tsx'), 'utf8')
  assert.match(group, /id: 'imei'/); assert.match(group, /<ImeiPage/)
  assert.match(source, /req\('GET', '\/api\/imei'.*'bare'\)/); assert.match(source, /req\('POST', '\/api\/imei', body\)/)
  assert.match(source, /s\.dispose\(\)/); assert.match(source, /clearInterval/)
  for (const forbidden of ['localStorage', 'sessionStorage', 'console.', '<form', 'onSubmit', 'URLSearchParams']) assert.ok(!source.includes(forbidden))
  assert.match(source, /inputMode="numeric"/); assert.match(source, /autoComplete="off"/)
})
