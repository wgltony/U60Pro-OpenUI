"""Host-only lifecycle injection: real transaction shell, fake proc/procd/uci.

B31's Lua/luci.jsonc JSON adapter is deliberately replaced by a strict Python
adapter here; these tests do not claim to execute Lua or any device service.
Fake lifecycle signals never target host PIDs. The shell's identity checks and
journal run as-is; lock-inheritance tests use explicitly owned host subprocesses.
"""
import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import select
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[1]
BIN = '/data/bin/tailscaled'
STATE = '/data/tailscale'
TS_ARGV = [BIN, f'--state={STATE}/tailscaled.state', f'--statedir={STATE}',
           '--socket=/var/run/tailscale/tailscaled.sock', '--tun=tailscale0', '--port=41641']

BACKEND = r'''#!/usr/bin/env python3
import hashlib,json,os,shutil,sys
from pathlib import Path
root=Path(os.environ['U60_TEST_ROOT'])
op,*args=sys.argv[1:]
with (root/'events.log').open('a') as log: log.write(json.dumps([op,*args])+'\n')
fail=(root/'fail').read_text().splitlines() if (root/'fail').exists() else []
if op in fail or ':'.join([op,*args]) in fail: sys.exit(71)
def write(path, text):
    path.parent.mkdir(parents=True,exist_ok=True); path.write_text(text)
def persistent_runtime_child():
    # Opt-in only: inherit the real shell descriptors just like nohup/daemon
    # startup, but never execute a device service or act on a supplied PID.
    if os.environ.get('U60_TEST_PERSIST_RUNTIME') != ':'.join([op,*args]): return
    status=int(os.environ['U60_TEST_CHILD_STATUS_FD'])
    parent_gate=int(os.environ['U60_TEST_PARENT_GATE_FD'])
    child_gate=int(os.environ['U60_TEST_CHILD_GATE_FD'])
    pid=os.fork()
    if pid==0:
        try:
            os.setsid()
            null=os.open(os.devnull,os.O_RDWR)
            for fd in (0,1,2): os.dup2(null,fd)
            if null>2: os.close(null)
            os.close(parent_gate)
            os.write(status,('child '+str(os.getpid())+'\n').encode())
            while os.read(child_gate,1)==b'?': os.write(status,b'child-alive\n')
            os.write(status,b'child-exit\n')
        finally: os._exit(0)
    os.close(child_gate)
    os.write(status,b'parent-ready\n')
    os.read(parent_gate,1)
persistent_runtime_child()
def spawn(kind):
    pid=int((root/'next-pid').read_text()) if (root/'next-pid').exists() else 800
    write(root/'next-pid',str(pid+1))
    binary={'agent':'/data/zte-agent','guard':'/data/bin/relay-band-guard.sh',
            'tailscale':'/data/bin/tailscaled','dashboard':'/data/bin/dashboard-uhttpd','dropbear':'/data/bin/dropbear'}[kind]
    assert (root/binary[1:]).exists(), 'starting absent binary'
    argv=[binary]; exe=binary
    if kind=='guard': argv=['/bin/sh',binary]; exe='/bin/busybox'
    if kind=='tailscale': argv=[binary,'--state=/data/tailscale/tailscaled.state','--statedir=/data/tailscale','--socket=/var/run/tailscale/tailscaled.sock','--tun=tailscale0','--port=41641']
    p=root/'proc'/str(pid); p.mkdir(parents=True)
    write(p/'stat',str(pid)+' (test comm) S '+' '.join(['0']*18+[str(pid*10)]+['0']*5))
    write(p/'status','Uid:\t0\t0\t0\t0\n')
    write(p/'exe.identity',f'1:{pid*100}')
    (p/'cmdline').write_bytes(b'\0'.join(a.encode() for a in argv)+b'\0')
    (p/'exe').symlink_to(exe)
    if kind=='tailscale':
        write(root/'procd.json',json.dumps({'openui-tailscale':{'instances':{'instance1':{'running':True,'pid':pid,'command':argv}}}}))
    if kind=='guard':
        write(root/'var/run/relay-band-guard.pid',f'{pid}|{pid*10}|{exe}|1:{pid*100}|'+hashlib.sha256((p/'cmdline').read_bytes()).hexdigest()+'\n')
    if kind in ('dropbear','dashboard'):
        port='08AE' if kind=='dropbear' else '1F90'
        (p/'fd').mkdir(); (p/'fd/3').symlink_to(f'socket:[{pid}]')
        with (root/'proc/net/tcp').open('a') as net: net.write(f'0: 00000000:{port} 00000000:0000 0A 0:0 00:0 0 0 0 {pid}\n')
if op=='procd-list': print((root/'procd.json').read_text() if (root/'procd.json').exists() else '{}')
elif op=='procd-parse':
    j=json.load(sys.stdin); assert isinstance(j,dict)
    if 'openui-tailscale' not in j:
        assert j=={}; print('absent'); sys.exit()
    s=j['openui-tailscale']; instances=s['instances']; assert isinstance(instances,dict) and len(instances)==1
    i=next(iter(instances.values())); c=i['command']
    assert isinstance(c,list) and len(c)==6 and i.get('user','root')=='root'
    assert c[0] in ('/data/bin/tailscaled','/data/mu5250-admin/bin/tailscaled')
    assert c[2].startswith('--statedir='); state=c[2].split('=',1)[1]
    assert state in ('/data/tailscale','/data/mu5250-admin/tailscale')
    assert c[1:]==['--state='+state+'/tailscaled.state','--statedir='+state,'--socket=/var/run/tailscale/tailscaled.sock','--tun=tailscale0','--port=41641']
    assert isinstance(i['running'],bool)
    if i['running']:
        assert type(i['pid']) is int and i['pid']>1
        print('running',i['pid'],c[0],state)
    else: print('stopped')
elif op=='procd-delete':
    if (root/'reuse-on-delete').exists():
        j=json.loads((root/'procd.json').read_text()); pid=next(iter(j['openui-tailscale']['instances'].values()))['pid']
        p=root/'proc'/str(pid); (p/'exe').unlink(); (p/'exe').symlink_to('/usr/bin/foreign')
        (p/'cmdline').write_bytes(b'/usr/bin/foreign\0')
    if (root/'zombie-on-delete').exists():
        j=json.loads((root/'procd.json').read_text()); pid=next(iter(j['openui-tailscale']['instances'].values()))['pid']
        stat=root/'proc'/str(pid)/'stat'; stat.write_text(stat.read_text().replace(') S ',') Z '))
    write(root/'procd.json','{}')
elif op=='exe-id': print((root/'proc'/args[0]/'exe.identity').read_text())
elif op=='signal':
    p=root/'proc'/args[0]; assert p.parent==root/'proc'
    shutil.rmtree(p)
    pf=root/'var/run/relay-band-guard.pid'
    if pf.exists() and pf.read_text().split('|')[0]==args[0]: pf.unlink()
elif op=='fota-get': print((root/'mode').read_text() if (root/'mode').exists() else '1')
elif op=='fota-call':
    if not (root/'fota-noapply').exists(): write(root/'mode',args[0])
    print((root/'fota-reply').read_text() if (root/'fota-reply').exists() else '{}')
elif op=='start':
    kind=args[0]
    if 'start-noop:'+kind in fail: sys.exit()
    if kind=='agent':
        assert (root/'data/local/tmp/start_zte_agent.sh').exists()
        spawn('agent')
        script=(root/'data/local/tmp/start_zte_agent.sh').read_text()
        if '#start-sidecars' in script:
            for child,path in [('guard','data/bin/relay-band-guard.sh'),('tailscale','etc/init.d/openui-tailscale')]:
                if (root/path).exists(): spawn(child)
    else: spawn(kind)
elif op in ('wait','http-restart'): pass
else: raise AssertionError(op)
'''


