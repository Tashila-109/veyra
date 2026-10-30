"""Smoke repair regressions. Real scene/batch/mode entries; only raw OS I/O is modelled."""
import hashlib, json, os, pathlib, signal, subprocess
from types import SimpleNamespace
import orchestration as o, frame_probe
from raw_test_io import RawFixture
from self_tests import Fixture, require_rejection

class SceneIO(RawFixture):
    def __init__(self,*args):
        super().__init__(*args);self.lane='fake'
    def __enter__(self):
        super().__enter__()
        # The worker is an OS stub; staging still reads typed on-disk input through the real driver.
        for name in ['parent-start-thread','parent-wake','long-reply','ok-reply','approval-deny']:
            (self.repo/'crates/engine/tests/fixtures/claude'/(name+'.jsonl')).write_text(json.dumps(
                {'type':'emit_inbound','line':{'type':'result','subtype':'success'}})+'\n')
        self.running=True;self.tasks=[];self.scene_live={};self.scene_queries=[];self.scene_stops=[];self.scene_number=0
        self.look['control_labels']={'Name("send")':'Send'}
        self.scene_view='new_thread';self.drawer_open=False;self.quit_open=False;self.fault=None
        for name in ['pid','pgid','owned-app-pids']:(self.proof/name).write_text(str(self.pid))
        (self.proof/'window-id').write_text('23');(self.data/'veyra.lock').write_text(str(self.pid))
        rec=self.proof/'rec';rec.mkdir();(self.proof/'rec-current').write_text(str(rec))
        return self
    def dump(self):
        value=super().dump();value['view']=self.scene_view;value['tasks']=self.tasks
        if self.fault=='late-family-child-live' and self.clock.now<20:
            value['tasks']=[dict(task,state='Running') for task in self.tasks]
        value['transcript'].update(items=[{'kind':'user','text':'fixture scene'}],family_card={'rows':[{'status':'Working'}]} if self.tasks else None)
        value['quit_sheet']['open']=self.quit_open
        value['composer']['dock']={'kind':'approval'}
        if self.drawer_open:
            value['drawer']={'open':True,'active_tab':1,'height':236,'tabs':[{'number':1,'label':'shell','exit_code':None,'exited':False,'error':None}],
                'terminal':{'grid':['VEYRA_TUI_READY'],'focused':self.focus=='drawer','columns':80,'rows':10},'close_sheet':None}
        else:value['drawer']={'open':False,'terminal':None,'tabs':[]}
        return value
    def command(self,argv,**options):
        argv=[str(x) for x in argv];name=pathlib.Path(argv[0]).name;out=None
        if name=='sqlite3':
            query=argv[-1]
            if query=='select count(*) from threads where live_pgid is not null':
                self.scene_queries.append(query);out=str(1+sum(self.scene_live.values())) # unrelated warm session
            elif query.startswith('select live_pgid is null from threads where id='):
                self.scene_queries.append(query);thread=query.split("'")[1];out='0' if self.scene_live.get(thread,False) else '1'
            elif query.startswith('select ended_at is not null'):out='1'
            elif query.startswith('select child_thread_id'):
                task=self.tasks[-1];out=task['child']+'|'+task['id']
            elif query.startswith('select threads.id,count(turns.id)'):out='\n'.join(row['id']+'|1' for row in self.threads)
        elif name=='veyra' and 'ctl' in argv:
            cmd=argv[argv.index('ctl')+1:]
            if cmd[0]=='action':
                action=cmd[1].split('::')[-1];data=json.loads(cmd[2]) if len(cmd)>2 else {}
                if action=='NewThread':self.scene_view='new_thread'
                elif action=='OpenSettings':self.scene_view='settings';out='{"ok":true}'
                elif action=='SelectThread':
                    self.selected=next(row for row in self.threads if row['id']==data['thread']);self.scene_view='chat';out='{"ok":true}'
                elif action=='StopThread':
                    thread=data['thread'];self.scene_stops.append(thread)
                    if self.tasks and thread==self.tasks[-1]['parent']:
                        task=self.tasks[-1];self.scene_live[task['child']]=self.fault in ['child-live','late-family-child-live']
                        next(row for row in self.threads if row['id']==thread).update(
                            status='Done' if self.fault=='parent-state' else 'Held',hold='child_report' if self.fault=='parent-hold' else 'stopped')
                        task['state']={'Settled':{'outcome':{'Completed':{}} if self.fault=='child-outcome' else {'Interrupted':{'reason':'Thread stopped'}},
                            'delivery':{} if self.fault=='no-wake' else {'Pending':{'wake':'fixture-wake'}}}}
                    elif not self.tasks or thread!=self.tasks[-1]['child']:self.scene_live[thread]=self.fault=='scene-live'
                    out='{"ok":true}'
                elif action=='DebugFocusShellDrawer':self.focus='drawer';out='{"ok":true}'
                elif action=='FocusComposer':self.focus='composer';out='{"ok":true}'
                elif action=='Quit':self.quit_open=True;out='{"ok":true}'
            elif cmd[:2]==['keys','secondary-enter']:
                self.scene_number+=1;root=f'{self.scene_number:08x}-1111-2222-3333-444444444444'
                row={'id':root,'status':'Done','header_status':'Done','hold':None,'title':self.text,'kind':'thread','lineage':'Root'}
                self.threads.append(row);self.selected=row;self.scene_view='chat';self.text='';self.scene_live[root]=self.fault=='scene-live'
                if row['title']=='smoke-family':
                    child=root[:-1]+'5';self.threads.append({'id':child,'status':'Working','kind':'thread'})
                    self.scene_live[root]=self.scene_live[child]=True
                    self.tasks.append({'id':'fixture-task','parent':root,'child':child,'state':'Running'})
                out='{"ok":true}'
            elif cmd[:2]==['keys','secondary-j']:self.drawer_open=True;out='{"ok":true}'
            elif cmd[:2]==['keys','secondary-w']:self.drawer_open=False;out='{"ok":true}'
            elif cmd[:2]==['keys','escape']:self.quit_open=False;out='{"ok":true}'
        if out is not None:return SimpleNamespace(returncode=0,stdout=out if options.get('text') else out.encode(),stderr='' if options.get('text') else b'')
        return super().command(argv,**options)

