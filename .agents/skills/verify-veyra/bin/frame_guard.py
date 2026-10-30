"""Slice-6 paired CPU guard, then both idle focus states and forced Working motion states."""
import json, os, pathlib, re, signal, statistics, subprocess, sys, time
import orchestration as o
import quiet_machine

def judge(ratios):
    assert len(ratios) in [5,10]
    if statistics.median(ratios)>1.10:
        if sum(value>1 for value in ratios)>=(4 if len(ratios)==5 else 8): return 'fail'
        if len(ratios)==5: return 'extend'
    return 'pass'

def chain(guard, smoke):
    """The smoke is never invoked when the guard failed or its cleanup failed."""
    result = guard()
    if not result['passed'] or not result.get('cleanup_passed'): return result
    return smoke()

def run_guard(args, expected, section_reader):
    root = args.evidence/'guard'
    root.mkdir(exist_ok=False)
    baseline = args.baseline.resolve()
    checks = {}
    for digest,name in re.findall(r'^([0-9a-f]{64})  (.+)$',(baseline/'identity.txt').read_text(),re.M):
        assert name in ['debug/veyra','debug/veyra-fake-cli','debug/veyra-test-cli'], 'unexpected baseline file'
        assert o.sha(baseline/name)==digest, 'baseline binary changed'
        checks[name] = digest
    assert len(checks)==3, 'incomplete baseline'
    revision = baseline.name
    assert re.fullmatch('[0-9a-f]{40}',revision), 'baseline directory must name its build revision'
    receipt = {'passed':False,'cleanup_passed':False,'pairs':[],'idle':{},'working':{},'attempts':[],
        'identity':expected,'baseline':str(baseline),'baseline_checks':checks,'placement':[24,60,960,740],
        'recording_off':True,'builds_during_set':0}
    def frozen():
        assert o.identity()==expected, 'guard candidate identity changed'
        o.require_tier(args.tier_s,expected)
        o.require_go(args.go,expected,args.deadline,guard=True)
        assert all(o.sha(baseline/name)==digest for name,digest in checks.items()), 'guard baseline changed'
    def one(side,label,scene='stream',focus=None,motion=None):
        for attempt in range(1,4):
            frozen()
            name = label+'-a'+str(attempt)
            phase = 'stream' if scene=='stream' else 'scenes'
            command = [sys.executable,str(o.BIN/'frame_probe.py'),phase,side,name,'--scene',scene]
            if focus: command += ['--idle-focus',focus]
            if motion: command += ['--motion',motion]
            quiet = quiet_machine.wait_for_quiet(root/(name+'-quiet.json'),command)
            frozen()
            env = dict(os.environ,VEYRA_FRAME_GUARD_PROOF=str(root),VEYRA_FRAME_BASELINE=str(baseline),
                VEYRA_FRAME_BASELINE_REV=revision,VEYRA_FRAME_TOOLS=str(args.tier_s.resolve().parent/'tools'),
                VEYRA_VERIFY_DEADLINE=str(args.deadline),PYTHONDONTWRITEBYTECODE='1',
                VEYRA_VERIFY_TIER_S=str(args.tier_s.resolve()),VEYRA_VERIFY_GO=str(args.go.resolve()))
            env.pop('VEYRA_RECORD',None)
            with (root/(name+'.log')).open('w') as log:
                child = subprocess.Popen(command,cwd=o.REPO,env=env,stdout=log,stderr=subprocess.STDOUT,
                    start_new_session=True)
                quiet_machine.record_launch(root/(name+'-quiet.json'),quiet,child.pid)
                try: code = child.wait(timeout=max(.01,args.deadline-time.time()))
                except BaseException:
                    # The owned probe catches TERM and executes its Cleanup before we reap it.
                    child.send_signal(signal.SIGTERM)
                    child.wait(timeout=120)
                    raise
            path = root/phase/name/'result.json'
            row = json.loads(path.read_text())
            receipt['attempts'].append({'side':side,'label':name,'scene':scene,'receipt':str(path),'exit':code})
            assert row['cleanup_exit']==0 and row['scratch_removed'] and row['data_removed'], 'guard cleanup failure'
            assert row['owned_rule_cleanup']['passed'], 'guard owned-rule cleanup failure'
            assert row['recording_off'], 'guard recording unexpectedly enabled'
            frozen()
            if code == 0 and row['passed']:
                if scene=='stream':
                    assert row['batches_per_second']>=30 and row['largest_gap_ms']<=100, 'guard rate/gap failure'
                    assert isinstance(row['repaints'],int), 'stream repaint counter missing'
                return row
            observed = row.get('observed') or {}
            # Exactly one 60 s cover retry before the first pair, with the full unfiltered guard on retry.
            cover_before_pair = not receipt['pairs'] and not receipt.get('prepair_cover_retried') \
                and (row.get('cover_trip') or 'covered' in row.get('error',''))
            if attempt == 1 and cover_before_pair and args.deadline-time.time()>60:
                receipt['prepair_cover_retried'] = True
                time.sleep(60)
                continue
            raise o.SafetyStop('frame guard failed: '+name)
        raise o.SafetyStop('guard attempts exhausted')
    try:
        frozen()
        for index in range(1,11):
            a,b = one('baseline',f'baseline-{index:02d}'),one('candidate',f'candidate-{index:02d}')
            receipt['pairs'].append({'pair':index,'ratio':b['core_fraction']/a['core_fraction'],
                'baseline':a,'candidate':b})
            o.write(root/'summary.json',receipt)
            if index not in [5,10]: continue
            verdict = judge([row['ratio'] for row in receipt['pairs']])
            receipt['cpu_verdict'] = verdict
            if verdict == 'fail': raise o.SafetyStop('paired CPU rule failed')
            if verdict == 'pass': break
        for focus in ['drawer','composer']:
            a = one('baseline','baseline-idle-'+focus,'idle',focus=focus)
            b = one('candidate','candidate-idle-'+focus,'idle',focus=focus)
            receipt['idle'][focus] = {'baseline':a,'candidate':b,'passed':b['repaints']<=a['repaints']}
            assert receipt['idle'][focus]['passed'], 'idle repaint regression'
        for motion in ['normal','reduced']:
            row = one('candidate','candidate-working-'+motion,'working',motion=motion)
            receipt['working'][motion] = row
            if motion=='normal': assert row['repaints_per_second']<=12, 'Working repaint budget'
        frozen()
        receipt.update(passed=True,cleanup_passed=True)
    except BaseException as error:
        receipt.update(error=str(error),passed=False)
        raise
    finally:
        o.write(root/'summary.json',receipt)
        o.write(args.evidence/'guard-done.json',{'passed':receipt['passed'],'finished_epoch':time.time(),
            'release_arch_plan_pause':True,'identity':expected})
    return receipt
