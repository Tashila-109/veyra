#!/usr/bin/env python3
"""Retain red/green offline mutation receipts for the accepted M9 maintenance Musts.

Only defective scratch copies are edited; no product file, Git object, app or native surface is touched.
Usage: python3 bin/repair_proofs.py --evidence DIR
"""
import argparse, ast, difflib, hashlib, json, os, pathlib, shutil, subprocess, sys, tempfile
import orchestration as o

CASES = [
    ('1-backdrop','backdrop_paths','backdrop'),
    ('2-cover-set','cover_set','cover'),
    ('3-baseline-pin','baseline_pin','baseline'),
    ('4-part-6d','part_6d','idle'),
    ('5-go','go_stop','go'),
    ('5-start-tier','start_tier_stop','start-tier'),
    ('5-start-go','start_go_stop','start-go'),
    ('5-fatal','fatal_stop','fatal'),
    ('5-missing-backdrop','missing_backdrop','missing'),
    ('5-section-identity','section_identity','section-identity'),
    ('6-default-mode','evidence_mode','mode'),
    ('7-mutant-awaiting','mutant_pending','pending'),
    ('7-mutant-hash','mutant_hash','hash'),
    ('8-capture-recovery','capture_recovery','capture'),
    ('10-child-signals','child_signals','signals'),
]

def replace_once(text, old, new):
    assert text.count(old)==1, 'mutation site is not unique: '+old
    return text.replace(old,new,1)

def replace_function(text, name, body):
    node=next(node for node in ast.parse(text).body if isinstance(node,ast.FunctionDef) and node.name==name)
    lines=text.splitlines(True)
    # Preserve the signature; these helpers use one-line signatures.
    lines[node.lineno:node.end_lineno]=[body+'\n']
    return ''.join(lines)

