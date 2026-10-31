"""Permanent planted-fault contract: each exact AST plant must make check-only name its guard.

Only temporary skill copies change. Each case runs the complete public check-only entry twice:
planted red, then byte-for-byte restored green. No test case, receipt or guard is patched in the
running checkout. AST selectors fail closed if a call site is removed, duplicated or renamed.
"""
import ast, difflib, hashlib, json, os, pathlib, shutil, subprocess, sys, tempfile
from dataclasses import dataclass
import orchestration as o
from heredocs import HEREDOC

@dataclass(frozen=True)
class Plant:
    name: str
    guard: str
    file: str
    function: str
    operation: str
    selector: str
    replacement: str = ''
    section: str = ''

# A selector is the precise expression/statement AST inside one named runtime function.
# Repeated historical probes remain separate plants where their call sites differ.
PLANTS = [
    Plant('code-copy-ready-record','code-copy-safety','features/chat-send-stream.md','compare_copy','call-nth','save#1','pass',section='Code blocks'),
    Plant('code-copy-ready-owner','code-copy-safety','features/chat-send-stream.md','compare_copy','if-test','ready.exists()','False',section='Code blocks'),
    Plant('toast-copy-ready-record','toast-copy-safety','features/toasts.md','','call-nth','save#1','pass',section='Driving it with veyra ctl'),
    Plant('toast-copy-ready-owner','toast-copy-safety','features/toasts.md','','if-test','ready.exists()','False',section='Driving it with veyra ctl'),
    Plant('code-copy-before-copy','code-copy-safety','features/chat-send-stream.md','compare_copy','if-test','helper.poll() is None','True',section='Code blocks'),
    Plant('code-copy-reason','code-copy-safety','features/chat-send-stream.md','compare_copy','after-assignment','reason',"reason='Pasteboard comparison failed'",section='Code blocks'),
    Plant('code-copy-record','code-copy-safety','features/chat-send-stream.md','compare_copy','call-nth','save#3','pass',section='Code blocks'),
    Plant('code-copy-fatal','code-copy-safety','features/chat-send-stream.md','compare_copy','if-test','helper.returncode==2','False',section='Code blocks'),
    Plant('toast-copy-before-copy','toast-copy-safety','features/toasts.md','','if-test','helper.poll() is None','True',section='Driving it with veyra ctl'),
    Plant('toast-copy-reason','toast-copy-safety','features/toasts.md','','after-assignment','reason',"reason='Pasteboard comparison failed'",section='Driving it with veyra ctl'),
    Plant('toast-copy-record','toast-copy-safety','features/toasts.md','','call-nth','save#3','pass',section='Driving it with veyra ctl'),
    Plant('toast-copy-fatal','toast-copy-safety','features/toasts.md','','if-test','helper.returncode==2','False',section='Driving it with veyra ctl'),
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
    Plant('child-spawn-signal-mask','child-signals','bin/orchestration.py','start_owned','expression','signal.pthread_sigmask(signal.SIG_BLOCK,SIGNALS)','signal.pthread_sigmask(signal.SIG_BLOCK,[])'),
]

