"""Slice-6 guard: one authenticated baseline, one paired set, one pre-pair cover restart."""
import json, pathlib, statistics, subprocess, sys, time
import orchestration as o
import quiet_machine

BASELINE_REVISION = '5397b7b936f0e69c7c24c892f3f585328681b800'
BASELINE_IDENTITY = 'b2357c36d7a81924d41f97d94b17dc54a0983d7785ffd5895c549c328032e708'
BASELINE_MANIFEST_SHA256 = 'ff8737f21fae030c4bf6ea4c2fdbf0aa3e5bb840c44fc24c047e12e34e9cad02'
BASELINE_BINARIES = {
    'debug/veyra':'4d107bdecdb46adc73bd7b96de98af3372635a4787431f3ed7e816cc59c3e3c6',
    'debug/veyra-fake-cli':'eb628b2115ef690ccaf3da2ec955b689faca2b508205bbd5ae5035f6ae791396',
    'debug/veyra-test-cli':'ee212f857c3fdbcf30835fb104a5b8f449e5a351548d7e77301463d242a5972b',
}

def approved_baseline():
    common = pathlib.Path(subprocess.check_output(['git','rev-parse','--path-format=absolute','--git-common-dir'],
        cwd=o.REPO,text=True).strip()).resolve()
    return common.parent/'.tstack/tasks/v001/evidence/perf/bin'/BASELINE_REVISION

def authenticate_baseline(baseline):
    baseline = pathlib.Path(baseline).resolve()
    assert baseline == approved_baseline().resolve(), 'unapproved slice-5 baseline directory'
    manifest = baseline/'identity.txt'
    assert o.sha(manifest) == BASELINE_MANIFEST_SHA256, 'unapproved slice-5 baseline manifest'
    assert BASELINE_IDENTITY in manifest.read_text(), 'slice-5 code identity missing'
    for name,digest in BASELINE_BINARIES.items():
        assert o.sha(baseline/name) == digest, 'unapproved slice-5 baseline binary: '+name
    return dict(BASELINE_BINARIES)

class CoverTrip(o.SafetyStop):
    def __init__(self, row):
        super().__init__('frame guard cover trip')
        self.row = row

def judge(ratios):
    assert len(ratios) in [5,10]
    if statistics.median(ratios)>1.10:
        if sum(value>1 for value in ratios)>=(4 if len(ratios)==5 else 8): return 'fail'
        if len(ratios)==5: return 'extend'
    return 'pass'

def chain(guard, smoke):
    result = guard()
    if not result['passed'] or not result.get('cleanup_passed'): return result
    return smoke()

def measure_set(one, receipt, persist, deadline, wait=time.sleep):
    """An unsuccessful first pre-measurement cover check discards the set, never a single side of a pair."""
    receipt['sets'] = []
    for attempt in range(2):
        receipt.update(pairs=[],idle={},working={})
        pair_started = False
        prefix = 'set-'+str(attempt+1)+'-'
        try:
            for index in range(1,11):
                a = one('baseline',prefix+f'baseline-{index:02d}')
                pair_started = True
                b = one('candidate',prefix+f'candidate-{index:02d}')
                receipt['pairs'].append({'pair':index,'ratio':b['core_fraction']/a['core_fraction'],
                    'baseline':a,'candidate':b})
                persist()
                if index not in [5,10]: continue
                verdict = judge([row['ratio'] for row in receipt['pairs']])
                receipt['cpu_verdict'] = verdict
                if verdict == 'fail': raise o.SafetyStop('paired CPU rule failed')
                if verdict == 'pass': break
            for focus in ['drawer','composer']:
                a = one('baseline',prefix+'baseline-idle-'+focus,'idle',focus=focus)
                b = one('candidate',prefix+'candidate-idle-'+focus,'idle',focus=focus)
                receipt['idle'][focus] = {'baseline':a,'candidate':b,'passed':b['repaints']<=a['repaints']}
                assert receipt['idle'][focus]['passed'], 'idle repaint regression'
            for motion in ['normal','reduced']:
                row = one('candidate',prefix+'candidate-working-'+motion,'working',motion=motion)
                receipt['working'][motion] = row
                if motion=='normal': assert row['repaints_per_second']<=12, 'Working repaint budget'
            receipt['sets'].append({'attempt':attempt+1,'state':'complete'})
            return
        except CoverTrip as error:
            can_restart = not pair_started and error.row.get('measurement_started') is False
            if not can_restart or attempt != 0: raise
            receipt['sets'].append({'attempt':attempt+1,'state':'discarded','reason':'pre-pair cover trip',
                'pairs':receipt['pairs'],'idle':receipt['idle'],'working':receipt['working']})
            receipt.update(pairs=[],idle={},working={})
            persist()
            assert deadline-time.time()>60, 'pre-pair cover restart cannot fit before cutoff'
            wait(60)

