"""Offline orchestration checks. Native boundaries are stubbed; prepare() and the section scheduler are real."""
import ast, hashlib, importlib.machinery, importlib.util, json, os, pathlib, shutil, signal, tempfile, time
from types import SimpleNamespace
from unittest.mock import patch
import freshness, frame_guard, orchestration as o, pages

def rejects(operation):
    try: operation()
    except (AssertionError,ValueError,KeyError,FileNotFoundError): return
    raise AssertionError('negative control unexpectedly passed')

def fake_tier(root, expected):
    root.mkdir()
    rows = []
    for name,argv in o.COMMANDS.items():
        path = root/(name+'.txt')
        path.write_text('offline self-test receipt; no command executed\n')
        rows.append({'name':name,'argv':argv,'exit':0,'output':path.name,'sha256':o.sha(path)})
    (root/'tools').mkdir()
    for name in o.TOOLS: (root/'tools'/name).write_text('self-test bytes, never executable\n')
    value = {'schema':1,'passed':True,'identity':expected,'commands':rows,
        'started_epoch':time.time()-2,'finished_epoch':time.time()-1,
        'tools':{name:o.sha(root/'tools'/name) for name in o.TOOLS}}
    path = root/'receipt.json'
    o.write(path,value)
    return path,value

def admission_and_prepare(runner,root):
    expected = {'head':'a'*40,'diff_sha256':'b'*64,'untracked_sha256':{},'binaries':{'debug/veyra':'c'*64}}
    tier,value = fake_tier(root/'tier',expected)
    o.require_tier(tier,expected)
    rejects(lambda:o.require_tier(root/'missing',expected))
    rejects(lambda:o.require_tier(tier,dict(expected,head='d'*40)))
    rejects(lambda:o.require_tier(tier,expected,now=time.time()+24*3600))
    for change in [{'passed':False},{'commands':value['commands'][:-1]},{'tools':{}}]:
        o.write(tier,dict(value,**change))
        rejects(lambda:o.require_tier(tier,expected))
    o.write(tier,value)
    output = tier.parent/value['commands'][0]['output']
    original = output.read_bytes()
    output.write_bytes(b'changed')
    rejects(lambda:o.require_tier(tier,expected))
    output.write_bytes(original)
    go = root/'go.json'
    o.write(go,{'launch_authorized':True,'identity':expected,'issued_epoch':time.time()-1,
        'cutoff_epoch':time.time()+600,'frame_cost_guard_authorized':True,'arch_plan_paused':True})
    argv = ['--smoke','--base','fixture','--tier-s',str(tier),'--go',str(go),
        '--evidence',str(root/'smoke'),'--deadline',str(time.time()+300)]
    # Call the real parser, selection, plan construction, receipt/GO validation and environment preparation.
    with patch.object(runner,'checkout_identity',return_value=expected),patch.object(pages,'select',return_value=['settings']), \
            patch.object(pages,'changed',return_value=[]):
        found,plan,args,env,identity = runner.prepare(argv,validate=False)
        assert identity==expected and ('settings','Native smoke') in plan
        assert ('settings','Window background and display options') not in plan
        assert ('settings','Glass smoke') not in plan
        assert env['VEYRA_VERIFY_TIER_S']==str(tier.resolve())
        assert plan[:3]==[('SKILL','Launch'),('SKILL','Doctor'),runner.FIRST]
        assert plan[-2:]==[('SKILL','Doctor'),('SKILL','Evidence')]
        rejects(lambda:runner.prepare([*argv,'--real-quiet'],validate=False))
        _,blocked,_,_,_ = runner.prepare([*argv,'--sections','settings/Native smoke'],validate=False)
        assert [step for step in blocked if step[0]=='settings']==[('settings','Native smoke')]
        with patch.object(pages,'changed',return_value=['crates/veyra/src/ui/theme.rs']), \
                patch.object(pages,'production_hunks',return_value=['production']):
            _,glass,_,_,_ = runner.prepare([*argv,'--then-guard','--baseline',str(root)],validate=False)
            assert ('settings','Glass smoke') in glass
        with patch.object(pages,'changed',return_value=['crates/veyra/src/ui/theme.rs']), \
                patch.object(pages,'production_hunks',return_value=[]):
            assert ('settings','Glass smoke') not in runner.prepare(argv,validate=False)[1]
        # Real main() dispatch must call guard before the native smoke, and a failed guard cannot launch it.
        for passing in [False,True]:
            order=[]
            def guard(*unused): order.append('guard'); return {'passed':passing,'cleanup_passed':passing}
            def native(*unused): order.append('smoke'); return 0
            with patch.object(runner,'prepare',return_value=(found,plan,SimpleNamespace(then_guard=True,
                    evidence=root/('chain-'+str(passing))),env,expected)), \
                    patch.object(frame_guard,'run_guard',side_effect=guard),patch.object(runner,'run_native',side_effect=native):
                (root/('chain-'+str(passing))).mkdir()
                assert runner.main()==(0 if passing else 1)
            assert order==(['guard','smoke'] if passing else ['guard'])
    assert not (root/'smoke/status-proposals.json').exists(), 'smoke stamped full-run status'

