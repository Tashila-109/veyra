import hashlib, importlib, json, math, os, secrets, shutil, signal, sqlite3, subprocess, sys, tempfile, threading, time
from pathlib import Path

HELPERS=Path(__file__).resolve().parent
V=HELPERS.parents[3]
PROBE_RECTANGLE=(24,60,960,740)
E=Path(os.environ['VEYRA_FRAME_GUARD_PROOF']).resolve()
TOOLS=Path(os.environ['VEYRA_FRAME_TOOLS']).resolve()
sys.path.insert(0,str(HELPERS))
BASE=Path(os.environ['VEYRA_FRAME_BASELINE']).resolve()
import orchestration as admission
admitted = admission.identity(V)
admission.require_tier(os.environ['VEYRA_VERIFY_TIER_S'],admitted)
admission.require_go(os.environ['VEYRA_VERIFY_GO'],admitted,float(os.environ['VEYRA_VERIFY_DEADLINE']),guard=True)
phase,side,label=sys.argv[1:4]
profile=False
idle_focus=sys.argv[sys.argv.index('--idle-focus')+1] if '--idle-focus' in sys.argv else 'composer'
assert idle_focus in ['composer','drawer']
scene=sys.argv[sys.argv.index('--scene')+1] if '--scene' in sys.argv else 'stream'
motion=sys.argv[sys.argv.index('--motion')+1] if '--motion' in sys.argv else 'normal'
assert motion in ['normal','reduced']
out=E/phase/label
out.mkdir(parents=True)
(out/'invocation.json').write_text(json.dumps([sys.executable,*sys.argv],indent=2)+'\n')
(out/'worker-pid').write_text(str(os.getpid())+'\n')
run=secrets.token_hex(4)
scratch=Path(tempfile.mkdtemp(prefix='veyra-frame-work-',dir='/tmp'))
data=Path('/tmp/veyra-frame-'+run)
data.mkdir()
workspace=scratch/'workspace'
(workspace/'target/debug').mkdir(parents=True)
(workspace/'crates').symlink_to(V/'crates')
(workspace/'.agents').symlink_to(V/'.agents')
binary=(BASE if side=='baseline' else V/'target')/'debug/veyra'
fake=binary.with_name('veyra-fake-cli')
for path in [binary,fake]: (workspace/'target/debug'/path.name).symlink_to(path)
proof=workspace/'target/verify-veyra'/run
(proof/'fixtures').mkdir(parents=True)
(proof/'lane').write_text('fake\n')
(proof/'pages-run').write_text('')
for name in ['capture-window','winid','backdrop','front-app','visible-frame']:
    (proof/name).symlink_to(TOOLS/name)
(proof/'helper-ownership.json').write_text(json.dumps({'run':run,'workspace':str(workspace),
    'scratch':str(scratch),'data':str(data),'proof':str(proof)})+'\n')
(out/'ownership.json').write_bytes((proof/'helper-ownership.json').read_bytes())
for name,provider in [('probe.jsonl','claude'),('probe-codex.jsonl','codex'),('probe-grok.jsonl','grok')]:
    source='probe-initialize.jsonl' if provider=='claude' else 'probe-ready.jsonl' if provider=='codex' else name
    shutil.copyfile(V/'crates/engine/tests/fixtures'/provider/source,proof/'fixtures'/name)
(data/'project').mkdir()
(data/'appcwd').mkdir()
(data/'login-shell').write_text('#!/bin/sh\nexec /bin/sh "$@"\n')
(data/'login-shell').chmod(0o700)
os.environ.update(RUN=run,VEYRA_VERIFY_TMP='/tmp/veyra-frame-',PYTHONDONTWRITEBYTECODE='1')
os.environ.pop('VEYRA_VERIFY_RULE_INDEX',None)
os.chdir(workspace)
sys.path.insert(0,str(V/'.agents/skills/verify-veyra/bin'))
d=importlib.import_module('driver')
d.ENV['SHELL']=str(data/'login-shell')
d.ENV.pop('VEYRA_RECORD',None)

