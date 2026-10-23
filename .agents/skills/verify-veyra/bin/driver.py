"""Shared helpers for verify-veyra's sections: one run's process records, bounded polls, fixtures and proof.

Import it from the checkout root with PYTHONPATH=.agents/skills/verify-veyra/bin and RUN exported.
It sets no application state; its writes are fixture copies, screenshots and proof.
"""
import ctypes, errno, hashlib, json, os, pathlib, re, shutil, signal, sqlite3, struct, subprocess, sys, time
assert pathlib.Path('crates/veyra/Cargo.toml').is_file(), 'run sections from the checkout root'
RUN = os.environ['RUN']
PROOF = pathlib.Path.cwd()/'target/verify-veyra'/RUN
DATA = pathlib.Path(os.environ.get('VEYRA_VERIFY_TMP','/tmp/veyra-verify-') + RUN)
assert str(DATA).startswith('/tmp/veyra-'), DATA
DB = DATA / 'veyra.db'
FIXTURES = PROOF / 'fixtures'
SOURCE = pathlib.Path('crates/engine/tests/fixtures/claude')
CODEX_SOURCE = pathlib.Path('crates/engine/tests/fixtures/codex')
GROK_SOURCE = pathlib.Path('crates/engine/tests/fixtures/grok')
TABLES = 'grants items projects requests settings tasks threads turns'.split()
LANE = (PROOF/'lane').read_text().strip()
assert LANE in ['fake','live'], LANE
ENV = {key:value for key,value in os.environ.items() if not (
    (key.startswith('CLAUDE_CODE_') and not key.startswith('CLAUDE_CODE_USE_'))
    or key in ['CLAUDE_PID','CLAUDE_EFFORT','CLAUDECODE','CLAUDE_AGENT_SDK_VERSION'])}
ENV['VEYRA_DATA_DIR'] = str(DATA)
ENV['VEYRA_REAP_PIDS'] = ''
for name in ['VEYRA_CLAUDE_BIN','VEYRA_CODEX_BIN','VEYRA_GROK_BIN','VEYRA_FAKE_FIXTURES','VEYRA_FAKE_FIXTURE','VEYRA_RECORD','VEYRA_DEBUG_SHELL_FIXTURE']:
    ENV.pop(name,None)
BRANCH_BIN = DATA/'branch-bin'
if LANE == 'fake':
    (DATA/'home').mkdir(exist_ok=True)
    ENV['HOME'] = str(DATA/'home')
    ENV['PATH'] = str(BRANCH_BIN)+os.pathsep+ENV['PATH']
    ENV['CLAUDE_CONFIG_DIR'] = str(DATA/'claude-home')
    ENV['VEYRA_DEBUG_SHELL_FIXTURE'] = str(FIXTURES.resolve()/'drawer.jsonl')
    if not (FIXTURES/'drawer.jsonl').exists():
        shutil.copyfile('crates/engine/tests/fixtures/tui/echo.jsonl',FIXTURES/'drawer.jsonl')
    ENV.update(VEYRA_CLAUDE_BIN=str(DATA/'fake-cli'),VEYRA_CODEX_BIN=str(DATA/'fake-cli'),
        VEYRA_GROK_BIN=str(DATA/'fake-cli'),
        VEYRA_FAKE_FIXTURES=str(FIXTURES.resolve()),
        VEYRA_RECORD=str(PROOF.resolve()/'rec'))
else:
    ENV.pop('CLAUDE_CONFIG_DIR',None)
    ENV['VEYRA_RECORD'] = str(PROOF.resolve()/'rec')
lib = ctypes.CDLL(None,use_errno=True)
lib.sysctl.argtypes = [ctypes.POINTER(ctypes.c_int),ctypes.c_uint,ctypes.c_void_p,
    ctypes.POINTER(ctypes.c_size_t),ctypes.c_void_p,ctypes.c_size_t]
def no_environment(pid):
    status = subprocess.run(['ps','-o','stat=','-p',str(pid)],capture_output=True,text=True).stdout.strip()
    return not status or status.startswith('Z')