def defect(skill, kind):
    names={'backdrop':'bin/frame_probe.py','cover':'bin/frame_guard.py','baseline':'bin/frame_guard.py',
        'idle':'bin/frame_scene.py','go':'bin/orchestration.py','start-tier':'bin/driver.py','start-go':'bin/driver.py',
        'fatal':'bin/run','missing':'bin/driver.py','section-identity':'bin/run','mode':'SKILL.md',
        'pending':'bin/mutants.py','hash':'bin/mutants.py','capture':'bin/run','signals':'bin/orchestration.py'}
    path=skill/names[kind];old=path.read_text();text=old
    if kind=='backdrop': text=replace_once(text,"shutil.copy2(TOOLS/'backdrop',executable)","shutil.copy2(HELPERS/'tools/backdrop',executable)")
    elif kind=='cover': text=replace_once(text,"can_restart = not pair_started and error.row.get('measurement_started') is False","can_restart = not receipt['pairs']")
    elif kind=='baseline':
        text=replace_once(text,'    checks = authenticate_baseline(baseline)','    checks = _self_consistent_baseline(baseline)')
        text=replace_once(text,'        authenticate_baseline(baseline)','        _self_consistent_baseline(baseline)')
        text+='''\ndef _self_consistent_baseline(baseline):
    import re
    rows=dict((name,digest) for digest,name in re.findall(r'^([0-9a-f]{64})  (.+)$',(baseline/'identity.txt').read_text(),re.M))
    assert len(rows)==3 and all(o.sha(baseline/name)==digest for name,digest in rows.items())
    assert re.fullmatch('[0-9a-f]{40}',baseline.name)
    return rows
'''
    elif kind=='idle': text=replace_once(text,"if part != '6d': return None",'if True: return None')
    elif kind=='go': text=replace_function(text,'require_go',"    return json.loads(pathlib.Path(path).read_text())")
    elif kind=='start-tier': text=replace_once(text,"    require_tier(os.environ['VEYRA_VERIFY_TIER_S'],expected)\n",'')
    elif kind=='start-go': text=replace_once(text,"    require_go(os.environ['VEYRA_VERIFY_GO'],expected,float(os.environ['VEYRA_VERIFY_DEADLINE']))\n",'')
    elif kind=='fatal': text=replace_once(text,"bool(re.search(r'(?m)^(?:\\w+\\.)?SafetyStop:',text))",'False')
    elif kind=='missing':
        text=replace_once(text,"    if not (PROOF/'backdrop-windows.json').is_file():\n        save('safety-stop',{'kind':'privacy','reason':'missing owned backdrop'})\n        raise SafetyStop('missing owned backdrop; cannot enforce privacy')\n",'')
    elif kind=='section-identity': text=replace_once(text,'            if checkout_identity() != expected:','            if False:')
    elif kind=='mode': text=replace_once(text,"database('final',configured_default_mode())","database('final',sql('select default_mode from settings'))")
    elif kind=='pending':
        text=replace_once(text,"                if judge=='read':\n                    row.update(read_seal=seal_capture(folder,outcome,case['capture']),\n                        state='awaiting lead read',killed=False,survived=False)\n",'')
    elif kind=='hash': text=replace_once(text,"    assert digest==sealed['capture_sha256'], 'mutant read hash does not match this run'",'    pass  # submitted read hash is not checked')
    elif kind=='capture': text=replace_once(text,"(observed or {}).get('capture_exhausted') is True","(observed or {}).get('kind') == 'capture'")
    elif kind=='signals': text=replace_once(text,'previous = signal.pthread_sigmask(signal.SIG_BLOCK,SIGNALS)','previous = signal.pthread_sigmask(signal.SIG_BLOCK,[])')
    path.write_text(text)
    return path.relative_to(skill).as_posix(),old,text

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--evidence',type=pathlib.Path,required=True)
    args=parser.parse_args()
    args.evidence.mkdir(parents=True,exist_ok=False)
    receipt={'native_runs':0,'git_writes':0,'cases':[],'passed':False}
    original={str(path.relative_to(o.BIN.parent)):o.sha(path) for path in o.BIN.parent.rglob('*')
        if path.is_file() and '__pycache__' not in path.parts}
    env=dict(os.environ,PYTHONDONTWRITEBYTECODE='1',GIT_OPTIONAL_LOCKS='0')
    try:
        for label,case,kind in CASES:
            scratch=pathlib.Path(tempfile.mkdtemp(prefix='veyra-mverify-r1-mutant-',dir='/tmp'))
            folder=args.evidence/label;folder.mkdir()
            try:
                skill=scratch/'.agents/skills/verify-veyra'
                shutil.copytree(o.BIN.parent,skill,ignore=shutil.ignore_patterns('__pycache__'))
                name,before,after=defect(skill,kind)
                patch=''.join(difflib.unified_diff(before.splitlines(True),after.splitlines(True),fromfile=name,tofile=name))
                (folder/'defect.diff').write_text(patch)
                command=[sys.executable,str(skill/'bin/repair_tests.py'),case]
                red=subprocess.run(command,cwd=scratch,env=env,capture_output=True,text=True,timeout=30)
                (folder/'red.txt').write_text(red.stdout+red.stderr)
                assert red.returncode!=0, label+' survived its planted defect'
                (skill/name).write_text(before)
                green=subprocess.run(command,cwd=scratch,env=env,capture_output=True,text=True,timeout=30)
                (folder/'green.txt').write_text(green.stdout+green.stderr)
                assert green.returncode==0, label+' did not pass after exact repair'
                row={'item':label.split('-')[0],'case':case,'mutation':kind,'red_exit':red.returncode,'green_exit':green.returncode,
                    'defect_file':name,'defective_source_sha256':hashlib.sha256(after.encode()).hexdigest(),
                    'repaired_source_sha256':hashlib.sha256(before.encode()).hexdigest(),
                    'test_source_sha256':o.sha(skill/'bin/repair_tests.py'),
                    'red_sha256':o.sha(folder/'red.txt'),'green_sha256':o.sha(folder/'green.txt'),'native_boundaries':'stubbed or forbidden'}
                o.write(folder/'receipt.json',row)
                receipt['cases'].append(row)
                print('PASS '+label+' red='+str(red.returncode)+' green=0',flush=True)
            finally:
                shutil.rmtree(scratch)
            o.write(args.evidence/'receipt.json',receipt)
        assert {str(path.relative_to(o.BIN.parent)):o.sha(path) for path in o.BIN.parent.rglob('*')
            if path.is_file() and '__pycache__' not in path.parts}==original, 'proof run changed the skill'
        receipt['passed']=True
    finally: o.write(args.evidence/'receipt.json',receipt)
    return 0

if __name__=='__main__': sys.exit(main())
