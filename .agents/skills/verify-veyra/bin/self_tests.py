"""Entry-point verification with process, clock, HID, window and capture I/O fixtures.

Guard functions, admission, probe entry/sampling, set orchestration, driver.start and the
runner's batch loop execute unchanged. Fixture trust hashes describe tiny on-disk binaries;
no guard function is replaced. Unknown OS commands fail closed instead of launching anything.
"""
import contextlib, importlib.machinery, importlib.util, io, json, os, pathlib, shutil
import signal, subprocess, sys, tempfile, time, traceback
from types import SimpleNamespace
from unittest.mock import patch
import frame_guard as fg, frame_probe, orchestration as o, pages

REAL_TIME=time.time

def require_rejection(call, contains=''):
    try: call()
    except (AssertionError,o.SafetyStop,FileNotFoundError,SystemExit) as error:
        assert contains in str(error), (contains,str(error))
        return
    raise AssertionError('fault accepted: '+contains)

class Clock:
    def __init__(self): self.now=0.;self.epoch=REAL_TIME()
    def monotonic(self): return self.now
    def time(self): return self.epoch+self.now
    def sleep(self,seconds): self.now+=seconds

class WindowIO:
    """Window/dump/clock boundary for the real probe main; no guard decision is fabricated."""
    def __init__(self,fixture): self.f=fixture;self.clock=fixture.clock;self.started=False;self.samples=0
    def start(self,args,tools,baseline,result):
        self.args=args;self.started=True;self.f.window_starts+=1
        assert all((tools/name).is_file() for name in o.TOOLS)
    def read(self,name):
        state=self.f.idle_state(self.args.idle_focus)
        if name=='initial':
            if not self.f.modern: state.pop('palette',None);state.pop('toasts',None)
            else: state['toasts']=[]
        if name=='scene-sample':
            self.samples+=1
            if self.f.transient_toast and self.samples==2: state['toasts'][0]['text']='transient defect'
        return state
    def prepare(self,args,backdrop):
        assert backdrop.read_text()=='helper backdrop'
        self.front('prepared')  # OS window preparation
    def front(self,name):
        if self.f.cover and self.f.cover(self.args,name): raise AssertionError('owned window is covered')
    def stream(self):
        self.front('stream')
        return {'core_fraction':self.f.cpu_ratio if self.args.side=='candidate' else 1.,
            'batches_per_second':60.,'largest_gap_ms':20,'repaints':10}
    def working(self,motion): self.front('working');return {'repaints_per_second':6.,'repaints':60,'seconds':10.}
    def finish(self,result): return {}
    def close(self,output):
        return {'cleanup_exit':0,'scratch_removed':True,'data_removed':True,'recording_off':True,
            'owned_rule_cleanup':{'passed':True}}

