const { test } = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')
const vm = require('node:vm')
const ts = require('typescript')

// Transpile the dependency-free browser session, following test-esim.cjs.
// All APIs, state and clocks here are synthetic: never contact the device.
function load() {
  const context = { exports: {}, require: () => { throw new Error('Unexpected runtime dependency') } }
  const source = fs.readFileSync(path.join(__dirname, '../src/data/adminServices.ts'), 'utf8')
  vm.runInNewContext(ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  }).outputText, context)
  return context.exports
}
const plain = x => JSON.parse(JSON.stringify(x))
const fixture = () => ({ revision: 0, stamp: 1, busy: false, adb_running: true, adb_exposed: true,
  adb_boot: true, ssh_running: true, ssh_boot: false, adb_available: true, ssh_available: true })
const deferred = () => { let resolve; const promise = new Promise(r => { resolve = r }); return { promise, resolve } }
function setup() {
  const { AdminServiceSession } = load()
  let clock = 100
  const state = { remote: fixture(), writes: [], reads: 0, views: [] }
  const session = new AdminServiceSession({
    read: async () => { state.reads++; return { ...state.remote } },
    write: async body => {
      state.writes.push(plain(body))
      state.remote = { ...state.remote, revision: state.remote.revision + 1,
        [`${body.service}_${body.action === 'boot' ? 'boot' : 'running'}`]: body.enabled }
      return { ...state.remote }
    },
  }, view => state.views.push(view), () => clock)
  return { state, session, tick: ms => { clock += ms } }
}

test('service schema rejects absent, malformed, overflow and nonboolean data without false OFF', () => {
  const { parseAdminServices } = load()
  assert.equal(parseAdminServices(fixture()).revision, 0)
  assert.equal(parseAdminServices(fixture()).stamp, 1) // Device epoch need not match browser clock.
  for (const key of Object.keys(fixture())) {
    const missing = fixture(); delete missing[key]
    assert.throws(() => parseAdminServices(missing), /Invalid service status/)
  }
  for (const key of ['busy', 'adb_running', 'adb_exposed', 'adb_boot', 'ssh_running', 'ssh_boot', 'adb_available', 'ssh_available']) {
    for (const bad of [0, 1, '0', 'false', null, undefined]) assert.throws(() => parseAdminServices({ ...fixture(), [key]: bad }))
  }
  for (const revision of [-1, 0.5, Number.MAX_SAFE_INTEGER + 1, '1']) assert.throws(() => parseAdminServices({ ...fixture(), revision }))
  for (const stamp of [0, -1, Infinity, '1']) assert.throws(() => parseAdminServices({ ...fixture(), stamp }))
  assert.deepEqual(plain(parseAdminServices({ ...fixture(), private_extra: 'synthetic-do-not-retain' })), fixture())
})

test('initial load, polling and cancelled confirmations never write', async () => {
  const { session, state } = setup()
  assert.equal(session.fresh(), false)
  assert.equal(await session.change('adb', 'runtime', false, async () => true), 'blocked')
  await session.refresh(); await session.refresh()
  assert.equal(session.fresh(), true)
  assert.equal(await session.change('adb', 'runtime', false, async () => false), 'cancelled')
  assert.equal(state.writes.length, 0)
  assert.equal(state.reads, 2)
})

test('four independent exact request bodies; boot never changes runtime and vice versa', async () => {
  for (const [service, action, enabled] of [['adb', 'runtime', false], ['adb', 'boot', false], ['ssh', 'runtime', false], ['ssh', 'boot', true]]) {
    const { session, state } = setup()
    await session.refresh()
    const before = { ...state.remote }
    assert.equal(await session.change(service, action, enabled, async () => true), 'applied')
    assert.deepEqual(state.writes, [{ revision: 0, service, action, enabled }])
    assert.equal(state.reads, 2) // Re-read immediately after confirmation, before PUT.
    assert.equal(state.remote[`${service}_${action === 'runtime' ? 'boot' : 'running'}`], before[`${service}_${action === 'runtime' ? 'boot' : 'running'}`])
  }
})