def scene_drains_ignore_other_threads(f):
    import smoke
    # Every scene runs its actual driver path. The family parent and an unrelated session stay live.
    for page in ['multi-agent','shell-drawer','chat-send-stream','settings','approvals-questions','stop-interrupt','restart-recovery','new-thread']:
        before=len(f.scene_queries);known=set(f.scene_live)
        smoke.drive(page)
        queries=f.scene_queries[before:]
        expected=[f.tasks[-1]['child']] if page=='multi-agent' else sorted(set(f.scene_live)-known)
        assert queries==["select live_pgid is null from threads where id='"+thread+"'" for thread in expected], 'smoke drain omitted or queried the wrong scene thread IDs: '+page
        assert not f.drawer_open and f.scene_view=='new_thread'
        if expected and page!='multi-agent':
            with SceneIO(f.root/('live-'+page),f.runner) as io:
                io.fault='scene-live';began=io.clock.now
                require_rejection(lambda:smoke.drive(page),'timed out')
                created=set(io.scene_live)
                assert len(created)==1 and all(io.scene_live.values()), 'fixture did not retain the scene provider'
                assert io.scene_queries and set(io.scene_queries)=={"select live_pgid is null from threads where id='"+next(iter(created))+"'"}, 'live scene provider was not queried'
                assert 30<=io.clock.now-began<31 and not io.drawer_open and io.scene_view=='new_thread', 'live scene did not stop within its existing bound and restore'
    parent,child=f.tasks[0]['parent'],f.tasks[0]['child']
    assert f.scene_live[parent] and not f.scene_live[child], 'held parent must remain valid while child exits'
    assert next(row for row in f.threads if row['id']==parent)['hold']=='stopped'
    assert f.tasks[0]['state']['Settled']['delivery']['Pending']['wake']
    for fault in ['child-live','parent-state','parent-hold','child-outcome','no-wake','late-family-child-live']:
        with SceneIO(f.root/fault,f.runner) as io:
            io.fault=fault;began=io.clock.now
            require_rejection(lambda:smoke.drive('multi-agent'),'timed out')
            assert 30<=io.clock.now-began<31, 'scene stop poll bound changed'
            assert io.scene_view=='new_thread', 'scene return path was skipped after failed drain'