def marked_processes(parents=None, retry_exiting=False):
    remaining = []
    rows = subprocess.check_output(['ps','-axo','pid=,uid=,stat='],text=True).splitlines()
    for row in rows:
        text,uid,status = row.split()
        if int(uid) != os.geteuid() or status.startswith('Z'): continue
        pid = int(text)
        mib = (ctypes.c_int*3)(1,49,pid)
        size = ctypes.c_size_t()
        if lib.sysctl(mib,3,None,ctypes.byref(size),None,0) != 0:
            error = ctypes.get_errno()
            if error == errno.ESRCH or (error == errno.EINVAL and no_environment(pid)): continue
            if error == errno.EINVAL or (retry_exiting and error == errno.EIO):
                remaining.append(pid)
                continue
            raise OSError(error,f'marker size read failed for pid {pid}')
        if size.value == 0: continue
        buffer = ctypes.create_string_buffer(size.value)
        if lib.sysctl(mib,3,buffer,ctypes.byref(size),None,0) != 0:
            error = ctypes.get_errno()
            if error == errno.ESRCH or (error == errno.EINVAL and no_environment(pid)): continue
            if error == errno.EINVAL or (retry_exiting and error == errno.EIO):
                remaining.append(pid)
                continue
            raise OSError(error,f'marker read failed for pid {pid}')
        raw = buffer.raw[:size.value]
        if not raw: continue
        try:
            argc = struct.unpack_from('i',raw)[0]
            if argc < 0: raise ValueError('invalid argument count')
            offset = raw.index(b'\0',4)+1
            while offset < len(raw) and raw[offset] == 0: offset += 1
            for _ in range(argc): offset = raw.index(b'\0',offset)+1
        except (ValueError,struct.error):
            if not no_environment(pid): remaining.append(pid)
            continue
        markers = [value.split(b'=',1)[1] for value in raw[offset:].split(b'\0')
            if value.startswith(b'VEYRA_PARENT_PID=')]
        for value in markers:
            if not value.isdigit() or int(value) <= 0: continue
            parent = int(value)
            if parents is not None:
                orphan = parent in parents
            else:
                try: os.kill(parent,0)
                except ProcessLookupError: orphan = True
                else: orphan = False
            if orphan:
                remaining.append(pid)
                break
    return remaining
def saved_marked_processes(marked, pids, groups):
    owned = []
    for pid in marked:
        try: group = os.getpgid(pid)
        except ProcessLookupError: continue
        if pid in pids or group in groups: owned.append(pid)
    return owned
def page_done(name):
    with (PROOF/'pages-run').open('a') as out: out.write(name+'\n')
def save(name, value):
    (PROOF / (name + '.json')).write_text(json.dumps(value, indent=2) + '\n')
    return value
def ctl(name, *args, expected=0, stderr_contains=None, log=True):
    result = subprocess.run(['target/debug/veyra', 'ctl', *args], env=ENV, capture_output=True, text=True)
    if log or result.returncode != expected:
        with (PROOF/'drive.log').open('a') as out:
            out.write('$ veyra ctl ' + ' '.join(args) + '\n'
                + result.stdout + result.stderr + f'exit={result.returncode}\n')
    (PROOF/(name+'.json')).write_text(result.stdout)
    (PROOF/(name+'.stderr')).write_text(result.stderr)
    assert result.returncode == expected, (name, expected, result.returncode, result.stdout, result.stderr)
    if expected == 1: assert stderr_contains, 'exit 1 needs a named stderr expectation'
    if stderr_contains: assert stderr_contains in result.stderr, (name,result.stderr,stderr_contains)
    return json.loads(result.stdout) if result.stdout.startswith('{') else None
def poll(name, predicate):
    end = time.monotonic() + 30
    while time.monotonic() < end:
        dump = ctl(name, 'dump', log=False)
        if predicate(dump):
            with (PROOF/'drive.log').open('a') as out: out.write(f'$ poll {name} matched\n')
            return dump
        time.sleep(.05)
    if 'focus' in name: observe_failure(name,'focus-timeout')
    raise AssertionError(name + ' timed out')
def action(name, operation, data=None):
    args = ['action', 'veyra::' + operation]
    if data is not None: args.append(json.dumps(data))
    assert ctl(name, *args) == {'ok': True}
def identity():
    pid = int((PROOF/'pid').read_text())
    pgid = int((PROOF/'pgid').read_text())
    assert os.getpgid(pid) == pgid == pid
    assert (DATA/'veyra.lock').read_text().strip() == str(pid)
    dump = ctl('identity', 'dump')
    assert dump['pid'] == pid and dump['data_dir'] == str(DATA)
    assert dump['build_rev'] == subprocess.check_output(['git','rev-parse','HEAD'], text=True).strip()
    return pid
def window_id(pid):
    end = time.monotonic() + 30
    while time.monotonic() < end:
        found = subprocess.run([str(PROOF/'winid'),str(pid)],capture_output=True,text=True)
        if found.returncode == 0: return found.stdout.strip()
        time.sleep(.05)
    raise AssertionError('no on-screen window for owned pid '+str(pid))
def capture(name):
    try:
        guarded_capture(name)
    except Exception:
        observe_failure(name,'capture')
        raise
def cg_windows():
    """Read WindowServer metadata in memory; never retain another window's title."""
    import plistlib
    cg = ctypes.CDLL('/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics')
    cf = ctypes.CDLL('/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation')
    cg.CGWindowListCopyWindowInfo.argtypes = [ctypes.c_uint32,ctypes.c_uint32]
    cg.CGWindowListCopyWindowInfo.restype = ctypes.c_void_p
    cf.CFPropertyListCreateData.argtypes = [ctypes.c_void_p,ctypes.c_void_p,ctypes.c_long,
        ctypes.c_ulong,ctypes.c_void_p]
    cf.CFPropertyListCreateData.restype = ctypes.c_void_p
    cf.CFDataGetLength.argtypes = [ctypes.c_void_p]
    cf.CFDataGetLength.restype = ctypes.c_long
    cf.CFDataGetBytePtr.argtypes = [ctypes.c_void_p]
    cf.CFDataGetBytePtr.restype = ctypes.c_void_p
    cf.CFRelease.argtypes = [ctypes.c_void_p]
    windows = cg.CGWindowListCopyWindowInfo(1,0)
    assert windows, 'WindowServer list unavailable'
    data = None
    try:
        data = cf.CFPropertyListCreateData(None,windows,100,0,None)
        assert data, 'WindowServer metadata unavailable'
        rows = plistlib.loads(ctypes.string_at(cf.CFDataGetBytePtr(data),cf.CFDataGetLength(data)))
        return [{key:row[key] for key in ['kCGWindowNumber','kCGWindowOwnerPID','kCGWindowLayer',
            'kCGWindowBounds'] if key in row} for row in rows]
    finally:
        if data: cf.CFRelease(data)
        cf.CFRelease(windows)