class Fixture:
    """A disposable filesystem and OS command boundary. Runtime code is never patched by function."""
    def __init__(self,root,runner):
        self.root=root.absolute();self.runner=runner;self.clock=Clock();self.stack=contextlib.ExitStack()
        self.calls=[];self.probes=[];self.window_starts=0;self.modern=False;self.transient_toast=False
        self.cover=None;self.cpu_ratio=1.;self.after_probe=None;self.before_probe=None;self.capture_failures=0
        self.failures_used=0;self.idle=200;self.lock=False;self.running=False;self.pid=99999999
        self.batch_script={};self.app_launches=0;self.mode='full_access';self.background='solid';self.stale_image=False;self.cleanup_failure=False;self.yabai=False;self.window_flags={};self.frame={'x':20,'y':40,'w':1280,'h':800};self.run='0123abcd'
    def __enter__(self):
        self.cwd=pathlib.Path.cwd();self.module=sys.modules.get('driver')
        self.repo=self.root/'repo';(self.repo/'crates/veyra/src/ui').mkdir(parents=True)
        (self.repo/'crates/veyra/Cargo.toml').write_text('')
        (self.repo/'crates/veyra/src/ui/theme.rs').write_text('original source\n')
        (self.repo/'.agents').symlink_to(o.BIN.parents[2])
        for name in o.BINARY_NAMES:
            p=self.repo/'target'/name;p.parent.mkdir(parents=True,exist_ok=True);p.write_text('fixture '+name)
        self.baseline=self.repo/'.tstack/tasks/v001/evidence/perf/bin'/fg.BASELINE_REVISION
        (self.baseline/'debug').mkdir(parents=True)
        for name in fg.BASELINE_BINARIES: (self.baseline/name).write_text('baseline '+name)
        self.pins={name:o.sha(self.baseline/name) for name in fg.BASELINE_BINARIES}
        (self.baseline/'identity.txt').write_text(fg.BASELINE_IDENTITY+'\n'+''.join(digest+'  '+name+'\n' for name,digest in self.pins.items()))
        self.stack.enter_context(patch.multiple(fg,BASELINE_MANIFEST_SHA256=o.sha(self.baseline/'identity.txt'),BASELINE_BINARIES=self.pins))
        for module,name,value in [(o,'REPO',self.repo),(self.runner,'REPO',self.repo)]: self.stack.enter_context(patch.object(module,name,value))
        self.stack.enter_context(patch.object(subprocess,'check_output',side_effect=self.check_output))
        self.stack.enter_context(patch.object(subprocess,'run',side_effect=self.command))
        self.stack.enter_context(patch.object(subprocess,'Popen',side_effect=self.popen))
        self.stack.enter_context(patch.object(os,'getpgid',side_effect=lambda pid:pid))
        self.stack.enter_context(patch.object(time,'time',side_effect=self.clock.time))
        self.stack.enter_context(patch.object(time,'monotonic',side_effect=self.clock.monotonic))
        self.stack.enter_context(patch.object(time,'sleep',side_effect=self.clock.sleep))
        old_which=shutil.which
        self.stack.enter_context(patch.object(shutil,'which',side_effect=lambda name,**kw:('/fixture/yabai' if self.yabai else None) if name=='yabai' else old_which(name,**kw)))
        self.stack.enter_context(patch.dict(os.environ,{'RUN':self.run,'VEYRA_VERIFY_TMP':str(self.root)+'/data-',
            'VEYRA_VERIFY_DEADLINE':str(self.clock.time()+3600),'PYTHONDONTWRITEBYTECODE':'1',
            'VEYRA_VERIFY_RULE_INDEX':str(self.root/'rule-index.jsonl')}))
        self.expected=o.code_identity(self.repo)
        self.tier=self.root/'tier/receipt.json';self.write_tier(self.tier,self.expected)
        self.go=self.root/'go.json';self.write_go()
        self.stack.enter_context(patch.dict(os.environ,{'VEYRA_VERIFY_TIER_S':str(self.tier),
            'VEYRA_VERIFY_GO':str(self.go),'VEYRA_FRAME_BASELINE':str(self.baseline),'VEYRA_FRAME_TOOLS':str(o.tier_tools(self.tier)),
            'VEYRA_FRAME_GUARD_PROOF':str(self.root/'direct-probes')}))
        self.evidence=self.root/'evidence';self.evidence.mkdir()
        self.args=SimpleNamespace(evidence=self.evidence,baseline=self.baseline,tier_s=self.tier,go=self.go,
            deadline=self.clock.time()+3600,part='6c',native_mutants=None,gate=False,slice_end=False,scratch=str(self.root)+'/data-')
        os.chdir(self.repo)
        self.driver=self.load_driver(self.run)
        return self
    def __exit__(self,*error):
        os.chdir(self.cwd)
        if self.module is None: sys.modules.pop('driver',None)
        else: sys.modules['driver']=self.module
        self.stack.close()
    def write_tier(self,path,expected):
        path.parent.mkdir(parents=True,exist_ok=True);(path.parent/'tools').mkdir(exist_ok=True)
        rows=[]
        for name,argv in o.COMMANDS.items():
            out=path.parent/(name+'.txt');out.write_text('fixture command output')
            rows.append({'name':name,'argv':argv,'exit':0,'output':out.name,'sha256':o.sha(out)})
        for name in o.TOOLS: (path.parent/'tools'/name).write_text('helper '+name)
        o.write(path,{'schema':1,'passed':True,'identity':expected,'started_epoch':self.clock.time()-2,
            'finished_epoch':self.clock.time()-1,'commands':rows,'tools':{name:o.sha(path.parent/'tools'/name) for name in o.TOOLS}})
    def write_go(self,**changes):
        o.write(self.go,dict({'launch_authorized':True,'identity':self.expected,'issued_epoch':self.clock.time()-1,
            'cutoff_epoch':self.clock.time()+3600,'frame_cost_guard_authorized':True,'arch_plan_paused':True},**changes))
    def idle_state(self,focus='composer'):
        state=self.dump()
        state.update(focus=focus,selected_thread={'status':'Done'},look={'background':'translucent'},
            drawer={'open':True,'tabs':[{'running':False}], 'terminal':{'focused':focus=='drawer'}},
            palette={'open':False},toasts=[{'kind':'notice','family':None,'text':'resting toast'}])
        if not self.modern: state.pop('palette');state.pop('toasts')
        return state
    def dump(self):
        self.repaint=getattr(self,'repaint',10)+1
        state={'pid':self.pid,'data_dir':str(self.data),'build_rev':'1'*40,'appearance':'dark','login_path':'loaded',
            'view':'new_thread','sidebar':{'width':240,'rows':[]},'selected_thread':None,'transcript':{'items':[]},
            'composer_text':'','composer':{'focused':True,'control_focus':None,'placeholder':'','stop_visible':False},
            'settings':{'default_mode':self.mode},'notice':None,'quit_sheet':{'open':False},'focus':'composer',
            'composer_caret':0,'drawer':None,'providers':[],'look':{'background':self.background},'perf':{'repaints':self.repaint},'window_active':True}
        if self.modern: state.update(palette={'open':False},toasts=[])
        return state
    def load_driver(self,run):
        self.run=run;self.proof=self.repo/'target/verify-veyra'/run
        self.proof.mkdir(parents=True,exist_ok=True);(self.proof/'fixtures').mkdir(exist_ok=True)
        (self.proof/'lane').write_text('live\n')
        if not (self.proof/'configured-default-mode').exists(): (self.proof/'configured-default-mode').write_text('full_access\n')
        (self.proof/'pages-run').write_text('app-launch-main\n')
        if not (self.proof/'input-idle-launch.json').exists():
            o.write(self.proof/'input-idle-launch.json',{'run':run,'launch_monotonic':self.clock.monotonic(),
                'launch_epoch':self.clock.time(),'gate_seconds':10})
        for name,p in o.helper_paths(o.tier_tools(self.tier)).items(): shutil.copy2(p,self.proof/name)
        self.data=pathlib.Path(str(self.root)+'/data-'+run);self.data.mkdir(exist_ok=True);(self.data/'appcwd').mkdir(exist_ok=True)
        (self.data/'logs').mkdir(exist_ok=True)
        (self.data/'logs/veyra.log').write_text(f'veyra ready pid={self.pid} data_dir={self.data}\n')
        (self.data/'veyra.db').write_text('fixture database; SQL is an OS boundary')
        spec=importlib.util.spec_from_file_location('driver',o.BIN/'driver.py');module=importlib.util.module_from_spec(spec)
        sys.modules['driver']=module;spec.loader.exec_module(module)
        return module
    def check_output(self,argv,**options):
        result=self.command(argv,**options)
        if result.returncode: raise subprocess.CalledProcessError(result.returncode,argv,output=result.stdout,stderr=result.stderr)
        return result.stdout
    def command(self,argv,**options):
        argv=[str(part) for part in argv];name=pathlib.Path(argv[0]).name;out='';code=0
        if name=='git':
            if '--git-common-dir' in argv: out=str(self.repo/'git-metadata')+'\n'
            elif 'rev-parse' in argv: out='1'*40+'\n'
            elif 'ls-files' in argv or 'status' in argv: out=''
            elif 'diff' in argv:
                if '--binary' in argv:
                    content=(self.repo/'crates/veyra/src/ui/theme.rs').read_text();out='' if content=='original source\n' else content
                elif '--name-only' in argv: out='.agents/skills/verify-veyra/bin/run\n'
                else: out='@@ -0,0 +1 @@\n+fixture helper\n'
            elif 'show' in argv:
                file=argv[-1].partition(':')[2];path=self.repo/file
                if path.is_file(): out=path.read_text()
                else: code=1
            else: raise AssertionError('unexpected Git read: '+repr(argv))
        elif name=='ioreg':
            if 'IOHIDSystem' in argv: out='"HIDIdleTime" = '+str(self.idle*1_000_000_000)
            else: out='CGSSessionScreenIsLocked' if self.lock else ''
        elif name=='pgrep': out=str(self.pid) if self.running else '';code=0 if out else 1
        elif name=='sysctl': out='{ 0.1 0.1 0.1 }'
        elif name=='defaults': out='Dark'
        elif name=='sqlite3':
            query=argv[-1]
            out={'pragma user_version':'4','pragma journal_mode':'wal','.tables':'grants items projects requests settings tasks threads turns',
                'select default_mode from settings':self.mode,'select live_pid from threads where live_pid is not null':''}.get(query,'0')
        elif name=='veyra' and 'ctl' in argv:
            command=argv[argv.index('ctl')+1:]
            if command[0]=='dump': out=json.dumps(self.dump())
            else: out='{"ok":true}'
        elif name=='yabai':
            if '--add' in argv:
                label=next(p[6:] for p in argv if p.startswith('label='))
                assert label in (self.proof/'owned-yabai-labels').read_text().splitlines()
                assert '--one-shot' in argv and 'app=^veyra$' in argv and 'manage=off' in argv
            elif 'query' in argv:
                out=json.dumps(dict({'id':23,'pid':self.pid,'app':'veyra','frame':self.frame,'is-floating':True,
                    'split-type':'none','split-child':'none','stack-index':0},**self.window_flags))
            elif '--resize' in argv:
                w,h=map(int,argv[-1].split(':')[1:]);self.frame.update(w=w,h=h)
            elif '--move' in argv:
                x,y=map(int,argv[-1].split(':')[1:]);self.frame.update(x=x,y=y)
            else: raise AssertionError('unexpected owned yabai operation')
        elif name=='visible-frame': out=json.dumps({'visible_frame':[2560,23,2560,1385]})
        elif name=='winid': out='24,60,960,740' if '--bounds' in argv else '23'
        elif name=='capture-window':
            if self.failures_used<self.capture_failures: self.failures_used+=1;code=1
        elif name=='screencapture':
            image=pathlib.Path(argv[-1]);image.write_bytes(b'fresh synthetic image')
            if self.stale_image: os.utime(image,ns=(1,1))
        else: raise AssertionError('OS command forbidden in offline fixture: '+repr(argv))
        if options.get('check') and code: raise subprocess.CalledProcessError(code,argv)
        if not options.get('text'): out=out.encode()
        err='' if options.get('text') else b''
        return SimpleNamespace(returncode=code,stdout=out,stderr=err)
    def popen(self,argv,**options):
        self.calls.append([str(p) for p in argv]);fixture=self
        if str(argv[0])=='bash' or str(argv[0]).endswith('python3') or (len(argv)>1 and str(argv[1]).endswith('frame_probe.py')):
            assert o.SIGNALS <= signal.pthread_sigmask(signal.SIG_BLOCK,[]), 'child spawn signals were not blocked'
        class Child:
            pid=fixture.pid;returncode=None
            def __init__(self):
                if str(argv[0])!='bash' and not pathlib.Path(str(argv[0])).name.startswith('python'):
                    self.communicate()
            def poll(self): return self.returncode
            def send_signal(self,number): self.returncode=0
            def wait(self,timeout=None):
                if pathlib.Path(str(argv[0])).name.startswith('python') and str(argv[1]).endswith('frame_probe.py'):
                    fixture.probes.append(list(argv[2:]));index=len(fixture.probes)
                    if fixture.before_probe: fixture.before_probe(index)
                    try:
                        with patch.dict(os.environ,options['env']): frame_probe.main(list(argv[2:]),WindowIO(fixture))
                        self.returncode=0
                    except BaseException: self.returncode=1
                    if fixture.after_probe: fixture.after_probe(index)
                    return self.returncode
                if self.returncode is None: self.communicate(timeout)
                return self.returncode
            def communicate(self,timeout=None):
                previous=os.environ.copy()
                try:
                    if 'env' in options: os.environ.update(options['env'])
                    if str(argv[0])=='bash':
                        run=os.environ['RUN'];fixture.driver=fixture.load_driver(run)
                        fixture.running=True
                        for name in ['pid','pgid']: (fixture.proof/name).write_text(str(fixture.pid))
                        (fixture.proof/'owned-app-pids').write_text(str(fixture.pid));(fixture.proof/'window-id').write_text('23')
                        (fixture.data/'veyra.lock').write_text(str(fixture.pid))
                        body=str(argv[-1]);key=fixture.batch_script.get(body)
                        if key is not None: exec(key,{'__name__':'__fixture_section__'})
                        if body=='CLEANUP' or body==fixture.runner.sections()[('SKILL','Cleanup')]:
                            if fixture.cleanup_failure: self.returncode=1;return '','owned cleanup did not finish'
                            fixture.running=False;shutil.rmtree(fixture.data)
                        self.returncode=0;return '',''
                    if 'ctl' not in [str(p) for p in argv]:
                        fixture.app_launches+=1;fixture.running=True
                        (fixture.data/'veyra.lock').write_text(str(fixture.pid))
                        ready=f'veyra ready pid={fixture.pid} data_dir={fixture.data}\n'
                        options['stderr'].write(ready);options['stderr'].flush()
                        self.returncode=None;return '',''
                    raise AssertionError('unexpected child')
                except BaseException:
                    self.returncode=1;return '',traceback.format_exc()
                finally:
                    os.environ.clear();os.environ.update(previous)
        return Child()
    def batch(self,script=None,full=False):
        self.args.gate=False;self.args.slice_end=full
        found={('SKILL','Launch'):'LAUNCH',('SKILL','Doctor'):'DOCTOR',('SKILL','Evidence'):'EVIDENCE',
            ('SKILL','Cleanup'):'CLEANUP',('SKILL','Recover section'):'RECOVER',('page','Target'):'TARGET',('page','Later'):'LATER'}
        self.batch_script={'TARGET':script or 'pass'}
        env=dict(os.environ,VEYRA_VERIFY_TMP=str(self.root)+'/data-')
        handlers={n:signal.getsignal(n) for n in o.SIGNALS};mask=signal.pthread_sigmask(signal.SIG_BLOCK,[])
        try: return self.runner.run_native(found,[('SKILL','Launch'),('page','Target'),('page','Later')],self.args,env,self.expected)
        finally:
            for n,h in handlers.items(): signal.signal(n,h)
            signal.pthread_sigmask(signal.SIG_SETMASK,mask)
    def outcome(self): return json.loads((self.evidence/'outcome.json').read_text())