def capture_checks(root):
    image = root/'capture.png'
    image.write_bytes(b'identical pixels')
    os.utime(image,ns=(100,100))
    first = freshness.record(image,99,11,20)
    # A new capture can have precisely the same pixel digest after a state-only repaint.
    os.utime(image,ns=(200,200))
    second = freshness.record(image,199,11,21,first)
    assert first['sha256']==second['sha256']
    rejects(lambda:freshness.record(image,199,11,20,first))
    rejects(lambda:freshness.record(image,201,11,22,second))
    calls=[]
    def tripped(): calls.append('guard'); image.write_bytes(b'covered'); raise AssertionError('covered')
    try: freshness.retry_capture(tripped,lambda:image.unlink(missing_ok=True))
    except AssertionError: pass
    assert calls==['guard']*3 and not image.exists(), 'capture retry/discard bound changed'
    calls=[]
    def privacy(): calls.append('privacy'); raise o.SafetyStop('missing backdrop')
    try: freshness.retry_capture(privacy,lambda:image.unlink(missing_ok=True))
    except o.SafetyStop: pass
    assert calls==['privacy'], 'unenforceable privacy failure retried'
    calls=[]
    def cannot_discard(): raise OSError('planted discard failure')
    try: freshness.retry_capture(tripped,cannot_discard)
    except o.SafetyStop: pass
    else: raise AssertionError('failed discard did not stop for privacy')
    assert calls==['guard'], 'failed discard retried a capture'

def collect_all(runner,root):
    """Run the actual attempt scheduler through blocked capture, independent assertion and next section."""
    old_mask = signal.pthread_sigmask(signal.SIG_BLOCK,[])
    repo = root/'attempt-repo'
    proof = repo/'target/verify-veyra/0123abcd'
    proof.mkdir(parents=True)
    out = root/'attempt-output'
    env = {'RUN':'0123abcd','VEYRA_VERIFY_TMP':str(root)+'/data-'}
    found = {('SKILL',name):name for name in ['Launch','Doctor','Evidence','Cleanup','Recover section']}
    found.update({('page',name):name for name in ['covered','broken','independent']})
    plan = [('SKILL','Launch'),('SKILL','Doctor'),('page','covered'),('page','broken'),
        ('page','independent'),('SKILL','Evidence')]
    calls=[]
    class Child:
        def __init__(self,section): self.section=section; self.pid=99999999; self.returncode=0
        def communicate(self,timeout):
            calls.append(self.section)
            if self.section=='covered':
                o.write(proof/'focus-failure.json',{'failure_epoch':time.time(),'kind':'capture','capture_exhausted':True})
                self.returncode=1
            elif self.section=='broken': self.returncode=1
            return ('','AssertionError: plain message\n' if self.returncode else '')
        def poll(self): return self.returncode
    def start(argv,**unused): return Child(argv[-1]),old_mask
    record={'run':'0123abcd','failed':None,'problems':[],'interrupted':None}
    # A pid marker makes finally exercise the real Cleanup dispatch, entirely through the stubbed child.
    (proof/'pid').write_text('99999999\n')
    try:
        with patch.object(runner,'REPO',repo),patch.object(runner,'start_owned',side_effect=start), \
                patch.object(runner,'checkout_identity',return_value={'test':'identity'}), \
                patch.object(runner,'failure_capture',return_value={'stubbed':True}),patch.object(runner,'INTERRUPT',{'signal':None}):
            runner.attempt(found,plan,out,env,time.time()+60,{'test':'identity'},record)
        assert 'independent' in calls and calls[-1]=='Cleanup', calls
        assert [row['kind'] for row in record['sections'] if row['exit']]==['blocked','failed'],record
        assert runner.verdict(record).startswith('FAILED: independent'),record
        assert record['evidence']['run'] and record['cleanup_exit']==0
    finally:
        signal.pthread_sigmask(signal.SIG_SETMASK,old_mask)