def display_rectangles():
    class Rect(ctypes.Structure):
        _fields_ = [(name,ctypes.c_double) for name in ['x','y','width','height']]
    cg = ctypes.CDLL('/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics')
    cg.CGGetActiveDisplayList.argtypes = [ctypes.c_uint32,ctypes.POINTER(ctypes.c_uint32),
        ctypes.POINTER(ctypes.c_uint32)]
    cg.CGDisplayBounds.argtypes = [ctypes.c_uint32]
    cg.CGDisplayBounds.restype = Rect
    count = ctypes.c_uint32()
    assert cg.CGGetActiveDisplayList(0,None,ctypes.byref(count)) == 0
    displays = (ctypes.c_uint32*count.value)()
    assert cg.CGGetActiveDisplayList(count.value,displays,ctypes.byref(count)) == 0
    return [(int(display),tuple(getattr(cg.CGDisplayBounds(display),key)
        for key in ['x','y','width','height'])) for display in displays[:count.value]]
def region_snapshot(pid, window, backdrop, rectangle=None):
    """All region guards, including the existing frontmost guard, immediately around a capture."""
    assert not locked()
    subprocess.run([str((PROOF/'capture-window').resolve()),str(pid),str(window)],check=True)
    rows = cg_windows()
    def bounds(row):
        frame = row['kCGWindowBounds']
        return tuple(frame[key] for key in ['X','Y','Width','Height'])
    def contains(outer, inner):
        x,y,w,h = outer
        a,b,c,d = inner
        return c > 0 and d > 0 and x <= a and y <= b and a+c <= x+w and b+d <= y+h
    def intersects(a,b):
        return a[0] < b[0]+b[2] and b[0] < a[0]+a[2] and a[1] < b[1]+b[3] and b[1] < a[1]+a[3]
    index = next((i for i,row in enumerate(rows) if row.get('kCGWindowNumber') == int(window)
        and row.get('kCGWindowOwnerPID') == int(pid) and row.get('kCGWindowLayer') == 0),None)
    assert index is not None, 'owned window is off-screen'
    frame = bounds(rows[index])
    assert frame == tuple(round(value) for value in frame), 'owned window has non-integral bounds'
    assert rectangle is None or frame == tuple(rectangle), 'owned window rectangle changed'
    owned = {int(value) for value in (PROOF/'backdrop-owned-pids').read_text().split()}
    assert int(backdrop['pid']) in owned, 'backdrop was not started by this run'
    candidates = [row for row in rows[index+1:] if row.get('kCGWindowOwnerPID') == int(backdrop['pid'])
        and row.get('kCGWindowNumber') in backdrop['windows'] and row.get('kCGWindowLayer') == 0
        and contains(bounds(row),frame)]
    assert candidates, 'rectangle is not wholly inside one owned backdrop window'
    background = candidates[0]
    displays = [(identifier,screen) for identifier,screen in display_rectangles()
        if contains(screen,frame) and contains(screen,bounds(background))]
    assert displays, 'rectangle and backdrop are not on one display'
    above = [row['kCGWindowNumber'] for row in rows[:index] if intersects(bounds(row),frame)]
    assert not above, 'owned window is covered by intersecting windows: '+str(above)
    return {'rectangle':frame,'window':int(window),'pid':int(pid),
        'backdrop_window':background['kCGWindowNumber'],'backdrop_bounds':bounds(background),
        'display':displays[0],'frontmost':True,'intersecting_above':0}
def region_capture(target, pid, window):
    """Discard a region capture when any guard changes; make at most three attempts in total."""
    backdrop = json.loads((PROOF/'backdrop-windows.json').read_text())
    attempts = []
    for attempt in range(3):
        target.unlink(missing_ok=True)
        try:
            before = region_snapshot(pid,window,backdrop)
            rectangle = before['rectangle']
            region = ','.join(str(round(value)) for value in rectangle)
            subprocess.run(['screencapture','-x','-R',region,str(target)],check=True)
            after = region_snapshot(pid,window,backdrop,rectangle)
            assert before == after, 'region guards changed during capture'
            save(target.stem+'-region-guards',{'attempts':attempts,'before':before,'after':after})
            return
        except Exception as error:
            target.unlink(missing_ok=True)
            attempts.append({'attempt':attempt+1,'error':str(error)})
            save(target.stem+'-region-guards',{'attempts':attempts})
            if attempt == 2: raise
            time.sleep(.1)
