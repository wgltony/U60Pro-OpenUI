const { test } = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')
const vm = require('node:vm')
const ts = require('typescript')

function load(relative) {
  const context = { exports: {}, require: name => assert.fail(`unexpected import ${name}`) }
  const source = fs.readFileSync(path.join(__dirname, relative), 'utf8')
  vm.runInNewContext(ts.transpileModule(source, { compilerOptions: {
    module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022,
  } }).outputText, context)
  return context.exports
}
const { cfState, dailyMb, gainMs, mbps } = load('../src/features/proxy/cfoptModel.ts')
const probe = (ip, colo, tls, kbps = null) => ({ ip, colo, connect_ms: 30, tls_ms: tls, speed_kbps: kbps })
const base = {
  revision: 1, enabled: true, interval_hours: 4, intervals: [0, 4], speed_test: true, running: false, phase: null,
  applied: null, applied_at: 0, baseline: null, candidates: [], nodes: [], last_run: 0, last_attempt: 0, error: null,
  next_run: null, tested: 0, data_kb: 0, last_run_age_s: null, next_run_in_s: null, clash_running: true,
}

test('the card leads with off, running, Clash off, no node, error, waiting or active', () => {
  assert.equal(cfState({ ...base, enabled: false, running: true }), 'off')
  assert.equal(cfState({ ...base, running: true, clash_running: false }), 'running')
  assert.equal(cfState({ ...base, clash_running: false }), 'clash_off')
  assert.equal(cfState({ ...base, error: 'No node in the active subscription runs through Cloudflare' }), 'no_node')
  assert.equal(cfState({ ...base, error: 'Clash is not running', applied: probe('1.1.1.1', 'EWR', 200) }), 'error')
  assert.equal(cfState(base), 'waiting')
  assert.equal(cfState({ ...base, applied: probe('1.1.1.1', 'EWR', 200) }), 'active')
})

test('speeds, daily data and the gain over the domain’s own address', () => {
  assert.equal(mbps(21666), '22')
  assert.equal(mbps(5400), '5.4')
  assert.equal(mbps(null), null)
  assert.equal(dailyMb({ ...base, interval_hours: 0 }), null)
  assert.equal(dailyMb(base), 39)
  assert.equal(dailyMb({ ...base, speed_test: false, interval_hours: 6 }), 2)
  assert.equal(dailyMb({ ...base, data_kb: 6312, interval_hours: 4 }), 38)
  const applied = probe('162.159.45.145', 'EWR', 221)
  assert.equal(gainMs({ ...base, applied, baseline: probe('172.67.203.173', 'ORD', 390) }), 169)
  assert.equal(gainMs({ ...base, applied, baseline: applied }), null)
  assert.equal(gainMs({ ...base, applied }), null)
})
