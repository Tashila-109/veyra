"""M9 maintenance repair regressions; every native boundary is forbidden or stubbed.

Run one case with `python3 bin/repair_tests.py CASE`; check-only runs them all.
The mutation proof runner executes these same cases against defective scratch copies.
"""
import ast, contextlib, copy, importlib.machinery, importlib.util, io, json, os, pathlib
import shutil, signal, subprocess, sys, tempfile, time
from types import SimpleNamespace
from unittest.mock import patch
import frame_guard as fg, frame_scene, mutants, orchestration as o, pages

class BoundaryReached(BaseException):
    """A native operation would have happened; tests never cross this boundary."""

def boundary(*args,**kwargs): raise BoundaryReached('native boundary reached')

def stop(call, text='', kind=(AssertionError,o.SafetyStop)):
    try: call()
    except kind as error:
        assert text in str(error),(text,str(error))
        return
    raise AssertionError('planted fault was accepted: '+text)

def function(path, name, namespace):
    node = next(node for node in ast.parse(path.read_text()).body if isinstance(node,ast.FunctionDef) and node.name==name)
    node = copy.deepcopy(node)
    node.decorator_list = []
    exec(compile(ast.Module(body=[node],type_ignores=[]),str(path),'exec'),namespace)
    return namespace[name]

def runner_module():
    loader=importlib.machinery.SourceFileLoader('repair_runner',str(o.BIN/'run'))
    spec=importlib.util.spec_from_loader(loader.name,loader)
    runner=importlib.util.module_from_spec(spec)
    sys.modules[loader.name]=runner
    loader.exec_module(runner)
    return runner

def backdrop_paths(runner,root):
    tools,proof,data=root/'tools',root/'proof',root/'data'
    for directory in [tools,proof,data]: directory.mkdir()
    for name in o.TOOLS:
        (tools/name).write_bytes(('admitted '+name).encode())
        assert (o.BIN/(name+'.swift')).is_file(), 'missing helper source '+name
    resolved=o.helper_paths(tools)
    assert set(resolved)==set(o.TOOLS)
    # Execute the actual probe helper-link loop and backdrop copy, stopping at Popen.
    tree=ast.parse((o.BIN/'frame_probe.py').read_text())
    links=next(node for node in tree.body if isinstance(node,ast.For)
        and any(isinstance(item,ast.Constant) and item.value=='capture-window' for item in ast.walk(node.iter))
        and any(isinstance(item,ast.Call) and isinstance(item.func,ast.Attribute) and item.func.attr=='symlink_to'
            for item in ast.walk(node)))
    namespace={'proof':proof,'TOOLS':tools,'resolved_helpers':resolved}
    exec(compile(ast.Module(body=[links],type_ignores=[]),'frame helper links','exec'),namespace)
    for path in proof.iterdir(): assert path.resolve(strict=True)==resolved[path.name].resolve()
    def popen(argv,**unused):
        assert pathlib.Path(argv[0]).read_bytes()==(tools/'backdrop').read_bytes()
        boundary()
    namespace.update(data=data,HELPERS=o.BIN,shutil=shutil,subprocess=SimpleNamespace(Popen=popen))
    take=function(o.BIN/'frame_probe.py','start_backdrop',namespace)
    try: take()
    except BoundaryReached: pass
    else: raise AssertionError('backdrop test crossed its launch boundary')
    assert (data/'perf-backdrop').read_bytes()==(tools/'backdrop').read_bytes()
    # Smoke's copy prefix must resolve the same admitted helper without opening a window.
    (proof/'window-id').write_text('23')
    drive=next(node for node in ast.parse((o.BIN/'glass_smoke.py').read_text()).body if isinstance(node,ast.FunctionDef))
    prefix=drive.body[:next(i for i,node in enumerate(drive.body) if isinstance(node,ast.Try))]
    driver=SimpleNamespace(prepare=lambda:None,ctl=lambda *unused:{'look':{}},owned_instance_pid=lambda:11,
        PROOF=proof,DATA=data,owned_window_record=lambda *unused:{'frame':{}},
        shutil=SimpleNamespace(which=lambda _:None,copy2=shutil.copy2))
    with patch.dict(sys.modules,driver=driver):
        exec(compile(ast.Module(body=prefix,type_ignores=[]),'glass helper copy','exec'),{})
    assert (data/'glass-smoke-backdrop').read_bytes()==(tools/'backdrop').read_bytes()
    (tools/'backdrop').unlink()
    stop(lambda:o.helper_paths(tools),'admitted helper missing: backdrop')