def measured_pair(labels, take, rectangle=(24,60,960,740)):
    """Retake both halves at one placement if either half needed the owned-window fallback."""
    attempts = []
    for attempt in range(3):
        for label in labels:
            rectangle, placed = take(label,rectangle)
            if placed: break
        else:
            save(labels[0]+'-pair-placement',{'attempts':attempts,'rectangle':rectangle,'passed':True})
            return
        attempts.append({'attempt':attempt+1,'fallback_half':label,'rectangle':rectangle,
            'discarded':list(labels)})
        for label in labels:
            (PROOF/(label+'.png')).unlink(missing_ok=True)
        save(labels[0]+'-pair-placement',{'attempts':attempts,'passed':False})
        assert attempt < 2, 'measurement placement did not settle within three attempts'
def measured_pair_self_test():
    """Plant fallback on one half only; neither retained image may predate that placement."""
    labels = ['pair-test-coarse','pair-test-inverse']
    for fallback_half in labels:
        calls, retained = [], {}
        def take(label,rectangle):
            calls.append((label,tuple(rectangle)))
            placed = label == fallback_half and len(calls) <= 2
            rectangle = (24,421,960,740) if placed else rectangle
            retained[label] = (len(calls),tuple(rectangle))
            (PROOF/(label+'.png')).write_bytes(bytes([len(calls)]))
            return rectangle,placed
        measured_pair(labels,take)
        assert [label for label,_ in calls][-2:] == labels, calls
        assert retained[labels[0]][0] > (1 if fallback_half == labels[0] else 2), retained
        assert retained[labels[0]][1] == retained[labels[1]][1] == (24,421,960,740), retained
        assert all((PROOF/(label+'.png')).read_bytes() == bytes([retained[label][0]]) for label in labels)
        save(fallback_half+'-self-test',{'calls':calls,'retained':retained})
        for label in labels: (PROOF/(label+'.png')).unlink()
    print('PASS measured_pair_self_test: fallback on either half discards both and retakes one rectangle')
def guarded_capture(name):
    assert not locked()
    window = (PROOF/'window-id').read_text().strip()
    pid = (PROOF/'pid').read_text().strip()
    helper = str((PROOF/'capture-window').resolve())
    for step in range(2):
        frame = ctl(name+'-before-frame-'+str(step),'dump')['perf']['repaints']
        action(name+'-activate-'+str(step),'ActivateWindow')
        poll(name+'-rendered-'+str(step),lambda d: d['perf']['repaints'] > frame)
    time.sleep(.1)
    subprocess.run([helper,pid,window],check=True)
    dump = ctl(name+'-capture-state','dump')
    visible = {key:dump[key] for key in ['view','sidebar','selected_thread','transcript','composer_text',
        'composer','settings','notice','quit_sheet','focus','composer_caret']}
    drawer = dump['drawer']
    visible['drawer'] = None if drawer is None else {'open':drawer['open']}
    if drawer and drawer['open']:
        active = next(tab for tab in drawer['tabs'] if tab['number']==drawer['active_tab'])
        visible['drawer'].update(height=drawer['height'],active_tab=drawer['active_tab'],
            tabs=[{key:tab[key] for key in ['number','label']} for tab in drawer['tabs']],
            status={key:active[key] for key in ['exit_code','exited','error']},close_sheet=drawer['close_sheet'],
            terminal=None if drawer['terminal'] is None else {key:drawer['terminal'][key] for key in ['grid','focused','columns','rows']})
    visible['providers'] = [probe['readiness'] for probe in dump['providers']]
    state = json.dumps(visible,sort_keys=True)
    target = PROOF/(name+'.png')
    if dump['look']['background'] == 'translucent':
        region_capture(target,pid,window)
    else:
        subprocess.run(['screencapture','-x','-o','-l',window,str(target)],check=True)
    subprocess.run([helper,pid,window],check=True)
    digest = hashlib.sha256(target.read_bytes()).hexdigest()
    previous = PROOF/'capture-last.json'
    if previous.exists():
        old = json.loads(previous.read_text())
        assert state == old['state'] or digest != old['digest'], 'stale screenshot after state change: '+name
    previous.write_text(json.dumps({'state':state,'digest':digest,'name':name})+'\n')
def foreground_ended(name, thread):
    query = "select count(*) from turns where thread_id='"+thread+"' and state='ended'"
    end = time.monotonic()+30
    while time.monotonic()<end:
        if sql(query)=='1': return
        time.sleep(.05)
    raise AssertionError(name+' foreground did not end')
def background_working(name, thread):
    foreground_ended(name,thread)
    ctl(name+'-not-done','wait',thread,'Done','1',expected=1,stderr_contains='veyra ctl wait timed out:')
    busy = ctl(name,'dump')
    assert busy['selected_thread']['id']==thread and busy['selected_thread']['status']=='Working'
    return busy
def sql(query):
    value = subprocess.check_output(['sqlite3',str(DB),query],text=True).strip()
    with (PROOF/'sql.log').open('a') as log: log.write(query+'\n'+value+'\n')
    return value
