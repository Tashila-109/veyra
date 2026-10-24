# Settings

Status: passed, 2026-10-24, 2672dee, human display-settings lane not run (fake CLI RUN 561dceec), fdb45f558f37cd52102be2bf0bf71c166f82585bd587e78a095fe1d606f5f33b
Covers: crates/veyra/src/ui/settings.rs crates/veyra/src/ui/new_thread.rs crates/engine/src/provider/** crates/engine/tests/fixtures/claude/**

## Sub-features

- `settings-background`: `veyra::SetWindowBackground`, its ⌘⌥B cycle and relaunch preserve the device choice.
- `settings-display-options`: `look` names each forced Solid flag and Reduce Motion without modifying macOS settings.
The forced Window background row selects Solid and disables both choices; its action and ⌘⌥B keep the stored choice, which returns when the flags clear.

- `settings-entry`: ⌘, and the menu action open one in-window Settings view.
- `settings-modes`: Full access is the fresh stored default; all three choices publish SettingsChanged.
- `settings-new-only`: Ask first applies to a new thread while the existing Full access thread stays unchanged.
- `settings-persistence`: SQLite stores Ask first and a relaunch keeps it, its new-thread chip and both thread configurations.
- `settings-exit`: Escape, SelectThread and SelectProject leave Settings through actions.
- `settings-focus-transition`: NewThread immediately after Escape reaches the rendered composer through both its action and key.
- `settings-header-branch`: Chat shows the run-owned branch; Settings and NewThread clear it.

## How to get to it (user POV)

Choose Settings… in the Veyra menu or press ⌘, (`veyra::OpenSettings`).
The main column shows Default mode for new threads, Ask first, Accept edits and Full access.
Choose a mode (`veyra::SetDefaultMode`) to read its permission consequence and store it.
The hint says Applies to threads you create from now on.
Escape or selecting a thread leaves Settings.
It does not reconfigure existing threads or open a second window.
⌘⌥D cycles the default mode; ⌘⌥1 opens the first active thread and ⌘⌥⇧1 selects the first project.

## Driving it with veyra ctl

Run Launch and Doctor first, in the fake lane.
This page can run alone after them, or after new-thread and chat-send-stream on the same DATA.
The initial stored default must still be Full access.
It creates a Full access thread, chooses Ask first, creates an Ask first thread and relaunches.
It turns on Launch's run-owned git shim by creating `$DATA/branch-on` and removes it at the end, so no relaunch is needed to change PATH.
Use ctl wait for both completed turns.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
if command -v sccache >/dev/null; then export RUSTC_WRAPPER=sccache; fi
set -eu
RUN=${RUN:?export the RUN that Launch printed}
PROOF=target/verify-veyra/$RUN
PYTHONPATH=.agents/skills/verify-veyra/bin python3 - <<'PY'
from driver import *
assert LANE == 'fake'
prepare()
assert sql('select default_mode from settings') == 'full_access'
for marker in ['branch-started','branch-finished']: (DATA/marker).unlink(missing_ok=True)
(DATA/'branch-on').touch()
action('settings-initial-new','NewThread')
assert ctl('settings-initial-chip','dump')['composer']['mode'] == 'full_access'
first = create('settings-full','reply with the word ok')
poll('settings-branch-started',lambda d: (DATA/'branch-started').exists())
ctl('settings-leave-before-branch','keys','secondary-,')
poll('settings-late-branch-start',lambda d: d['view']=='settings')
poll('settings-late-branch-finished',lambda d: (DATA/'branch-finished').exists())
assert ctl('settings-late-branch-after','dump')['header_branch']==''
capture('settings-late-branch')
ctl('settings-late-branch-return','keys','escape')
ctl('settings-full-done','wait',first,'Done')
poll('settings-header-chat',lambda d: d['header_branch']=='fixture-branch')
capture('settings-branch-return')
ctl('settings-open-key','keys','secondary-,')
initial = poll('settings-first',lambda d: d['view']=='settings')
assert initial['view'] == 'settings' and initial['settings']['default_mode'] == 'full_access'
assert initial['selected_thread']['config']['mode'] == 'full_access'
assert initial['header_branch'] == ''
capture('settings-full-access')
ctl('settings-invalid-mode','action','veyra::SetDefaultMode',json.dumps({'mode':'invalid'}),
    expected=1,stderr_contains='veyra ctl driver error:')
assert 'unknown variant' in ctl('settings-command-error','dump')['notice']
capture('settings-command-error')
for mode in ['ask_first','accept_edits','full_access']:
    ctl('settings-mode-key-'+mode,'keys','secondary-alt-d')
    assert poll('settings-key-'+mode+'-after',lambda d: d['settings']['default_mode']==mode)['view'] == 'settings'
for mode in ['accept_edits','full_access','ask_first']:
    action('settings-mode-'+mode,'SetDefaultMode',{'mode':mode})
    changed = poll('settings-mode-'+mode+'-after',lambda d: d['settings']['default_mode'] == mode)
    assert changed['view'] == 'settings'
    assert sql('select default_mode from settings') == mode
    assert changed['selected_thread']['config']['mode'] == 'full_access'
save('settings-ask-first',changed)
capture('settings')
ctl('settings-no-input','text','not a message',expected=1,
    stderr_contains='veyra ctl driver error: the focused element has no text input handler')
assert ctl('settings-no-input-after','dump')['composer_text'] == initial['composer_text']
ctl('settings-escape','keys','escape')
existing = poll('settings-existing',lambda d: d['view']=='chat' and d['selected_thread']['id']==first)
assert existing['view'] == 'chat' and existing['selected_thread']['id'] == first
assert existing['composer']['mode'] == 'full_access'
assert existing['settings']['default_mode'] == 'ask_first'
poll('settings-header-existing',lambda d: d['header_branch']=='fixture-branch')
action('settings-next-new','NewThread')
next_thread = poll('settings-next-chip',lambda d: d['view']=='new_thread')
assert next_thread['composer']['mode'] == 'ask_first'
assert next_thread['header_branch'] == ''
second = create('settings-ask','reply with the word ok')
new = ctl('settings-new-wait','wait',second,'Done')
save('settings-new',new)
assert new['composer']['mode'] == 'ask_first'
assert sql("select json_extract(config_json,'$.mode') from threads where id='"+first+"'") == 'full_access'
assert sql("select json_extract(config_json,'$.mode') from threads where id='"+second+"'") == 'ask_first'
poll('settings-sessions-ended',lambda d: sql('select count(*) from threads where live_pgid is not null') == '0')
restart('settings-restart')
reloaded = poll('settings-relaunch',lambda d: d['settings']['default_mode'] == 'ask_first')
assert reloaded['composer']['mode'] == 'ask_first'
assert {row['id']:row['mode'] for row in reloaded['sidebar']['rows'] if row['kind']=='thread'}[first] == 'full_access'
assert {row['id']:row['mode'] for row in reloaded['sidebar']['rows'] if row['kind']=='thread'}[second] == 'ask_first'
database('settings-persisted','ask_first')
action('settings-open-action','OpenSettings')
assert poll('settings-action-view',lambda d: d['view']=='settings')['view'] == 'settings'
action('settings-select-existing','SelectThread',{'thread':first})
selected = poll('settings-selected-existing',lambda d: d['view']=='chat' and d['selected_thread']['id']==first)
assert selected['composer']['mode'] == 'full_access'
action('settings-open-again','OpenSettings')
ctl('settings-select-first-key','keys','secondary-alt-1')
assert poll('settings-selected-first-key',lambda d: d['view']=='chat')['view'] == 'chat'
action('settings-reopen-before-project','OpenSettings')
project = next(row['id'] for row in selected['sidebar']['rows'] if row['kind']=='project')
action('settings-select-project','SelectProject',{'project':project})
assert poll('settings-project-exit',lambda d: d['view']=='new_thread')['view'] == 'new_thread'
assert ctl('settings-project-chip','dump')['composer']['mode'] == 'ask_first'
action('settings-reopen-before-project-key','OpenSettings')
ctl('settings-select-project-key','keys','secondary-alt-shift-1')
assert poll('settings-project-key-exit',lambda d: d['view']=='new_thread')['view'] == 'new_thread'
for command,argument in [('action','veyra::NewThread'),('keys','secondary-n')]:
    action('settings-transition-select-'+command,'SelectThread',{'thread':first})
    action('settings-transition-open-'+command,'OpenSettings')
    poll('settings-transition-opened-'+command,lambda d: d['view']=='settings')
    ctl('settings-transition-escape-'+command,'keys','escape')
    poll('settings-transition-chat-'+command,lambda d: d['view']=='chat')
    ctl('settings-transition-new-'+command,command,argument)
    transition = poll('settings-transition-'+command,lambda d: d['view']=='new_thread')
    assert transition['view'] == 'new_thread' and transition['selected_thread'] is None
    assert transition['composer']['mode'] == 'ask_first'
capture('settings-relaunch')
action('settings-final-open','OpenSettings')
assert poll('settings-final',lambda d: d['view']=='settings')['view'] == 'settings'
(DATA/'branch-on').unlink()
page_done('settings')
print('PASS default persistence: fresh Full access; stored Ask first; old thread Full access; '
    'new thread Ask first; relaunch keeps all three')
PY
```

### Window background and display options

The background choice is stored independently of the permission default.
This section starts only run-owned backdrop windows and always closes them, including its deliberate occluder.
Reduce Transparency, Increase Contrast and Reduce Motion are driven by `veyra::DebugDisplayOptions` without changing macOS settings.
Before each glass capture and after relaunch, the recipe asks yabai only about the run's owned pid and recorded window id, floats that window if managed, and places it at 24,60 with 960×740 bounds.
Floating a tiled window lets yabai re-tile its neighbours; the recipe changes no config, rule, space or layout setting.
An unavailable service or untracked window leaves the guards to decide.
If the guard records intersecting windows, the recipe makes one further placement on the same backdrop display, clearing all recorded bounds and keeping 960×740 whenever it fits.
The cursor stays untouched, every window still counts, and no available rectangle leaves the original guard failure intact.
The Translucent review set at the human gate needs the same owned-window placement and one-time fallback.
The measurement uses guarded region captures in every mode over the coarse backdrop and its inverse, with one completed chat, an unfocused composer and an idle fake shell drawer held in the same state.
If either half needs the placement fallback, it discards both halves and retakes the pair at that rectangle within three attempts.
It records luminance means and standard deviations, pixel differences and hashes for the sidebar, content, header, transcript, composer and drawer, with no capture outside the owned backdrop rectangle.
The whole-content statistic excludes only the two outer right-hand 12×12-point squares clipped by the native rounded corners, scaled to the capture backing scale.
Every differing pixel in the full content rectangle must lie inside those squares; the four named content regions retain exact equality without a tolerance.
The measurement uses the blur check's NumPy and Pillow dependencies.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
if command -v sccache >/dev/null; then export RUSTC_WRAPPER=sccache; fi
set -eu
RUN=${RUN:?export the RUN that Launch printed}
PROOF=target/verify-veyra/$RUN
PYTHONPATH=.agents/skills/verify-veyra/bin python3 - <<'PY_BACKGROUND'
from driver import *
import math
import numpy as np
from PIL import Image
assert LANE == 'fake'
prepare()
action('background-open-settings','OpenSettings')
action('display-options-baseline','DebugDisplayOptions',{})
action('background-solid','SetWindowBackground',{'background':'solid'})
poll('background-solid-applied',lambda d:d['look']['background']=='solid')
assert sql('select window_background from settings') == 'solid'
capture('settings-solid')
processes = []
def place_glass_window(label, rectangle=(24,60,960,740)):
    tool = shutil.which('yabai')
    pid = identity()
    window = int((PROOF/'window-id').read_text())
    def query():
        result = subprocess.run([tool,'-m','query','--windows','--window',str(window)],
            capture_output=True,text=True)
        if result.returncode: return None
        row = json.loads(result.stdout)
        assert row['id']==window and row['pid']==pid, 'placement refused a foreign window'
        return row
    if not tool:
        save(label+'-placement',{'skipped':'yabai unavailable','pid':pid,'window':window})
        return
    before = query()
    if before is None or (before.get('is-managed') is False and not before.get('is-floating')):
        save(label+'-placement',{'skipped':'owned window unavailable to yabai','pid':pid,'window':window})
        return
    if not before['is-floating']:
        subprocess.run([tool,'-m','window',str(window),'--toggle','float'],check=True,capture_output=True)
    x,y,width,height = rectangle
    for operation,value in [('--resize',f'abs:{width}:{height}'),('--move',f'abs:{x}:{y}')]:
        current = query()
        assert current is not None, 'owned window disappeared during placement'
        subprocess.run([tool,'-m','window',str(window),operation,value],check=True,capture_output=True)
    after = query()
    assert after is not None
    save(label+'-placement',{'pid':pid,'window':window,'before':before['frame'],
        'after':after['frame'],'floating':after['is-floating']})
    return after['frame']
def glass_contains(outer, inner):
    x,y,width,height = outer
    a,b,w,h = inner
    return w>0 and h>0 and x<=a and y<=b and a+w<=x+width and b+h<=y+height
def clear_glass_rectangle(background, obstacles):
    x,y,width,height = background
    for w,h in [(960,740),(960,560),(760,740),(760,560)]:
        xs = {math.ceil(x),math.ceil(x+24),math.floor(x+width-w)}
        ys = {math.ceil(y),math.ceil(y+60),math.floor(y+height-h)}
        for a,b,c,d in obstacles:
            xs.update([math.floor(a-w),math.ceil(a+c)])
            ys.update([math.floor(b-h),math.ceil(b+d)])
        for a in sorted(xs,key=lambda value:abs(value-x-24)):
            for b in sorted(ys,key=lambda value:abs(value-y-60)):
                rectangle = (a,b,w,h)
                if glass_contains(background,rectangle) and not any(
                    a<ox+ow and ox<a+w and b<oy+oh and oy<b+h for ox,oy,ow,oh in obstacles):
                    return rectangle
def retry_clear_glass(label, error, capture_again=None):
    receipt = PROOF/(label+'-region-guards.json')
    failed = json.loads(receipt.read_text())
    save(label+'-region-guards-before-placement',failed)
    ids = set()
    for reason in [str(error),*[attempt['error'] for attempt in failed['attempts']]]:
        if 'intersecting windows:' in reason:
            ids.update(int(value) for value in re.findall(r'\d+',reason.split('intersecting windows:',1)[1]))
    windows = cg_windows()
    rows = []
    def bounds(row):
        return tuple(row['kCGWindowBounds'][key] for key in ['X','Y','Width','Height'])
    for row in windows:
        if row['kCGWindowNumber'] not in ids: continue
        owner = subprocess.run(['ps','-p',str(row['kCGWindowOwnerPID']),'-o','comm='],
            capture_output=True,text=True).stdout.strip()
        rows.append({'window':row['kCGWindowNumber'],'owner_app':pathlib.Path(owner).name,
            'layer':row['kCGWindowLayer'],'bounds':row['kCGWindowBounds']})
    save(label+'-intersections',rows)
    record = {'ran':True,'repositioned':False,'intersections':rows,'guard_reason':str(error)}
    save(label+'-placement-fallback',record)
    if {row['window'] for row in rows} != ids: raise error
    pid = identity()
    window = int((PROOF/'window-id').read_text())
    current = next((row for row in windows if row['kCGWindowNumber']==window
        and row['kCGWindowOwnerPID']==pid and row['kCGWindowLayer']==0),None)
    if current is None: raise error
    backdrop = json.loads((PROOF/'backdrop-windows.json').read_text())
    owned = {int(value) for value in (PROOF/'backdrop-owned-pids').read_text().split()}
    backgrounds = [(bounds(row),display) for row in windows if row['kCGWindowOwnerPID']==backdrop['pid']
        and backdrop['pid'] in owned and row['kCGWindowNumber'] in backdrop['windows']
        and row['kCGWindowLayer']==0 and glass_contains(bounds(row),bounds(current))
        for display in display_rectangles() if glass_contains(display[1],bounds(row))]
    if not backgrounds: raise error
    background,display = backgrounds[0]
    obstacles = [tuple(row['bounds'][key] for key in ['X','Y','Width','Height']) for row in rows]
    rectangle = clear_glass_rectangle(background,obstacles)
    if rectangle is None: raise error
    record.update(rectangle=rectangle,display=display,backdrop_bounds=background)
    if place_glass_window(label+'-fallback',rectangle) is None: raise error
    record['repositioned'] = True
    save(label+'-placement-fallback',record)
    try: (capture_again or original_capture)(label)
    except Exception:
        record['outcome'] = 'failed'
        save(label+'-placement-fallback',record)
        raise
    record['outcome'] = 'passed'
    save(label+'-placement-fallback',record)
original_capture = capture
def capture(label):
    glass = ctl(label+'-look-for-placement','dump')['look']['background']=='translucent'
    if glass:
        place_glass_window(label)
    try: original_capture(label)
    except AssertionError as error:
        if not glass or 'intersecting windows:' not in str(error): raise
        retry_clear_glass(label,error)
def start_backdrop(label, inverted=False):
    output = PROOF/(label+'.stdout')
    process = subprocess.Popen([str((PROOF/'backdrop').resolve()),'--coarse',*(['--invert'] if inverted else [])],
        stdout=output.open('w'),stderr=(PROOF/(label+'.stderr')).open('w'))
    processes.append(process)
    with (PROOF/'backdrop-owned-pids').open('a') as out: out.write(str(process.pid)+'\n')
    deadline = time.monotonic()+10
    while 'backdrop ready ' not in output.read_text():
        assert process.poll() is None, 'owned backdrop exited'
        assert time.monotonic()<deadline, 'owned backdrop was not ready'
        time.sleep(.05)
    rows = [json.loads(line.removeprefix('backdrop window ')) for line in output.read_text().splitlines()
        if line.startswith('backdrop window ')]
    assert rows and all(row['pid']==process.pid for row in rows)
    return process,{'pid':process.pid,'windows':[row['window'] for row in rows]}
def stop_backdrop(process):
    if process.poll() is None: process.terminate()
    try: process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)
def measure_capture(label, rectangle):
    place_glass_window(label,rectangle)
    frame = ctl(label+'-before-frame','dump')['perf']['repaints']
    action(label+'-activate','ActivateWindow')
    poll(label+'-rendered',lambda d:d['perf']['repaints']>frame)
    time.sleep(.25)
    def take(name):
        before = ctl(name+'-before-measurement','dump')
        region_capture(PROOF/(name+'.png'),identity(),int((PROOF/'window-id').read_text()))
        after = ctl(name+'-after-measurement','dump')
        for state in [before,after]:
            for tab in state['drawer']['tabs']:
                for counter in ['frame_requests','repaints']: tab.pop(counter)
        for key in ['view','look','sidebar','selected_thread','composer_text','composer','drawer','focus']:
            assert before[key]==after[key], 'measurement state changed: '+key
    placed = False
    try: take(label)
    except AssertionError as error:
        if 'intersecting windows:' not in str(error): raise
        retry_clear_glass(label,error,take)
        placed = True
    guards = json.loads((PROOF/(label+'-region-guards.json')).read_text())
    return guards['before']['rectangle'],placed
def luminance_stats(pixels, included=None):
    values = pixels.astype(float) @ np.array([.2126,.7152,.0722])
    if included is not None: values = np.ma.array(values,mask=~included)
    return {'mean':float(values.mean()),'stdev':float(values.std()),
        'row_mean_stdev':float(values.mean(axis=1).std()),
        'column_mean_stdev':float(values.mean(axis=0).std())}
def compare_backdrops(mode, labels, state):
    guards = [json.loads((PROOF/(label+'-region-guards.json')).read_text()) for label in labels]
    rectangle = guards[0]['before']['rectangle']
    assert all(guard['before']['rectangle']==rectangle for guard in guards), 'measurement rectangle changed'
    images = [np.asarray(Image.open(PROOF/(label+'.png')).convert('RGB')) for label in labels]
    assert images[0].shape==images[1].shape
    width,height = rectangle[2:]
    scale = images[0].shape[1]/width
    assert images[0].shape[0]==height*scale
    sidebar = state['sidebar']['width']
    drawer = state['drawer']['height']
    regions = {'sidebar':(8,80,sidebar-8,height-16),'content':(sidebar+2,2,width-2,height-2),
        'header':(sidebar+8,8,width-8,40),
        'transcript':(sidebar+40,60,width-40,min(150,height-drawer-100)),
        'composer':(sidebar+16,height-drawer-80,width-16,height-drawer-8),
        'drawer':(sidebar+16,height-drawer+48,width-16,height-16)}
    record = {'mode':mode,'rectangle':rectangle,'scale':scale,'captures':[
        {'name':label,'sha256':hashlib.sha256((PROOF/(label+'.png')).read_bytes()).hexdigest()}
        for label in labels],'regions':{}}
    for name,box in regions.items():
        x,y,right,bottom = [round(value*scale) for value in box]
        assert right>x and bottom>y
        parts = [pixels[y:bottom,x:right] for pixels in images]
        difference = np.abs(parts[0].astype(float)-parts[1].astype(float))
        included = np.ones(difference.shape[:2],dtype=bool)
        corners = []
        if name=='content':
            corners = [[round((width-12)*scale),0,round(width*scale),round(12*scale)],
                [round((width-12)*scale),round((height-12)*scale),
                    round(width*scale),round(height*scale)]]
            for left,top,edge,base in corners:
                included[max(0,top-y):min(bottom,base)-y,max(0,left-x):min(right,edge)-x] = False
        changed = bool(np.any(difference[included]))
        if name=='content':
            assert not changed, 'content differs outside the two native corner squares'
        sample = difference[included]
        record['regions'][name] = {'rectangle_pixels':[x,y,right,bottom],
            'excluded_corner_squares_pixels':corners,'included_pixels':int(included.sum()),
            'full_rectangle_differing_pixels':int(np.any(difference,axis=2).sum()),
            'coarse':luminance_stats(parts[0],included),'inverse':luminance_stats(parts[1],included),
            'difference_mean':float(sample.mean()),'difference_stdev':float(sample.std()),
            'difference_max':float(sample.max()),'identical':not changed}
    save('settings-backdrop-measurement-'+mode,record)
    for name,region in record['regions'].items():
        assert region['identical'] == (mode!='translucent' or name!='sidebar'), (mode,name,region)
    return record
try:
    backdrop,records = start_backdrop('settings-backdrop')
    save('backdrop-windows',records)
    action('background-translucent','SetWindowBackground',{'background':'translucent'})
    glass = poll('settings-translucent',lambda d:d['look']['background']=='translucent')
    assert glass['look']['stored']=='translucent' and glass['look']['forced_by'] is None
    assert sql('select window_background from settings') == 'translucent'
    capture('settings-translucent')
    ctl('background-toggle-solid','keys','secondary-alt-b')
    poll('background-key-solid',lambda d:d['look']['background']=='solid')
    ctl('background-toggle-translucent','keys','secondary-alt-b')
    poll('background-key-translucent',lambda d:d['look']['background']=='translucent')
    capture('settings-translucent-key')
    place_glass_window('occlusion-precondition')
    fault,_ = start_backdrop('settings-occluder')
    try:
        region_capture(PROOF/'settings-occluded.png',int((PROOF/'pid').read_text()),int((PROOF/'window-id').read_text()))
    except AssertionError as error:
        assert 'covered by intersecting windows' in str(error), str(error)
        save('settings-occlusion-rejected',{'rejected':True,'error':str(error)})
    else: raise AssertionError('the run-owned occluder was accepted')
    finally: stop_backdrop(fault)
    assert not (PROOF/'settings-occluded.png').exists()
    capture('settings-translucent-after-fault')
    for flags,reason,label in [({'reduce_transparency':True},'Reduce Transparency','reduce-transparency'),
        ({'increase_contrast':True},'Increase Contrast','increase-contrast'),
        ({'reduce_transparency':True,'increase_contrast':True},
            'Reduce Transparency and Increase Contrast','both')]:
        action('display-options-'+label,'DebugDisplayOptions',flags)
        forced = poll('settings-forced-'+label,lambda d:d['look']['background']=='solid'
            and d['look']['forced_by']==reason)
        assert forced['look']['stored']=='translucent'
        assert sql('select window_background from settings') == 'translucent'
        for entry in ['action','key']:
            if entry=='action':
                action('background-forced-action-'+label,'SetWindowBackground',{'background':'solid'})
            else:
                ctl('background-forced-key-'+label,'keys','secondary-alt-b')
            unchanged = ctl('settings-forced-'+label+'-'+entry,'dump')
            assert unchanged['look']==forced['look'], (label,entry,unchanged['look'])
            assert sql('select window_background from settings') == 'translucent'
        capture('settings-forced-'+label)
    action('display-options-motion','DebugDisplayOptions',{'reduce_motion':True})
    motion = poll('settings-reduced-motion',lambda d:d['look']['reduce_motion']
        and d['look']['background']=='translucent')
    assert motion['look']['forced_by'] is None
    capture('settings-reduced-motion')
    action('display-options-reset','DebugDisplayOptions',{})
    poll('settings-flags-cleared',lambda d:not any(d['look'][flag] for flag in
        ['reduce_transparency','increase_contrast','reduce_motion']))
    restart('background-persistence')
    poll('background-path-ready',lambda d:d['login_path']!='pending')
    restored = poll('settings-background-relaunch',lambda d:d['look']['stored']=='translucent')
    assert sql('select window_background from settings') == 'translucent'
    place_glass_window('background-relaunch')
    action('background-restored-display-options','DebugDisplayOptions',{})
    action('background-restored-settings','OpenSettings')
    poll('settings-restored-translucent',lambda d:d['look']['background']=='translucent')
    capture('settings-background-relaunch')
    action('background-restore-solid','SetWindowBackground',{'background':'solid'})
    poll('settings-background-final',lambda d:d['look']['background']=='solid')
    capture('settings-background-final')
    drawer_fixture()
    thread = create('settings-glass-planes','reply with the word ok')
    ctl('settings-glass-chat-done','wait',thread,'Done')
    drawer_keys('settings-glass-drawer-open','cmd-j','composer','drawer',draft='',caret=0)
    state = poll('settings-glass-drawer-ready',lambda d:d['drawer']['terminal'] is not None
        and 'VEYRA_TUI_READY' in ''.join(d['drawer']['terminal']['grid']))
    shell_pid = state['drawer']['tabs'][0]['pid']
    save('settings-glass-shell',{'pid':shell_pid})
    measurements = []
    for mode,background,flags in [('translucent','translucent',{}),('solid','solid',{}),
        ('reduce-transparency','translucent',{'reduce_transparency':True}),
        ('increase-contrast','translucent',{'increase_contrast':True})]:
        action('measure-unforce-'+mode,'DebugDisplayOptions',{})
        action('measure-background-'+mode,'SetWindowBackground',{'background':background})
        action('measure-flags-'+mode,'DebugDisplayOptions',dict(reduce_motion=True,**flags))
        state = poll('measure-look-'+mode,lambda d:d['look']['background']==
            ('translucent' if mode=='translucent' else 'solid') and d['look']['reduce_motion'])
        labels = ['settings-measure-'+mode+suffix for suffix in ['-coarse','-inverse']]
        def take_half(label,rectangle):
            global backdrop
            stop_backdrop(backdrop)
            backdrop,records = start_backdrop(label,label.endswith('-inverse'))
            save('backdrop-windows',records)
            return measure_capture(label,rectangle)
        measured_pair(labels,take_half)
        measurements.append(compare_backdrops(mode,labels,state))
    save('settings-backdrop-measurements',measurements)
    drawer_keys('settings-glass-drawer-close','cmd-w','drawer','composer')
    wait_gone(shell_pid)
    action('measure-restore-flags','DebugDisplayOptions',{})
    action('measure-restore-solid','SetWindowBackground',{'background':'solid'})
    action('measure-restore-settings','OpenSettings')
    poll('measure-restored',lambda d:d['view']=='settings' and d['look']['background']=='solid')
finally:
    for process in reversed(processes): stop_backdrop(process)
    (PROOF/'backdrop-windows.json').unlink(missing_ok=True)
page_done('settings-background')
print('PASS stored background, key cycle, guarded glass, occlusion rejection, display flags and relaunch')
PY_BACKGROUND
```

## Observable success

- `settings-first.json` has `view: settings`, `settings.default_mode: full_access` and the first thread's Full access configuration; inspect `settings-full-access.png`.
- Every mode change is asserted in both dump and `select default_mode from settings`; the direct SetDefaultMode key cycles all three.
- `settings-command-error.json` and its PNG show a rejected mode as a visible notice without changing the setting.
- `settings-ask-first.json` and `settings.png` show Ask first, its full consequence copy and the applies-from-now hint.
- `settings-existing.json` keeps the old Full access composer; `settings-new.json` has the new Ask first thread.
- Both thread configuration rows are checked directly in SQLite.
- `settings-relaunch.json` and `settings-persisted-database.txt` retain Ask first after a new pid; both thread rows retain their own modes.
- `settings-relaunch.png` shows the post-relaunch New thread view and its Ask first chip, which remain visible when new sidebar rows are below the window.
- Escape, SelectThread and SelectProject each assert the resulting view, and OpenSettings is driven by both its key and named action.
- `settings-transition-action.json` and `settings-transition-keys.json` show NewThread immediately after Escape, with no selected thread and the Ask first composer.
- `settings-late-branch-after.json` and its PNG assert Settings keeps an empty header after the delayed branch child finishes.
- `settings-header-chat.json` shows `header_branch: fixture-branch`; Settings and NewThread dumps assert it is empty.
- `settings-branch-return.png` captures that chat before reopening Settings, so the next Full access capture differs visibly even when the selected row is below the sidebar viewport.
- `drive.log` records commands, replies and exits; the text request while Settings is focused exits 1 instead of editing a hidden composer.

### Evidence contract

```json
{
  "required": {
    "settings": [
      "settings-first.json",
      "settings-ask-first.json",
      "settings-relaunch.json",
      "settings.png",
      "settings-existing.json",
      "settings-new.json",
      "settings-command-error.png",
      "settings-command-error.json"
    ],
    "settings-background": [
      "settings-solid.png",
      "settings-translucent.png",
      "settings-translucent-region-guards.json",
      "settings-translucent-after-fault.png",
      "settings-occlusion-rejected.json",
      "settings-forced-reduce-transparency.png",
      "settings-forced-increase-contrast.png",
      "settings-forced-both.png",
      "settings-reduced-motion.png",
      "settings-background-relaunch.json",
      "settings-background-final.png",
      "settings-backdrop-measurements.json",
      "settings-measure-translucent-coarse.png",
      "settings-measure-translucent-inverse.png",
      "settings-measure-solid-coarse.png",
      "settings-measure-solid-inverse.png",
      "settings-measure-reduce-transparency-coarse.png",
      "settings-measure-reduce-transparency-inverse.png",
      "settings-measure-increase-contrast-coarse.png",
      "settings-measure-increase-contrast-inverse.png",
      "settings-measure-translucent-coarse-region-guards.json",
      "settings-measure-translucent-inverse-region-guards.json",
      "settings-measure-solid-coarse-region-guards.json",
      "settings-measure-solid-inverse-region-guards.json",
      "settings-measure-reduce-transparency-coarse-region-guards.json",
      "settings-measure-reduce-transparency-inverse-region-guards.json",
      "settings-measure-increase-contrast-coarse-region-guards.json",
      "settings-measure-increase-contrast-inverse-region-guards.json"
    ]
  },
  "pairs": {
    "settings": [
      [
        "settings-full-access",
        "settings"
      ]
    ],
    "settings-background": [
      [
        "settings-solid",
        "settings-translucent"
      ],
      [
        "settings-translucent",
        "settings-forced-reduce-transparency"
      ]
    ]
  }
}
```

## Gotchas

Settings retains all three defaults.
A provider applies its offered modes when preparing a new composer; see [stored Grok modes](permission-matrix.md#stored-grok-modes) for compatibility.
The zero-thread default and offered-menu proof is in [new-thread](new-thread.md).

Settings is not a second native window.
The driver dispatches through the same OpenSettings action as the menu, but does not prove a native menu click.
The helper waits for provider groups to finish before relaunch; it does not add provider reaping.
The default is stored, not a process-local preference.
The run-owned git shim answers only the branch read, only while `branch-on` exists, and disappears with DATA during Cleanup.
Update Status from candidate acceptance.
