"""Restored negative contracts, exercised through the production caller/CLI/scene entries."""
import contextlib, io, json, os, pathlib, shutil, signal, subprocess, sys
from unittest.mock import patch
import frame_guard as fg, frame_probe, mutants, orchestration as o
from self_tests import require_rejection
from raw_test_io import RawFixture

def raw(case): case.fixture_type=RawFixture;return case

def guard_at(f,ratios):
    folder=f.root/('set-'+str(len(list(f.root.glob('set-*')))))
    folder.mkdir();f.args.evidence=folder;f.cpu_ratios=ratios;f.probes=[]
    return fg.run_guard(f.args,f.expected)

def cpu_rules(f):
    for name,ratios,length,fail in [
        ('4 of 5',[1.2]*4+[.8],5,True),('3 of 5 extends',[1.2]*3+[.8]*2+[.8]*5,10,False),
        ('8 of 10',[1.2]*3+[.8]*2+[1.2]*5,10,True),('7 of 10',[1.2]*3+[.8]*2+[1.2]*4+[.8],10,False),
        ('ef4db79 extend',[.9,1.11,1.12,1.13,.9]+[.9]*5,10,False),('ef4db79 pass',[1.01]*5,5,False)]:
        if fail: require_rejection(lambda:guard_at(f,ratios),'paired CPU rule failed')
        else: assert guard_at(f,ratios)['passed'], name
        receipt=json.loads((f.args.evidence/'guard/summary.json').read_text())
        assert len(receipt['pairs'])==length, 'CPU boundary/extend: '+name
        assert receipt['cpu_verdict']==('fail' if fail else 'pass'), name

def cadence(f):
    for rate,gap in [(29,100),(30,101)]:
        f.rate,f.gap=rate,gap
        require_rejection(lambda:guard_at(f,[1.]),'guard rate/gap failure')
        assert len(f.probes)==1, 'cadence failure launched another probe'
    f.rate,f.gap=30,100
    assert guard_at(f,[1.])['passed'], 'cadence boundary rejected'

def interrupted_guard_child(f):
    original=signal.pthread_sigmask(signal.SIG_BLOCK,[])
    f.interrupt_probe=True
    require_rejection(lambda:fg.run_guard(f.args,f.expected),'fixture interrupted probe wait')
    assert f.wait_masks[0]==original, 'spawn mask was not restored inside supervised wait'
    assert f.child_events[-2:]==[('signal',signal.SIGTERM),('wait',120)], 'probe child must receive SIGTERM and be reaped'

def end_identity(f):
    f.end_change=True
    assert f.batch()==1
    assert f.outcome()['status']=='FAILED: the identity changed during the run', 'end-of-run identity change ignored'
    assert not f.outcome()['attempts'][0].get('fatal_stop'), 'test must reach terminal identity check'

def capture_discard(f):
    f.capture_failures=3;unlink=os.unlink
    def failing(path,*args,**kw):
        if pathlib.Path(path).name=='discard.png': raise OSError('fixture capture discard refused')
        return unlink(path,*args,**kw)
    with patch.object(os,'unlink',side_effect=failing):
        assert f.batch("from driver import *; capture('discard')")==1
    row=f.outcome()['attempts'][0]
    assert row.get('fatal_stop') and f.failures_used==1, 'capture discard failure must stop immediately for privacy'
    assert 'capture discard failed; privacy cannot be enforced' in (f.evidence/'attempt-1/fresh-shell.txt').read_text()

def stale_repaint(f):
    assert f.batch("from driver import *; save('capture-last',{'pid':int((PROOF/'pid').read_text()),'repaints':10**9}); capture('stale')")==1
    assert f.outcome()['status'].startswith('BLOCKED'), 'stale repaint was accepted'
    assert 'stale screenshot: window has not repainted' in (f.evidence/'attempt-1/fresh-shell.txt').read_text()
    assert not (f.evidence/'attempt-1/run/stale.png').exists()


def interrupted_verdict(f):
    f.interrupt_target=True
    assert f.batch()==1 and f.outcome()['status'].startswith('FAILED: interrupted by SIGTERM')