scene_drains_ignore_other_threads.fixture_type=SceneIO

def failures_record_hid_and_lock(f):
    f.idle=173;f.lock=True;f.extra_scripts={'LATER':"raise AssertionError('second ordinary failure')"}
    assert f.batch("from driver import *; poll('plain-timeout',lambda state:False)")==1
    rows=[row for row in f.outcome()['attempts'][0]['sections'] if row['kind']=='failed']
    assert len(rows)==2
    times=json.loads((f.evidence/'attempt-1/sections.json').read_text())
    for row in rows:
        seen=row['observed'];timing=next(t for t in times if t['section']==row['section'])
        assert seen.get('failure_idle_seconds')==173 and seen.get('screen_locked') is True, 'every failed section needs fresh HID and lock telemetry'
        assert seen['failure_epoch']>=timing['start_epoch']
    assert rows[0]['observed']['observation_source']=='driver', 'poll timeout must observe at its failure site'
    assert rows[1]['observed']['observation_source']=='section-exit', 'generic assertion must be observed at section exit'
    with Fixture(f.root/'spawn-failure',f.runner) as io:
        io.crash_target_spawn=True
        assert io.batch()==1
        assert io.outcome()['attempts'][0]['runner_observed']['failure_idle_seconds']==io.idle
    with Fixture(f.root/'cleanup-failure',f.runner) as io:
        io.cleanup_failure=True
        assert io.batch()==1
        cleanup=json.loads((io.evidence/'attempt-1/sections.json').read_text())[-1]
        assert cleanup['section']=='SKILL/Cleanup' and cleanup['observed']['screen_locked'] is False
    with Fixture(f.root/'human-input',f.runner) as io:
        io.idle=0
        assert io.batch("raise AssertionError('human input fixture')")==1
        row=io.outcome()['attempts'][0]
        assert next(r for r in row['sections'] if r['kind']=='failed')['observed']['human_input'] is True
        assert not row.get('failure_captures'), 'human input authorized a failure capture'

def failures_keep_separate_captures(f):
    f.vary_capture=True;f.extra_scripts={'LATER':"raise AssertionError('later failure')"}
    assert f.batch("raise AssertionError('first failure')")==1
    captures=f.outcome()['attempts'][0]['failure_captures']
    assert len(captures)==2 and len({row['path'] for row in captures})==2, 'failures reused a capture path'
    assert len({row['sha256'] for row in captures})==2
    for row in captures:
        path=pathlib.Path(row['path']);fresh=json.loads(pathlib.Path(row['freshness']).read_text())
        assert path.exists() and hashlib.sha256(path.read_bytes()).hexdigest()==row['sha256']==fresh['sha256'], 'earlier failure bytes were overwritten'
        assert row['section'] in ['page/Target','page/Later']

def repeated_section_keeps_capture_ordinals(f):
    f.vary_capture=True
    assert f.batch("raise AssertionError('same section fails twice')",steps=[('page','Target'),('page','Target')])==1
    attempt=f.outcome()['attempts'][0]
    assert [row['section'] for row in attempt['sections'] if row['kind']=='failed']==['page/Target','page/Target']
    captures=attempt['failure_captures']
    assert len(captures)==2 and all(row['section']=='page/Target' for row in captures)
    assert len({row['path'] for row in captures})==len({row['freshness'] for row in captures})==2, 'repeated section reused its PNG or freshness ordinal'
    assert len({row['sha256'] for row in captures})==2, 'fixture did not produce two different frames'
    for row in captures:
        path=pathlib.Path(row['path']);fresh=json.loads(pathlib.Path(row['freshness']).read_text())
        assert hashlib.sha256(path.read_bytes()).hexdigest()==row['sha256']==fresh['sha256'], 'an earlier repeated-section capture was overwritten'

