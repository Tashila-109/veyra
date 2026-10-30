"""Permanent planted-fault contract: each exact AST plant must make check-only name its guard.

Only temporary skill copies change. Each case runs the complete public check-only entry twice:
planted red, then byte-for-byte restored green. No test case, receipt or guard is patched in the
running checkout. AST selectors fail closed if a call site is removed, duplicated or renamed.
"""
import ast, difflib, hashlib, os, pathlib, shutil, subprocess, sys, tempfile
from dataclasses import dataclass
import orchestration as o

@dataclass(frozen=True)
class Plant:
    name: str
    guard: str
    file: str
    function: str
    operation: str
    selector: str
    replacement: str = ''

# A selector is the precise expression/statement AST inside one named runtime function.
# Repeated historical probes remain separate plants where their call sites differ.
PLANTS = [
    Plant('admission-go','prepare-go','bin/orchestration.py','require_go','body','','return {}'),
    Plant('start-tier','start-tier','bin/driver.py','start','remove-call','require_tier'),
    Plant('start-go','start-go','bin/driver.py','start','remove-call','require_go'),
    Plant('start-tier-and-go','start-tier','bin/driver.py','start','remove-calls','require_tier,require_go'),
    Plant('safety-stop-fatal','safety-fatal','bin/run','attempt','expression',"bool(re.search(r'(?m)^(?:\w+\.)?SafetyStop:',text))",'False'),
    Plant('missing-owned-backdrop','backdrop-missing','bin/driver.py','region_capture','remove-if',"not (PROOF/'backdrop-windows.json').is_file()"),
    Plant('identity-before-section','section-identity','bin/run','attempt','if-test','checkout_identity() != expected','False'),
    Plant('sample-part','idle-samples','bin/frame_scene.py','measure_idle','expression','assert_idle_state(observed,focus,part,scene)',"assert_idle_state(observed,focus,'6c',scene)"),
    Plant('awaiting-read-batch','mutant-awaiting','bin/run','run_native','expression',"not result['passed']","result['state']=='failed'"),
    Plant('per-probe-baseline','baseline-frozen','bin/frame_guard.py','run_guard.frozen','remove-call','authenticate_baseline'),
    Plant('guard-tools-directory','guard-tools','bin/frame_guard.py','run_guard.one','expression','str(o.tier_tools(args.tier_s))',"str(o.BIN/'tools')"),
    Plant('probe-hardcoded-part','probe-part','bin/frame_probe.py','main','after-assignment','args',"args.part='6c'"),
    Plant('probe-baseline-entry','baseline-entry','bin/frame_probe.py','main','remove-call','authenticate_baseline'),
    Plant('backdrop-from-tools','guard-tools','bin/frame_probe.py','main','expression',"tools/'backdrop'","o.BIN/'tools/backdrop'"),
    Plant('cover-retry-after-first-side','cover-late','bin/frame_guard.py','measure_set','expression',"not pair_started and error.row.get('measurement_started') is False","not receipt['pairs']"),
    Plant('cover-retry-without-wait','cover-restart','bin/frame_guard.py','measure_set','expression','(wait or time.sleep)(60)','None'),
    Plant('cover-retry-twice','cover-second','bin/frame_guard.py','measure_set','expression','not can_restart or attempt != 0','not can_restart'),
    Plant('baseline-canonical-pin','baseline-pin','bin/frame_guard.py','authenticate_baseline','remove-assert',"baseline == approved_baseline().resolve()"),
    Plant('baseline-manifest-pin','baseline-manifest','bin/frame_guard.py','authenticate_baseline','remove-assert',"o.sha(manifest) == BASELINE_MANIFEST_SHA256"),
    Plant('baseline-binary-pin','baseline-bytes','bin/frame_guard.py','authenticate_baseline','remove-assert',"o.sha(baseline/name) == digest"),
    Plant('guard-go-call','guard-go','bin/frame_guard.py','run_guard.frozen','remove-call','require_go'),
    Plant('prepare-go-call','prepare-go','bin/run','prepare','remove-call','require_go'),
    Plant('probe-go-call','probe-go','bin/frame_probe.py','main','remove-call','require_go'),
    Plant('probe-part-default','part-required','bin/frame_scene.py','parse_probe','part-default',''),
    Plant('runner-part-default','part-required','bin/run','prepare','part-default',''),
    Plant('candidate-tree-part','part-tree','bin/frame_scene.py','require_part','remove-assert',"part=='6d'"),
    Plant('scene-6d-fields','scene-6d','bin/frame_scene.py','require_6d_dump','body','','return None'),
    Plant('scene-6d-idle','probe-part','bin/frame_scene.py','idle_variant','expression',"part != '6d'",'True'),
    Plant('evidence-independent-mode','evidence-mode','SKILL.md','Evidence','expression',"configured_default_mode()","sql('select default_mode from settings')"),
    Plant('capture-exhaustion','capture-exhausted','bin/run','classify','expression',"(observed or {}).get('capture_exhausted') is True",'False'),
    Plant('capture-recovery','capture-recovered','bin/run','classify','expression',"(observed or {}).get('capture_exhausted') is True","(observed or {}).get('kind')=='capture'"),
    Plant('mutant-capture-binding','mutant-binding','bin/mutants.py','record_mutant_read','remove-assert',"digest==sealed['capture_sha256']"),
    Plant('mutant-record-reuse','mutant-reuse','bin/mutants.py','run_mutants','expression','previous is not None',"previous is not None and not previous['passed']"),
    Plant('mutant-slice-end-record','mutant-slice-end','bin/run','run_native','expression','recorded_verdicts(args.evidence,expected)','None'),
    Plant('mutant-capture-revalidation','mutant-tamper','bin/mutants.py','recorded_verdicts','remove-assert',"seal_capture(folder,outcome,seal['capture_name'])==seal"),
    Plant('cleanup-verdict','cleanup-stop','bin/run','verdict','if-test',"record.get('cleanup_exit') != 0 or record.get('scratch_left')",'False'),
    Plant('owned-window-unmanaged','start-window','bin/driver.py','owned_window_record','expression','require_unmanaged(row)','row'),
    Plant('launch-hid-call','launch-hid','SKILL.md','Launch','remove-call','gate'),
    Plant('old-image-file','capture-old-image','bin/freshness.py','record','remove-assert',"metadata.st_size > 0 and metadata.st_mtime_ns >= started_ns"),
    Plant('child-spawn-signal-mask','child-signals','bin/orchestration.py','start_owned','expression','signal.SIG_BLOCK,SIGNALS','signal.SIG_BLOCK,[]'),
]