def missing_proof(f):
    def fail(*args,**kw): raise OSError('fixture evidence link refused')
    with patch.object(os,'link',side_effect=fail): assert f.batch()==1
    assert f.outcome()['status']=='FAILED: required evidence was not kept'

def collected_failures(f):
    f.capture_failures=3;f.extra_scripts={'LATER':"raise AssertionError('independent failure')"}
    assert f.batch("from driver import *; capture('blocked')")==1
    kinds=[r['kind'] for r in f.outcome()['attempts'][0]['sections']]
    assert 'blocked' in kinds and 'failed' in kinds and f.outcome()['status'].startswith('FAILED: independent')
    row=f.outcome()['attempts'][0]
    assert [r['kind'] for r in row['sections'] if r['exit']]==['blocked','failed']
    assert f.calls[-1][-1]=='CLEANUP' and row['evidence']['run'] and row['cleanup_exit']==0

def yabai_labels(f):
    f.yabai=True
    script="from driver import *; (PROOF/'owned-yabai-labels').write_text('foreign-label\\n')"
    assert f.batch(script)==1 and f.outcome()['status']=='FAILED: Cleanup did not pass'
    assert not any('--remove' in cmd and cmd[-1]=='foreign-label' for cmd in f.calls), 'unowned yabai label was removed'

def yabai_failures(f):
    f.yabai=True;f.rule_add_failure=True
    require_rejection(lambda:f.driver.start('add-failure'),'owned one-shot rule could not be installed')
    assert not f.app_launches
    labels=(f.proof/'owned-yabai-labels').read_text().splitlines();assert len(labels)==1, 'failed rule add lost its recorded ownership'
    before=len([cmd for cmd in f.calls if '--remove' in cmd])
    f.rule_add_failure=False
    f.driver.start('valid-first');f.driver.start('valid-second')
    labels=(f.proof/'owned-yabai-labels').read_text().splitlines();assert len(labels)==3
    index=f.root/'bad-index.jsonl';index.write_text(json.dumps({'proof':str(f.proof),'run':f.run})+'\n{incomplete')
    result=subprocess.run([sys.executable,str(o.BIN/'driver.py'),'--cleanup-yabai-index',str(index)],text=True)
    assert result.returncode!=0, 'incomplete owned rule index accepted'
    removed=[cmd[-1] for cmd in f.calls if '--remove' in cmd][before:]
    assert removed==labels, 'incomplete index did not drain every valid owned rule exactly once'

def yabai_removal(f):
    f.yabai=True
    script="from driver import *; (PROOF/'owned-yabai-labels').write_text('veyra-verify-'+RUN+'-'+'a'*32+'\\n')"
    for mode in ['error','bare','consumed']:
        f.rule_remove=mode;f.evidence=f.root/('removal-'+mode);f.evidence.mkdir();f.args.evidence=f.evidence
        code=f.batch(script)
        assert code==(0 if mode=='consumed' else 1), 'owned removal result: '+mode
        assert f.outcome()['status']==('DONE' if mode=='consumed' else 'FAILED: Cleanup did not pass')

def prepare_case(f,path):
    source=f.repo/'crates/veyra/src/ui/theme.rs';original=source.read_bytes()
    saved={name:(f.repo/'target'/name).read_bytes() for name in o.BINARY_NAMES}
    binaries=f.root/'mutated-binaries';binaries.mkdir(exist_ok=True)
    try:
        source.write_text('mutated source\n')
        for name in o.BINARY_NAMES:
            p=binaries/name;p.parent.mkdir(parents=True,exist_ok=True);p.write_text('mutated '+name)
            (f.repo/'target'/name).write_bytes(p.read_bytes())
        identity=o.code_identity();tier=f.root/'mutated-tier/receipt.json';f.write_tier(tier,identity)
    finally:
        source.write_bytes(original)
        for name,data in saved.items(): (f.repo/'target'/name).write_bytes(data)
    go=f.root/'mutant-go.json';value=json.loads(f.go.read_text());value['identity']=identity;o.write(go,value)
    case={'name':'visual','property':'glass','file':'crates/veyra/src/ui/theme.rs','old':'original source','new':'mutated source',
        'sections':['settings/Glass smoke'],'capture':'scene','scene_runs_mutated_line':True,'judge':'read',
        'binaries':str(binaries),'identity':identity,'tier_s':str(tier),'go':str(go)}
    o.write(path,{'parent_identity':f.expected,'cases':[case]})
    f.args.native_mutants=path;f.write_go(mutant_manifest_sha256=o.sha(path))
    return case