def make_backend(root):
    """Shared explicit empty-runtime backend for historical file snapshot tests."""
    root = Path(root)
    # The real firmware always mounts /data before installation starts.
    (root / 'data').mkdir(exist_ok=True)
    (root / 'shim').mkdir(exist_ok=True)
    backend = root / 'shim/runtime-backend'
    backend.write_text(BACKEND)
    backend.chmod(0o700)
    # Execute the production shell guard unchanged on macOS and non-root CI.
    # Only the explicit disposable U60_TEST_ROOT is modeled as UID 0. flock is
    # real host fcntl locking; Lua's small JSON predicate is a strict Python
    # adapter, not a claim that vendor Lua or real device services were tested.
    adapters = {
        'stat': '''#!/usr/bin/env python3
import os,sys
from pathlib import Path
assert sys.argv[1]=='-c'
p=Path(sys.argv[3]); p.resolve().relative_to(Path(os.environ['U60_TEST_ROOT']).resolve())
s=p.lstat(); assert s.st_uid==os.getuid()
v={'%u':'0','%a':format(s.st_mode & 0o7777,'o'),'%h':str(s.st_nlink)}
fmt=sys.argv[2]; assert fmt in ('%u:%a','%u:%a:%h')
for key,value in v.items(): fmt=fmt.replace(key,value)
print(fmt)
''',
        'flock': '''#!/usr/bin/env python3
import fcntl,sys
assert sys.argv[1]=='-n'
try: fcntl.flock(int(sys.argv[2]),fcntl.LOCK_EX|fcntl.LOCK_NB)
except BlockingIOError: sys.exit(1)
''',
        'lua': '''#!/usr/bin/env python3
import json,sys
from pathlib import Path
code=sys.stdin.read(); assert "j.stage=='complete'" in code and 'j.uncertain==false' in code
raw=Path(sys.argv[-1]).read_bytes(); assert len(raw)<=65536
j=json.loads(raw); assert isinstance(j,dict) and type(j.get('schema')) is int and j['schema']==1 and j.get('stage')=='complete' and j.get('uncertain') is False
''',
    }
    for name, text in adapters.items():
        path = root / 'shim' / name
        path.write_text(text)
        path.chmod(0o700)
    (root / 'proc/net').mkdir(parents=True, exist_ok=True)
    for name in ('tcp', 'tcp6'):
        (root / 'proc/net' / name).write_text('')
    return str(backend)


