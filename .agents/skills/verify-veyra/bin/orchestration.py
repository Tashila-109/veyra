"""Identity and offline Tier S admission shared by every native mode."""
import contextlib, hashlib, json, os, pathlib, shutil, signal, subprocess, time

BIN = pathlib.Path(__file__).resolve().parent
REPO = BIN.parents[3]
BINARY_NAMES = ['debug/veyra','debug/veyra-fake-cli','release/veyra']
TOOLS = ['capture-window','winid','backdrop','pasteboard','visible-frame','front-app']
SIGNALS = {signal.SIGTERM,signal.SIGHUP,signal.SIGINT}
COMMANDS = {
    'build': ['cargo','build','--workspace'],
    'tests': ['cargo','test','--workspace'],
    'clippy': ['cargo','clippy','--all-targets','--','-D','warnings'],
    'fmt': ['cargo','fmt','--check'],
    'release-no-run': ['cargo','test','--workspace','--release','--no-run'],
    'check-only': [str(BIN/'run'),'--check-only'],
    'check-guards': [str(BIN/'run'),'--check-guards'],
    'release-build': ['cargo','build','--release','-p','veyra'],
}

class SafetyStop(RuntimeError):
    """Ownership, cleanup, identity, restore, or unenforceable privacy failure."""

def admit_native(path, deadline, invocation):
    """The shared native admission call; callers revalidate their frozen inputs after waiting."""
    import quiet_machine
    receipt=quiet_machine.wait_for_quiet(path,invocation,deadline=deadline)
    if time.time()>=deadline: raise quiet_machine.QuietMachineHold('GO cutoff reached; no native launch')
    return receipt

def start_owned(args, **options):
    """Block terminating signals across spawn; the caller's try restores the returned mask first."""
    previous = signal.pthread_sigmask(signal.SIG_BLOCK,SIGNALS)
    try:
        return subprocess.Popen(args,start_new_session=True,
            preexec_fn=lambda: signal.pthread_sigmask(signal.SIG_UNBLOCK,SIGNALS),**options),previous
    except BaseException:
        signal.pthread_sigmask(signal.SIG_SETMASK,previous)
        raise

@contextlib.contextmanager
def owned_child(args, **options):
    """Supervised native workers receive TERM and finish their own Cleanup on every interrupted wait."""
    child,previous = start_owned(args,**options)
    try:
        signal.pthread_sigmask(signal.SIG_SETMASK,previous)
        yield child
    except BaseException:
        if child.poll() is None:
            child.send_signal(signal.SIGTERM)
            child.wait(timeout=120)
        raise

def helper_paths(tools):
    tools = pathlib.Path(tools)
    resolved = {name:tools/name for name in TOOLS}
    for name,path in resolved.items():
        assert path.is_file(), 'admitted helper missing: '+name
    return resolved

def tier_tools(receipt):
    """The sole compiled-tools folder used by admission and every probe environment."""
    folder = pathlib.Path(receipt).resolve().parent/'tools'
    helper_paths(folder)
    return folder

def sha(path):
    digest = hashlib.sha256()
    with pathlib.Path(path).open('rb') as source:
        while block := source.read(1024*1024): digest.update(block)
    return digest.hexdigest()

def write(path, value):
    path = pathlib.Path(path)
    path.write_text(json.dumps(value,indent=2)+'\n')

def source_identity(repo=None):
    repo = REPO if repo is None else repo
    def git(*args):
        return subprocess.check_output(['git','-c','core.quotepath=off',*args],cwd=repo,
            env=dict(os.environ,GIT_OPTIONAL_LOCKS='0'))
    untracked = git('ls-files','--others','--exclude-standard','-z').decode().split('\0')
    return {'head':git('rev-parse','HEAD').decode().strip(),
        'diff_sha256':hashlib.sha256(git('diff','--binary','HEAD')).hexdigest(),
        'untracked_sha256':{name:sha(pathlib.Path(repo)/name) for name in untracked if name}}

def code_identity(repo=None):
    repo = REPO if repo is None else repo
    return dict(source_identity(repo),binaries={name:sha(pathlib.Path(repo)/'target'/name) for name in BINARY_NAMES})