def cover_set(runner,root):
    def row(label): return {'label':label,'core_fraction':1.0,'repaints':0,'repaints_per_second':0}
    for trip in ['candidate-first','later','second-prepair','mid-baseline','first-prepair']:
        calls,waits=[],[]
        receipt={}
        def one(side,label,scene='stream',**unused):
            calls.append(label)
            fault = (trip=='candidate-first' and label.endswith('candidate-01')) \
                or (trip=='later' and label.endswith('baseline-02')) \
                or (trip=='second-prepair' and label.endswith('baseline-01')) \
                or (trip in ['first-prepair','mid-baseline'] and len(calls)==1)
            if fault: raise fg.CoverTrip({'measurement_started':trip=='mid-baseline'})
            return row(label)
        call=lambda:fg.measure_set(one,receipt,lambda:None,time.time()+300,wait=waits.append)
        if trip=='first-prepair':
            call()
            assert waits==[60] and calls[:2]==['set-1-baseline-01','set-2-baseline-01']
            assert receipt['sets'][0]['state']=='discarded' and len(receipt['pairs'])==5
            assert all(pair[side]['label'].startswith('set-2-') for pair in receipt['pairs'] for side in ['baseline','candidate'])
        else:
            stop(call,'cover trip')
            if trip=='candidate-first': assert calls==['set-1-baseline-01','set-1-candidate-01'] and not waits
            elif trip=='mid-baseline': assert len(calls)==1 and not waits
            elif trip=='second-prepair': assert calls==['set-1-baseline-01','set-2-baseline-01'] and waits==[60]
            else: assert calls[-1]=='set-1-baseline-02' and not waits

def baseline_pin(runner,root):
    root=root.resolve()
    approved=root/'approved'/fg.BASELINE_REVISION
    impostor=root/('a'*40)
    for base in [approved,impostor]:
        (base/'debug').mkdir(parents=True)
        for name in fg.BASELINE_BINARIES: (base/name).write_bytes(b'impostor '+name.encode())
        lines=['identity '+fg.BASELINE_IDENTITY]+[o.sha(base/name)+'  '+name for name in fg.BASELINE_BINARIES]
        (base/'identity.txt').write_text('\n'.join(lines)+'\n')
    args=SimpleNamespace(evidence=root/'impostor-run',baseline=impostor,tier_s=root/'unused-tier',go=root/'unused-go',
        deadline=time.time()+300,part='6c')
    args.evidence.mkdir()
    # Use the real run_guard entry, so deleting its pin check also reaches a forbidden boundary.
    with patch.object(fg,'approved_baseline',return_value=approved),patch.object(o,'code_identity',return_value={}), \
            patch.object(o,'require_tier'),patch.object(o,'require_go'),patch.object(fg,'measure_set',side_effect=boundary):
        stop(lambda:fg.run_guard(args,{}),'unapproved slice-5 baseline')
    same_name=root/'elsewhere'/fg.BASELINE_REVISION
    shutil.copytree(impostor,same_name)
    with patch.object(fg,'approved_baseline',return_value=approved):
        stop(lambda:fg.authenticate_baseline(same_name),'unapproved slice-5 baseline directory')
        real_sha=o.sha
        def approved_hash(path):
            path=pathlib.Path(path)
            if path==approved/'identity.txt': return fg.BASELINE_MANIFEST_SHA256
            if path.is_relative_to(approved): return fg.BASELINE_BINARIES[path.relative_to(approved).as_posix()]
            return real_sha(path)
        with patch.object(o,'sha',side_effect=approved_hash):
            assert fg.authenticate_baseline(approved)==fg.BASELINE_BINARIES
        with patch.object(o,'sha',return_value='0'*64):
            stop(lambda:fg.authenticate_baseline(approved),'unapproved slice-5 baseline manifest')
        def bad_binary(path): return '0'*64 if pathlib.Path(path)==approved/'debug/veyra' else approved_hash(path)
        with patch.object(o,'sha',side_effect=bad_binary):
            stop(lambda:fg.authenticate_baseline(approved),'unapproved slice-5 baseline binary')