class RuntimeTransactions(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        backend = make_backend(self.root)
        # macOS mv lacks -T; emulate only the replacement operation in tests.
        shim = self.root / 'shim/mv'
        shim.write_text('#!/usr/bin/env python3\nimport os,sys\na=sys.argv[1:]\nif a[0]=="-Tf": os.replace(a[1],a[2])\nelse: os.execv("/bin/mv",["mv",*a])\n')
        shim.chmod(0o700)
        self.env = {**os.environ, 'U60_TEST_ROOT': str(self.root), 'U60_TEST_BACKEND': backend,
                    'PATH': f'{shim.parent}:{os.environ["PATH"]}'}
        self.write('etc/rc.local', '#!/bin/sh\n: "stock flash protection"\nexit 0\n')
        self.rc = (self.root / 'etc/rc.local').read_bytes()
        self.write('etc/config/zwrt_zte_dm', 'config dm_update\n option dm_update_mode 1\n')
        self.write('etc/config/wireless', 'original wireless bytes\n')
        self.transaction = self.root / 'data/local/tmp/open-u60-transactions/test-123'

    def tearDown(self):
        self.temp.cleanup()

    def write(self, path, value):
        target = self.root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(value)
        return target

    def action(self, action, scope=None, success=True):
        command = ['sh', str(ROOT / 'scripts/device/transaction.sh'), action, 'test-123', 'aabb']
        if scope:
            command.append(scope)
        result = subprocess.run(command, env=self.env, text=True, capture_output=True, timeout=20)
        if success:
            self.assertEqual(result.returncode, 0, result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0, result.stdout)
        return result

    def backend(self, *args):
        subprocess.run([self.env['U60_TEST_BACKEND'], *args], env=self.env, check=True, capture_output=True)

    def events(self):
        return [json.loads(line) for line in (self.root / 'events.log').read_text().splitlines()]

    def clear_events(self):
        (self.root / 'events.log').write_text('')

    def assert_runtime_child_does_not_retain_imei_lock(self, operation):
        """Real inherited descriptors, gated parent and a test-owned fork only."""
        status_read, status_write = os.pipe()
        parent_read, parent_write = os.pipe()
        child_read, child_write = os.pipe()
        descriptors = {status_read, status_write, parent_read, parent_write, child_read, child_write}
        process = None
        child_pid = None
        child_exited = False
        try:
            # The shell owns FD9. Keep all protocol descriptors outside that
            # range, even when other tests or the runner already have FDs open.
            inherited = []
            for fd in (status_write, parent_read, child_read):
                high = fcntl.fcntl(fd, fcntl.F_DUPFD_CLOEXEC, 32)
                descriptors.add(high)
                inherited.append(high)
                os.close(fd)
                descriptors.remove(fd)
            status_write, parent_read, child_read = inherited
            env = {**self.env, 'U60_TEST_PERSIST_RUNTIME': operation,
                   'U60_TEST_CHILD_STATUS_FD': str(status_write),
                   'U60_TEST_PARENT_GATE_FD': str(parent_read),
                   'U60_TEST_CHILD_GATE_FD': str(child_read)}
            command = ['sh', str(ROOT / 'scripts/device/transaction.sh'), 'restore', 'test-123', 'aabb']
            process = subprocess.Popen(command, env=env, text=True, stdout=subprocess.PIPE,
                                       stderr=subprocess.PIPE, pass_fds=inherited)
            for fd in inherited:
                os.close(fd)
                descriptors.remove(fd)

            def message():
                data = bytearray()
                while not data.endswith(b'\n'):
                    self.assertTrue(select.select([status_read], [], [], 10)[0],
                                    f'test-owned child {child_pid} did not acknowledge')
                    byte = os.read(status_read, 1)
                    self.assertTrue(byte, 'test-owned child channel closed unexpectedly')
                    data.extend(byte)
                return data.decode().rstrip('\n')

            ready = set()
            while 'parent-ready' not in ready or child_pid is None:
                event = message()
                if event.startswith('child '):
                    child_pid = int(event.split()[1])
                    self.assertGreater(child_pid, 1)  # Recorded for diagnostics, never signalled.
                else:
                    self.assertEqual(event, 'parent-ready')
                ready.add(event)
            lock = self.root / 'data/openui-imei/controller.lock'
            with lock.open('r+b') as separate_opener:
                # runtime() must close only its child copy: the transaction
                # parent still excludes another controller throughout restore.
                self.assertIsNone(process.poll())
                self.assertTrue((self.transaction.parent / 'active').exists())
                with self.assertRaises(BlockingIOError):
                    fcntl.flock(separate_opener, fcntl.LOCK_EX | fcntl.LOCK_NB)
                os.close(parent_write)
                descriptors.remove(parent_write)
                stdout, stderr = process.communicate(timeout=20)
                self.assertEqual(process.returncode, 0, stderr or stdout)
                self.assertFalse((self.transaction.parent / 'active').exists())
                # The inherited-descriptor child is demonstrably still alive
                # after restore exits, without PID lookup or arbitrary signals.
                os.write(child_write, b'?')
                self.assertEqual(message(), 'child-alive')
                fcntl.flock(separate_opener, fcntl.LOCK_EX | fcntl.LOCK_NB)
                fcntl.flock(separate_opener, fcntl.LOCK_UN)
            os.close(child_write)
            descriptors.remove(child_write)
            self.assertEqual(message(), 'child-exit')
            child_exited = True
            # EOF proves all forked copies closed the dedicated status channel.
            self.assertTrue(select.select([status_read], [], [], 10)[0])
            self.assertEqual(os.read(status_read, 1), b'')
        finally:
            # Closing both gates releases only this fixture's backend/fork, even
            # on assertion failure. No PID obtained from disk is ever signalled.
            for fd in (parent_write, child_write):
                if fd in descriptors:
                    os.close(fd)
                    descriptors.remove(fd)
            try:
                if process is not None:
                    try:
                        process.communicate(timeout=20)
                    except subprocess.TimeoutExpired:
                        process.kill()  # Exact Popen created above, not a discovered host PID.
                        process.communicate(timeout=10)
                    if child_pid is not None and not child_exited:
                        self.assertTrue(select.select([status_read], [], [], 10)[0],
                                        f'test-owned child {child_pid} did not exit after gate closure')
                        while True:
                            data = os.read(status_read, 4096)
                            if not data:
                                break
                            self.assertTrue(select.select([status_read], [], [], 10)[0],
                                            f'test-owned child {child_pid} retained its status pipe')
            finally:
                for fd in descriptors:
                    os.close(fd)

    def install_sidecars(self, agent=True):
        for path in ('data/bin/tailscaled', 'data/bin/tailscale', 'etc/init.d/openui-tailscale', 'data/bin/relay-band-guard.sh'):
            self.write(path, 'old ' + path)
        if agent:
            self.write('data/zte-agent', 'old agent')
            self.write('data/local/tmp/start_zte_agent.sh', '#!/bin/sh\n#start-sidecars\n')

    def assert_pending(self):
        self.assertEqual((self.transaction / 'state').read_text().strip(), 'pending')
        base = self.transaction.parent
        self.assertEqual((base / 'active').read_text().strip(), 'test-123')
        self.assertEqual((base / 'lock/owner').read_text().strip(), 'test-123')

    def test_restore_agent_child_does_not_inherit_imei_lock(self):
        self.install_sidecars()
        self.write('data/local/tmp/start_zte_agent.sh', '#!/bin/sh\n')
        self.backend('start', 'agent')
        self.action('begin')
        self.action('quiesce', 'all')
        self.assert_runtime_child_does_not_retain_imei_lock('start:agent')

    def test_legacy_restore_agent_child_does_not_inherit_imei_lock(self):
        self.install_sidecars()
        self.write('data/local/tmp/start_zte_agent.sh', '#!/bin/sh\n')
        self.backend('start', 'agent')
        self.action('begin')
        self.action('quiesce', 'all')
        # Legacy recovery has no target inventory but must isolate runtime FDs.
        inventory = self.transaction / 'targets'
        self.assertEqual(inventory.resolve().parent, self.transaction.resolve())
        inventory.unlink()
        self.assert_runtime_child_does_not_retain_imei_lock('start:agent')

    def test_restore_http_child_does_not_inherit_imei_lock(self):
        self.write('etc/config/uhttpd', 'old HTTP config\n')
        self.action('begin')
        self.action('quiesce', 'none')
        self.write('etc/config/uhttpd', 'changed HTTP config\n')
        self.assert_runtime_child_does_not_retain_imei_lock('http-restart')

    def test_running_services_quiesce_before_replace_and_restore_original_versions(self):
        self.install_sidecars()
        self.backend('start', 'agent')
        self.write('data/local/tmp/relay-band-guard.disabled', 'wlan2\n')
        original = {p: (self.root / p).read_bytes() for p in ('etc/config/zwrt_zte_dm', 'etc/config/wireless', 'data/local/tmp/relay-band-guard.disabled')}
        self.action('begin')
        self.clear_events()
        self.action('quiesce', 'all')
        self.assertEqual([e for e in self.events() if e[0] == 'signal'], [['signal', '800'], ['signal', '801'], ['signal', '802']])
        self.write('data/zte-agent', 'bad agent')
        self.write('data/bin/tailscaled', 'bad tailscaled')
        self.write('etc/config/zwrt_zte_dm', 'bad FOTA config')
        self.write('mode', '0')
        self.write('etc/config/wireless', 'bad wireless')
        (self.root / 'data/local/tmp/relay-band-guard.disabled').unlink()
        self.backend('start', 'agent')
        self.clear_events()
        self.action('restore')
        events = self.events()
        last_stop = max(i for i, e in enumerate(events) if e[0] == 'signal')
        self.assertLess(last_stop, next(i for i, e in enumerate(events) if e[0] == 'fota-call'))
        self.assertLess(last_stop, next(i for i, e in enumerate(events) if e[0] == 'start'))
        self.assertEqual((self.root / 'data/zte-agent').read_text(), 'old agent')
        self.assertEqual((self.root / 'data/bin/tailscaled').read_text(), 'old data/bin/tailscaled')
        self.assertEqual((self.root / 'mode').read_text(), '1')
        self.assertEqual((self.root / 'etc/rc.local').read_bytes(), self.rc)
        for path, value in original.items():
            self.assertEqual((self.root / path).read_bytes(), value)
        self.assertEqual((self.transaction / 'state').read_text().strip(), 'restored')
        self.assertFalse((self.transaction.parent / 'active').exists())
        self.action('restore')  # retained journal is repeatable

    def test_originally_stopped_sidecars_stay_stopped_despite_agent_startup(self):
        self.install_sidecars()
        # An old agent is running, but its optional sidecars were stopped.
        self.write('data/local/tmp/start_zte_agent.sh', '#!/bin/sh\n')
        self.backend('start', 'agent')
        self.write('data/local/tmp/start_zte_agent.sh', '#!/bin/sh\n#start-sidecars\n')
        self.action('begin')
        self.action('quiesce', 'all')
        self.backend('start', 'agent')
        self.action('restore')
        self.assertEqual(json.loads((self.root / 'procd.json').read_text()), {})
        self.assertFalse((self.root / 'var/run/relay-band-guard.pid').exists())
        self.assertEqual(len(list((self.root / 'proc').glob('[0-9]*'))), 1)

    def test_first_install_stops_registered_deleted_binary_without_init_script(self):
        self.action('begin')
        self.action('quiesce', 'all')
        self.install_sidecars(agent=False)
        self.backend('start', 'tailscale')
        self.backend('start', 'guard')
        # Both the init script and binary have disappeared; procd and /proc
        # still hold the new runtime. This must NOT be mistaken for no service.
        (self.root / 'etc/init.d/openui-tailscale').unlink()
        (self.root / 'data/bin/tailscaled').unlink()
        exe = self.root / 'proc/800/exe'; exe.unlink(); exe.symlink_to(BIN + ' (deleted)')
        self.action('restore')
        self.assertEqual(json.loads((self.root / 'procd.json').read_text()), {})
        self.assertFalse(list((self.root / 'proc').glob('[0-9]*')))
        self.assertFalse((self.root / 'data/bin/relay-band-guard.sh').exists())

    def test_tailscale_scope_does_not_stop_or_rewind_live_agent_and_relay(self):
        self.install_sidecars()
        self.backend('start', 'agent')
        self.action('begin')
        self.clear_events()
        self.action('quiesce', 'tailscale')
        self.write('etc/config/wireless', 'relay changed meanwhile')
        self.write('data/local/tmp/relay-band-guard.disabled', 'wlan0\n')
        self.write('data/bin/tailscaled', 'failed update')
        self.backend('start', 'tailscale')
        self.action('restore')
        signals = [e[1] for e in self.events() if e[0] == 'signal']
        self.assertNotIn('800', signals)
        self.assertNotIn('801', signals)
        self.assertEqual((self.root / 'etc/config/wireless').read_text(), 'relay changed meanwhile')
        self.assertEqual((self.root / 'data/local/tmp/relay-band-guard.disabled').read_text(), 'wlan0\n')

    def test_dashboard_scope_none_never_touches_unaffected_sidecars(self):
        self.install_sidecars()
        self.backend('start', 'agent')
        self.action('begin')
        self.action('quiesce', 'none')
        self.clear_events()
        self.write('etc/config/wireless', 'relay changed meanwhile')
        self.write('data/local/tmp/relay-band-guard.disabled', 'wlan0\n')
        self.write('data/bin/dashboard-uhttpd', 'new dashboard')
        self.write('data/local/tmp/start_dashboard.sh', '#!/bin/sh\n')
        self.backend('start', 'dashboard')
        self.clear_events()
        self.action('restore')
        self.assertEqual([e[1] for e in self.events() if e[0] == 'signal'], ['803'])
        self.assertFalse(any(e[0] in ('procd-delete', 'start') for e in self.events()[1:]))
        self.assertEqual((self.root / 'etc/config/wireless').read_text(), 'relay changed meanwhile')
        self.assertEqual((self.root / 'data/local/tmp/relay-band-guard.disabled').read_text(), 'wlan0\n')

    def test_guard_pid_reuse_or_unknown_argv_is_rejected_without_signalling(self):
        for mutation in ('reused-pid', 'reused-inode', 'extra-argv', 'trailing-empty-arg', 'embedded-newline'):
            with self.subTest(mutation=mutation):
                if self.transaction.exists():
                    shutil.rmtree(self.transaction.parent)
                self.install_sidecars(agent=False)
                self.backend('start', 'guard')
                self.action('begin')
                pid = (self.root / 'var/run/relay-band-guard.pid').read_text().split('|')[0]
                process = self.root / 'proc' / pid
                if mutation == 'reused-pid':
                    stat = (process / 'stat').read_text(); (process / 'stat').write_text(stat.replace(str(int(pid) * 10), '999999'))
                elif mutation == 'reused-inode':
                    (process / 'exe.identity').write_text('9:999999')
                elif mutation == 'trailing-empty-arg':
                    (process / 'cmdline').write_bytes(b'/bin/sh\0/data/bin/relay-band-guard.sh\0\0')
                elif mutation == 'embedded-newline':
                    (process / 'cmdline').write_bytes(b'/bin/sh\n/data/bin/relay-band-guard.sh\0')
                else:
                    (process / 'cmdline').write_bytes(b'/bin/sh\0/data/bin/relay-band-guard.sh\0foreign-argument\0')
                self.clear_events()
                self.action('quiesce', 'all', success=False)
                self.assertFalse(any(e[0] == 'signal' for e in self.events()))
                self.assert_pending()
                shutil.rmtree(process)
                (self.root / 'var/run/relay-band-guard.pid').unlink()
                self.action('restore')
                for live in (self.root / 'proc').glob('[0-9]*'):
                    shutil.rmtree(live)
                (self.root / 'var/run/relay-band-guard.pid').unlink()

    def test_unknown_procd_command_rejected_and_unrelated_tailscale_untouched(self):
        self.install_sidecars(agent=False)
        self.action('begin')
        self.backend('start', 'tailscale')
        service = json.loads((self.root / 'procd.json').read_text())
        service['openui-tailscale']['instances']['instance1']['command'][0] = '/usr/sbin/tailscaled'
        self.write('procd.json', json.dumps(service))
        self.clear_events()
        self.action('quiesce', 'tailscale', success=False)
        self.assertFalse(any(e[0] in ('signal', 'procd-delete') for e in self.events()))
        self.assert_pending()

    def test_unregistered_foreign_tailscale_is_never_selected(self):
        self.install_sidecars(agent=False)
        self.backend('start', 'tailscale')
        self.write('procd.json', '{}')
        process = self.root / 'proc/800'
        (process / 'exe').unlink()
        (process / 'exe').symlink_to('/usr/sbin/tailscaled')
        (process / 'cmdline').write_bytes(b'/usr/sbin/tailscaled\0--state=/tmp/foreign.state\0')
        self.action('begin')
        self.clear_events()
        self.action('quiesce', 'all')
        self.action('restore')
        self.assertTrue(process.exists())
        self.assertFalse(any(e[0] in ('signal', 'procd-delete') for e in self.events()))

    def test_pid_reused_after_procd_delete_is_not_killed_and_stays_pending(self):
        self.install_sidecars(agent=False)
        self.backend('start', 'tailscale')
        self.action('begin')
        self.write('reuse-on-delete', 'yes')
        self.clear_events()
        self.action('quiesce', 'tailscale', success=False)
        self.assertFalse(any(e[0] == 'signal' for e in self.events()))
        self.assertTrue((self.root / 'proc/800').exists())
        self.assert_pending()

    def test_stop_and_restart_errors_keep_pending_and_recovery_retryable(self):
        self.install_sidecars()
        self.write('data/local/tmp/start_zte_agent.sh', '#!/bin/sh\n')
        for service in ('agent', 'guard', 'tailscale'):
            self.backend('start', service)
        self.action('begin')
        self.action('quiesce', 'all')
        self.write('data/local/tmp/start_zte_agent.sh', '#!/bin/sh\n#start-sidecars\n')
        self.backend('start', 'agent')
        self.write('fail', 'procd-delete')
        self.action('restore', success=False)
        self.assert_pending()
        self.write('fail', 'start:tailscale')
        # Older agent startup did not start sidecars, so the explicit service
        # restart must run and its failure must not become a restored marker.
        self.action('restore', success=False)
        self.assert_pending()
        self.write('fail', '')
        self.action('restore')
        self.assertEqual((self.transaction / 'state').read_text().strip(), 'restored')

    def test_fota_setter_failure_remains_pending_until_retry(self):
        self.action('begin')
        self.action('quiesce', 'none')
        self.write('mode', '0')
        self.write('etc/config/zwrt_zte_dm', 'failed hardening')
        self.write('fail', 'fota-call:1')
        self.action('restore', success=False)
        self.assert_pending()
        self.assertEqual((self.root / 'mode').read_text(), '0')
        self.write('fail', '')
        self.action('restore')
        self.assertEqual((self.root / 'mode').read_text(), '1')

    def test_unsupported_fota_mode_fails_before_mutation(self):
        self.write('mode', '2')
        self.action('begin', success=False)
        self.assertFalse(self.transaction.exists())
        self.assertEqual((self.root / 'etc/rc.local').read_bytes(), self.rc)
        self.assertFalse(any(e[0] in ('signal', 'procd-delete') for e in self.events()))

    def test_ambiguous_agent_argv_is_not_accepted_by_projection(self):
        self.install_sidecars()
        self.backend('start', 'agent')
        self.action('begin')
        command = self.root / 'proc/800/cmdline'
        command.write_bytes(b'/data/zte-agent\0\0')
        self.clear_events()
        self.action('quiesce', 'all', success=False)
        self.assertFalse(any(e[0] == 'signal' for e in self.events()))
        self.assert_pending()
        command.write_bytes(b'/data/zte-agent\0')
        self.action('restore')

    def test_zombie_reaped_by_procd_needs_no_signal(self):
        self.install_sidecars(agent=False)
        self.backend('start', 'tailscale')
        self.action('begin')
        self.write('zombie-on-delete', 'yes')
        self.clear_events()
        self.action('quiesce', 'tailscale')
        self.assertFalse(any(e[0] == 'signal' for e in self.events()))
        self.assertIn(') Z ', (self.root / 'proc/800/stat').read_text())
        self.action('restore')
        self.assertEqual((self.transaction / 'state').read_text().strip(), 'restored')

    def test_each_sidecar_restart_error_keeps_pending_but_tries_the_other(self):
        self.install_sidecars(agent=False)
        for service in ('guard', 'tailscale'):
            self.backend('start', service)
        self.action('begin')
        self.action('quiesce', 'all')
        for broken, other in [('guard', 'tailscale'), ('tailscale', 'guard')]:
            with self.subTest(broken=broken):
                self.write('fail', 'start:' + broken)
                self.clear_events()
                self.action('restore', success=False)
                self.assert_pending()
                self.assertIn(['start', other], self.events())
        self.write('fail', '')
        self.action('restore')

    def test_fota_error_or_unknown_ack_still_restarts_old_management_listener(self):
        self.write('data/bin/dropbear', 'old dropbear')
        self.write('data/local/tmp/start_dropbear.sh', '#!/bin/sh\n')
        self.backend('start', 'dropbear')
        self.action('begin')
        self.action('quiesce', 'none')
        self.write('data/bin/dropbear', 'failed replacement')
        self.write('mode', '0')
        self.write('fota-noapply', 'yes')
        for reply in ('{"error":"rejected"}', '{"result":"unknown"}', 'not json', '{\v}', '{\f}'):
            with self.subTest(reply=reply):
                self.write('fota-reply', reply)
                self.clear_events()
                self.action('restore', success=False)
                self.assert_pending()
                self.assertIn(['start', 'dropbear'], self.events())
                self.assertEqual((self.root / 'data/bin/dropbear').read_text(), 'old dropbear')
                self.assertEqual((self.root / 'mode').read_text(), '0')
        self.write('fota-reply', '{}')
        (self.root / 'fota-noapply').unlink()
        self.action('restore')

    def test_failed_active_publication_retains_durable_snapshot_and_lock(self):
        shim = self.root / 'shim/mv'
        shim.write_text('#!/usr/bin/env python3\nimport os,sys\na=sys.argv[1:]\nif a[-1].endswith("/active"): sys.exit(71)\nif a[0]=="-Tf": os.replace(a[1],a[2])\nelse: os.execv("/bin/mv",["mv",*a])\n')
        self.action('begin', success=False)
        self.assertEqual((self.transaction / 'state').read_text().strip(), 'pending')
        self.assertEqual((self.transaction.parent / 'lock/owner').read_text().strip(), 'test-123')
        self.assertFalse((self.transaction.parent / 'active').exists())
        self.action('restore')
        self.assertEqual((self.transaction / 'state').read_text().strip(), 'restored')


class DeploymentLifecycle(unittest.TestCase):
    def test_quiesce_order_and_scope_and_failure_recovery(self):
        sys.path.insert(0, str(ROOT / 'scripts'))
        spec = importlib.util.spec_from_file_location('deploy_runtime', ROOT / 'scripts/deploy-components.py')
        deploy = importlib.util.module_from_spec(spec); spec.loader.exec_module(deploy)
        class Fake:
            def __init__(self, fail=False): self.commands = []; self.fail = fail
            def reachable(self): pass
            def identity(self): return {'model': 'ZTE MU5250', 'firmware': 'test', 'fingerprint': 'aabb'}
            def push(self, *_): pass
            def shell(self, command):
                self.commands.append(command)
                if self.fail and ' quiesce ' in command: raise RuntimeError('injected quiesce failure')
                return '1000000\n1000000\n1000000\n100' if 'df -Pk' in command else ''
        with tempfile.TemporaryDirectory() as temporary:
            binary = Path(temporary) / 'agent'
            data = bytearray(64); data[:6] = b'\x7fELF\x02\x01'; data[18:20] = (183).to_bytes(2, 'little')
            binary.write_bytes(data)
            ts = Path(temporary) / 'ts'; ts.mkdir()
            for name in ('tailscale', 'tailscaled'): (ts / name).write_bytes(data)
            dashboard = Path(temporary) / 'dashboard'; dashboard.write_bytes(b'test tar mock')
            for kind, scope in [('agent', 'all'), ('tailscale', 'tailscale'), ('dashboard', 'none')]:
                with self.subTest(kind=kind):
                    args = SimpleNamespace(agent=str(binary) if kind == 'agent' else None,
                        tailscale=str(ts) if kind == 'tailscale' else None,
                        dashboard=str(dashboard) if kind == 'dashboard' else None,
                        harden=False, auto_adb=False, bundle=None, gateway='192.168.0.1',
                        adb_serial=None, dry_run=False, identity_file=None)
                    fake = Fake()
                    with patch.object(deploy, 'Transport', return_value=fake), patch.dict(os.environ, {'ZTE_AGENT_PASSWORD': 'test'}), \
                         patch.object(deploy, 'check_startup'), patch.object(deploy, 'verify_credentials'), \
                         patch.object(deploy, 'validate_dashboard', return_value=10), patch.object(deploy.time, 'sleep'):
                        deploy.deploy(args)
                    commands = fake.commands
                    begin = next(i for i, c in enumerate(commands) if ' begin ' in c)
                    quiesce = next(i for i, c in enumerate(commands) if ' quiesce ' in c)
                    self.assertEqual(quiesce, begin + 1)
                    self.assertTrue(commands[quiesce].endswith(' ' + scope))
                    self.assertLess(quiesce, next(i for i, c in enumerate(commands) if ' complete ' in c))
                    failed = Fake(fail=True)
                    with patch.object(deploy, 'Transport', return_value=failed), patch.dict(os.environ, {'ZTE_AGENT_PASSWORD': 'test'}), \
                         patch.object(deploy, 'check_startup'), patch.object(deploy, 'validate_dashboard', return_value=10):
                        with self.assertRaisesRegex(RuntimeError, 'injected quiesce failure'): deploy.deploy(args)
                    self.assertTrue(any(' restore ' in c for c in failed.commands))
                    self.assertFalse(any(' complete ' in c for c in failed.commands))


if __name__ == '__main__':
    unittest.main()