class UnknownHIDIO(Fixture):
    def command(self,argv,**options):
        if pathlib.Path(str(argv[0])).name=='ioreg' and 'IOHIDSystem' in argv:
            fault=getattr(self,'hid_fault',None)
            if fault=='unavailable':raise OSError('fixture HID service unavailable')
            if fault=='timeout':raise subprocess.TimeoutExpired(argv,options.get('timeout',5))
            if fault in ['missing','malformed']:
                out='' if fault=='missing' else '"HIDIdleTime" = unavailable'
                return SimpleNamespace(returncode=0,stdout=out if options.get('text') else out.encode(),stderr='')
        return super().command(argv,**options)

def unknown_hid_never_authorizes_capture(f):
    for fault in ['unavailable','timeout','missing','malformed','missing-clock']:
        with UnknownHIDIO(f.root/fault,f.runner) as io:
            io.hid_fault=fault
            prefix="from driver import PROOF; (PROOF/'input-idle-launch.json').unlink(); " if fault=='missing-clock' else ''
            assert io.batch(prefix+"raise AssertionError('unknown HID failure')")==1
            attempt=io.outcome()['attempts'][0];row=next(row for row in attempt['sections'] if row['kind']=='failed')
            assert row['observed']['human_input'] is None and row['observed']['classification']=='unknown', 'unknown HID was assumed safe'
            if fault!='missing-clock':assert row['observed']['failure_idle_seconds'] is None
            assert not attempt.get('failure_captures') and io.capture_number==0, 'unknown HID authorized a capture'
            assert not any(len(cmd)>1 and cmd[1]=='-c' and pathlib.Path(cmd[0]).name.startswith('python') for cmd in io.calls), 'unknown HID spawned a failure capture worker'
            assert attempt['cleanup_exit']==0 and not attempt['scratch_left'], 'unknown HID skipped Cleanup'

def mode_args(f,mode):
    common=['--evidence',str(f.evidence),'--tier-s',str(f.tier),'--go',str(f.go),'--deadline',str(f.args.deadline),'--scratch',f.args.scratch]
    return common+({'smoke':['--smoke','--base','HEAD'],'sections':['--sections','settings/Native smoke'],
        'gate':['--gate'],'slice':['--slice-end'],'guard':['--smoke','--base','HEAD','--then-guard','--baseline',str(f.baseline),'--part','6c']}[mode])

def main(f,argv):
    handlers={n:signal.getsignal(n) for n in o.SIGNALS};mask=signal.pthread_sigmask(signal.SIG_BLOCK,[])
    try:return f.runner.main(argv,validate=False)
    finally:
        for n,h in handlers.items():signal.signal(n,h)
        signal.pthread_sigmask(signal.SIG_SETMASK,mask)

def native_mode_args(f,mode):
    argv=mode_args(f,'slice' if mode=='mutants' else mode)
    if mode=='mutants':
        from negative_tests import prepare_case
        manifest=f.root/'mutants.json';prepare_case(f,manifest)
        f.write_go(cutoff_epoch=f.args.deadline,mutant_manifest_sha256=o.sha(manifest))
        argv += ['--native-mutants',str(manifest)]
    return argv

def native_modes_require_quiet(f):
    for mode in ['smoke','sections','gate','slice','guard','mutants']:
        with Fixture(f.root/mode,f.runner) as io:
            io.load=9
            argv=native_mode_args(io,mode)
            assert main(io,argv) in [1,2], 'busy machine entered native mode '+mode
            assert not io.calls and not io.probes and not io.app_launches, 'quiet gate was skipped before '+mode
            receipts=list(io.evidence.rglob('*quiet*.json'))
            assert receipts and all(json.loads(p.read_text())['state']=='held' for p in receipts)
            assert io.clock.now==1200, 'quiet-machine timeout changed'
    with Fixture(f.root/'admitted',f.runner) as io:
        io.load=9;io.load_until=30
        assert main(io,mode_args(io,'smoke'))==0
        assert io.launch_times and min(io.launch_times)>=30, 'launch preceded quiet admission'
        assert json.loads((io.evidence/'quiet-machine.json').read_text())['state']=='admitted'
    with Fixture(f.root/'bad-load',f.runner) as io:
        io.load_error=True
        assert main(io,mode_args(io,'smoke'))==2 and not io.calls, 'load query error admitted native work'
    with RawFixture(f.root/'direct-probe',f.runner) as io:
        io.load=9
        require_rejection(lambda:frame_probe.main(['scenes','candidate','held','--part','6c','--scene','working']),'did not drop below 5.0')
        assert io.app_launches==0, 'direct native probe skipped quiet admission'

