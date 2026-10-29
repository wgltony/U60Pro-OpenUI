const { test } = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')
const vm = require('node:vm')
const ts = require('typescript')
const fixture = require('./tailscale-fixture.json')

function load(relative, client = {}) {
  const context = { exports: {}, require: name => {
    if (name === './adminServices') return load('../src/data/adminServices.ts')
    assert.equal(name, './client')
    return client
  } }
  const source = fs.readFileSync(path.join(__dirname, relative), 'utf8')
  vm.runInNewContext(ts.transpileModule(source, { compilerOptions: {
    module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022,
  } }).outputText, context)
  return context.exports
}
const plain = value => JSON.parse(JSON.stringify(value))

test('Tailscale uses the existing client and exact native GET/PUT contract', async () => {
  const calls = []
  const { api } = load('../src/data/api.ts', {
    get: async url => { calls.push({ method: 'GET', url }); return fixture },
    req: async (method, url, body, token, timeout) => { calls.push({ method, url, body: plain(body), token, timeout }); return fixture },
  })
  assert.deepEqual(plain(await api.tailscale()), fixture)
  for (const body of [
    { expected_revision: 7, action: 'connect' },
    { expected_revision: 7, action: 'disconnect' },
    { expected_revision: 7, action: 'select_exit', exit_node: 'peer-exit-demo' },
    { expected_revision: 7, action: 'select_exit', exit_node: '' },
  ]) {
    assert.deepEqual(plain(await api.tailscaleSet(body)), fixture)
    assert.deepEqual(calls.at(-1), { method: 'PUT', url: '/api/tailscale', body, token: undefined, timeout: 120000 })
  }
  assert.deepEqual(calls[0], { method: 'GET', url: '/api/tailscale' })
  await api.proxySet({ backend: 'tailscale', expected_revision: 7 })
  assert.deepEqual(calls.at(-1).body, { backend: 'tailscale', expected_revision: 7 })
  assert.equal(calls.at(-1).url, '/api/proxy')
})

test('exit choices exclude offline peers and duplicates; clear uses an empty string', () => {
  const { eligibleExitNodes, tailscaleUpdate } = load('../src/features/proxy/tailscaleModel.ts')
  const status = { ...fixture, exit_nodes: [...fixture.exit_nodes,
    { id: 'offline', name: 'Offline', ip: '100.64.0.30', online: false },
    { ...fixture.exit_nodes[0] }, { id: '', name: 'Invalid', ip: '', online: true },
  ] }
  assert.deepEqual(plain(eligibleExitNodes(status)), fixture.exit_nodes)
  assert.equal(eligibleExitNodes(status)[0].name, 'us-west-exit')
  assert.equal(eligibleExitNodes(status)[0].hostname, 'instance-demo')
  assert.deepEqual(plain(tailscaleUpdate(status, 'select_exit', '')), { expected_revision: 7, action: 'select_exit', exit_node: '' })
  assert.deepEqual(plain(tailscaleUpdate(status, 'select_exit', 'peer-exit-demo')), { expected_revision: 7, action: 'select_exit', exit_node: 'peer-exit-demo' })
  assert.throws(() => tailscaleUpdate(status, 'select_exit', 'offline'), /current online exit node/)
  assert.throws(() => tailscaleUpdate(status, 'select_exit', '100.64.0.20'), /current online exit node/)
  assert.deepEqual(plain(tailscaleUpdate(status, 'connect', 'ignored')), { expected_revision: 7, action: 'connect' })
  assert.deepEqual(plain(tailscaleUpdate(status, 'disconnect')), { expected_revision: 7, action: 'disconnect' })
})

test('shared-client failures are propagated unchanged for standard 401/409 handling', async () => {
  for (const status of [401, 409, 503]) {
    const error = Object.assign(new Error('synthetic response'), { status })
    const { api } = load('../src/data/api.ts', { req: async () => { throw error } })
    await assert.rejects(api.tailscaleSet({ expected_revision: 7, action: 'connect' }), value => value === error)
  }
})