def probe(f,label='direct',part='6c',scene='idle'):
    return frame_probe.main(['scenes','candidate',label,'--scene',scene,'--part',part],WindowIO(f))

def guard_tools_and_backdrop(f):
    result=fg.run_guard(f.args,f.expected)
    assert result['passed'] and len(result['pairs'])==5 and len(f.probes)==16
    assert all((pathlib.Path(row['receipt']).parent/'backdrop').read_text()=='helper backdrop' for row in result['attempts'])

def probe_uses_requested_part(f):
    f.modern=True
    result=probe(f,part='6d')
    assert result['part']=='6d' and result['idle_scene']['toasts'][0]['text']=='resting toast'

def idle_checks_every_sample(f):
    f.modern=True;f.transient_toast=True
    require_rejection(lambda:probe(f,part='6d'),'toast changed')

def part_is_required(f):
    with contextlib.redirect_stderr(io.StringIO()):
        require_rejection(lambda:frame_probe.main(['stream','candidate','missing'],WindowIO(f)))
    require_rejection(lambda:f.runner.prepare(['--then-guard','--smoke','--base','HEAD','--baseline',str(f.baseline),
        '--evidence',str(f.evidence),'--tier-s',str(f.tier),'--go',str(f.go),'--deadline',str(f.args.deadline)],validate=False),'explicit --part')
    assert not f.window_starts