def require_tier(path, expected, now=None):
    """Reject missing, partial, stale, forged-output or different-input receipts before any app process starts."""
    path = pathlib.Path(path).resolve()
    receipt = json.loads(path.read_text())
    now = time.time() if now is None else now
    assert receipt.get('schema') == 1 and receipt.get('passed') is True, 'incomplete Tier S receipt'
    assert receipt.get('identity') == expected, 'Tier S identity differs (HEAD, diff, untracked helpers or binaries)'
    assert 0 <= now-receipt['finished_epoch'] <= 24*3600, 'Tier S receipt is not fresh (24 h)'
    assert receipt['started_epoch'] <= receipt['finished_epoch'], 'Tier S clock order'
    rows = receipt['commands']
    assert [row['name'] for row in rows] == list(COMMANDS), 'Tier S command set is incomplete'
    for row in rows:
        assert row['argv'] == COMMANDS[row['name']] and row['exit'] == 0, 'Tier S failed command'
        output = path.parent/row['output']
        assert output.is_relative_to(path.parent) and output.is_file(), 'Tier S output missing'
        assert sha(output) == row['sha256'], 'Tier S output changed'
    tools = receipt['tools']
    assert set(tools) == set(TOOLS), 'Tier S compiled helper set is incomplete'
    for name,helper in helper_paths(tier_tools(path)).items():
        assert sha(helper) == tools[name], 'Tier S helper binary changed'
    return receipt

def require_go(path, expected, deadline, guard=False):
    go = json.loads(pathlib.Path(path).read_text())
    assert go.get('launch_authorized') is True and go.get('identity') == expected, 'fresh GO for this identity required'
    assert go['issued_epoch'] <= time.time() < go['cutoff_epoch'], 'away window is not armed'
    assert deadline <= go['cutoff_epoch'], 'deadline exceeds GO cutoff'
    if guard:
        assert go.get('frame_cost_guard_authorized') is True and go.get('arch_plan_paused') is True, 'guard GO/pause required'
    return go

def tier_s(folder):
    """One offline receipt, with compiled capture helpers; no window, ctl command or capture."""
    folder = pathlib.Path(folder).resolve()
    folder.mkdir(parents=True,exist_ok=False)
    initial = source_identity()
    receipt = {'schema':1,'started_epoch':time.time(),'passed':False,'commands':[]}
    env = dict(os.environ,PYTHONDONTWRITEBYTECODE='1',PATH='/opt/homebrew/opt/rustup/bin:'+os.environ['PATH'])
    env.pop('CARGO_TARGET_DIR',None)
    if shutil.which('sccache',path=env['PATH']): env['RUSTC_WRAPPER'] = 'sccache'
    try:
        for name,argv in COMMANDS.items():
            output = folder/(name+'.txt')
            with output.open('w') as log:
                result = subprocess.run(argv,cwd=REPO,env=env,stdout=log,stderr=subprocess.STDOUT)
            receipt['commands'].append({'name':name,'argv':argv,'exit':result.returncode,
                'output':output.name,'sha256':sha(output)})
            assert source_identity() == initial, 'Tier S source identity changed'
            assert result.returncode == 0, 'Tier S '+name+' failed'
            if name == 'tests':
                text = output.read_text()
                import re
                for test in ['platform_boundary','fixtures_hold_no_secret_shapes','verify_map_names',
                        'semantic_placeholders_and_input_clipping','composer_accent_and_unscrolled_header_rule',
                        'semantic_approval_projection_and_control_bounds','approval_command_plates_and_decision_controls',
                        'option_cursor_paints_only_the_current_row','settings_card_tracks_and_lit_thumbs',
                        'status_marks_have_distinct_raster_shapes_and_working_steps','family_status_marks_follow_child_status',
                        'headless_stream_render_proxy']:
                    assert re.search(r'^test .*\b'+test+r' \.\.\. ok$',text,re.M), 'missing named test '+test
        (folder/'tools').mkdir()
        for name in TOOLS:
            with (folder/(name+'-build.txt')).open('w') as log:
                subprocess.run(['swiftc','-O',str(BIN/(name+'.swift')),'-o',str(folder/'tools'/name)],
                    cwd=REPO,env=env,stdout=log,stderr=subprocess.STDOUT,check=True)
        debug = subprocess.check_output(['strings',str(REPO/'target/debug/veyra')])
        release = subprocess.check_output(['strings',str(REPO/'target/release/veyra')])
        for marker in [b'control.sock',b'DebugDoubleClickShellDrawer']:
            assert marker in debug and marker not in release, 'release driver marker leak'
        assert source_identity() == initial, 'Tier S source changed during helper build'
        receipt.update(identity=code_identity(),tools={name:sha(folder/'tools'/name) for name in TOOLS},passed=True)
    finally:
        receipt['finished_epoch'] = time.time()
        write(folder/'receipt.json',receipt)
    require_tier(folder/'receipt.json',receipt['identity'])
    return receipt

def assertion_from_transcript(path):
    """AssertionError may contain a literal tuple, a plain message, or no message at all."""
    import ast
    lines = [line.partition(':')[2].strip() for line in pathlib.Path(path).read_text().splitlines()
        if line == 'AssertionError' or line.startswith('AssertionError:')]
    if not lines: return ()
    try: result = ast.literal_eval(lines[-1])
    except (ValueError,SyntaxError): return (lines[-1],)
    return tuple(result) if isinstance(result,(tuple,list)) else (result,)