def start_without_recording(label):
    assert not d.locked()
    assert not d.foreign_veyra(), 'a Veyra this run did not start is running'
    saved=d.sql('select live_pid from threads where live_pid is not null') if d.DB.exists() else ''
    d.ENV['VEYRA_REAP_PIDS']=','.join(saved.split())
    app_env=d.ENV.copy()
    assert 'VEYRA_RECORD' not in app_env
    (proof/'app-environment-keys.json').write_text(json.dumps({'keys':sorted(app_env),
        'veyra_record_absent':True,'values_recorded':False},indent=2)+'\n')
    d.add_window_rule(label)
    app=subprocess.Popen([str((workspace/'target/debug/veyra').resolve())],cwd=data/'appcwd',env=app_env,
        stdout=(proof/(label+'.stdout')).open('w'),stderr=(proof/(label+'.stderr')).open('w'),
        start_new_session=True)
    pid=app.pid
    (proof/'pid').write_text(str(pid)+'\n')
    (proof/'pgid').write_text(str(os.getpgid(pid))+'\n')
    with (proof/'owned-app-pids').open('a') as log: log.write(str(pid)+'\n')
    deadline=time.monotonic()+30
    ready=f'veyra ready pid={pid} data_dir={data}'
    while time.monotonic()<deadline:
        assert app.poll() is None, (label,app.returncode)
        if ready in (proof/(label+'.stderr')).read_text(): break
        time.sleep(.05)
    assert ready in (proof/(label+'.stderr')).read_text()
    with (proof/'stderr.log').open('a') as log: log.write((proof/(label+'.stderr')).read_text())
    window=d.window_id(pid)
    assert window
    (proof/'window-id').write_text(window+'\n')
    d.owned_window_record(label)
    d.identity()
    d.normalize_owned_window(label)
    return pid
d.start=start_without_recording

def stage_records_without_recording(records):
    assert d.sql('select count(*) from threads')=='0'
    assert d.sql('select count(*) from turns')=='0'
    fixture=d.fixture_source(records)
    for epoch in range(1,9): d.link_fixture(f't1-s{epoch}.jsonl',fixture)
    (proof/'fixture-staging.json').write_text(json.dumps({'source':'fresh SQLite database',
        'threads':0,'turns':0,'lane':1,'epochs':list(range(1,9)),
        'reads_recordings':False,'fixture_sha256':hashlib.sha256(fixture.read_bytes()).hexdigest()},indent=2)+'\n')
d.stage_records=stage_records_without_recording
expected_rev=os.environ['VEYRA_FRAME_BASELINE_REV'] if side=='baseline' else subprocess.check_output(['git','rev-parse','HEAD'],cwd=V,text=True).strip()
result={'phase':phase,'side':side,'profiled':profile,'label':label,'run':run,
    'binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),
    'fake_sha256':hashlib.sha256(fake.read_bytes()).hexdigest(),'expected_rev':expected_rev,'passed':False}
original_ctl=d.ctl
streaming=False

def frontmost(label):
    pid=int((proof/'pid').read_text())
    command=[str(proof/'front-app'),str(pid)]
    (proof/(label+'-frontmost-invocation.json')).write_text(json.dumps(command)+'\n')
    raw=json.loads(subprocess.check_output(command,text=True))
    value={key:raw[key]for key in ['owned_pid','frontmost_is_owned']}
    assert value['owned_pid']==pid and isinstance(value['frontmost_is_owned'],bool)
    (proof/(label+'-frontmost.json')).write_text(json.dumps(value,indent=2)+'\n')
    return value

def note_foreign(ownership,step):
    if not ownership['frontmost_is_owned']:
        result['foreign_activation']={'step':step,**ownership}

def checked_ctl(name,*args,**kwargs):
    if streaming and d.locked():
        d.observe_failure(name,'capture')
        raise AssertionError('screen is locked')
    value=original_ctl(name,*args,**kwargs)
    if streaming and args and args[0]=='dump' and value.get('window_active') is False:
        ownership=frontmost('inactive')
        note_foreign(ownership,name)
        d.observe_failure(name,'focus-timeout')
        raise AssertionError('window became inactive during stream; frontmost_is_owned='+str(ownership['frontmost_is_owned']))
    if streaming and args and args[0]=='dump':
        guarded_region(name)
    return value
d.ctl=checked_ctl
def identity():
    pid=int((proof/'pid').read_text())
    assert os.getpgid(pid)==pid==int((proof/'pgid').read_text())
    assert (data/'veyra.lock').read_text().strip()==str(pid)
    value=d.ctl('identity','dump')
    assert value['pid']==pid and value['data_dir']==str(data) and value['build_rev']==expected_rev
    return pid