class MutantChild:
    def __init__(self,f,argv): self.f=f;self.argv=argv;self.returncode=None;self.pid=f.pid
    def poll(self): return self.returncode
    def send_signal(self,number): self.f.child_events.append(('signal',number));self.returncode=0
    def wait(self,timeout=None):
        self.f.child_events.append(('wait',timeout))
        if self.returncode is not None:return self.returncode
        f=self.f
        f.wait_masks.append(signal.pthread_sigmask(signal.SIG_BLOCK,[]))
        assert (f.repo/'crates/veyra/src/ui/theme.rs').read_text()=='mutated source\n', 'mutant child did not receive the installed source'
        if getattr(f,'interrupt_mutant',False):
            (f.repo/'target/debug/veyra').write_text('interrupted child bytes')
            raise RuntimeError('fixture interrupted mutant')
        folder=pathlib.Path(self.argv[self.argv.index('--evidence')+1]);proof=folder/'attempt-1/run';proof.mkdir(parents=True)
        image=proof/'scene.png';image.write_bytes(b'fresh mutant OS capture')
        o.write(proof/'scene-freshness.json',{'image':image.name,'sha256':o.sha(image)})
        (folder/'attempt-1/fresh-shell.txt').write_text('OS worker completed\n')
        o.write(folder/'outcome.json',{'status':'DONE','attempts':[{'run':'facebeef','cleanup_exit':0,'identity_unchanged':True,'sections':[{'kind':'passed'}]}]})
        self.returncode=0;return 0

def mutant_restore(f):
    prepare_case(f,f.root/'fresh-mutants.json');f.interrupt_mutant=True
    original_mask=signal.pthread_sigmask(signal.SIG_BLOCK,[])
    original={name:(f.repo/'target'/name).read_bytes() for name in o.BINARY_NAMES}
    try: mutants.run_mutants(f.args,f.expected)
    except RuntimeError as error: assert 'fixture interrupted mutant' in str(error)
    else: raise AssertionError('mutant interruption did not escape')
    assert (f.repo/'crates/veyra/src/ui/theme.rs').read_text()=='original source\n', 'mutant source not restored'
    assert all((f.repo/'target'/name).read_bytes()==data for name,data in original.items()), 'mutant binaries not restored'
    assert not (f.evidence/'mutants/1/mutation-in-flight.json').exists(), 'restore marker was not cleared'
    assert f.child_events[-2:]==[('signal',signal.SIGTERM),('wait',120)], 'interrupted mutant child must receive SIGTERM and be reaped'
    assert f.wait_masks[0]==original_mask, 'spawn mask was not restored inside mutant wait'

def fresh_read_mutant(f):
    prepare_case(f,f.root/'fresh-mutants.json')
    result=mutants.run_mutants(f.args,f.expected)
    assert result['state']=='awaiting lead read' and not result['passed'], 'read-judged mutant must await its own lead read'
    row=result['cases'][0];assert row['read_seal']['run']=='facebeef' and not row['killed'] and not row['survived']