PLANTS += [
    Plant('backdrop-kill-ownership','backdrop-kill','bin/driver.py','stop_owned_backdrop','remove-call','verify_owned_backdrop'),
    Plant('child-own-session','child-signals','bin/orchestration.py','start_owned','keyword','Popen:start_new_session','False'),
    Plant('mutant-child-term','mutant-restore','bin/orchestration.py','owned_child','call-statement','send_signal',"if not str(args[0]).endswith('/bin/run'): child.send_signal(signal.SIGTERM)"),
    Plant('idle-after-state','idle-samples','bin/frame_scene.py','measure_idle','expression','assert_idle_state(after,focus,part,scene)','None'),
    Plant('candidate-part-forward','probe-part','bin/frame_guard.py','run_guard.one','expression',"args.part if side=='candidate' else '6c'","'6c'"),
    Plant('backdrop-reopen-result','backdrop-activation','bin/driver.py','reopen_owned_backdrop','expression','result','None'),
    Plant('gate-self-test-bounded','launch-hid','bin/driver.py','gate','remove-assert',"time.time() < deadline"),
    Plant('runner-completion-verdict','verdict-complete','bin/run','verdict','if-test',"record.get('all_passed') and not record.get('failed')",'True'),
    Plant('probe-child-term','probe-term','bin/orchestration.py','owned_child','remove-call','send_signal'),
    Plant('terminal-identity','end-identity','bin/run','verdict','if-test',"not record.get('identity_unchanged')",'False'),
    Plant('discard-failure','discard-stop','bin/freshness.py','retry_capture.remove','remove-raise',''),
    Plant('yabai-unowned-label','yabai-labels','bin/driver.py','cleanup_window_rules','if-test',"not re.fullmatch('veyra-verify-'+run+r'-[0-9a-f]{32}',label)",'False'),
    Plant('mutant-source-restore','mutant-restore','bin/mutants.py','installed','expression','target.write_bytes(original)','None'),
    Plant('mutant-binary-restore','mutant-restore','bin/mutants.py','installed','expression',"shutil.copy2(saved/name,o.REPO/'target'/name)",'None'),
    Plant('cpu-five-upper','cpu-rules','bin/frame_guard.py','judge','expression','4 if len(ratios)==5 else 8','5 if len(ratios)==5 else 8'),
    Plant('cpu-five-lower','cpu-rules','bin/frame_guard.py','judge','expression','4 if len(ratios)==5 else 8','3 if len(ratios)==5 else 8'),
    Plant('cpu-ten-upper','cpu-rules','bin/frame_guard.py','judge','expression','4 if len(ratios)==5 else 8','4 if len(ratios)==5 else 9'),
    Plant('cpu-ten-lower','cpu-rules','bin/frame_guard.py','judge','expression','4 if len(ratios)==5 else 8','4 if len(ratios)==5 else 7'),
    Plant('cpu-median-threshold','cpu-rules','bin/frame_guard.py','judge','expression','1.10','1.11'),
    Plant('cpu-extend','cpu-rules','bin/frame_guard.py','judge','expression',"'extend'","'pass'"),
    Plant('stale-repaint','stale-repaint','bin/freshness.py','record','remove-assert',"repaint > previous['repaints']"),
    Plant('cadence-budget','cadence','bin/frame_guard.py','run_guard.one','remove-assert',"row['batches_per_second']>=30 and row['largest_gap_ms']<=100"),
    Plant('cadence-rate','cadence','bin/frame_guard.py','run_guard.one','expression',"row['batches_per_second']>=30",'True'),
    Plant('cadence-gap','cadence','bin/frame_guard.py','run_guard.one','expression',"row['largest_gap_ms']<=100",'True'),
    Plant('normal-motion','native-motion','bin/frame_native.py','working','expression',"motion=='reduced'",'True'),
    Plant('frame-recording-off','native-recording','bin/frame_native.py','start','keyword','start:recording','True'),
    Plant('native-read-pid','native-reads','bin/frame_native.py','read','remove-assert',"state['pid']==self.pid"),
    Plant('native-read-group','native-reads','bin/frame_native.py','read','remove-assert',"os.getpgid(self.pid)==self.pid==int((self.proof/'pgid').read_text())"),
    Plant('native-read-lock','native-reads','bin/frame_native.py','read','remove-assert',"(self.data/'veyra.lock').read_text().strip()==str(self.pid)"),
    Plant('native-read-revision','native-reads','bin/frame_native.py','read','remove-assert',"state['build_rev']==self.result['expected_rev']"),
    Plant('native-read-data','native-reads','bin/frame_native.py','read','remove-assert',"state['data_dir']==str(self.data)"),
]