d.identity=identity
def load(): return subprocess.check_output(['sysctl','-n','vm.loadavg'],text=True).strip()
def cpu(pid):
    row=subprocess.check_output(['ps','-o','pid=,pgid=,rss=,stat=,cputime=','-p',str(pid)],text=True).split()
    assert int(row[0])==int(row[1])==pid
    total=0.0
    for part in row[-1].split(':'): total=60*total+float(part)
    return total
def front(label):
    if d.locked():
        d.observe_failure(label,'capture')
        raise AssertionError('screen is locked')
    command=[str(proof/'capture-window'),str(result['pid']),(proof/'window-id').read_text().strip()]
    (proof/(label+'-front-invocation.json')).write_text(json.dumps(command)+'\n')
    row=subprocess.run(command,capture_output=True,text=True)
    (proof/(label+'-front.json')).write_text(json.dumps({'exit':row.returncode,'stderr':row.stderr})+'\n')
    if row.returncode:
        result['cover_trip'] = 'covered' in row.stderr or 'intersecting' in row.stderr
        note_foreign(frontmost('inactive'),label)
        d.observe_failure(label,'capture')
    assert row.returncode==0,'frontmost guard failed'
    if (proof/'backdrop-windows.json').exists():guarded_region(label)
def guarded_region(label):
    try:
        value=d.region_snapshot(result['pid'],int((proof/'window-id').read_text()),
            json.loads((proof/'backdrop-windows.json').read_text()),PROBE_RECTANGLE)
        (proof/(label+'-region-guard.json')).write_text(json.dumps(value,indent=2)+'\n')
    except Exception:
        d.observe_failure(label,'capture')
        raise
def start_backdrop():
    executable=data/'perf-backdrop'
    shutil.copy2(HELPERS/'tools/backdrop',executable)
    log=proof/'backdrop.stdout'
    process=subprocess.Popen([str(executable)],stdout=log.open('w'),stderr=(proof/'backdrop.stderr').open('w'))
    (proof/'backdrop-owned-pids').write_text(str(process.pid)+'\n')
    (proof/'notifications-backdrop-pid').write_text(str(process.pid)+'\n')
    (proof/'notifications-backdrop-ownership.json').write_text(json.dumps({
        'pid':process.pid,'executable':str(executable)})+'\n')
    thread=threading.Thread(target=lambda:process.wait(),daemon=True);thread.start()
    end=time.monotonic()+10
    while True:
        assert process.poll()is None,'owned backdrop has died'
        rows=[json.loads(line.split('backdrop window ',1)[1])for line in log.read_text().splitlines()
              if line.startswith('backdrop window ')]
        if rows and 'backdrop ready pid='in log.read_text():break
        assert time.monotonic()<end,'owned backdrop did not become ready'
        time.sleep(.05)
    assert all(row['pid']==process.pid for row in rows)
    (proof/'backdrop-windows.json').write_text(json.dumps({'pid':process.pid,
        'windows':[row['window']for row in rows]},indent=2)+'\n')
    d.verify_owned_backdrop(process.pid,executable)
def place_probe_window():
    pid=result['pid']
    window=int((proof/'window-id').read_text())
    before=d.owned_window_record('probe-placement-before',pid,window)
    assert before is not None,'owned probe placement requires yabai'
    x,y,width,height=PROBE_RECTANGLE
    assert before['frame']['w']==width and before['frame']['h']==height,'owned probe size changed'
    visible=json.loads((proof/'launch-normalized-window.json').read_text())['visible_frame']
    vx,vy,vw,vh=visible
    assert vx<=x and vy<=y and x+width<=vx+vw and y+height<=vy+vh,'proved rectangle is outside the owned display'
    tool=shutil.which('yabai')
    assert tool,'owned probe placement requires yabai'
    operations=[]
    for operation,value in [('--resize',f'abs:{width}:{height}'),('--move',f'abs:{x}:{y}')]:
        d.owned_window_record('probe-placement-check',pid,window)
        command=[tool,'-m','window',str(window),operation,value]
        operations.append({'invocation':command})
        (proof/'probe-placement-operations.json').write_text(json.dumps(operations,indent=2)+'\n')
        changed=subprocess.run(command,capture_output=True,text=True)
        operations[-1].update(exit=changed.returncode,stderr=changed.stderr)
        (proof/'probe-placement-operations.json').write_text(json.dumps(operations,indent=2)+'\n')
        assert changed.returncode==0,'owned probe placement failed'
    end=time.monotonic()+5
    while True:
        after=d.owned_window_record('probe-placement-after',pid,window)
        if after['frame']=={'x':x,'y':y,'w':width,'h':height}:break
        assert time.monotonic()<end,'owned probe did not reach the proved rectangle'
        time.sleep(.05)
    result['probe_placement']={'before':before['frame'],'after':after['frame'],'visible_frame':visible,
        'rectangle':list(PROBE_RECTANGLE),
        'note':"probe placement aligned with the s6c final's proved region rectangle; identical for both sides"}