def read_decision_guards(f):
    from self_tests import recorded_mutant, decide
    seal=recorded_mutant(f);image=pathlib.Path(seal['capture']);original=image.read_bytes()
    receipt_path=f.evidence/'mutants/receipt.json';receipt=json.loads(receipt_path.read_text())
    other=f.evidence/'mutants/archive-2';shutil.copytree(f.evidence/'mutants/archive-1',other)
    proof=other/'native/attempt-1/run';other_image=proof/'scene.png';other_image.write_bytes(b'another run capture')
    o.write(proof/'scene-freshness.json',{'image':'scene.png','sha256':o.sha(other_image)})
    outcome=json.loads((other/'native/outcome.json').read_text());outcome['attempts'][0]['run']='cafebabe';o.write(other/'native/outcome.json',outcome)
    second=dict(seal,run='cafebabe',capture=str(other_image.resolve()),capture_sha256=o.sha(other_image),
        freshness_sha256=o.sha(proof/'scene-freshness.json'),native_outcome_sha256=o.sha(other/'native/outcome.json'))
    receipt['cases'].append({'name':'another read','folder':str(other),'state':'awaiting lead read','killed':False,'survived':False,'read_seal':second})
    receipt['expected_cases']=2;o.write(receipt_path,receipt);before=receipt_path.read_bytes()
    require_rejection(lambda:decide(f,dict(seal,capture_sha256=second['capture_sha256'])),'hash does not match')
    assert receipt_path.read_bytes()==before, 'rejected hash changed receipt'
    image.write_bytes(b'changed native capture')
    require_rejection(lambda:decide(f,seal),'fresh native image');image.write_bytes(original)
    assert decide(f,seal)==0 and json.loads(receipt_path.read_text())['state']=='awaiting lead read'
    require_rejection(lambda:decide(f,seal,'survived'),'already decided')
    assert decide(f,second,'survived')==0 and json.loads(receipt_path.read_text())['state']=='failed'

def go_variants(f):
    changes=[{'identity':{}},{'issued_epoch':f.clock.time()+10},{'cutoff_epoch':f.clock.time()-1},
        {'arch_plan_paused':False}]
    for index,change in enumerate(changes):
        f.write_go(**change)
        f.args.deadline=(change['cutoff_epoch']-1) if 'cutoff_epoch' in change else f.clock.time()+3600
        f.args.evidence=f.root/('go-negative-'+str(index));f.args.evidence.mkdir()
        require_rejection(lambda:fg.run_guard(f.args,f.expected))
        assert not f.probes, 'invalid GO launched probe'
    f.write_go();f.args.deadline=json.loads(f.go.read_text())['cutoff_epoch']+1
    f.args.evidence=f.root/'go-deadline';f.args.evidence.mkdir()
    require_rejection(lambda:fg.run_guard(f.args,f.expected),'deadline exceeds GO cutoff')


def cover_remaining(f):
    for name,condition,count in [('mid-baseline',lambda a,n:a.label.endswith('baseline-01') and n=='stream',1),
            ('later-pair',lambda a,n:a.label.endswith('baseline-02') and n=='prepared',3)]:
        f.args.evidence=f.root/name;f.args.evidence.mkdir();f.probes=[];f.cover=condition
        began=f.clock.now
        require_rejection(lambda:fg.run_guard(f.args,f.expected),'cover trip')
        assert len(f.probes)==count and f.clock.now==began, 'late cover trip waited or restarted: '+name

def idle_variants(f):
    from self_tests import WindowIO
    f.modern=True
    original=f.idle_state  # raw dump I/O fixture, not a guard function
    for focus in ['composer','drawer']:
        for label,change,message in [('palette',lambda s:s['palette'].update(open=True),'palette closed'),
                ('toast',lambda s:s.update(toasts=[]),'toast at rest')]:
            def dump(focus='composer'):
                value=original(focus);change(value);return value
            with patch.object(f,'idle_state',side_effect=dump):
                require_rejection(lambda:frame_probe.main(['scenes','candidate',label+'-'+focus,'--scene','idle','--part','6d','--idle-focus',focus],WindowIO(f)),message)
        f.transient_toast=True
        try: require_rejection(lambda:frame_probe.main(['scenes','candidate','changed-'+focus,'--scene','idle','--part','6d','--idle-focus',focus],WindowIO(f)),'toast changed')
        finally: f.transient_toast=False