PLANTS += [
    Plant('measured-pair-rectangle','measured-pair','bin/driver.py','measured_pair','expression','take(label,rectangle)','(rectangle,take(label,rectangle)[1])'),
    Plant('measured-pair-retake','measured-pair','bin/driver.py','measured_pair','if-test','placed','False'),
    Plant('guard-stops-smoke','guard-stops-smoke','bin/frame_guard.py','judge','if-test','statistics.median(ratios)>1.10','False'),
    Plant('tier-output-hash','tier-complete','bin/orchestration.py','require_tier','remove-assert',"sha(output) == row['sha256']"),
    Plant('interrupted-verdict','interrupt-verdict','bin/run','verdict','if-test',"record['interrupted']",'False'),
    Plant('proof-verdict','proof-verdict','bin/run','verdict','if-test',"not all(record.get('evidence',{'run':False}).values())",'False'),
    Plant('collect-all-verdict','collect-all','bin/run','verdict','if-test',"any(row['kind']=='failed' for row in rows)",'False'),
    Plant('yabai-rule-add','yabai-errors','bin/driver.py','add_window_rule','remove-assert',"result.returncode==0"),
    Plant('yabai-rule-remove','yabai-removal','bin/driver.py','cleanup_window_rules','if-test','result.returncode and not missing','False'),
    Plant('fresh-mutant-awaiting','mutant-fresh-read','bin/mutants.py','run_mutants','if-test',"judge=='read'",'False'),
    Plant('duplicate-mutant-read','mutant-read-guards','bin/mutants.py','record_mutant_read','remove-assert',"row['state']=='awaiting lead read'"),
    Plant('go-identity','go-variants','bin/orchestration.py','require_go','expression',"go.get('identity') == expected",'True'),
    Plant('go-issued','go-variants','bin/orchestration.py','require_go','expression',"go['issued_epoch'] <= time.time() < go['cutoff_epoch']", "time.time() < go['cutoff_epoch']"),
    Plant('go-cutoff','go-variants','bin/orchestration.py','require_go','expression',"go['issued_epoch'] <= time.time() < go['cutoff_epoch']", "go['issued_epoch'] <= time.time()"),
    Plant('go-deadline','go-variants','bin/orchestration.py','require_go','remove-assert',"deadline <= go['cutoff_epoch']"),
    Plant('go-pause','go-variants','bin/orchestration.py','require_go','expression',"go.get('arch_plan_paused') is True",'True'),
    Plant('cover-mid-measurement','cover-extra','bin/frame_guard.py','measure_set','expression',"error.row.get('measurement_started') is False",'True'),
    Plant('idle-palette-closed','idle-variants','bin/frame_scene.py','idle_variant','remove-assert',"palette['open'] is False"),
    Plant('idle-resting-toast','idle-variants','bin/frame_scene.py','idle_variant','remove-assert',"toasts and all(isinstance(toast,dict) and toast.get('kind') and toast.get('text') for toast in toasts)"),
    Plant('backdrop-activation-checks','backdrop-kill','bin/driver.py','verify_owned_backdrop','body','','return pathlib.Path(executable).resolve()'),
    Plant('backdrop-before-reopen','backdrop-activation','bin/driver.py','reopen_owned_backdrop','call-nth','verify_owned_backdrop#1','pass'),
    Plant('backdrop-after-reopen','backdrop-activation','bin/driver.py','reopen_owned_backdrop','call-nth','verify_owned_backdrop#2','pass'),
    Plant('backdrop-sigterm','native-recording','bin/driver.py','stop_owned_backdrop','expression','(signal.SIGTERM,signal.SIGKILL)','(signal.SIGKILL,)'),
    Plant('backdrop-sigkill','backdrop-signals','bin/driver.py','stop_owned_backdrop','expression','(signal.SIGTERM,signal.SIGKILL)','(signal.SIGTERM,)'),
    Plant('backdrop-escalation-ownership','backdrop-escalation','bin/driver.py','stop_owned_backdrop','call-statement','verify_owned_backdrop','if sig==signal.SIGTERM: verify_owned_backdrop(pid,executable)'),
    Plant('backdrop-dead-refusal','backdrop-dead','bin/driver.py','stop_owned_backdrop','first-if','_backdrop_executable(pid) is None','False'),
    Plant('probe-admission-cause','probe-admission','bin/frame_guard.py','run_guard.one','if-test','not path.is_file()','False'),
    Plant('fixture-skip','fixture-skipped','bin/pages.py','self_test','expression',"'SKIPPED select-pages real commit fixture fd4adea..04ac4f3: commit unavailable'", "'PASS select-pages real commit fixture fd4adea..04ac4f3: commit unavailable'"),
    Plant('prepare-glass-selection','prepare-selection','bin/run','prepare','after-assignment','glass','glass=False'),
    Plant('prepare-quiet','prepare-quiet','bin/run','prepare','remove-assert','not args.real_quiet or args.gate'),
    Plant('plant-list-shrink','plant-registry','bin/guard_matrix.py','','list-remove',"'awaiting-read-batch'"),
    Plant('plant-list-rename','plant-registry','bin/guard_matrix.py','','expression',"'awaiting-read-batch'","'awaiting-read-batch-renamed'"),
    Plant('capture-discard-all','capture-old-image','bin/freshness.py','retry_capture','handler-call','Exception:remove','pass'),
    Plant('privacy-no-retry','backdrop-missing','bin/freshness.py','retry_capture','handler-raise','SafetyStop','continue'),
]