sampler=None
reaper=None
watch=None
def interrupted(number, frame):
    raise KeyboardInterrupt('owned frame probe interrupted')
signal.signal(signal.SIGTERM,interrupted)
signal.signal(signal.SIGINT,interrupted)
try:
    d.gate()
    result['gate']=json.loads((proof/'input-idle-launch.json').read_text())
    result['load_start']=load()
    d.make_git_shim()
    result['pid']=d.start('launch')
    reaper=threading.Thread(target=lambda:os.waitpid(result['pid'],0),daemon=True)
    reaper.start()
    from window_wait import wait_for_active
    def record_wait(value):
        result['activation']=value
        result['activation_wait_seconds']=value['wait_seconds']
        (proof/'activation-wait.json').write_text(json.dumps(value,indent=2)+'\n')
        if not value['window_active']: note_foreign(value['frontmost'],'launch-window-wait')
    try:
        wait_for_active(lambda:d.action('launch-activate','ActivateWindow'),
            lambda:d.ctl('launch-window-wait','dump',log=False),lambda:frontmost('launch'),record_wait)
    except AssertionError:
        d.observe_failure('launch-window-wait','focus-timeout')
        raise
    initial=d.ctl('initial','dump')
    if 'login_path' in initial: d.poll('login-ready',lambda value:value['login_path']!='pending')
    if 'look' in initial:
        d.action('solid','SetWindowBackground',{'background':'solid'})
    d.prepare()
    start_backdrop()
    place_probe_window()
    d.action('backdrop-return-focus','ActivateWindow')
    d.poll('backdrop-window-active',lambda value:value['window_active'])
    guarded_region('prepared')
    result['bounds']=subprocess.check_output([str(proof/'winid'),str(result['pid']),'--bounds'],text=True).strip()
    if scene=='stream':
        records=[json.loads(line) for line in (d.SOURCE/'stream-2000.jsonl').read_text().splitlines()]
        expected=''.join(record['line']['event']['delta']['text'] for record in records if record.get('type')=='emit_inbound'
            and record['line'].get('event',{}).get('delta',{}).get('type')=='text_delta')
        assert len(expected)==2000
        d.stage('stream-2000')
        d.action('perf-new','NewThread')
        d.ctl('perf-text','text','stream 2000 deltas')
        d.action('perf-activate','ActivateWindow')
        before=d.ctl('perf-before','dump')
        before_front=frontmost('before')
        if not before['window_active']:
            note_foreign(before_front,'perf-before')
            d.observe_failure('perf-before','focus-timeout')
            raise AssertionError('window inactive before stream after successful launch activation')
        front('before')
        assert before['perf']['max_batch_gap_ms']==0
        result['measurement_load_start']=load()
        from commit_watch import CommitWatch
        watch=CommitWatch(d.DB).start()
        cpu0=cpu(result['pid'])
        started=time.monotonic()
        streaming=True
        d.ctl('perf-send','keys','secondary-enter')
        thread=None
        mid=False
        captured=False
        next_active_check=started+1
        deadline=started+60
        while True:
            assert time.monotonic()<deadline,'stream deadline'
            if thread is None:
                rows=d.sql('select id from threads').split()
                thread=rows[0] if rows else None
            elif d.sql("select count(*) from turns where thread_id='"+thread+"' and state='ended'")=='1':
                break
            elapsed=time.monotonic()-started
            if time.monotonic()>=next_active_check:
                d.ctl('perf-active-'+str(int(elapsed)),'dump',log=False)
                next_active_check=time.monotonic()+1
            if profile and sampler is None and elapsed>=1:
                command=['sample',str(result['pid']),'5','-file',str(proof/'sample.txt')]
                (proof/'sample-invocation.json').write_text(json.dumps(command)+'\n')
                sampler=subprocess.Popen(command,stdout=(proof/'sample.stdout').open('w'),stderr=(proof/'sample.stderr').open('w'))
                result['sampler_pid']=sampler.pid
            if not mid and elapsed>=5:
                front('midstream')
                mid=True
            if profile and not captured and elapsed>=7:
                d.capture('streaming')
                captured=True
            time.sleep(.2)
        d.ctl('perf-wait','wait',thread,'Done','30')
        after=d.ctl('perf-after','dump')
        streaming=False
        after_dump=time.time()
        cpu1=cpu(result['pid'])
        observed=watch.stop()
        watch=None
        text_writes=sum(change['kind']=='text_write' and change['t']<=after_dump for change in observed['changes'])
        (proof/'commit-watch.json').write_text(json.dumps(observed,indent=2)+'\n')
        result['measurement_load_end']=load()
        result['wall_seconds']=time.monotonic()-started
        front('after')
        frontmost('after')
        items=[item for item in after['transcript']['items'] if item['kind']=='assistant']
        assert len(items)==1 and items[0]['text']==expected and items[0]['done'] and items[0]['delta_count']==2000
        assert d.sql("select json_extract(body_json,'$.AssistantText.text')||'|'||done from items where kind='assistant_text'")==expected+'|1'
        turn_start,turn_end,outcome=d.sql("select started_at||'|'||ended_at||'|'||outcome_json from turns").split('|')
        assert json.loads(outcome)=='Completed'
        turn_seconds=(int(turn_end)-int(turn_start))/1000
        batches=after['perf']['applied_batches']-before['perf']['applied_batches']
        result.update(cpu_before=cpu0,cpu_after=cpu1,cpu_seconds=cpu1-cpu0,turn_seconds=turn_seconds,
            core_fraction=(cpu1-cpu0)/turn_seconds,commits=after['perf']['commits']-before['perf']['commits'],
            repaints=after['perf']['repaints']-before['perf']['repaints'],
            batches=batches,batches_per_second=batches/turn_seconds,largest_gap_ms=after['perf']['max_batch_gap_ms'],
            exact_2000_deltas=True,stored_text_exact=True,outcome='Completed',window_active=True)
        result['commits_text_writes']=text_writes
        result['commits_fixed']=result['commits']-text_writes
        result['commits_10s']=result['commits_fixed']+5
        d.poll('sessions-ended',lambda value:d.sql('select count(*) from threads where live_pgid is not null')=='0')
        if sampler is not None:
            result['sampler_exit']=sampler.wait(timeout=20)
            assert result['sampler_exit']==0
    else:
        threads=[]
        if scene=='idle':
            d.drawer_fixture()
            thread=d.create('idle','Idle with an owned shell')
            threads.append(thread)
            d.ctl('idle-done','wait',thread,'Done')
            d.action('idle-translucent','SetWindowBackground',{'background':'translucent'})
            d.poll('idle-translucent-applied',lambda value:value['look']['background']=='translucent')
            d.ctl('idle-open-drawer','keys','secondary-j')
            d.poll('idle-shell-ready',lambda value:value['drawer']['terminal'] is not None
                and 'VEYRA_TUI_READY' in ''.join(value['drawer']['terminal']['grid'])
                and not value['drawer']['tabs'][0]['running'])
            if idle_focus=='composer':
                d.action('idle-composer-focus','FocusComposer')
                d.poll('idle-focus-applied',lambda value:value['focus']=='composer')
            else:
                d.action('idle-drawer-focus','DebugFocusShellDrawer')
                d.poll('idle-focus-applied',lambda value:value['focus']=='drawer'
                    and value['drawer']['terminal']['focused'])
            result.update(idle_variant='a' if idle_focus=='composer' else 'b',
                focus_target='composer' if idle_focus=='composer' else 'drawer bare prompt; steady cursor')
        else:
            assert scene=='working'
            records=[json.loads(line) for line in (d.SOURCE/'long-reply.jsonl').read_text().splitlines()]
            fixture=d.fixture_source(records)
            for lane in range(1,6):
                for epoch in range(1,9): d.link_fixture(f't{lane}-s{epoch}.jsonl',fixture)
            for index in range(5):
                thread=d.create('working-'+str(index),'Working '+str(index),scenario=None)
                threads.append(thread)
                d.ctl('working-ready-'+str(index),'wait',thread,'Working')
            d.action('working-nontext-focus','ScrollTranscriptUp')
            d.poll('working-nontext-focused',lambda value:value['focus']=='none')
            result['focus_target']='non-text transcript; ScrollTranscriptUp focuses Shell root'
        d.action('scene-activate','ActivateWindow')
        d.poll('scene-active',lambda value:value['window_active'])
        if scene=='working':
            machine_flags={key:initial.get('look',{}).get(key,False) for key in
                ['reduce_motion','reduce_transparency','increase_contrast']}
            override={**machine_flags,'reduce_motion':motion=='reduced'}
            d.action('scene-motion-override','DebugDisplayOptions',override)
            applied=d.poll('scene-motion-applied',lambda value:all(value['look'][key]==flag for key,flag in override.items())
                and value['look']['background']=='solid' and value['focus']=='none')
            result.update(motion=motion,machine_flags_at_launch=machine_flags,display_options_override=override,
                override_applied=True,override_dump=applied,macos_settings_changed=False)
            (proof/'display-options-override.json').write_text(json.dumps({'action':'veyra::DebugDisplayOptions',
                'payload':override,'machine_flags_at_launch':machine_flags,'applied_look':applied['look'],
                'macos_settings_changed':False},indent=2)+'\n')
        time.sleep(.5)
        before=d.ctl('scene-before','dump')
        front('scene-before')
        if scene=='idle':
            assert not any(row.get('status')=='Working' for row in before['sidebar']['rows'])
            assert before['focus']==idle_focus and before['drawer']['open']
            assert before['selected_thread']['status']=='Done'
            assert before['look']['background']=='translucent'
            assert not before['drawer']['tabs'][0]['running']
            assert before['drawer']['terminal']['focused']==(idle_focus=='drawer')
        else:
            assert sum(row.get('status')=='Working' for row in before['sidebar']['rows'])==5
            assert before['focus']=='none'
            assert all(before['look'][key]==flag for key,flag in override.items())
            assert before['look']['background']=='solid'
        streaming=True
        started=time.monotonic()
        observations=[]
        while time.monotonic()-started<10:
            time.sleep(min(.1,max(0,10-(time.monotonic()-started))))
            observed=d.ctl('scene-sample','dump',log=False)
            if scene=='idle':
                assert observed['focus']==idle_focus and observed['drawer']['open']
                assert not any(row.get('status')=='Working' for row in observed['sidebar']['rows'])
                assert observed['selected_thread']['status']=='Done'
                assert not observed['drawer']['tabs'][0]['running']
            else:
                assert observed['focus']=='none'
                assert all(observed['look'][key]==flag for key,flag in override.items())
                assert observed['look']['background']=='solid'
                assert sum(row.get('status')=='Working' for row in observed['sidebar']['rows'])==5
            terminal=observed['drawer'].get('terminal')
            observations.append({'seconds':time.monotonic()-started,'repaints':observed['perf']['repaints'],
                'terminal_repaints':terminal['repaints'] if terminal else None,
                'terminal_frame_requests':[tab['frame_requests'] for tab in observed['drawer'].get('tabs',[])],
                'focus':observed['focus'],'window_active':observed['window_active']})
        after=d.ctl('scene-after','dump')
        if scene=='working':
            assert all(after['look'][key]==flag for key,flag in override.items())
            assert after['look']['background']=='solid'
        duration=time.monotonic()-started
        streaming=False
        front('scene-after')
        delta=after['perf']['repaints']-before['perf']['repaints']
        result.update(scene=scene,seconds=duration,repaints=delta,repaints_per_second=delta/duration,
            observations=observations,before=before,after=after,measurement_complete=True,
            focus_before=before['focus'],focus_after=after['focus'],
            caret_source='dump.focus, drawer.terminal.focused and per-sample counters')
        changed=[]
        previous=before['perf']['repaints']
        for row in observations:
            if row['repaints']!=previous:
                changed.append(row['seconds'])
                previous=row['repaints']
        result['shell_counter_change_sample_times']=changed
        result['sampled_shell_change_intervals_ms']=[1000*(b-a) for a,b in zip(changed,changed[1:])]
        if scene=='idle':
            result['terminal_repaints']=after['drawer']['terminal']['repaints']-before['drawer']['terminal']['repaints']
            result['terminal_frame_requests']=after['drawer']['tabs'][0]['frame_requests']-before['drawer']['tabs'][0]['frame_requests']
        if scene=='idle':
            result['idle_rule_evaluated_by_supervisor']=True
            shell_pid=int(before['drawer']['tabs'][0]['pid'])
            d.action('idle-close-focus','DebugFocusShellDrawer')
            d.poll('idle-close-focused',lambda value:value['focus']=='drawer')
            d.ctl('idle-close-tab','keys','secondary-w')
            d.poll('idle-drawer-hidden',lambda value:not value['drawer']['open'])
            shell_end=time.monotonic()+5
            while True:
                try: os.kill(shell_pid,0)
                except ProcessLookupError: break
                assert time.monotonic()<shell_end, 'owned bare-prompt shell remained after close'
                time.sleep(.05)
            result.update(owned_shell_pid=shell_pid,owned_shell_gone=True)
        else:
            result['within_working_budget']=delta/duration<=12
            for thread in threads: d.action('scene-stop-'+thread,'StopThread',{'thread':thread})
        d.poll('scene-sessions-ended',lambda value:d.sql('select count(*) from threads where live_pgid is not null')=='0')
    result['passed']=True