def idle_dump(focus):
    return {'focus':focus,'drawer':{'open':True,'tabs':[{'running':False}], 'terminal':{'focused':focus=='drawer'}},
        'sidebar':{'rows':[]},'selected_thread':{'status':'Done'},'look':{'background':'translucent'},
        'palette':{'open':False},'toasts':[{'kind':'notice','family':None,'text':'Write a message'}]}

def part_6d(runner,root):
    for focus in ['drawer','composer']:
        state=idle_dump(focus)
        old={key:value for key,value in state.items() if key not in ['palette','toasts']}
        stop(lambda:frame_scene.assert_idle_state(old,focus,'6d'),'6d idle scene unavailable',o.SafetyStop)
        assert frame_scene.assert_idle_state(old,focus,'6c') is None
        expected=frame_scene.assert_idle_state(state,focus,'6d')
        for _ in range(10): assert frame_scene.assert_idle_state(copy.deepcopy(state),focus,'6d',expected)==expected
        opened=copy.deepcopy(state);opened['palette']['open']=True
        stop(lambda:frame_scene.assert_idle_state(opened,focus,'6d',expected),'palette closed')
        empty=copy.deepcopy(state);empty['toasts']=[]
        stop(lambda:frame_scene.assert_idle_state(empty,focus,'6d',expected),'toast at rest')
        changed=copy.deepcopy(state);changed['toasts'][0]['text']='Changed'
        stop(lambda:frame_scene.assert_idle_state(changed,focus,'6d',expected),'toast changed')
    code=ast.parse((o.BIN/'frame_probe.py').read_text())
    assert sum(isinstance(node,ast.Call) and isinstance(node.func,ast.Name) and node.func.id=='assert_idle_state'
        for node in ast.walk(code))==3, 'idle variant must guard before, every sample and after'
    assert "'--part',args.part if side=='candidate' else '6c'" in (o.BIN/'frame_guard.py').read_text()


def go_fixture(root,expected):
    path=root/'go.json'
    value={'launch_authorized':True,'identity':expected,'issued_epoch':time.time()-1,'cutoff_epoch':time.time()+600,
        'frame_cost_guard_authorized':True,'arch_plan_paused':True}
    o.write(path,value)
    return path,value

def go_stop(runner,root):
    expected={'head':'approved'}
    path,value=go_fixture(root,expected)
    assert o.require_go(path,expected,time.time()+300,guard=True)==value
    for change in [{'launch_authorized':False},{'identity':{}},{'issued_epoch':time.time()+10},
            {'cutoff_epoch':time.time()-1},{'frame_cost_guard_authorized':False},{'arch_plan_paused':False}]:
        o.write(path,dict(value,**change))
        stop(lambda:o.require_go(path,expected,time.time()+300,guard=True))
    o.write(path,value)
    stop(lambda:o.require_go(path,expected,value['cutoff_epoch']+1),'deadline exceeds GO cutoff')

def start_stop(runner,root,which):
    from self_tests import fake_tier
    expected={'head':'approved','diff_sha256':'x','untracked_sha256':{},'binaries':{}}
    tier,value=fake_tier(root/'tier',expected)
    go,approval=go_fixture(root,expected)
    start=function(o.BIN/'driver.py','start',{'os':os,'locked':boundary})
    env={'VEYRA_VERIFY_TIER_S':str(tier),'VEYRA_VERIFY_GO':str(go),'VEYRA_VERIFY_DEADLINE':str(time.time()+300)}
    with patch.dict(os.environ,env),patch.object(o,'code_identity',return_value=expected):
        try: start('positive')
        except BoundaryReached: pass
        else: raise AssertionError('start did not reach its pre-native boundary')
        if which=='tier':
            o.write(tier,dict(value,identity={'head':'other'}))
            stop(lambda:start('negative'),'Tier S identity differs')
        else:
            o.write(go,dict(approval,launch_authorized=False))
            stop(lambda:start('negative'),'fresh GO for this identity required')