def database(label, mode):
    values = [sql('pragma user_version'),sql('pragma journal_mode'),
        *sql('.tables').split(),sql('select default_mode from settings')]
    assert values == ['4','wal',*TABLES,mode], (label,values)
    (PROOF/(label+'-database.txt')).write_text('\n'.join(values)+'\n')
    (PROOF/(label+'-inode.txt')).write_text(str(DB.stat().st_ino)+'\n')
    save(label,ctl(label+'-dump','dump'))
def wait_gone(pid):
    end = time.monotonic() + 5
    while time.monotonic() < end:
        try: os.waitpid(pid,os.WNOHANG)
        except ChildProcessError: pass
        try: os.kill(pid,0)
        except ProcessLookupError: return
        time.sleep(.05)
    raise AssertionError(f'owned pid {pid} did not exit')
def stop(label, crash=False):
    pid = identity()
    live = sql('select count(*) from threads where live_pgid is not null')
    assert live == '0', 'finish provider turns before relaunch'
    if crash: os.kill(pid,signal.SIGKILL)
    else: ctl(label+'-quit','quit')
    wait_gone(pid)
def locked():
    return 'CGSSessionScreenIsLocked' in subprocess.check_output(['ioreg','-n','Root','-d1','-a'],text=True)
def input_idle_seconds():
    for line in subprocess.check_output(['ioreg','-c','IOHIDSystem'],text=True).splitlines():
        if 'HIDIdleTime' in line: return int(line.split()[-1])//1_000_000_000
    raise AssertionError('HID idle reading unavailable')
def veyra_pids():
    found = subprocess.run(['pgrep','-x','veyra'],capture_output=True,text=True)
    assert found.returncode in (0,1), found.stderr
    return {int(pid) for pid in found.stdout.split()}
def owned_app_pids():
    path = PROOF/'owned-app-pids'
    return {int(pid) for pid in path.read_text().split()} if path.exists() else set()
def foreign_veyra():
    """Running veyra pids this run did not start: neither an owned app nor marked with an owned app's pid."""
    pids = veyra_pids()
    owned = owned_app_pids()
    if not pids - owned: return []
    unreadable = set(marked_processes(set()))
    return sorted(pids - owned - (set(marked_processes(owned)) - unreadable))
def gate():
    """Hold the first launch until the Mac is unlocked, idle 120 to 9,000 s and free of veyra for 10 s."""
    deadline = float(os.environ.get('VEYRA_VERIFY_DEADLINE') or time.time()+1800)
    began = quiet = time.monotonic()
    while True:
        assert time.time() < deadline, 'launch gate not met before the deadline'
        if veyra_pids(): quiet = time.monotonic()
        idle = input_idle_seconds()
        if 120 <= idle <= 9000 and time.monotonic()-quiet >= 10 and not locked(): break
        time.sleep(.5)
    clock = {'run':RUN,'launch_idle_seconds':idle,'quiet_seconds':time.monotonic()-quiet,
        'gate_seconds':time.monotonic()-began,'launch_monotonic':time.monotonic(),'launch_epoch':time.time(),
        'failure':None}
    save('input-idle-launch',clock)
    save('focus-failure',clock)
def gate_self_test():
    """Drive gate() on a virtual clock with stubbed idle, lock and pgrep readers; each bound and reader must hold it."""
    global input_idle_seconds, locked, veyra_pids, time
    readers = input_idle_seconds, locked, veyra_pids, time
    class Clock:
        now = 0.0
        def monotonic(self): return self.now
        def time(self): return 1e9+self.now
        def sleep(self, seconds): self.now += seconds
    def released(idle=200, lock=False, busy_until=-1.0):
        """The virtual second gate() let the launch go, or None when it held until the two-minute deadline."""
        global input_idle_seconds, locked, veyra_pids, time
        clock = time = Clock()
        input_idle_seconds, locked = (lambda: idle), (lambda: lock)
        veyra_pids = lambda: {1} if clock.now < busy_until else set()
        os.environ['VEYRA_VERIFY_DEADLINE'] = str(1e9+120)
        try: gate()
        except AssertionError as error:
            assert 'deadline' in str(error), error
            return None
        return clock.now
    try:
        assert released(idle=119) is None, 'gate() released at 119 s of idle'
        assert released(idle=9001) is None, 'gate() released at 9,001 s of idle'
        assert released(lock=True) is None, 'gate() released while the screen was locked'
        for idle in [120,9000]:
            assert 10 <= (released(idle=idle) or -1) < 11, f'gate() did not release 10 s into {idle} s of idle'
        assert 29 <= (released(busy_until=20) or -1) < 31, 'gate() did not wait 10 s after the last running veyra'
    finally:
        input_idle_seconds, locked, veyra_pids, time = readers
    print('PASS gate() holds below 120 s and above 9,000 s of idle, while locked, and until 10 s after the last veyra')
def observe_failure(step, kind):
    """Record whether human input came after launch, so a supervisor retries only a failure no person caused."""
    path = PROOF/'input-idle-launch.json'
    if not path.exists(): return
    clock = json.loads(path.read_text())
    idle = input_idle_seconds()
    elapsed = time.monotonic()-clock['launch_monotonic']
    clock.update(failure=step,kind=kind,failure_idle_seconds=idle,seconds_since_launch=elapsed,
        failure_epoch=time.time(),human_input=idle < elapsed,
        classification='human-input' if idle < elapsed else 'no-human-input')
    save('focus-failure',clock)