except BaseException as error:
    result['passed']=False
    result['error']=str(error)
    import traceback
    (proof/'traceback.txt').write_text(traceback.format_exc())
finally:
    if watch is not None: watch.stop()
    if sampler is not None and sampler.poll() is None:
        sampler.terminate()
        sampler.wait(timeout=10)
    rec_paths=[str(path.relative_to(root)) for root in [proof,data] for path in root.rglob('rec')]
    recording_proof={'recording_directories':rec_paths,'rec_current_exists':(proof/'rec-current').exists(),
        'environment_key_proof':'app-environment-keys.json','values_recorded':False}
    (proof/'recording-absent.json').write_text(json.dumps(recording_proof,indent=2)+'\n')
    result['recording_off']=not rec_paths and not recording_proof['rec_current_exists']
    if not result['recording_off']:
        result.update(passed=False,error='recording path unexpectedly written')
    result['load_end']=load()
    idle=d.input_idle_seconds()
    clock=json.loads((proof/'input-idle-launch.json').read_text()) if (proof/'input-idle-launch.json').exists() else {}
    result['human_input']=bool(clock and idle<time.monotonic()-clock['launch_monotonic'])
    result['observed']=json.loads((proof/'focus-failure.json').read_text())if(proof/'focus-failure.json').exists()else None
    import importlib.machinery, importlib.util
    loader=importlib.machinery.SourceFileLoader('frame_cleanup',str(HELPERS/'run'))
    spec=importlib.util.spec_from_loader(loader.name,loader)
    runner=importlib.util.module_from_spec(spec);loader.exec_module(runner)
    cleanup_source=runner.sections()[('SKILL','Cleanup')]
    owned_app=(proof/'pid').exists()
    if owned_app:
        if reaper is None:
            owned_pid=int((proof/'pid').read_text())
            reaper=threading.Thread(target=lambda:os.waitpid(owned_pid,0),daemon=True)
            reaper.start()
        env=os.environ.copy()
        env.update(PYTHONPATH=str(V/'.agents/skills/verify-veyra/bin'),RUN=run,VEYRA_VERIFY_TMP='/tmp/veyra-frame-')
        with (proof/'cleanup.log').open('w') as log:
            clean=subprocess.run(['bash','-c',cleanup_source],env=env,stdout=log,stderr=subprocess.STDOUT)
        result['cleanup_exit']=clean.returncode
    else:
        d.cleanup_window_rules(proof,run)
        shutil.rmtree(data)
        result['cleanup_exit']=0
    result['owned_rule_cleanup']=json.loads(max(proof.glob('yabai-rule-cleanup-*.json'),key=lambda p:p.stat().st_mtime_ns).read_text())if(proof/'owned-yabai-labels').exists()else {'passed':True,'skipped':'no labels'}
    assert result['owned_rule_cleanup']['passed']
    shutil.copytree(proof,out/'run',copy_function=shutil.copy2)
    if result['cleanup_exit']==0 and not data.exists(): shutil.rmtree(scratch)
    if data.exists() and not owned_app: shutil.rmtree(data)
    result.update(scratch_removed=not scratch.exists(),data_removed=not data.exists(),idle_end=idle)
    (out/'result.json').write_text(json.dumps(result,indent=2)+'\n')
    assert result['cleanup_exit']==0 and result['scratch_removed'] and result['data_removed'],out/'result.json'
sys.exit(0 if result['passed'] else 1)