def start_tier_stop(runner,root): start_stop(runner,root,'tier')
def start_go_stop(runner,root): start_stop(runner,root,'go')

def scheduler(runner,root,scenario):
    repo=root/'repo';proof=repo/'target/verify-veyra/0123abcd';proof.mkdir(parents=True)
    (proof/'pid').write_text('99999999')
    found={('SKILL',name):name for name in ['Launch','Doctor','Evidence','Cleanup','Recover section']}
    found.update({('page',name):name for name in ['target','independent']})
    plan=[('SKILL','Launch'),('SKILL','Doctor'),('page','target'),('page','independent'),('SKILL','Evidence')]
    calls=[];changed=False
    mask=signal.pthread_sigmask(signal.SIG_BLOCK,[])
    class Child:
        pid=99999999
        returncode=0
        def __init__(self,name): self.name=name
        def communicate(self,timeout):
            nonlocal changed
            calls.append(self.name)
            if self.name=='target':
                if scenario=='identity': changed=True
                else:
                    self.returncode=1
                    o.write(proof/'focus-failure.json',{'kind':'capture','capture_exhausted':True,'failure_epoch':time.time()})
                    return '', 'orchestration.SafetyStop: capture discard failed; privacy cannot be enforced\n'
            return '',''
        def poll(self): return self.returncode
    def start(argv,**unused): return Child(argv[-1]),mask
    def current(): return {'expected':not changed}
    record={'run':'0123abcd','failed':None,'problems':[],'interrupted':None}
    try:
        with patch.object(runner,'REPO',repo),patch.object(runner,'start_owned',side_effect=start), \
                patch.object(runner,'checkout_identity',side_effect=current), \
                patch.object(runner,'failure_capture',side_effect=boundary),patch.object(runner,'INTERRUPT',{'signal':None}):
            runner.attempt(found,plan,root/'output',{'RUN':'0123abcd','VEYRA_VERIFY_TMP':str(root)+'/data-'},
                time.time()+60,{'expected':True},record)
    finally: signal.pthread_sigmask(signal.SIG_SETMASK,mask)
    assert 'target' in calls and 'independent' not in calls and calls[-1]=='Cleanup',calls
    assert record.get('fatal_stop'),record
    if scenario=='identity': assert record['fatal_stop']['kind']=='identity'
    else: assert record['fatal_stop']['kind']=='fatal'


def fatal_stop(runner,root): scheduler(runner,root,'fatal')
def section_identity(runner,root): scheduler(runner,root,'identity')

def missing_backdrop(runner,root):
    proof=root/'proof';proof.mkdir()
    records=[]
    namespace={'PROOF':proof,'save':lambda name,value:records.append((name,value)),
        'json':json,'region_snapshot':boundary,'subprocess':SimpleNamespace(run=boundary)}
    take=function(o.BIN/'driver.py','region_capture',namespace)
    stop(lambda:take(proof/'image.png',1,2),'missing owned backdrop',o.SafetyStop)
    assert records==[('safety-stop',{'kind':'privacy','reason':'missing owned backdrop'})]
    assert not (proof/'image.png').exists()

def capture_recovery(runner,root):
    proof=root/'proof';proof.mkdir()
    calls=[]
    def guarded(name):
        calls.append(name)
        if len(calls)==1: raise AssertionError('covered')
    def observe(name,kind): o.write(proof/'focus-failure.json',{'kind':kind,'failure_epoch':time.time()})
    namespace={'PROOF':proof,'json':json,'time':time,'guarded_capture':guarded,'observe_failure':observe,
        'save':lambda name,value:o.write(proof/(name+'.json'),value)}
    function(o.BIN/'driver.py','capture_observation',namespace)
    capture=function(o.BIN/'driver.py','capture',namespace)
    capture('retry')
    seen=json.loads((proof/'focus-failure.json').read_text())
    assert len(calls)==2 and seen['capture_exhausted'] is False
    assert runner.classify(1,seen)=='failed', 'an assertion after a recovered capture must be FAILED'
    namespace['guarded_capture']=lambda name:(_ for _ in ()).throw(AssertionError('covered'))
    stop(lambda:capture('exhausted'),'covered')
    seen=json.loads((proof/'focus-failure.json').read_text())
    assert seen['capture_exhausted'] is True and runner.classify(1,seen)=='blocked'