def make_git_shim():
    """Put a run-owned git first on the fake lane's PATH; it answers the branch read only while branch-on exists."""
    import shlex
    BRANCH_BIN.mkdir(exist_ok=True)
    git = BRANCH_BIN/'git'
    git.write_text('#!/bin/sh\n'
        'test -e '+shlex.quote(str(DATA/'branch-on'))+' || PATH='+shlex.quote(os.environ['PATH'])+' exec git "$@"\n'
        'test "$1" = branch && test "$2" = --show-current || exit 1\n'
        'touch '+shlex.quote(str(DATA/'branch-started'))+'\n'
        'sleep 0.5\n'
        'touch '+shlex.quote(str(DATA/'branch-finished'))+'\n'
        "printf '%s\\n' fixture-branch\n")
    git.chmod(0o700)
def start(label):
    assert not locked()
    foreign = foreign_veyra()
    assert not foreign, f'a veyra this run did not start is running: {foreign}'
    saved = sql('select live_pid from threads where live_pid is not null') if DB.exists() else ''
    ENV['VEYRA_REAP_PIDS'] = ','.join(saved.split())
    if LANE == 'fake':
        rec = PROOF/'rec'/f'{len(owned_app_pids())+1:02d}-{label}'
        ENV['VEYRA_RECORD'] = str(rec)
        (PROOF/'rec-current').write_text(str(rec)+'\n')
    app = subprocess.Popen([str(pathlib.Path('target/debug/veyra').resolve())],cwd=DATA/'appcwd',env=ENV,
        stdout=(PROOF/(label+'.stdout')).open('w'),stderr=(PROOF/(label+'.stderr')).open('w'),
        start_new_session=True)
    pid = app.pid
    (PROOF/'pid').write_text(str(pid)+'\n')
    (PROOF/'pgid').write_text(str(os.getpgid(pid))+'\n')
    with (PROOF/'owned-app-pids').open('a') as out: out.write(str(pid)+'\n')
    end = time.monotonic() + 30
    ready = f'veyra ready pid={pid} data_dir={DATA}'
    while time.monotonic() < end:
        assert app.poll() is None, (label,app.returncode)
        if ready in (PROOF/(label+'.stderr')).read_text(): break
        time.sleep(.05)
    assert ready in (PROOF/(label+'.stderr')).read_text()
    with (PROOF/'stderr.log').open('a') as out: out.write((PROOF/(label+'.stderr')).read_text())
    window = window_id(pid)
    assert window
    (PROOF/'window-id').write_text(window+'\n')
    identity()
    return pid
def restart(label, crash=False):
    stop(label,crash)
    return start(label)
def drawer_fixture(name='echo', job=False):
    assert LANE == 'fake'
    source = pathlib.Path('crates/engine/tests/fixtures/tui')/(name+'.jsonl')
    records = [json.loads(line) for line in source.read_text().splitlines()]
    if name == 'echo':
        records.insert(0,dict(type='emit_raw',base64='G1s/MjAwNGg='))
    if job:
        records = [dict(type='transcript_start',provider='shell',protocol='terminal',cli_version='fixture',
            scenario='shell-background-job',source='scripted'),*records]
    link_fixture('drawer.jsonl',fixture_source(records))
def drawer_keys(name, chord, before, after, draft=None, caret=None):
    state = ctl(name+'-before','dump')
    assert state['focus'] == before, (name,state['focus'],before)
    ctl(name+'-key','keys',chord)
    state = poll(name,lambda d:d['focus'] == after)
    if draft is not None: assert state['composer_text'] == draft
    if caret is not None: assert state['composer_caret'] == caret
    return state
def fixture_source(records):
    text = '\n'.join(json.dumps(record) for record in records)+'\n'
    folder = FIXTURES/'records'
    folder.mkdir(exist_ok=True)
    source = folder/(hashlib.sha256(text.encode()).hexdigest()+'.jsonl')
    if not source.exists(): source.write_text(text)
    return source
def link_fixture(name, source):
    target = FIXTURES/name
    if target.exists() and target.samefile(source): return
    pending = target.with_name(target.name+'.next')
    pending.unlink(missing_ok=True)
    os.link(source,pending)
    os.replace(pending,target)
def lanes_used():
    """Each lane the running app has assigned, with its last spawn, read from that app's own recordings.

    A stopped app returns none, because its next launch starts the fake CLI's lanes again at one.
    """
    try: os.kill(int((PROOF/'pid').read_text()),0)
    except ProcessLookupError: return {}
    observed = {}
    for path in pathlib.Path((PROOF/'rec-current').read_text().strip()).glob('t*-s*.jsonl'):
        match = re.fullmatch(r't(\d+)-s(\d+)(?:-\d+)?\.jsonl',path.name)
        if match:
            lane,epoch = map(int,match.groups())
            observed[lane] = max(observed.get(lane,0),epoch)
    return observed
def next_lane():
    return max(lanes_used(),default=0)+1