def candidate_part_matches_dump(f):
    require_rejection(lambda:probe(f,label='legacy-wrong',part='6a'),'requires --part 6c')
    f.modern=True
    require_rejection(lambda:probe(f,part='6c'),'requires --part 6d')

def missing_6d_fields_stop_probe(f):
    require_rejection(lambda:probe(f,part='6d'),'6d idle scene unavailable')

def baseline_pin_stops_set(f):
    impostor=f.root/'impostor';shutil.copytree(f.baseline,impostor);f.args.baseline=impostor
    require_rejection(lambda:fg.run_guard(f.args,f.expected),'unapproved slice-5 baseline directory')
    assert not f.probes

def baseline_manifest_is_pinned(f):
    (f.baseline/'identity.txt').write_text('self-consistent impostor manifest')
    require_rejection(lambda:fg.run_guard(f.args,f.expected),'unapproved slice-5 baseline manifest')
    assert not f.probes

def baseline_bytes_are_pinned(f):
    (f.baseline/'debug/veyra').write_text('self-consistent impostor binary')
    require_rejection(lambda:fg.run_guard(f.args,f.expected),'unapproved slice-5 baseline binary')
    assert not f.probes

def baseline_rechecked_between_probes(f):
    def change(index):
        if index==1: (f.baseline/'debug/veyra').write_text('changed between probes')
    f.after_probe=change
    require_rejection(lambda:fg.run_guard(f.args,f.expected),'unapproved slice-5 baseline binary')
    assert len(f.probes)==1, 'baseline re-check must stop before a second child'