def evidence_mode(runner,root):
    proof=root/'proof';proof.mkdir()
    (proof/'pages-run').write_text('settings\n')
    (proof/'configured-default-mode').write_text('full_access\n')
    db=root/'veyra.db';db.write_text('synthetic')
    actual={'mode':'full_access'}
    tables='grants items projects requests settings tasks threads turns'.split()
    def sql(query):
        return {'pragma user_version':'4','pragma journal_mode':'wal','.tables':' '.join(tables),
            'select default_mode from settings':actual['mode']}[query]
    namespace={'PROOF':proof,'DB':db,'TABLES':tables,'sql':sql,'json':json,'ctl':lambda *args:{'ok':True},
        'save':lambda *args:None}
    for name in ['configured_default_mode','action','database']: function(o.BIN/'driver.py',name,namespace)
    namespace['action']('configured','SetDefaultMode',{'mode':'ask_first'})
    assert namespace['configured_default_mode']()=='ask_first' and actual['mode']=='full_access'
    namespace.update(owned_instance_pid=lambda:1,capture=boundary)
    body=runner.HEREDOC.search(runner.sections()[('SKILL','Evidence')])['body']
    with patch.dict(sys.modules,driver=SimpleNamespace(**namespace)):
        stop(lambda:exec(compile(body,'actual Evidence section','exec'),{}),'final')
        actual['mode']='ask_first'
        try: exec(compile(body,'actual Evidence section','exec'),{})
        except BoundaryReached: pass
        else: raise AssertionError('positive Evidence check did not reach capture boundary')

def native_fixture(folder,run,text='image'):
    proof=folder/'native/attempt-1/run';proof.mkdir(parents=True,exist_ok=True)
    image=proof/'scene.png';image.write_bytes((run+text).encode())
    o.write(proof/'scene-freshness.json',{'image':'scene.png','sha256':o.sha(image)})
    (folder/'native/attempt-1/fresh-shell.txt').write_text('native stub completed without assertion\n')
    outcome={'status':'DONE','attempts':[{'run':run,'cleanup_exit':0,'identity_unchanged':True,
        'sections':[{'kind':'passed'}]}]}
    o.write(folder/'native/outcome.json',outcome)
    return outcome,image

def mutant_pending(runner,root):
    expected={'head':'parent'}
    evidence=root/'evidence';evidence.mkdir()
    prepared=root/'prepared.json'
    o.write(prepared,{'parent_identity':expected,'cases':[]})
    go=root/'go.json';o.write(go,{'mutant_manifest_sha256':o.sha(prepared)})
    case={'name':'visual','file':'unused.rs','old':'old','new':'new','property':'glass','sections':['settings/Glass smoke'],
        'scene_runs_mutated_line':True,'capture':'scene','judge':'read','binaries':root,'identity':expected,
        'tier_s':root/'tier','go':str(go)}
    @contextlib.contextmanager
    def installed(*unused): yield
    @contextlib.contextmanager
    def child(*unused,**options):
        class Child:
            def wait(self,timeout):
                native_fixture(evidence/'mutants/1','11111111')
                return 0
        yield Child()
    args=SimpleNamespace(evidence=evidence,native_mutants=prepared,go=go,deadline=time.time()+100)
    with patch.object(mutants,'manifest',return_value=[case]),patch.object(mutants,'installed',side_effect=installed), \
            patch.object(o,'code_identity',return_value=expected),patch.object(o,'require_tier'),patch.object(o,'owned_child',side_effect=child):
        result=mutants.run_mutants(args,expected)
    assert result['state']=='awaiting lead read' and result['passed'] is False
    row=result['cases'][0]
    assert not row['killed'] and not row['survived'] and row['read_seal']['run']=='11111111'
    digest=row['read_seal']['capture_sha256']
    assert mutants.record_mutant_read(evidence,'11111111',digest,'killed')['passed']