def stage_records(records):
    source = fixture_source(records)
    rows = sql('select threads.id,count(turns.id) from threads left join turns on turns.thread_id=threads.id group by threads.id order by threads.id')
    turns = [int(row.split('|')[1]) for row in rows.splitlines()]+[0]
    observed = lanes_used()
    for lane,count in enumerate(turns,1):
        last = observed.get(lane,0)
        for epoch in range(last+1,max(last,count)+9):
            link_fixture(f't{lane}-s{epoch}.jsonl',source)
def stage(scenario, slow=False, turns=1):
    records = [json.loads(line) for line in (SOURCE/(scenario+'.jsonl')).read_text().splitlines()]
    if slow:
        expanded = []
        for record in records:
            wire = record.get('line',{})
            if wire.get('event',{}).get('delta',{}).get('text') == 'ok':
                for text in ['o','k']:
                    part = json.loads(json.dumps(record))
                    part['line']['event']['delta']['text'] = text
                    part['after_ms'] = 500
                    expanded.append(part)
            else:
                if wire.get('type') == 'result': record['after_ms'] = 1500
                expanded.append(record)
        records = expanded
    if scenario == 'background-tasks':
        for record in records:
            wire = record.get('line',{})
            ambient_only = wire.get('tasks') == [{'id':'ambient-work','ambient':True}]
            if wire.get('subtype') == 'background_tasks_changed' and ambient_only:
                record['after_ms'] = 3000
            if wire.get('subtype') == 'background_tasks_changed' and wire.get('tasks') == []:
                record['after_ms'] = 3000
    if turns > 1:
        assert scenario == 'ok-reply'
        start = next(i for i,record in enumerate(records) if record.get('type')=='expect_outbound'
            and record.get('line',{}).get('type')=='user')
        repeat = records[start:-1]
        ending = records[-1]
        records = records[:-1]
        for turn in range(2,turns+1):
            text = json.dumps(repeat).replace('<id:prompt>', '<id:prompt'+str(turn)+'>')
            records.extend(json.loads(text))
        records.append(ending)
    stage_records(records)
def codex_records(scenario, slow=False, root=True):
    records = [json.loads(line) for line in (CODEX_SOURCE/(scenario+'.jsonl')).read_text().splitlines()]
    expanded = []
    for record in records:
        wire = record.get('line',{})
        method = wire.get('method','')
        if root and record['type']=='expect_outbound' and method in ['thread/start','thread/resume']:
            wire['params'].setdefault('config',{
                'mcp_servers.veyra.command':'<any>', 'mcp_servers.veyra.args':['mcp'],
                'mcp_optional_startup_grace_ms':5000, 'mcp_servers.veyra.tool_timeout_sec':30,
                'mcp_servers.veyra.env':{'VEYRA_SOCKET':'<any>','VEYRA_MCP_TOKEN':'<any>'}})
        if slow and method=='item/agentMessage/delta' and wire['params']['delta']=='ok':
            for text in ['o','k']:
                part = json.loads(json.dumps(record))
                part['line']['params']['delta'] = text
                part['after_ms'] = 500
                expanded.append(part)
        else:
            if slow and method=='turn/completed': record['after_ms'] = 1000
            expanded.append(record)
    return expanded
def create_codex(name, scenario='ok-reply', slow=False, records=None, prompt='reply with the word ok'):
    assert LANE == 'fake'
    stage_records(codex_records(scenario,slow) if records is None else records)
    action(name+'-new','NewThread')
    action(name+'-provider','PickProvider',{'provider':'codex'})
    action(name+'-mode','PickPermission',{'mode':'ask_first'})
    ctl(name+'-text','text',prompt)
    before = int(sql('select count(*) from threads'))
    ctl(name+'-send','keys','secondary-enter')
    dump = poll(name+'-selected',lambda d: d['selected_thread'] is not None
        and int(sql('select count(*) from threads'))==before+1)
    assert dump['composer']['provider']=='codex'
    return dump['selected_thread']['id']
def grok_records(scenario, slow=False):
    records = [json.loads(line) for line in (GROK_SOURCE/(scenario+'.jsonl')).read_text().splitlines()]
    expanded = []
    for record in records:
        wire = record.get('line',{})
        if slow and wire.get('method')=='session/update' and wire['params']['update'].get('content',{}).get('text')=='ok':
            for text in ['o','k']:
                part = json.loads(json.dumps(record))
                part['line']['params']['update']['content']['text'] = text
                part['after_ms'] = 500
                expanded.append(part)
        else:
            if slow and wire.get('method')=='_x.ai/session/prompt_complete': record['after_ms'] = 1000
            expanded.append(record)
    return expanded
def create_grok(name, scenario='ok-reply', slow=False, records=None, prompt='reply with the word ok'):
    assert LANE == 'fake'
    stage_records(grok_records(scenario,slow) if records is None else records)
    action(name+'-new','NewThread')
    action(name+'-provider','PickProvider',{'provider':'grok'})
    action(name+'-mode','PickPermission',{'mode':'ask_first'})
    ctl(name+'-text','text',prompt)
    before = int(sql('select count(*) from threads'))
    ctl(name+'-send','keys','secondary-enter')
    dump = poll(name+'-selected',lambda d: d['selected_thread'] is not None
        and int(sql('select count(*) from threads'))==before+1)
    assert dump['composer']['provider']=='grok'
    return dump['selected_thread']['id']