@raw
def native_motion(f):
    sys.modules.pop('driver',None)
    result=frame_probe.main(['scenes','candidate','raw-normal','--part','6c','--scene','working','--motion','normal'])
    assert result['passed'] and result['display_options_override']['reduce_motion'] is False, 'normal motion was not forced'
    f.running=False;f.threads=[];f.look['reduce_motion']=False
    sys.modules.pop('driver',None)
    reduced=frame_probe.main(['scenes','candidate','raw-reduced','--part','6c','--scene','working','--motion','reduced'])
    assert reduced['passed'] and reduced['display_options_override']['reduce_motion'] is True, 'reduced motion was not forced'

@raw
def native_recording(f):
    sys.modules.pop('driver',None)
    result=frame_probe.main(['scenes','candidate','raw-recording','--part','6c','--scene','working','--motion','normal'])
    assert result['passed'] and result['recording_off'], 'frame launch recording-off proof failed'
    assert f.launch_env and all('VEYRA_RECORD' not in env for env in f.launch_env), 'frame launch recording was enabled'
    output=pathlib.Path(os.environ['VEYRA_FRAME_GUARD_PROOF'])/'scenes/raw-recording/run'
    assert all((output/name).read_bytes()==(f.tier.parent/'tools'/name).read_bytes() for name in o.TOOLS)
    assert f.signals==[(f.backdrop_pid,signal.SIGTERM)], 'exited owned backdrop received another signal'

@raw
def native_reads(f):
    for value,message in [('pid','pid'),('group','group'),('lock','lock'),('build_rev','build_rev'),('data_dir','data'),('screen','screen is locked')]:
        f.corruption=value;f.running=False;f.motion_payloads=[];f.threads=[];f.selected=None
        sys.modules.pop('driver',None)
        require_rejection(lambda:frame_probe.main(['scenes','candidate','raw-'+value,'--part','6c','--scene','working']),message)

CASES=[('cpu-rules',cpu_rules),('cadence',cadence),('probe-term',interrupted_guard_child),('end-identity',end_identity),
    ('discard-stop',capture_discard),('stale-repaint',stale_repaint),
    ('interrupt-verdict',interrupted_verdict),('proof-verdict',missing_proof),('collect-all',collected_failures),
    ('yabai-labels',yabai_labels),('yabai-errors',yabai_failures),('yabai-removal',yabai_removal),
    ('mutant-restore',mutant_restore),('mutant-fresh-read',fresh_read_mutant),('mutant-read-guards',read_decision_guards),
    ('go-variants',go_variants),('cover-extra',cover_remaining),('idle-variants',idle_variants),
    ('native-recording',native_recording),('native-motion',native_motion),('native-reads',native_reads)]

def owned_glass(f):
    f.running=True
    for name in ['pid','pgid','owned-app-pids']: (f.proof/name).write_text(str(f.pid))
    (f.proof/'window-id').write_text('23');(f.data/'veyra.lock').write_text(str(f.pid))

@raw
def backdrop_kill(f):
    import glass_smoke
    owned_glass(f)
    f.capture_hook=lambda:setattr(f,'backdrop_path',(f.data/'other-executable').resolve())
    require_rejection(glass_smoke.drive,'backdrop pid does not name the scratch bundle executable')
    assert not f.signals, 'backdrop was signalled without fresh executable ownership'

CASES += [('backdrop-kill',backdrop_kill)]

@raw
def measured_pair_entry(f):
    import importlib,glass_measure
    for half in ['coarse','inverse']:
        with RawFixture(f.root/half,f.runner) as io:
            owned_glass(io);io.look['background']='translucent';io.drawer_for_measure=True;io.occlude_half=half
            measure=importlib.reload(glass_measure)
            initial,_=measure.start_backdrop('initial')
            labels=['settings-measure-test-coarse','settings-measure-test-inverse']
            try:
                backdrop=measure.drive_pair(labels,initial)
                for label in labels:
                    guards=json.loads((io.proof/(label+'-region-guards.json')).read_text())
                    assert guards['before']['rectangle']==[24,421,960,740], 'measured_pair lost settled rectangle'
                    assert (io.proof/(label+'.png')).exists()
                events=[name for name,index in io.capture_events if name in [label+'.png' for label in labels]]
                wanted=[labels[0]+'.png']*2+[labels[1]+'.png'] if half=='coarse' else [label+'.png' for label in labels]*2
                assert events==wanted, 'measured_pair did not retake both halves after '+half+' placement'
                last=dict(io.capture_events)
                assert all((io.proof/(label+'.png')).read_bytes()==('raw capture '+str(last[label+'.png'])).encode() for label in labels), 'retained image is not the last capture'
            finally:
                for process in reversed(measure.processes):measure.stop_backdrop(process)