test('confirmation locks synchronously, rejects double actions, expires without replay', async () => {
  const { session, state, tick } = setup(); await session.refresh()
  const gate = deferred(); let confirms = 0
  const first = session.change('adb', 'runtime', false, () => { confirms++; return gate.promise })
  assert.equal(session.view.working, true)
  assert.equal(await session.change('ssh', 'boot', true, async () => { confirms++; return true }), 'blocked')
  await session.refresh(); assert.equal(state.reads, 1)
  tick(10_001); gate.resolve(true)
  assert.equal(await first, 'stale'); assert.equal(confirms, 1); assert.equal(state.writes.length, 0)
  assert.equal(session.canChange('adb', 'runtime', false), false)
  await session.refresh(); assert.equal(state.writes.length, 0)
})

test('revision or live-state change during confirmation never auto-follows the new state', async () => {
  for (const patch of [{ revision: 1 }, { busy: true }, { adb_running: false }, { adb_available: false }]) {
    const { session, state } = setup(); await session.refresh()
    const result = await session.change('adb', 'runtime', false, async () => { Object.assign(state.remote, patch); return true })
    assert.equal(result, 'stale'); assert.equal(state.writes.length, 0)
  }
})

test('ADB USB exposure is not daemon state; no start without an existing function', async () => {
  const { session, state } = setup()
  state.remote.adb_exposed = false; state.remote.adb_running = false; await session.refresh()
  assert.equal(session.canChange('adb', 'runtime', true), false)
  assert.equal(await session.change('adb', 'runtime', true, async () => true), 'blocked')
  state.remote.adb_running = true; await session.refresh()
  assert.equal(session.canChange('adb', 'runtime', false), true)
  state.remote.adb_available = false; await session.refresh()
  assert.equal(session.canChange('adb', 'boot', false), false)
  assert.equal(state.writes.length, 0)
})

test('offline, 409 and unverified writes invalidate state; no retries or optimistic OFF', async () => {
  const { AdminServiceSession } = load()
  for (const outcome of ['read-error', '409', 'wrong-state', 'busy']) {
    let reads = 0, writes = 0
    const session = new AdminServiceSession({
      read: async () => { reads++; if (outcome === 'read-error' && reads > 1) throw new Error('offline'); return fixture() },
      write: async () => {
        writes++
        if (outcome === '409') throw new Error('409')
        return { ...fixture(), busy: outcome === 'busy' }
      },
    }, () => {}, () => 10)
    await session.refresh()
    assert.equal(await session.change('adb', 'runtime', false, async () => true), 'failed')
    assert.equal(session.view.status, null); assert.equal(session.fresh(), false)
    assert.equal(session.view.working, false); assert.equal(writes, outcome === 'read-error' ? 0 : 1)
  }
})

test('unmount ignores late GET and cancels pending confirmations before write', async () => {
  const { AdminServiceSession } = load(); const gate = deferred(); const views = []
  const first = new AdminServiceSession({ read: () => gate.promise, write: async () => { throw new Error('must not write') } }, v => views.push(v), () => 1)
  const reading = first.refresh(); first.dispose(); const count = views.length; gate.resolve(fixture()); await reading
  assert.equal(views.length, count)
  const { session, state } = setup(); await session.refresh(); const confirmation = deferred()
  const changing = session.change('adb', 'runtime', false, () => confirmation.promise)
  session.dispose(); confirmation.resolve(true); assert.equal(await changing, 'blocked'); assert.equal(state.writes.length, 0)
})

test('settings integration removes unsupported Debug composition and mounts shared controls after USB', () => {
  const source = fs.readFileSync(path.join(__dirname, '../src/features/system/SettingsTab.tsx'), 'utf8')
  assert.ok(!source.includes("'debug'") && !source.includes('Debug (ADB)'))
  assert.match(source, /<UsbSection\s*\/>\s*<ServiceAccessSection\s*\/>/)
  const ui = fs.readFileSync(path.join(__dirname, '../src/features/system/ServiceAccessSection.tsx'), 'utf8')
  for (const pair of ["control('adb', 'runtime')", "control('adb', 'boot')", "control('ssh', 'runtime')", "control('ssh', 'boot')"]) assert.ok(ui.includes(pair))
  assert.ok(ui.includes('Unknown / stale') && ui.includes('USB exposure: {exposure}'))
})