def probe_authenticates_baseline(f):
    (f.baseline/'debug/veyra').write_text('impostor')
    require_rejection(lambda:probe(f),'unapproved slice-5 baseline binary')
    assert not f.window_starts

def cover_restarts_only_before_first_pair(f):
    f.cover=lambda args,name:args.label=='set-1-baseline-01' and name=='prepared'
    result=fg.run_guard(f.args,f.expected)
    assert result['passed'] and len(f.probes)==17 and f.clock.now>=60
    assert [s['state'] for s in result['sets']]==['discarded','complete']
    assert all(p['baseline']['label'].startswith('set-2-') for p in result['pairs'])

def cover_after_pair_started_stops_set(f):
    f.cover=lambda args,name:args.side=='candidate' and name=='prepared'
    require_rejection(lambda:fg.run_guard(f.args,f.expected),'cover trip')
    assert len(f.probes)==2 and f.clock.now<60, 'a late cover trip restarted the set'

def second_cover_trip_stops_set(f):
    f.cover=lambda args,name:name=='prepared'
    require_rejection(lambda:fg.run_guard(f.args,f.expected),'cover trip')
    assert len(f.probes)==2 and f.clock.now==60

def guard_go_required(f):
    f.write_go(frame_cost_guard_authorized=False)
    require_rejection(lambda:fg.run_guard(f.args,f.expected),'guard GO/pause required')
    assert not f.probes

def probe_go_required(f):
    f.write_go(launch_authorized=False)
    require_rejection(lambda:probe(f),'fresh GO')
    assert not f.window_starts

def start_tier_required(f):
    receipt=json.loads(f.tier.read_text());receipt['passed']=False;o.write(f.tier,receipt)
    require_rejection(lambda:f.driver.start('invalid-tier'),'incomplete Tier S')
    assert f.app_launches==0

def start_go_required(f):
    f.write_go(launch_authorized=False)
    require_rejection(lambda:f.driver.start('invalid-go'),'fresh GO')
    assert f.app_launches==0

def start_reaches_launch(f):
    assert f.driver.start('valid')==f.pid
    assert f.app_launches==1

def start_requires_unmanaged_owned_window(f):
    f.yabai=True
    for flags in [{'is-floating':False},{'split-type':'vertical'},{'split-child':'second_child'},{'stack-index':True}]:
        f.window_flags=flags;f.running=False
        require_rejection(lambda:f.driver.start('tiled'),'owned window joined the tile tree')
    f.window_flags={};f.running=False
    assert f.driver.start('placed')==f.pid
    assert f.driver.start('placed-again')==f.pid
    labels=(f.proof/'owned-yabai-labels').read_text().splitlines()
    assert len(labels)==len(set(labels))==6
    assert f.frame=={'x':3360,'y':346,'w':960,'h':740}