def expression(source):
    return ast.parse(source,mode='eval').body

def same(node, source):
    return ast.dump(node,include_attributes=False)==ast.dump(expression(source),include_attributes=False)

def function(tree, name):
    current=tree
    for part in name.split('.'):
        found=[node for node in ast.walk(current) if isinstance(node,(ast.FunctionDef,ast.AsyncFunctionDef)) and node.name==part]
        assert len(found)==1, 'plant function is missing or ambiguous: '+name
        current=found[0]
    return current

def apply(source, plant):
    tree=ast.parse(source)
    target=tree if plant.file=='SKILL.md' else function(tree,plant.function)
    edits=[]
    lines=source.splitlines(keepends=True)
    offsets=[0]
    for line in lines: offsets.append(offsets[-1]+len(line))
    def edit(node,replacement):
        edits.append((offsets[node.lineno-1]+node.col_offset,
            offsets[node.end_lineno-1]+node.end_col_offset,replacement))
    nodes=list(ast.walk(target))
    if plant.operation=='body':
        first,last=target.body[0],target.body[-1]
        edits.append((offsets[first.lineno-1]+first.col_offset,
            offsets[last.end_lineno-1]+last.end_col_offset,plant.replacement))
    elif plant.operation=='after-assignment':
        found=[n for n in target.body if isinstance(n,ast.Assign)
            and any(isinstance(t,ast.Name) and t.id==plant.selector for t in n.targets)]
        assert len(found)==1
        node=found[0];edit(node,ast.get_source_segment(source,node)+'\n'+' '*node.col_offset+plant.replacement)
    elif plant.name=='child-spawn-signal-mask':
        calls=[n for n in nodes if isinstance(n,ast.Call) and getattr(n.func,'attr',None)=='pthread_sigmask'
            and len(n.args)==2 and same(n.args[0],'signal.SIG_BLOCK') and same(n.args[1],'SIGNALS')]
        assert len(calls)==1;edit(calls[0].args[1],'[]')
    else:
        for node in nodes:
            if plant.operation=='expression' and isinstance(node,ast.expr) and same(node,plant.selector):
                edit(node,plant.replacement)
            elif plant.operation in ['remove-call','remove-calls'] and isinstance(node,ast.Expr) and isinstance(node.value,ast.Call):
                call=node.value.func;name=getattr(call,'id',None) or getattr(call,'attr',None)
                if name in plant.selector.split(','): edit(node,'pass')
            elif plant.operation=='if-test' and isinstance(node,ast.If) and same(node.test,plant.selector): edit(node.test,plant.replacement)
            elif plant.operation=='remove-if' and isinstance(node,ast.If) and same(node.test,plant.selector): edit(node,'pass')
            elif plant.operation=='remove-assert' and isinstance(node,ast.Assert) and same(node.test,plant.selector): edit(node,'pass')
            elif plant.operation=='part-default' and isinstance(node,ast.Call) and getattr(node.func,'attr',None)=='add_argument' \
                and node.args and isinstance(node.args[0],ast.Constant) and node.args[0].value=='--part':
                node.keywords=[k for k in node.keywords if k.arg not in ['required','default']]
                node.keywords.append(ast.keyword(arg='default',value=ast.Constant('6c')));edit(node,ast.unparse(node))
    assert len(edits)==(2 if plant.operation=='remove-calls' else 1), 'plant site is missing or ambiguous: '+plant.name+' '+str(len(edits))
    for start,stop,replacement in sorted(edits,reverse=True): source=source[:start]+replacement+source[stop:]
    ast.parse(source)  # A malformed plant is never accepted as red evidence.
    return source