test('remote management toggles exactly and reports off/starting/pending/active/error', () => {
  const { tailscaleUpdate, remoteAccessState } = load('../src/features/proxy/tailscaleModel.ts')
  const base = { ...fixture, revision: 9, backend_state: 'Running', lan_cidr: '192.168.0.0/24', remote_error: null }
  assert.deepEqual(plain(tailscaleUpdate({ ...base, remote_access: false }, 'remote_access')), { expected_revision: 9, action: 'remote_access', enabled: true })
  assert.deepEqual(plain(tailscaleUpdate({ ...base, remote_access: true }, 'remote_access')), { expected_revision: 9, action: 'remote_access', enabled: false })
  // An older agent without the field: the card is not shown.
  assert.equal(remoteAccessState({ ...fixture }), 'unsupported')
  assert.equal(remoteAccessState({ ...base, remote_access: false }), 'off')
  assert.equal(remoteAccessState({ ...base, remote_access: true, advertised_routes: [] }), 'starting')
  assert.equal(remoteAccessState({ ...base, remote_access: true, backend_state: 'Starting', advertised_routes: ['192.168.0.0/24'] }), 'starting')
  assert.equal(remoteAccessState({ ...base, remote_access: true, advertised_routes: ['192.168.0.0/24'], route_approved: false }), 'pending')
  assert.equal(remoteAccessState({ ...base, remote_access: true, advertised_routes: ['192.168.0.0/24'], route_approved: true }), 'active')
  assert.equal(remoteAccessState({ ...base, remote_access: true, advertised_routes: ['192.168.0.0/24'], remote_error: 'Tailscale is not enrolled' }), 'error')
  assert.equal(remoteAccessState({ ...base, remote_access: true, lan_cidr: null }), 'error')
})

test('sign-in requests carry only a valid name and key; stages follow the daemon', () => {
  const { tailscaleUpdate, setupStage } = load('../src/features/proxy/tailscaleModel.ts')
  const base = { ...fixture, self: null, backend_state: 'NeedsLogin', installed: true, login_active: false }
  assert.deepEqual(plain(tailscaleUpdate(base, 'login', '', { hostname: ' u60-pro ', authKey: '' })), { expected_revision: 7, action: 'login', hostname: 'u60-pro' })
  const fakeKey = 'tsk' + 'ey-example-not-a-real-key' // split so secret scanners don't flag a fake test value
  assert.deepEqual(plain(tailscaleUpdate(base, 'login', '', { hostname: '', authKey: fakeKey })), { expected_revision: 7, action: 'login', auth_key: fakeKey })
  assert.deepEqual(plain(tailscaleUpdate(base, 'login')), { expected_revision: 7, action: 'login' })
  assert.throws(() => tailscaleUpdate(base, 'login', '', { hostname: 'bad name', authKey: '' }), /letters, digits and hyphens/)
  assert.throws(() => tailscaleUpdate(base, 'login', '', { hostname: '', authKey: 'not-a-key' }), /tskey-/)
  assert.deepEqual(plain(tailscaleUpdate(base, 'logout')), { expected_revision: 7, action: 'logout' })
  assert.deepEqual(plain(tailscaleUpdate(base, 'cancel_login')), { expected_revision: 7, action: 'cancel_login' })
  assert.equal(setupStage(fixture), 'unsupported')
  assert.equal(setupStage(base), 'signin')
  assert.equal(setupStage({ ...base, login_active: true }), 'waiting')
  assert.equal(setupStage({ ...base, backend_state: 'NeedsMachineAuth' }), 'approval')
  assert.equal(setupStage({ ...base, backend_state: 'Unavailable' }), 'unavailable')
  assert.equal(setupStage({ ...base, installed: false }), 'unavailable')
  assert.equal(setupStage({ ...base, backend_state: 'Stopped', self: fixture.self ?? { id: 'n1', name: 'r', ips: [], online: false } }), 'ready')
})