PLANTS += [
    Plant('smoke-global-drain','smoke-scoped-drain','bin/smoke.py','drained','expression',
        '''all(d.sql("select live_pgid is null from threads where id='"+thread+"'")=='1' for thread in threads)''',
        '''d.sql('select count(*) from threads where live_pgid is not null')=='0' '''),
    Plant('smoke-child-still-live','smoke-scoped-drain','bin/smoke.py','drained','body','','return True'),
    Plant('smoke-parent-state','smoke-scoped-drain','bin/smoke.py','family_stopped','expression',"parent['status']=='Held'",'True'),
    Plant('smoke-parent-hold','smoke-scoped-drain','bin/smoke.py','family_stopped','expression',"parent['hold']=='stopped'",'True'),
    Plant('smoke-child-outcome','smoke-scoped-drain','bin/smoke.py','family_stopped','expression',"'Interrupted' in outcome",'True'),
    Plant('smoke-pending-wake','smoke-scoped-drain','bin/smoke.py','family_stopped','expression',"settled.get('delivery',{}).get('Pending',{}).get('wake')",'True'),
    Plant('failure-no-hid','failure-hid','bin/run','attempt.execute','expression',"observe(proof,title,seen.get('kind','section'),'section-exit')",'{}'),
    Plant('failure-poll-no-hid','failure-hid','bin/driver.py','poll','remove-call','observe_failure'),
    Plant('failure-reused-name','failure-captures','bin/run','failure_capture','after-assignment','label',"label='failure'"),
    Plant('native-skipped-quiet','quiet-native','bin/run','run_native','remove-call','admit_native'),
]