def run_guard(args, expected):
    root = args.evidence/'guard'
    root.mkdir(exist_ok=False)
    baseline = args.baseline.resolve()
    checks = authenticate_baseline(baseline)
    receipt = {'passed':False,'cleanup_passed':False,'pairs':[],'idle':{},'working':{},'attempts':[],
        'identity':expected,'baseline':str(baseline),'baseline_code_identity':BASELINE_IDENTITY,
        'baseline_manifest_sha256':BASELINE_MANIFEST_SHA256,'baseline_checks':checks,'placement':[24,60,960,740],
        'part':args.part,'recording_off':True,'builds_during_set':0}
    def persist(): o.write(root/'summary.json',receipt)
    def frozen():
        assert o.code_identity()==expected, 'guard candidate identity changed'
        o.require_tier(args.tier_s,expected)
        o.require_go(args.go,expected,args.deadline,guard=True)
        authenticate_baseline(baseline)
    def one(side,label,scene='stream',focus=None,motion=None):
        frozen()
        phase = 'stream' if scene=='stream' else 'scenes'
        command = [sys.executable,str(o.BIN/'frame_probe.py'),phase,side,label,'--scene',scene,
            '--part',args.part if side=='candidate' else '6c']
        if focus: command += ['--idle-focus',focus]
        if motion: command += ['--motion',motion]
        quiet = quiet_machine.wait_for_quiet(root/(label+'-quiet.json'),command)
        frozen()
        env = dict(__import__('os').environ,VEYRA_FRAME_GUARD_PROOF=str(root),VEYRA_FRAME_BASELINE=str(baseline),
            VEYRA_FRAME_BASELINE_REV=BASELINE_REVISION,VEYRA_FRAME_TOOLS=str(args.tier_s.resolve().parent/'tools'),
            VEYRA_VERIFY_DEADLINE=str(args.deadline),PYTHONDONTWRITEBYTECODE='1',
            VEYRA_VERIFY_TIER_S=str(args.tier_s.resolve()),VEYRA_VERIFY_GO=str(args.go.resolve()))
        env.pop('VEYRA_RECORD',None)
        with (root/(label+'.log')).open('w') as log:
            with o.owned_child(command,cwd=o.REPO,env=env,stdout=log,stderr=subprocess.STDOUT) as child:
                quiet_machine.record_launch(root/(label+'-quiet.json'),quiet,child.pid)
                code = child.wait(timeout=max(.01,args.deadline-time.time()))
        path = root/phase/label/'result.json'
        row = json.loads(path.read_text())
        receipt['attempts'].append({'side':side,'label':label,'scene':scene,'receipt':str(path),'exit':code})
        assert row['cleanup_exit']==0 and row['scratch_removed'] and row['data_removed'], 'guard cleanup failure'
        assert row['owned_rule_cleanup']['passed'], 'guard owned-rule cleanup failure'
        assert row['recording_off'], 'guard recording unexpectedly enabled'
        frozen()
        if code == 0 and row['passed']:
            if scene=='stream':
                assert row['batches_per_second']>=30 and row['largest_gap_ms']<=100, 'guard rate/gap failure'
                assert isinstance(row['repaints'],int), 'stream repaint counter missing'
            return row
        if row.get('cover_trip') or 'covered' in row.get('error',''): raise CoverTrip(row)
        raise o.SafetyStop('frame guard failed: '+label)
    try:
        frozen()
        measure_set(one,receipt,persist,args.deadline)
        frozen()
        receipt.update(passed=True,cleanup_passed=True)
    except BaseException as error:
        receipt.update(error=str(error),passed=False)
        raise
    finally:
        persist()
        o.write(args.evidence/'guard-done.json',{'passed':receipt['passed'],'finished_epoch':time.time(),
            'release_arch_plan_pause':True,'identity':expected})
    return receipt