def routing_and_retention(runner,root):
    transcript = root/'transcript.txt'
    for text,wanted in [('AssertionError: plain message\n',('plain message',)),
            ("AssertionError: ('kind', 'capture', 3)\n",('kind','capture',3)),('AssertionError\n',('',)),('no failure\n',())]:
        transcript.write_text(text)
        assert o.assertion_from_transcript(transcript)==wanted
    routes=json.loads((o.BIN/'mutant-routes.json').read_text())
    assert 'new-thread/Grok' in routes['wrapped-send-trailing']['sections']
    assert 'chat-send-stream/Grok' in routes['wrapped-send-trailing']['sections']
    assert 'approvals-questions/Codex' in routes['approval-dock-inset']['sections']
    assert 'settings/Window background and display options' in routes['sidebar-title-dimming']['sections']
    found=runner.sections()
    for route in routes.values():
        current = (o.REPO/route['file']).read_text()
        assert route['old'] in current or route['new'] in current, 'mutant line disappeared: '+route['file']
        assert all(tuple(section.split('/',1)) in found for section in route['sections'])
        source = ''.join(found[tuple(section.split('/',1))] for section in route['sections'])
        capture = route['capture'].removesuffix('-placeholder')
        prefix,_,suffix = capture.rpartition('-')
        assert capture in source or (prefix in source and suffix in source),route
    loader=importlib.machinery.SourceFileLoader('prune_self_test',str(o.BIN/'prune'))
    spec=importlib.util.spec_from_loader(loader.name,loader)
    prune=importlib.util.module_from_spec(spec);loader.exec_module(prune)
    evidence=root/'prune-evidence';evidence.mkdir()
    files=['study.txt','receipt.json','failure.png','smoke/capture.png','reviews/aggregate.md',
        'guard/summary.json','red/output.txt','green/output.txt','cited.txt','baseline.txt']
    for name in files:
        path=evidence/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_text('retained')
    cites=root/'citations.md';cites.write_text('cited.txt baseline.txt')
    planned=prune.plan(evidence,{name:'study' for name in files},[cites])
    assert [row['path'] for row in planned['delete']]==['study.txt'],planned
    assert all((evidence/name).exists() for name in files), 'dry run deleted evidence'
    import status_lines
    status_lines.propose(root,{'head':'a'*40},'DONE',runner.page_names())
    proposed=json.loads((root/'status-proposals.json').read_text())
    assert proposed['full_run'] and set(proposed['pages'])==set(runner.page_names())-pages.LIVE
    assert all(state=='passed' for state in proposed['pages'].values())
    import mutants
    repo=root/'mutant-repo';(repo/'src').mkdir(parents=True)
    source=repo/'src/owned.rs';source.write_bytes(b'original owned source')
    for name in o.BINARY_NAMES:
        path=repo/'target'/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_bytes(b'original binary')
    folder=root/'mutant-restore';folder.mkdir()
    with patch.object(o,'REPO',repo):
        try:
            with mutants.installed({'file':'src/owned.rs','old':'original','new':'mutated'},folder):
                assert source.read_bytes()==b'mutated owned source'
                (repo/'target/debug/veyra').write_bytes(b'mutated binary')
                raise ValueError('planted interrupted drive')
        except ValueError: pass
    assert source.read_bytes()==b'original owned source'
    assert all((repo/'target'/name).read_bytes()==b'original binary' for name in o.BINARY_NAMES)
    assert not (folder/'mutation-in-flight.json').exists()

def run(runner):
    root=pathlib.Path(tempfile.mkdtemp(prefix='veyra-mverify-self-test-',dir='/tmp'))
    handlers={number:signal.getsignal(number) for number in runner.SIGNALS}
    try:
        pages.self_test()
        capture_checks(root)
        admission_and_prepare(runner,root)
        collect_all(runner,root)
        routing_and_retention(runner,root)
        import repair_tests
        repair_tests.run_all(runner,root/'repair-regressions')
        assert frame_guard.judge([1.2]*5)=='fail'
        assert frame_guard.judge([.9,1.11,1.12,1.13,.9])=='extend'
        assert frame_guard.judge([1.01]*5)=='pass'
        print('PASS orchestration: real prepare() pass-through; actual guard-then-smoke dispatch and failed-guard refusal; '+
            'fresh complete same-identity Tier S and output hashes; collect-all/blocked continuation/Cleanup; '+
            '3 capture attempts and pixel-independent repaint/file freshness; plain-message parsing; mutant scene routing; retention dry run')
    finally:
        for number,handler in handlers.items(): signal.signal(number,handler)
        shutil.rmtree(root)