def mutant_hash(runner,root):
    evidence=root/'evidence';evidence.mkdir()
    rows=[];images=[]
    for index,run in enumerate(['11111111','22222222'],1):
        folder=evidence/'mutants'/str(index)
        outcome,image=native_fixture(folder,run)
        images.append(image)
        rows.append({'case':str(index),'folder':str(folder),'state':'awaiting lead read','killed':False,'survived':False,
            'read_seal':mutants.seal_capture(folder,outcome,'scene')})
    path=evidence/'mutants/receipt.json'
    o.write(path,mutants.summarize(rows,2))
    original=path.read_bytes()
    stop(lambda:mutants.record_mutant_read(evidence,'11111111',o.sha(images[1]),'killed'),'hash does not match this run')
    assert path.read_bytes()==original, 'rejected read changed the receipt'
    stop(lambda:mutants.record_mutant_read(evidence,'33333333',o.sha(images[0]),'killed'),'unknown or ambiguous')
    first=images[0].read_bytes();images[0].write_bytes(images[1].read_bytes())
    stop(lambda:mutants.record_mutant_read(evidence,'11111111',o.sha(images[0]),'killed'),'fresh native image')
    images[0].write_bytes(first)
    assert mutants.record_mutant_read(evidence,'11111111',o.sha(images[0]),'killed')['state']=='awaiting lead read'
    stop(lambda:mutants.record_mutant_read(evidence,'11111111',o.sha(images[0]),'survived'),'already decided')
    result=mutants.record_mutant_read(evidence,'22222222',o.sha(images[1]),'survived')
    assert result['state']=='failed' and result['passed'] is False

def child_signals(runner,root):
    events=[];original=signal.pthread_sigmask(signal.SIG_BLOCK,[])
    class Child:
        def poll(self): return None
        def send_signal(self,number): events.append(('signal',number))
        def wait(self,timeout): events.append(('wait',timeout));return 0
    def popen(*args,**kwargs):
        assert o.SIGNALS <= signal.pthread_sigmask(signal.SIG_BLOCK,[]), 'spawn was not masked'
        assert kwargs['start_new_session'] is True
        events.append('spawn')
        return Child()
    try:
        with patch.object(subprocess,'Popen',side_effect=popen):
            try:
                with o.owned_child(['stub']):
                    assert signal.pthread_sigmask(signal.SIG_BLOCK,[])==original
                    events.append('inside try')
                    raise ValueError('planted post-spawn callback failure')
            except ValueError: pass
    finally: signal.pthread_sigmask(signal.SIG_SETMASK,original)
    assert events==['spawn','inside try',('signal',signal.SIGTERM),('wait',120)]
    assert runner.start_owned is o.start_owned
    for name in ['frame_guard.py','mutants.py']:
        source=(o.BIN/name).read_text()
        assert 'o.owned_child(' in source and 'subprocess.Popen(' not in source, name+' duplicates unsafe supervision'

def skipped_fixture(runner,root):
    output=io.StringIO()
    with patch.object(subprocess,'run',return_value=SimpleNamespace(returncode=128)),contextlib.redirect_stdout(output):
        pages.self_test()
    text=output.getvalue()
    assert 'SKIPPED select-pages real commit fixture' in text
    assert 'PASS select-pages real commit fixture' not in text

CASES={name:globals()[name] for name in ['backdrop_paths','cover_set','baseline_pin','part_6d','go_stop',
    'start_tier_stop','start_go_stop','fatal_stop','missing_backdrop','section_identity','evidence_mode',
    'mutant_pending','mutant_hash','capture_recovery','child_signals','skipped_fixture']}

def run_all(runner,root):
    root.mkdir()
    for name,case in CASES.items():
        folder=root/name;folder.mkdir()
        case(runner,folder)
        print('PASS repair regression '+name)

if __name__=='__main__':
    root=pathlib.Path(tempfile.mkdtemp(prefix='veyra-mverify-r1-test-',dir='/tmp'))
    try:
        runner=runner_module()
        name=sys.argv[1]
        CASES[name](runner,root)
        print('PASS repair regression '+name)
    finally: shutil.rmtree(root)