def launch_hid_gate(f):
    # Execute Launch's real gate/start call sites, once for each denied OS condition.
    body=list(f.runner.HEREDOC.finditer(f.runner.sections()[('SKILL','Launch')]))[-1]['body']
    for idle,locked,busy in [(119,False,False),(9001,False,False),(200,True,False),(200,False,True)]:
        f.idle,f.lock,f.running=idle,locked,busy
        if busy: (f.proof/'owned-app-pids').write_text(str(f.pid))
        with patch.dict(os.environ,VEYRA_VERIFY_DEADLINE=str(f.clock.time()+20)):
            require_rejection(lambda:exec(body,{'__name__':'__fixture_launch__'}),'launch gate')
        assert not f.app_launches
    f.idle,f.lock,f.running=200,False,False
    (f.proof/'owned-app-pids').unlink(missing_ok=True)
    exec(body,{'__name__':'__fixture_launch__'})
    assert f.app_launches==1
    assert json.loads((f.proof/'input-idle-launch.json').read_text())['quiet_seconds']>=10

def safety_stop_is_fatal(f):
    assert f.batch("import orchestration as o; raise o.SafetyStop('fixture safety stop')")==1
    outcome=f.outcome();row=outcome['attempts'][0]
    assert row.get('fatal_stop') and not any(c[-1]=='LATER' for c in f.calls)

def identity_checked_before_every_section(f):
    assert f.batch("from pathlib import Path; Path('crates/veyra/src/ui/theme.rs').write_text('changed identity')")==1
    row=f.outcome()['attempts'][0]
    assert row.get('fatal_stop',{}).get('kind')=='identity' and not any(c[-1]=='LATER' for c in f.calls)

def capture_then_assertion_is_failed(f):
    f.capture_failures=1
    script="from driver import *; capture('recovered'); raise AssertionError('independent failure')"
    assert f.batch(script)==1
    row=f.outcome()['attempts'][0]
    target=next(r for r in row['sections'] if r['section']=='page/Target')
    assert target['kind']=='failed', target
    assert f.outcome()['status'].startswith('FAILED')
    fresh=json.loads((f.evidence/'attempt-1/run/recovered-freshness.json').read_text())
    assert fresh['sha256'] and f.failures_used==1

def exhausted_capture_is_blocked(f):
    f.capture_failures=3
    assert f.batch("from driver import *; capture('exhausted')")==1
    row=f.outcome()['attempts'][0]
    target=next(r for r in row['sections'] if r['section']=='page/Target')
    assert target['kind']=='blocked' and target['observed']['capture_exhausted'] is True, target
    assert f.outcome()['status'].startswith('BLOCKED') and f.failures_used==3

def missing_backdrop_is_fatal(f):
    f.background='translucent'
    assert f.batch("from driver import *; capture('private')")==1
    row=f.outcome()['attempts'][0]
    assert row.get('fatal_stop') and not any(c[-1]=='LATER' for c in f.calls)
    assert 'missing owned backdrop' in (f.evidence/'attempt-1/fresh-shell.txt').read_text()

def evidence_checks_independent_default(f):
    body=f.runner.HEREDOC.search(f.runner.sections()[('SKILL','Evidence')])['body']
    # The requested mode succeeds at the driver, while the SQL OS fixture deliberately remains Full access.
    script="from driver import *\naction('requested-mode','SetDefaultMode',{'mode':'ask_first'})\n"+body
    assert f.batch(script)==1
    transcript=(f.evidence/'attempt-1/fresh-shell.txt').read_text()
    assert "'final', ['4', 'wal'" in transcript and f.outcome()['status'].startswith('FAILED')
    assert (f.evidence/'attempt-1/run/configured-default-mode').read_text().strip()=='ask_first'

def recorded_mutant(f):
    folder=f.evidence/'mutants/1';proof=folder/'native/attempt-1/run';proof.mkdir(parents=True)
    image=proof/'scene.png';image.write_bytes(b'own fresh capture')
    o.write(proof/'scene-freshness.json',{'image':image.name,'sha256':o.sha(image)})
    outcome={'status':'DONE','attempts':[{'run':'deadbeef','cleanup_exit':0,'identity_unchanged':True}]}
    o.write(folder/'native/outcome.json',outcome)
    seal={'run':'deadbeef','capture':str(image.resolve()),'capture_name':'scene','capture_sha256':o.sha(image),
        'freshness_sha256':o.sha(proof/'scene-freshness.json'),'native_outcome_sha256':o.sha(folder/'native/outcome.json')}
    manifest=f.root/'prepared-mutants.json';o.write(manifest,{'parent_identity':f.expected,'cases':[]})
    f.args.native_mutants=manifest;f.write_go(mutant_manifest_sha256=o.sha(manifest))
    o.write(f.evidence/'mutants/receipt.json',{'cases':[{'name':'read scene','folder':str(folder),
        'state':'awaiting lead read','killed':False,'survived':False,'read_seal':seal}],
        'expected_cases':1,'parent_identity':f.expected,'manifest_sha256':o.sha(manifest)})
    return seal

def unread_mutant_stops_batch(f):
    recorded_mutant(f)
    assert f.batch(full=True)==2
    assert f.outcome()['status']=='awaiting lead read' and not f.calls

def decide(f,seal,verdict='killed'):
    with contextlib.redirect_stdout(io.StringIO()):
        return f.runner.prepare(['--evidence',str(f.evidence),'--mutant-read',seal['run'],seal['capture_sha256'],verdict],validate=False)