CASES += [('measured-pair',measured_pair_entry)]

@raw
def backdrop_activation(f):
    from backdrop_actions import activate
    owned_glass(f)
    expected=f.data/'notification-backdrop';foreign=f.data/'other-executable'
    for variant in ['outside','dead','foreign','before-reopen','after-reopen']:
        f.opens=[];f.signals=[];f.path_reads=0;f.path_hook=None;f.open_hook=None
        f.backdrop_alive=variant!='dead';f.backdrop_path=expected.resolve()
        if variant=='foreign': f.backdrop_path=foreign.resolve()
        if variant=='before-reopen':
            def change(index):
                return expected.resolve() if index==1 else foreign.resolve()
            f.path_hook=change
        if variant=='after-reopen': f.open_hook=lambda:setattr(f,'backdrop_path',foreign.resolve())
        argument=f.data.parent/'outside' if variant=='outside' else expected
        if variant=='outside': f.backdrop_path=argument.resolve()
        require_rejection(lambda:activate('activation-'+variant,f.backdrop_pid,argument,f.data/'bundle'))
        assert len(f.opens)==(1 if variant=='after-reopen' else 0), 'backdrop was reopened without ownership: '+variant
        assert not f.signals
    f.path_hook=None;f.open_hook=None;f.backdrop_alive=True;f.backdrop_path=expected.resolve();f.opens=[];f.path_reads=0
    reply=activate('activation-positive',f.backdrop_pid,expected,f.data/'bundle')
    assert len(f.opens)==1 and f.path_reads==3
    assert not f.signals, 'activation signalled the owned backdrop'
    assert getattr(reply,'returncode',None)==0, 'backdrop reopen command result was not preserved'

@raw
def backdrop_escalation(f):
    import glass_smoke
    owned_glass(f);f.keep_backdrop=True;swap_at=[None]
    def path(index):
        if not f.backdrop_alive:return None
        return (f.data/'foreign').resolve() if swap_at[0] is not None and f.clock.now>=swap_at[0] else f.backdrop_path
    def term(pid,number):
        if number==signal.SIGTERM: swap_at[0]=f.clock.now+5
    f.path_hook=path;f.on_signal=term
    require_rejection(glass_smoke.drive,'backdrop pid does not name')
    assert f.signals==[(f.backdrop_pid,signal.SIGTERM)], 'SIGKILL was sent after backdrop ownership changed'

@raw
def backdrop_dead_cleanup(f):
    owned_glass(f);f.backdrop_alive=True;f.backdrop_path=(f.data.parent/'outside').resolve()
    (f.proof/'notifications-backdrop-pid').write_text(str(f.backdrop_pid))
    o.write(f.proof/'notifications-backdrop-ownership.json',{'pid':f.backdrop_pid,'executable':str(f.backdrop_path)})
    outside=subprocess.run(['bash','-c',f.runner.sections()[('SKILL','Cleanup')]],text=True)
    assert outside.returncode==1 and not f.signals and 'outside scratch' in outside.stderr, 'outside backdrop was signalled'
    f.backdrop_alive=False
    o.write(f.proof/'notifications-backdrop-ownership.json',{'pid':f.backdrop_pid,'executable':str(f.data/'backdrop')})
    result=subprocess.run(['bash','-c',f.runner.sections()[('SKILL','Cleanup')]],text=True)
    assert result.returncode==0 and not f.signals, 'dead backdrop was signalled'