PLANTS += [
    Plant('smoke-scope-omission','smoke-scoped-drain','bin/smoke.py','drive','expression',
        '[family[1]] if family else threads','[family[1]] if family else []'),
    Plant('failure-capture-ordinal','capture-ordinal','bin/run','failure_capture','expression',
        "'failure-'+re.sub(r'[^a-z0-9]+','-',section.lower()).strip('-')+'-'+str(number)",
        "'failure-'+re.sub(r'[^a-z0-9]+','-',section.lower()).strip('-')+'-1'"),
    Plant('quiet-limit-six','quiet-limit','bin/quiet_machine.py','','expression','5.0','6.0'),
    Plant('quiet-limit-nine','quiet-limit','bin/quiet_machine.py','','expression','5.0','9.0'),
    Plant('unknown-hid-capture','unknown-hid','bin/run','attempt','expression',"seen.get('human_input') is False","not seen.get('human_input')"),
    Plant('unknown-hid-assumed-safe','unknown-hid','bin/failure_observation.py','observe','keyword','update:human_input','False'),
    Plant('quiet-helper-cutoff','quiet-cutoff','bin/orchestration.py','admit_native','keyword','wait_for_quiet:deadline','None'),
    Plant('quiet-wait-budget','quiet-cutoff','bin/quiet_machine.py','wait_for_quiet','expression',
        'min(TIMEOUT_SECONDS,max(0,deadline-time.time()))','TIMEOUT_SECONDS'),
    Plant('quiet-sleep-cutoff','quiet-cutoff','bin/quiet_machine.py','wait_for_quiet','expression',
        'budget - (monotonic() - started)','TIMEOUT_SECONDS - (monotonic() - started)'),
    Plant('quiet-read-cutoff','quiet-cutoff','bin/quiet_machine.py','wait_for_quiet','expression',
        'min(5, budget - (sample_started - started))','5'),
    Plant('quiet-runner-cutoff','quiet-cutoff','bin/run','run_native','expression',
        "orchestration.admit_native(args.evidence/'quiet-machine.json',args.deadline,['bin/run',*('/'.join(key) for key in plan)])",
        "orchestration.admit_native(args.evidence/'quiet-machine.json',time.time()+1200,['bin/run',*('/'.join(key) for key in plan)])"),
    Plant('quiet-guard-cutoff','quiet-cutoff','bin/frame_guard.py','run_guard.one','expression',
        "o.admit_native(root/(label+'-quiet.json'),args.deadline,command)",
        "o.admit_native(root/(label+'-quiet.json'),time.time()+1200,command)"),
    Plant('quiet-guard-without-deadline','quiet-cutoff','bin/frame_guard.py','run_guard.one','expression',
        "o.admit_native(root/(label+'-quiet.json'),args.deadline,command)",
        "quiet_machine.wait_for_quiet(root/(label+'-quiet.json'),command)"),
    Plant('quiet-probe-cutoff','quiet-cutoff','bin/frame_probe.py','main','expression',
        "o.admit_native(output/'quiet-machine.json',float(os.environ['VEYRA_VERIFY_DEADLINE']),['frame-probe',args.label])",
        "o.admit_native(output/'quiet-machine.json',float(os.environ['VEYRA_VERIFY_DEADLINE'])+1200,['frame-probe',args.label])"),
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
    target=tree if plant.file=='SKILL.md' or not plant.function else function(tree,plant.function)
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
        found=[n for n in ast.walk(target) if isinstance(n,ast.Assign)
            and any(isinstance(t,ast.Name) and t.id==plant.selector for t in n.targets)]
        assert len(found)==1
        node=found[0];edit(node,ast.get_source_segment(source,node)+'\n'+' '*node.col_offset+plant.replacement)
    elif plant.operation=='call-nth':
        name,index=plant.selector.split('#')
        calls=sorted([n for n in ast.walk(target) if isinstance(n,ast.Expr) and isinstance(n.value,ast.Call)
            and (getattr(n.value.func,'id',None) or getattr(n.value.func,'attr',None))==name],key=lambda n:n.lineno)
        edit(calls[int(index)-1],plant.replacement)
    elif plant.operation=='first-if':
        found=sorted([n for n in ast.walk(target) if isinstance(n,ast.If) and same(n.test,plant.selector)],key=lambda n:n.lineno)
        assert found;edit(found[0].test,plant.replacement)

    else:
        for node in nodes:
            if plant.operation=='list-remove' and isinstance(node,ast.Call) and getattr(node.func,'id',None)=='Plant' and node.args and same(node.args[0],plant.selector):
                assert lines[node.end_lineno-1][node.end_col_offset:].strip()==','
                edits.append((offsets[node.lineno-1],offsets[node.end_lineno],''))
            elif plant.operation=='expression' and isinstance(node,ast.expr) and same(node,plant.selector):
                edit(node,plant.replacement)
            elif plant.operation in ['remove-call','remove-calls','call-statement'] and isinstance(node,ast.Expr) and isinstance(node.value,ast.Call):
                call=node.value.func;name=getattr(call,'id',None) or getattr(call,'attr',None)
                if name in plant.selector.split(','): edit(node,plant.replacement or 'pass')
            elif plant.operation=='if-test' and isinstance(node,ast.If) and same(node.test,plant.selector): edit(node.test,plant.replacement)
            elif plant.operation=='remove-if' and isinstance(node,ast.If) and same(node.test,plant.selector): edit(node,'pass')
            elif plant.operation=='remove-assert' and isinstance(node,ast.Assert) and same(node.test,plant.selector): edit(node,'pass')
            elif plant.operation in ['handler-call','handler-raise'] and isinstance(node,ast.ExceptHandler) and getattr(node.type,'id',None)==plant.selector.split(':')[0]:
                for statement in node.body:
                    if plant.operation=='handler-raise' and isinstance(statement,ast.Raise): edit(statement,plant.replacement)
                    if plant.operation=='handler-call' and isinstance(statement,ast.Expr) and isinstance(statement.value,ast.Call) and getattr(statement.value.func,'id',None)==plant.selector.split(':')[1]: edit(statement,plant.replacement)
            elif plant.operation=='remove-raise' and isinstance(node,ast.Raise): edit(node,'pass')
            elif plant.operation=='keyword' and isinstance(node,ast.Call) and getattr(node.func,'attr',None)==plant.selector.split(':')[0]:
                for key in node.keywords:
                    if key.arg==plant.selector.split(':')[1]: edit(key.value,plant.replacement)
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
    if plant.file=='SKILL.md' or plant.section:
        import re
        if plant.file=='SKILL.md':
            heading=re.search(r'^## '+plant.function+r'\n',source,re.M);assert heading
            end=source.find('\n## ',heading.end());end=len(source) if end<0 else end
            section=source[heading.end():end]
            heredoc=next((h for h in HEREDOC.finditer(section) if h['end']=='PY'),None);assert heredoc
        else:
            heading=re.search(r'^##+ '+re.escape(plant.section)+r'\n',source,re.M);assert heading
            next_heading=re.search(r'^##+ ',source[heading.end():],re.M)
            end=heading.end()+next_heading.start() if next_heading else len(source)
            section=source[heading.end():end]
            heredoc=next(HEREDOC.finditer(section),None);assert heredoc
        body=apply(heredoc['body'],plant)
        start=heading.end()+heredoc.start('body');stop=heading.end()+heredoc.end('body')
        after=source[:start]+body.rstrip('\n')+source[stop:]
    else: after=apply(source,plant)
    path.write_text(after)
    return path,before,after.encode()

def run_matrix(evidence=None):
    """Each independent copy runs complete check-only red, then an exact restore and complete green."""
    from concurrent.futures import ThreadPoolExecutor
    check_registry()
    scratch=pathlib.Path(tempfile.mkdtemp(prefix='veyra-mverify-matrix-',dir='/tmp'))
    folder=pathlib.Path(evidence).absolute() if evidence else scratch/'receipt'
    folder.mkdir(parents=True,exist_ok=False)
    result={'schema':1,'passed':False,'native_runs':0,'git_writes':0,'cases':[],
        'manifest_sha256':o.sha(o.BIN/'guard-plants.json'),
        'source_sha256':{str(p.relative_to(o.BIN.parent)):o.sha(p) for p in sorted(o.BIN.parent.rglob('*'))
            if p.is_file() and '__pycache__' not in p.parts}}
    snapshot=scratch/'inputs'
    shutil.copytree(o.BIN.parent,snapshot,ignore=shutil.ignore_patterns('__pycache__'))
    assert {str(p.relative_to(snapshot)):o.sha(p) for p in snapshot.rglob('*') if p.is_file()}==result['source_sha256'], 'matrix inputs changed while copying'
    env=dict(os.environ,PYTHONDONTWRITEBYTECODE='1',GIT_OPTIONAL_LOCKS='0')
    def check(plant):
        case=folder/plant.name;case.mkdir()
        copy=scratch/plant.name
        skill=copy/'.agents/skills/verify-veyra'
        shutil.copytree(snapshot,skill)
        try:
            path,before,after=plant_copy(skill,plant)
            (case/'plant.diff').write_text(''.join(difflib.unified_diff(before.decode().splitlines(True),after.decode().splitlines(True),fromfile=plant.file,tofile=plant.file)))
            command=[sys.executable,str(skill/'bin/run'),'--check-only']
            try: red=subprocess.run(command,cwd=copy,env=env,capture_output=True,text=True,timeout=90)
            finally: path.write_bytes(before)
            (case/'red.txt').write_text(red.stdout+red.stderr)
            green=subprocess.run(command,cwd=copy,env=env,capture_output=True,text=True,timeout=90)
            (case/'green.txt').write_text(green.stdout+green.stderr)
            named='AssertionError: GUARD['+plant.guard+']:' in red.stdout+red.stderr
            return {'plant':plant.name,'guard':plant.guard,'file':plant.file,'function':plant.function,
                'red_exit':red.returncode,'red_named_guard':named,'green_exit':green.returncode,
                'before_sha256':hashlib.sha256(before).hexdigest(),'planted_sha256':hashlib.sha256(after).hexdigest(),
                'restored_sha256':o.sha(path),'red_sha256':o.sha(case/'red.txt'),'green_sha256':o.sha(case/'green.txt')}
        finally: shutil.rmtree(copy,ignore_errors=True)
    try:
        # Copies, mutations and receipts are disjoint. Each worker's child has its own OS fixtures.
        with ThreadPoolExecutor(max_workers=4) as pool:
            for row in pool.map(check,PLANTS):
                result['cases'].append(row);o.write(folder/'matrix.json',result)
                passed=row['red_exit']!=0 and row['red_named_guard'] and row['green_exit']==0 and row['restored_sha256']==row['before_sha256']
                print(('PASS ' if passed else 'FAIL ')+row['plant']+' GUARD['+row['guard']+'] red='+str(row['red_exit'])+' green=0',flush=True)
        bad=[r['plant'] for r in result['cases'] if not(r['red_exit']!=0 and r['red_named_guard'] and r['green_exit']==0 and r['restored_sha256']==r['before_sha256'])]
        assert not bad, 'matrix failed plants: '+', '.join(bad)
        assert {str(p.relative_to(o.BIN.parent)):o.sha(p) for p in o.BIN.parent.rglob('*') if p.is_file() and '__pycache__' not in p.parts}==result['source_sha256'], 'matrix source changed during checks'
        result.update(passed=True,plants_red=len(PLANTS),restores_green=len(PLANTS))
    finally:
        o.write(folder/'matrix.json',result)
        print('matrix receipt: '+str(folder/'matrix.json'),flush=True)
        shutil.rmtree(snapshot,ignore_errors=True)
        if evidence: shutil.rmtree(scratch,ignore_errors=True)
    return 0


def check_registry():
    expected=json.loads((o.BIN/'guard-plants.json').read_text())
    names=[p.name for p in PLANTS]
    assert names==expected['plants'] and len(names)==len(set(names)), 'GUARD[plant-registry]: plant names differ from committed manifest'
    from self_tests import CASES
    covered={p.guard for p in PLANTS}
    positives=set(expected['positive_cases'])
    cases={name for name,case in CASES}
    assert covered <= cases and positives <= cases and cases <= covered|positives, 'GUARD[plant-registry]: stop case lacks a plant or explicit positive classification'