def plant_copy(skill, plant):
    path=skill/plant.file;before=path.read_bytes();source=before.decode()
    if plant.file=='SKILL.md':
        import re
        heading=re.search(r'^## '+plant.function+r'\n',source,re.M);assert heading
        end=source.find('\n## ',heading.end());end=len(source) if end<0 else end
        section=source[heading.end():end]
        heredoc=re.search(r"<<'PY'\n(.*?)\nPY(?:\n|$)",section,re.S);assert heredoc
        body=apply(heredoc[1],plant)
        start=heading.end()+heredoc.start(1);stop=heading.end()+heredoc.end(1)
        after=source[:start]+body.rstrip('\n')+source[stop:]
    else: after=apply(source,plant)
    path.write_text(after)
    return path,before,after.encode()

def run_matrix(evidence=None):
    """The receipt is independent of stdout; failures preserve the matrix and both command transcripts."""
    scratch=pathlib.Path(tempfile.mkdtemp(prefix='veyra-mverify-matrix-',dir='/tmp'))
    folder=pathlib.Path(evidence).absolute() if evidence else scratch/'receipt'
    folder.mkdir(parents=True,exist_ok=False)
    result={'schema':1,'passed':False,'native_runs':0,'git_writes':0,'cases':[],
        'source_sha256':{str(p.relative_to(o.BIN.parent)):o.sha(p) for p in sorted(o.BIN.parent.rglob('*'))
            if p.is_file() and '__pycache__' not in p.parts}}
    env=dict(os.environ,PYTHONDONTWRITEBYTECODE='1',GIT_OPTIONAL_LOCKS='0')
    try:
        skill=scratch/'.agents/skills/verify-veyra'
        shutil.copytree(o.BIN.parent,skill,ignore=shutil.ignore_patterns('__pycache__'))
        for plant in PLANTS:
            case=folder/plant.name;case.mkdir()
            path,before,after=plant_copy(skill,plant)
            (case/'plant.diff').write_text(''.join(difflib.unified_diff(before.decode().splitlines(True),after.decode().splitlines(True),fromfile=plant.file,tofile=plant.file)))
            command=[sys.executable,str(skill/'bin/run'),'--check-only']
            try:
                red=subprocess.run(command,cwd=scratch,env=env,capture_output=True,text=True,timeout=45)
            finally: path.write_bytes(before)
            (case/'red.txt').write_text(red.stdout+red.stderr)
            green=subprocess.run(command,cwd=scratch,env=env,capture_output=True,text=True,timeout=45)
            (case/'green.txt').write_text(green.stdout+green.stderr)
            named='AssertionError: GUARD['+plant.guard+']:' in red.stdout+red.stderr
            row={'plant':plant.name,'guard':plant.guard,'file':plant.file,'function':plant.function,
                'red_exit':red.returncode,'red_named_guard':named,'green_exit':green.returncode,
                'before_sha256':hashlib.sha256(before).hexdigest(),'planted_sha256':hashlib.sha256(after).hexdigest(),
                'restored_sha256':o.sha(path),'red_sha256':o.sha(case/'red.txt'),'green_sha256':o.sha(case/'green.txt')}
            result['cases'].append(row);o.write(folder/'matrix.json',result)
            assert red.returncode!=0 and named, plant.name+' survived or failed without naming '+plant.guard
            assert green.returncode==0 and path.read_bytes()==before, plant.name+' did not restore green'
            print('PASS '+plant.name+' GUARD['+plant.guard+'] red='+str(red.returncode)+' green=0',flush=True)
        result.update(passed=True,plants_red=len(PLANTS),restores_green=len(PLANTS))
    finally:
        o.write(folder/'matrix.json',result)
        print('matrix receipt: '+str(folder/'matrix.json'),flush=True)
        # The default receipt remains available; copied executable inputs are always discarded.
        shutil.rmtree(scratch/'.agents',ignore_errors=True)
        if evidence: shutil.rmtree(scratch)
    return 0