@raw
def backdrop_signal_success(f):
    import glass_smoke
    owned_glass(f);f.keep_backdrop=True
    def kill(pid,number):
        if number==signal.SIGKILL:f.backdrop_alive=False
    f.on_signal=kill
    glass_smoke.drive()
    assert f.signals==[(f.backdrop_pid,sig) for _ in range(2) for sig in [signal.SIGTERM,signal.SIGKILL]], 'owned backdrop signal sequence changed'

CASES += [('backdrop-activation',backdrop_activation),('backdrop-signals',backdrop_signal_success),
    ('backdrop-escalation',backdrop_escalation),('backdrop-dead',backdrop_dead_cleanup)]

def admission_cause(f):
    f.before_probe=lambda index:f.write_go(launch_authorized=False)
    try: fg.run_guard(f.args,f.expected)
    except Exception as error:
        assert isinstance(error,o.SafetyStop) and 'fresh GO for this identity required' in str(error), 'probe admission cause was hidden'
        assert 'set-1-baseline-01' in str(error) and 'exit 1' in str(error)
        assert 'result.json' not in str(error)
    else: raise AssertionError('probe admission failure was ignored')

def fixture_skip(f):
    import pages
    output=io.StringIO()
    with contextlib.redirect_stdout(output): assert pages.main(['self-test']) is None
    assert 'SKIPPED select-pages real commit fixture' in output.getvalue()
    assert 'PASS select-pages real commit fixture' not in output.getvalue(), 'missing fixture reported PASS'

def prepare_quiet(f):
    require_rejection(lambda:f.runner.prepare(['--real-quiet'],validate=False),'--real-quiet requires --gate')

def manifest_is_pinned(f): pass  # --check-only checks it before entering this dispatch.

CASES += [('probe-admission',admission_cause),('fixture-skipped',fixture_skip),('prepare-quiet',prepare_quiet),
    ('plant-registry',manifest_is_pinned)]

def prepare_selection(f):
    source=f.repo/'crates/veyra/src/ui/theme.rs'
    f.changed_paths=['crates/veyra/src/ui/theme.rs','.agents/skills/verify-veyra/bin/run']
    argv=['--smoke','--base','HEAD','--evidence',str(f.evidence),'--tier-s',str(f.tier),'--go',str(f.go),
        '--deadline',str(f.args.deadline),'--scratch',f.args.scratch]
    for text,hunk,wanted in [('fn paint() {}\n','@@ -0,0 +1 @@\n+fn paint() {}\n',True),
            ('#[cfg(test)]\nfn testing() {}\n','@@ -0,0 +1,2 @@\n+#[cfg(test)]\n+fn testing() {}\n',False)]:
        source.write_text(text);f.git_hunk=hunk;f.expected=o.code_identity();f.write_tier(f.tier,f.expected);f.write_go()
        found,plan,args,env,expected=f.runner.prepare(argv,validate=False)
        assert (('settings','Glass smoke') in plan)==wanted, 'prepare selected wrong glass scene for production/test hunk'
    _,plan,_,_,_=f.runner.prepare([*argv,'--sections','settings/Native smoke'],validate=False)
    assert [step for step in plan if step[0]=='settings']==[('settings','Native smoke')]
    assert not (f.evidence/'status-proposals.json').exists()

CASES += [('prepare-selection',prepare_selection)]


def incomplete_runner_verdict(f):
    f.crash_target_spawn=True
    assert f.batch()==1, 'runner exception was accepted as DONE'
    outcome=f.outcome();row=outcome['attempts'][0]
    assert outcome['status']=='FAILED: not every planned section passed', 'incomplete runner did not reach completion verdict'
    assert row['all_passed'] is False and row['failed']['section']=='runner'
    assert row['cleanup_exit']==0 and row['evidence']['run'] and row['identity_unchanged']
    assert all(section['kind']=='passed' for section in row['sections']), 'test must bypass section failure verdicts'

CASES += [('verdict-complete',incomplete_runner_verdict)]