def quiet_limit_is_exclusive_five(f):
    for load in [4.99,5.0,5.5]:
        with Fixture(f.root/str(load),f.runner) as io:
            io.load=load
            code=main(io,mode_args(io,'smoke'));receipt=json.loads((io.evidence/'quiet-machine.json').read_text())
            if load==4.99:
                assert code==0 and io.launch_times and receipt['state']=='admitted', '4.99 must admit native work'
            else:
                assert code==2 and not io.calls and not io.probes and not io.app_launches, 'load '+str(load)+' must hold native work'
                assert receipt['state']=='held' and io.clock.now==1200, 'quiet boundary changed the existing timeout'
            assert receipt['load_limit_exclusive']==5.0, 'quiet limit changed from the independent 5.0 contract'

class SlowLoadIO(Fixture):
    def command(self,argv,**options):
        if pathlib.Path(str(argv[0])).name=='sysctl':
            timeout=options['timeout'];self.clock.sleep(min(2,timeout))
            if timeout<2:raise subprocess.TimeoutExpired(argv,timeout)
        return super().command(argv,**options)

def arm_short_cutoff(io):
    io.load=9;io.args.deadline=io.clock.time()+31
    io.write_go(cutoff_epoch=io.args.deadline)
    os.environ['VEYRA_VERIFY_DEADLINE']=str(io.args.deadline)

def quiet_wait_stops_at_go_cutoff(f):
    for mode in ['smoke','sections','gate','slice','guard','mutants']:
        with Fixture(f.root/mode,f.runner) as io:
            arm_short_cutoff(io)
            argv=native_mode_args(io,mode)
            assert json.loads(io.go.read_text())['cutoff_epoch']==io.args.deadline, 'fixture lost the short GO cutoff'
            assert main(io,argv)==(1 if mode=='guard' else 2), 'cutoff did not stop '+mode
            assert io.clock.now==31 and not io.calls and not io.probes and not io.app_launches, 'quiet admission extended the GO cutoff or launched '+mode
            outcome=io.outcome();assert 'GO cutoff reached' in outcome.get('error',outcome['status'])
            for path in io.evidence.rglob('*quiet*.json'):
                receipt=json.loads(path.read_text());assert receipt['state']=='held' and receipt['finished'] and receipt['elapsed_seconds']<=31
            assert not list(io.evidence.glob('attempt-*')) and io.data.exists(), 'cutoff entered a native batch'
            if mode=='guard':assert json.loads((io.evidence/'guard-done.json').read_text())['passed'] is False
    with RawFixture(f.root/'direct-probe',f.runner) as io:
        arm_short_cutoff(io)
        require_rejection(lambda:frame_probe.main(['scenes','candidate','cutoff','--part','6c','--scene','working']),'GO cutoff reached')
        assert io.clock.now==31 and not io.app_launches and not io.calls and not io.backdrop_alive, 'direct probe waited past cutoff or launched'
        result=json.loads((io.root/'direct-probes/scenes/cutoff/result.json').read_text())
        assert not result['passed'] and result['cleanup_exit']==0 and result['scratch_removed'] and result['data_removed'], 'cutoff probe did not close cleanly'
    with SlowLoadIO(f.root/'slow-load-read',f.runner) as io:
        arm_short_cutoff(io)
        assert main(io,mode_args(io,'smoke'))==2 and io.clock.now==31 and not io.calls, 'load read exceeded the remaining GO budget'
        receipt=json.loads((io.evidence/'quiet-machine.json').read_text());assert receipt['state']=='held' and receipt['finished']

CASES=[('smoke-scoped-drain',scene_drains_ignore_other_threads),('failure-hid',failures_record_hid_and_lock),
    ('failure-captures',failures_keep_separate_captures),('quiet-native',native_modes_require_quiet),
    ('capture-ordinal',repeated_section_keeps_capture_ordinals),('unknown-hid',unknown_hid_never_authorizes_capture),
    ('quiet-limit',quiet_limit_is_exclusive_five),('quiet-cutoff',quiet_wait_stops_at_go_cutoff)]