def mutant_read_bound_to_capture(f):
    seal=recorded_mutant(f)
    wrong=dict(seal,capture_sha256='0'*64)
    require_rejection(lambda:decide(f,wrong),'hash does not match')
    wrong=dict(seal,run='cafebabe')
    require_rejection(lambda:decide(f,wrong),'unknown or ambiguous')
    assert f.batch(full=True)==2 and not f.calls

def decided_mutants_reused(f):
    seal=recorded_mutant(f);assert decide(f,seal)==0
    assert f.batch(full=True)==0 and f.outcome()['status']=='DONE'
    assert all(c[0]=='bash' for c in f.calls), 'decided mutant was launched again'

def slice_end_reads_decided_mutants(f):
    seal=recorded_mutant(f);assert decide(f,seal,'survived')==0
    f.args.native_mutants=None
    assert f.batch(full=True)==1 and f.outcome()['status']=='failed' and not f.calls

def altered_decided_capture_is_refused(f):
    seal=recorded_mutant(f);assert decide(f,seal)==0
    pathlib.Path(seal['capture']).write_bytes(b'other capture')
    require_rejection(lambda:f.batch(full=True),'fresh native image')
    assert not f.calls

def prepare_and_guard_chain(f):
    before=f.expected
    argv=['--smoke','--base','HEAD','--then-guard','--baseline',str(f.baseline),'--part','6c',
        '--evidence',str(f.evidence),'--tier-s',str(f.tier),'--go',str(f.go),'--deadline',str(f.args.deadline),'--scratch',f.args.scratch]
    setup=f.runner.prepare(argv,validate=False)
    assert not isinstance(setup,int)
    found,plan,args,env,expected=setup
    assert expected==before and plan[:3]==[('SKILL','Launch'),('SKILL','Doctor'),f.runner.FIRST]
    assert plan[-2:]==[('SKILL','Doctor'),('SKILL','Evidence')]
    assert ('settings','Native smoke') in plan and not (f.evidence/'status-proposals.json').exists()
    def no_early_batch(index): assert not any(c[0]=='bash' for c in f.calls), 'smoke started before guard completed'
    f.after_probe=no_early_batch
    handlers={n:signal.getsignal(n) for n in o.SIGNALS};mask=signal.pthread_sigmask(signal.SIG_BLOCK,[])
    try: assert f.runner.main(argv,validate=False)==0
    finally:
        for n,h in handlers.items(): signal.signal(n,h)
        signal.pthread_sigmask(signal.SIG_SETMASK,mask)
    assert len(f.probes)==16 and any(c[0]=='bash' for c in f.calls)
    assert f.outcome()['status']=='DONE' and not (f.evidence/'status-proposals.json').exists()

def failed_guard_prevents_smoke(f):
    f.cpu_ratio=1.2
    argv=['--smoke','--base','HEAD','--then-guard','--baseline',str(f.baseline),'--part','6c',
        '--evidence',str(f.evidence),'--tier-s',str(f.tier),'--go',str(f.go),'--deadline',str(f.args.deadline),'--scratch',f.args.scratch]
    handlers={n:signal.getsignal(n) for n in o.SIGNALS}
    try: assert f.runner.main(argv,validate=False)==1
    finally:
        for n,h in handlers.items(): signal.signal(n,h)
    assert len(f.probes)==10 and not any(c[0]=='bash' for c in f.calls)
    assert f.outcome()['status'].startswith('FAILED: frame guard')

def tier_receipts_fail_closed_at_start(f):
    original=f.tier.read_bytes();receipt=json.loads(original)
    bad=[dict(receipt,identity=dict(f.expected,head='2'*40)),dict(receipt,finished_epoch=f.clock.time()-25*3600),
        dict(receipt,commands=receipt['commands'][:-1]),dict(receipt,tools={})]
    for value in bad:
        o.write(f.tier,value);require_rejection(lambda:f.driver.start('invalid-receipt'))
    f.tier.write_bytes(original)
    output=f.tier.parent/receipt['commands'][0]['output'];output.write_text('tampered command')
    require_rejection(lambda:f.driver.start('changed-output'),'Tier S output changed')
    assert not f.app_launches

def identical_pixels_can_be_fresh(f):
    assert f.batch("from driver import *; capture('same-a'); capture('same-b')")==0
    proof=f.evidence/'attempt-1/run'
    a=json.loads((proof/'same-a-freshness.json').read_text());b=json.loads((proof/'same-b-freshness.json').read_text())
    assert a['sha256']==b['sha256'] and b['repaints']>a['repaints'] and b['mtime_ns']>=a['mtime_ns']

def reused_old_image_is_blocked(f):
    f.stale_image=True
    assert f.batch("from driver import *; capture('old-image')")==1
    assert f.outcome()['status'].startswith('BLOCKED')
    assert 'old image file reused' in (f.evidence/'attempt-1/fresh-shell.txt').read_text()
    assert not (f.evidence/'attempt-1/run/old-image.png').exists()

