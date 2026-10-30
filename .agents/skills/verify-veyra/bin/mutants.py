"""Prepare at most three class C mutants offline, then route each prebuilt mutant to its admitted scene.

Exact original source and binary bytes are restored in finally. No Git operation writes anything.
Retained capture judging is a checker self-test; it never counts as a native mutant kill.
"""
import contextlib, json, pathlib, shutil, signal, subprocess, sys
import orchestration as o
import pages

def manifest(path):
    value = json.loads(pathlib.Path(path).read_text())
    cases = value['cases']
    assert 1 <= len(cases) <= 3 and len({case['name'] for case in cases})==len(cases), 'at most 3 native mutants per slice'
    for case in cases:
        assert case['property'] in ['glass','activation','clipboard','quit','pty'], 'native mutants require class C'
        assert pages.matches(case['file'],pages.LISTS[case['property']]), 'mutant file does not own this property'
        assert case['old'] and case['old'] != case['new'] and case['sections'], 'missing mutation or scene'
        assert case['capture'] and case.get('scene_runs_mutated_line') is True, 'draw-site routing must be reviewed'
    return cases

@contextlib.contextmanager
def installed(case, folder, binaries=None):
    """Journal first; exact restore before clearing the marker. A failed restore leaves the marker."""
    target = o.REPO/case['file']
    original = target.read_bytes()
    text = original.decode()
    assert text.count(case['old']) == 1, 'mutation is not unique'
    source = folder/'original-source'
    source.write_bytes(original)
    saved = folder/'original-binaries'
    saved.mkdir()
    for name in o.BINARY_NAMES:
        (saved/name).parent.mkdir(parents=True,exist_ok=True)
        shutil.copy2(o.REPO/'target'/name,saved/name)
    marker = folder/'mutation-in-flight.json'
    o.write(marker,{'source':str(target),'original':str(source),'sha256':o.sha(source),
        'binaries':{name:o.sha(saved/name) for name in o.BINARY_NAMES}})
    try:
        target.write_text(text.replace(case['old'],case['new'],1))
        if binaries:
            for name in o.BINARY_NAMES: shutil.copy2(pathlib.Path(binaries)/name,o.REPO/'target'/name)
        yield
    finally:
        previous = signal.pthread_sigmask(signal.SIG_BLOCK,{signal.SIGINT,signal.SIGHUP,signal.SIGTERM})
        try:
            target.write_bytes(original)
            for name in o.BINARY_NAMES: shutil.copy2(saved/name,o.REPO/'target'/name)
            assert target.read_bytes()==original, 'source restore failed; marker retained'
            assert all(o.sha(saved/name)==o.sha(o.REPO/'target'/name) for name in o.BINARY_NAMES), 'binary restore failed'
            o.write(folder/'restore.json',{'passed':True,'source_sha256':o.sha(target),
                'binaries':{name:o.sha(o.REPO/'target'/name) for name in o.BINARY_NAMES}})
            marker.unlink()
        finally: signal.pthread_sigmask(signal.SIG_SETMASK,previous)

def prepare_mutants(path, folder):
    cases = manifest(path)
    folder.mkdir(parents=True,exist_ok=False)
    initial = o.identity()
    prepared = []
    for index,case in enumerate(cases):
        out = folder/str(index+1)
        out.mkdir()
        with installed(case,out):
            tier = o.tier_s(out/'tier-s')
            binaries = out/'mutated-binaries'
            for name in o.BINARY_NAMES:
                (binaries/name).parent.mkdir(parents=True,exist_ok=True)
                shutil.copy2(o.REPO/'target'/name,binaries/name)
            prepared.append(dict(case,tier_s=str(out/'tier-s/receipt.json'),binaries=str(binaries),identity=tier['identity']))
        assert o.identity()==initial, 'prepare-mutants restore identity failed'
    o.write(folder/'prepared.json',{'source_manifest_sha256':o.sha(path),'parent_identity':initial,'cases':prepared})
    return prepared

def run_mutants(args, expected):
    prepared = json.loads(args.native_mutants.read_text())
    assert prepared['parent_identity']==expected, 'prepared mutants have another parent identity'
    # GO names each case's complete Tier S identity. No approval is synthesized by the runner.
    go = json.loads(args.go.read_text())
    assert go.get('mutant_manifest_sha256')==o.sha(args.native_mutants), 'GO does not authorize these native mutants'
    cases = manifest(args.native_mutants)
    results = []
    for index,case in enumerate(cases):
        folder = args.evidence/'mutants'/str(index+1)
        folder.mkdir(parents=True,exist_ok=False)
        try:
            with installed(case,folder,case['binaries']):
                current = o.identity()
                assert current==case['identity'], 'mutant source/binary identity changed'
                o.require_tier(case['tier_s'],current)
                assert case['go'], 'each mutant requires its own explicit GO for its identity'
                command = [str(o.BIN/'run'),'--sections',*case['sections'],'--tier-s',case['tier_s'],
                    '--go',case['go'],'--evidence',str(folder/'native'),'--deadline',str(args.deadline)]
                with (folder/'native.txt').open('w') as log:
                    child = subprocess.Popen(command,cwd=o.REPO,stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
                    try: code = child.wait(timeout=max(.01,args.deadline-__import__('time').time()))
                    except BaseException:
                        child.send_signal(signal.SIGTERM)
                        child.wait(timeout=120)  # its runner owns app/provider Cleanup
                        raise
                outcome = json.loads((folder/'native/outcome.json').read_text())
                attempts = outcome['attempts']
                assert attempts and all(row['cleanup_exit']==0 and row['identity_unchanged'] and not row.get('fatal_stop')
                    for row in attempts), 'mutant safety failure'
                assert not any(section['kind']=='blocked' for row in attempts for section in row['sections']), 'mutant guard blocked'
                transcript = folder/'native/attempt-1/fresh-shell.txt'
                assertion = o.assertion_from_transcript(transcript)
                captured = folder/'native/attempt-1/run'/(case['capture']+'.png')
                # A read verdict is bound to this fresh native image; old images cannot kill a mutant.
                read = json.loads(pathlib.Path(case['read_verdict']).read_text()) if case.get('read_verdict') else None
                killed = captured.is_file() and code != 0 and assertion == tuple(case.get('expected_assertion',[])) and bool(assertion)
                if read:
                    killed = captured.is_file() and read.get('capture_sha256')==o.sha(captured) and read.get('verdict')=='fail'
                results.append({'case':case['name'],'killed':killed,'survived':code==0 and not killed,
                    'assertion':assertion,'capture':str(captured),'read':read})
        finally:
            assert o.identity()==expected, 'native mutant restore failed'
            o.write(args.evidence/'mutants/receipt.json',{'cases':results,'passed':len(results)==len(cases) and all(row['killed'] for row in results)})
    return all(row['killed'] for row in results)