def create(name, prompt, slow=False, scenario='ok-reply'):
    if LANE == 'fake' and scenario is not None: stage(scenario,slow)
    action(name+'-new','NewThread')
    assert ctl(name+'-text','text',prompt) == {'ok':True}
    before = int(sql('select count(*) from threads'))
    ctl(name+'-send','keys','secondary-enter')
    dump = poll(name+'-selected',lambda d: d['selected_thread'] is not None
        and int(sql('select count(*) from threads')) == before+1)
    return dump['selected_thread']['id']
def stage_family(parent='waiting', child='complete'):
    identity()
    number = next_lane()
    records = [json.loads(line) for line in (SOURCE/'parent-start-thread.jsonl').read_text().splitlines()]
    first = next(i for i,record in enumerate(records) if record.get('line',{}).get('type') == 'result')
    if parent == 'running':
        records.insert(first, {'type':'expect_outbound','line':{'type':'control_request','request_id':'veyra-interrupt','request':{'subtype':'interrupt'}}})
        records[first+1]['after_ms'] = 0
        records[first+1]['line']['subtype'] = 'error_during_execution'
    elif parent == 'attention':
        approval = [json.loads(line) for line in (SOURCE/'approval-deny.jsonl').read_text().splitlines()]
        insertion = next(i for i,record in enumerate(records) if record.get('type') == 'mcp_expect' and record.get('line',{}).get('id') == 3)+1
        records[insertion:insertion] = approval[4:6]
    else:
        assert parent == 'waiting'
    if child == 'complete':
        child_records = [json.loads(line) for line in (SOURCE/'child-task.jsonl').read_text().splitlines()]
        for record in child_records:
            wire = record.get('line',{})
            if wire.get('type') == 'assistant':
                record['after_ms'] = 2000
                wire['message']['content'][0]['text'] = 'First report line\n**Second report line**\nThird report line\n\n- A list item\n- Another item\n\n~~~text\ncode line one\ncode line two\n~~~'
            if wire.get('type') == 'result': record['after_ms'] = 30000
    else:
        assert child == 'running'
        child_records = [json.loads(line) for line in (SOURCE/'long-reply.jsonl').read_text().splitlines()]
    def write(lane,epoch,records):
        link_fixture(f't{lane}-s{epoch}.jsonl',fixture_source(records))
    write(number,1,records)
    write(number+1,1,child_records)
    resumed = [json.loads(line) for line in (SOURCE/'parent-wake.jsonl').read_text().splitlines()]
    for epoch in [2,3]: write(number,epoch,resumed)
    return number
def create_family(name, parent='waiting', child='complete'):
    stage_family(parent,child)
    action(name+'-new','NewThread')
    ctl(name+'-text','text',name)
    before = int(sql('select count(*) from threads'))
    ctl(name+'-send','keys','secondary-enter')
    dump = poll(name+'-root',lambda d:d['selected_thread'] is not None
        and d['selected_thread']['lineage']=='Root' and d['selected_thread']['title']==name
        and int(sql('select count(*) from threads'))>=before+1)
    root = dump['selected_thread']['id']
    poll(name+'-task',lambda d:any(task['parent']==root for task in d['tasks']))
    row = sql("select child_thread_id||'|'||id from tasks where parent_thread_id='"+root+"'").split('|')
    assert len(row)==2
    return root,row[0],row[1]
def prepare():
    """Bring the run to new-thread's end state: one project, the fake CLI installed, every provider Ready and a
    new Claude composer, as a fresh launch shows.

    Every section after new-thread's first calls this, so it can run alone or after any earlier section.
    It creates no thread.
    """
    identity()
    if sql('select count(*) from projects') == '0':
        action('prepare-project','AddProject',{'path':str(DATA/'project')})
        poll('prepare-project-added',lambda d: any(row['kind']=='project' for row in d['sidebar']['rows']))
    if LANE == 'fake':
        if not (DATA/'fake-cli').exists():
            (DATA/'fake-cli').symlink_to(pathlib.Path('target/debug/veyra-fake-cli').resolve())
            action('prepare-recheck','RecheckProviders')
        poll('prepare-ready',lambda d: all(probe['readiness']=='ready' for probe in d['providers']))
    action('prepare-new','NewThread')
    action('prepare-claude','PickProvider',{'provider':'claude'})
    poll('prepare-composer',lambda d: d['view']=='new_thread' and d['composer']['provider']=='claude')
def select(name, thread):
    action(name+'-select','SelectThread',{'thread':thread})
    return poll(name,lambda d:d['selected_thread'] is not None and d['selected_thread']['id']==thread
        and len(d['transcript']['items'])>0)
def family_delivered(name, root, task):
    return poll(name,lambda d:sql("select delivery from tasks where id='"+task+"'")=='delivered'
        and any(row['id']==root and row['status']=='Done' for row in d['sidebar']['rows'] if row['kind']=='thread'))
def pending_wake(root):
    return sql("select count(*) from tasks where parent_thread_id='"+root+"' and delivery='pending'")=='1'