def assertion_parser_and_retention(runner):
    # These are offline data APIs, not native guard substitutes.
    with tempfile.TemporaryDirectory(prefix='veyra-mverify-data-',dir='/tmp') as root:
        root=pathlib.Path(root);transcript=root/'transcript.txt'
        for text,wanted in [('AssertionError: plain message\n',('plain message',)),
                ("AssertionError: ('kind', 'capture', 3)\n",('kind','capture',3)),('AssertionError\n',('',)),('no failure\n',())]:
            transcript.write_text(text);assert o.assertion_from_transcript(transcript)==wanted
        routes=json.loads((o.BIN/'mutant-routes.json').read_text());found=runner.sections()
        for route in routes.values():
            owner=o.REPO/route['file']
            if owner.exists():
                current=owner.read_text()
                assert route['old'] in current or route['new'] in current, 'mutant line disappeared: '+route['file']
            assert all(tuple(section.split('/',1)) in found for section in route['sections'])
        loader=importlib.machinery.SourceFileLoader('prune_data_test',str(o.BIN/'prune'))
        spec=importlib.util.spec_from_loader(loader.name,loader);prune=importlib.util.module_from_spec(spec);loader.exec_module(prune)
        evidence=root/'retention';evidence.mkdir()
        files=['study.txt','receipt.json','failure.png','smoke/capture.png','reviews/aggregate.md',
            'guard/summary.json','red/output.txt','green/output.txt','cited.txt','baseline.txt']
        for name in files:
            path=evidence/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_text('retained')
        cites=root/'citations.md';cites.write_text('cited.txt baseline.txt')
        assert [row['path'] for row in prune.plan(evidence,{name:'study' for name in files},[cites])['delete']]==['study.txt']
        assert all((evidence/name).exists() for name in files)

def cleanup_failure_prevents_done(f):
    f.cleanup_failure=True
    assert f.batch()==1
    assert f.outcome()['status']=='FAILED: Cleanup did not pass'
    assert f.outcome()['attempts'][0]['scratch_left']

def prepare_requires_go(f):
    f.write_go(launch_authorized=False)
    argv=['--smoke','--base','HEAD','--evidence',str(f.evidence),'--tier-s',str(f.tier),
        '--go',str(f.go),'--deadline',str(f.args.deadline),'--scratch',f.args.scratch]
    require_rejection(lambda:f.runner.prepare(argv,validate=False),'fresh GO')
    assert not f.calls

def child_launches_mask_signals(f):
    assert f.batch()==0 and f.outcome()['status']=='DONE'

CASES = [
    ('child-signals',child_launches_mask_signals),('cleanup-stop',cleanup_failure_prevents_done),('prepare-go',prepare_requires_go),    ('guard-tools',guard_tools_and_backdrop),('probe-part',probe_uses_requested_part),('idle-samples',idle_checks_every_sample),
    ('part-required',part_is_required),('part-tree',candidate_part_matches_dump),('scene-6d',missing_6d_fields_stop_probe),
    ('baseline-pin',baseline_pin_stops_set),('baseline-manifest',baseline_manifest_is_pinned),('baseline-bytes',baseline_bytes_are_pinned),('baseline-frozen',baseline_rechecked_between_probes),('baseline-entry',probe_authenticates_baseline),
    ('cover-restart',cover_restarts_only_before_first_pair),('cover-late',cover_after_pair_started_stops_set),('cover-second',second_cover_trip_stops_set),
    ('guard-go',guard_go_required),('probe-go',probe_go_required),('start-tier',start_tier_required),('start-go',start_go_required),
    ('start-entry',start_reaches_launch),('start-window',start_requires_unmanaged_owned_window),('launch-hid',launch_hid_gate),('safety-fatal',safety_stop_is_fatal),
    ('section-identity',identity_checked_before_every_section),('capture-recovered',capture_then_assertion_is_failed),
    ('capture-exhausted',exhausted_capture_is_blocked),('backdrop-missing',missing_backdrop_is_fatal),
    ('evidence-mode',evidence_checks_independent_default),('mutant-awaiting',unread_mutant_stops_batch),
    ('mutant-binding',mutant_read_bound_to_capture),('mutant-reuse',decided_mutants_reused),
    ('mutant-slice-end',slice_end_reads_decided_mutants),('mutant-tamper',altered_decided_capture_is_refused),('prepare-chain',prepare_and_guard_chain),
    ('guard-stops-smoke',failed_guard_prevents_smoke),('tier-complete',tier_receipts_fail_closed_at_start),
    ('capture-fresh-pixels',identical_pixels_can_be_fresh),('capture-old-image',reused_old_image_is_blocked)]

def run(runner):
    for name,case in CASES:
        with tempfile.TemporaryDirectory(prefix='veyra-mverify-entry-',dir='/tmp') as root:
            try:
                with Fixture(pathlib.Path(root),runner) as fixture, contextlib.redirect_stdout(io.StringIO()): case(fixture)
            except BaseException as error:
                raise AssertionError('GUARD['+name+']: '+str(error)) from error
        print('PASS GUARD['+name+']')
    assertion_parser_and_retention(runner)
    pages.self_test()
    print('PASS entry-point checks; OS fixtures only; no native launch')
