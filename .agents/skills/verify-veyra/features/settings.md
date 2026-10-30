# Settings

Status: passed, 2026-10-26, bd8a083, human display-settings lane not run (fake CLI RUN 8577dad2), b51396cd355f3a6eae4688e9fb0a2f32f53f3d939baac681871302d775882b71
Covers: crates/veyra/src/ui/drawer.rs crates/veyra/src/ui/header.rs crates/veyra/src/ui/settings.rs crates/veyra/src/ui/new_thread.rs crates/veyra/src/ui/sidebar.rs crates/engine/src/provider/** crates/engine/tests/fixtures/claude/**

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
- `settings-sidebar-band`: `veyra::DebugDragSidebar` and `veyra::DebugDoubleClickSidebar` synthesize pointer events in the owned window, without moving the real cursor; `sidebar.width` proves the drag and reset.
- `settings-glass-coverage`: `look.glass_coverage` reports the tint coverage in force, 0.80 in dark and 0.88 in light when Translucent, and 1 in Solid.
- The close and quit sheets retain native reads of `on_solid` in their danger buttons' trailing keycaps, excluding the captions and using painted control bounds.

Every placement step requires the owned window to be unmanaged and floating under the launch's temporary one-shot rule. A managed record fails with `owned window joined the tile tree` before any move or resize. The recipe never toggles floating or re-tiles a window, and needs no empty desktop.

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
capture = capture_content
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
action('settings-sidebar-baseline','ResetSidebar')
sidebar_before = poll('settings-sidebar-before',lambda d:d['sidebar']['width']==240)
action('settings-sidebar-drag','DebugDragSidebar',{'pixels':40})
sidebar_dragged = poll('settings-sidebar-dragged',lambda d:d['sidebar']['width']==280)
assert sidebar_dragged['view']=='settings'
capture('settings-sidebar-dragged')
action('settings-sidebar-double-click','DebugDoubleClickSidebar')
sidebar_reset = poll('settings-sidebar-reset',lambda d:d['sidebar']['width']==240)
assert sidebar_reset['view']=='settings'
capture('settings-sidebar-reset')
capture('settings-full-access')
placement_tool = shutil.which('yabai')
assert placement_tool, 'minimum-width Settings proof needs the owned-window placement tool'
owned_pid = int((PROOF/'pid').read_text())
owned_window = int((PROOF/'window-id').read_text())
def settings_frame():
    result = subprocess.run([placement_tool,'-m','query','--windows','--window',str(owned_window)],capture_output=True,text=True)
    (PROOF/'settings-minimum-placement.stderr').write_text(result.stderr)
    assert result.returncode==0, 'owned Settings window unavailable'
    row = json.loads(result.stdout)
    assert row['id']==owned_window and row['pid']==owned_pid, 'refused a foreign Settings window'
    return row
before = settings_frame()
require_unmanaged(before)
try:
    settings_frame()
    result = subprocess.run([placement_tool,'-m','window',str(owned_window),'--resize','abs:760:560'],capture_output=True,text=True)
    assert result.returncode==0
    end = time.monotonic()+5
    while True:
        after = settings_frame()
        if after['frame']['w']==760 and after['frame']['h']==560: break
        assert time.monotonic()<end, 'owned Settings window did not reach the minimum'
        time.sleep(.05)
    save('settings-minimum-placement',{'pid':owned_pid,'window':owned_window,'before':before['frame'],'after':after['frame']})
    capture('settings-minimum')
finally:
    settings_frame()
    result = subprocess.run([placement_tool,'-m','window',str(owned_window),'--resize',f"abs:{int(before['frame']['w'])}:{int(before['frame']['h'])}"],capture_output=True,text=True)
    assert result.returncode==0
    result = subprocess.run([placement_tool,'-m','window',str(owned_window),'--move',f"abs:{int(before['frame']['x'])}:{int(before['frame']['y'])}"],capture_output=True,text=True)
    assert result.returncode==0
    end = time.monotonic()+5
    while True:
        restored = settings_frame()
        if restored['frame']==before['frame']: break
        assert time.monotonic()<end, 'owned Settings window did not restore its bounds'
        time.sleep(.05)
    save('settings-minimum-restored',restored['frame'])
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
Before each glass capture and after relaunch, the recipe asks yabai only about the run's owned pid and recorded window id, requires the launch's unmanaged floating state, and places it at 24,60 with 960×740 bounds.
Floating a tiled window lets yabai re-tile its neighbours; the recipe changes no config, rule, space or layout setting.
An unavailable service or untracked window leaves the guards to decide.
If the guard records intersecting windows, the recipe makes one further placement on the same backdrop display, clearing all recorded bounds and keeping 960×740 whenever it fits.
The cursor stays untouched, every window still counts, and no available rectangle leaves the original guard failure intact.
The Translucent review set at the human gate needs the same owned-window placement and one-time fallback.
The plane measurement uses guarded region captures for Translucent and `-l` window captures for Solid and forced Solid over the coarse backdrop and its inverse, with one completed chat, an unfocused composer and an idle fake shell drawer held in the same state.
The same rectangle and all region guards surround each measurement, including each Solid window capture.
The operator's separate Settings pairs use the same capture methods in dark and light, after the lead approves the manual appearance step.
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
capture = capture_content
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
    assert before is not None, 'owned window unavailable to yabai'
    require_unmanaged(before)
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
        owner = row.get('kCGWindowOwnerName','unreported')
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
        pid,window = identity(),int((PROOF/'window-id').read_text())
        if before['look']['background']=='translucent':
            region_capture(PROOF/(name+'.png'),pid,window)
        else:
            backdrop = json.loads((PROOF/'backdrop-windows.json').read_text())
            target = PROOF/(name+'.png')
            target.unlink(missing_ok=True)
            try:
                guard_before = region_snapshot(pid,window,backdrop)
                subprocess.run(['screencapture','-x','-o','-l',str(window),str(target)],check=True)
                guard_after = region_snapshot(pid,window,backdrop,guard_before['rectangle'])
                assert guard_before==guard_after, 'measurement window guards changed'
            except BaseException as error:
                target.unlink(missing_ok=True)
                save(name+'-region-guards',{'attempts':[{'attempt':1,'error':str(error)}]})
                raise
            save(name+'-region-guards',{'attempts':[],'before':guard_before,'after':guard_after,
                'capture_method':'window -l'})
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
    assert abs(glass['look']['glass_coverage']-{'dark':0.80,'light':0.88}[glass['appearance']])<.000001
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
    from native_reads import sidebar_dims
    action('sheet-unforce','DebugDisplayOptions',{'reduce_motion':True})
    action('sheet-translucent','SetWindowBackground',{'background':'translucent'})
    poll('sheet-translucent-applied',lambda d:d['look']['background']=='translucent')
    drawer_fixture(job=True)
    busy = create('settings-sheet-thread','Sheet dimming',scenario='long-reply')
    ctl('settings-sheet-working','wait',busy,'Working')
    drawer_keys('settings-sheet-drawer','cmd-j','composer','drawer',draft='',caret=0)
    poll('settings-sheet-running-shell',lambda d:any(tab['running'] for tab in d['drawer']['tabs']))
    capture('settings-before-close-sheet')
    drawer_keys('settings-close-sheet-open','cmd-w','drawer','none')
    close_state = poll('settings-close-sheet',lambda d:d['drawer']['close_sheet'] is not None)
    capture('settings-translucent-close-sheet')
    sidebar_dims(PROOF,'settings-before-close-sheet','settings-translucent-close-sheet',close_state)
    drawer_keys('settings-close-sheet-cancel','escape','none','drawer')
    capture('settings-before-quit-sheet')
    drawer_keys('settings-quit-sheet-open','cmd-q','drawer','none')
    quit_state = poll('settings-quit-sheet',lambda d:d['quit_sheet']['open'])
    capture('settings-translucent-quit-sheet')
    sidebar_dims(PROOF,'settings-before-quit-sheet','settings-translucent-quit-sheet',quit_state)
    drawer_keys('settings-quit-sheet-cancel','escape','none','drawer')
    drawer_keys('settings-close-shell-again','cmd-w','drawer','none')
    drawer_keys('settings-close-shell-confirm','cmd-enter','none','composer')
    action('settings-sheet-stop','StopThread',{'thread':busy})
    ctl('settings-sheet-stopped','wait',busy,'Interrupted')
    action('settings-glass-return','SelectThread',{'thread':thread})
    poll('settings-glass-returned',lambda d:d['selected_thread']['id']==thread and d['drawer']['open'])
    action('settings-glass-return-focus','DebugFocusShellDrawer')
    poll('settings-glass-return-focused',lambda d:d['focus']=='drawer')
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

### Glass smoke

Only a production hunk in a glass file selects these guarded pairs.
The lead reads their native compositing; no OCR verdict runs.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
if command -v sccache >/dev/null; then export RUSTC_WRAPPER=sccache; fi
set -eu
PYTHONPATH=.agents/skills/verify-veyra/bin python3 - <<'PYGLASS'
from glass_smoke import drive
drive()
PYGLASS
```

### Native smoke

This capture runs only in a selected-page smoke; it leaves the page's Status unchanged.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
if command -v sccache >/dev/null; then export RUSTC_WRAPPER=sccache; fi
set -eu
PYTHONPATH=.agents/skills/verify-veyra/bin python3 - <<'PYSMOKE'
from smoke import drive
drive('settings')
PYSMOKE
```

## Observable success

Admitted test owners for this page: `settings_card_tracks_and_lit_thumbs`.
Their class and exact scope are in [proof ownership](../proof-ownership.md); every remaining ink, mark or clipping detail stays a native capture read.

On settled states, the separate `-placeholder` capture moves focus with `root::Tab`, then restores it with `FocusComposer`.
`semantic_placeholders_and_input_clipping` owns the admitted placeholder decisions and bounds.
`composer_accent_and_unscrolled_header_rule` owns the active accent case; `window_active` also gates that case.
The lead reads remaining placeholder ink, inactive borders and native clipping from the captures.

Each retained PNG and its exact dump queue a `*-native-read.json` for the lead.
Vision OCR contributes no verdict.
The admitted ownership and remaining native reads are listed in [proof ownership](../proof-ownership.md).
A queued read is pending until the lead records the capture hash and verdict.

- `settings-first.json` has `view: settings`, `settings.default_mode: full_access` and the first thread's Full access configuration; inspect `settings-full-access.png`.
- `settings-minimum.png` shows the label wrapping on the left and all three controls on the right at 760×560. Its placement and restoration receipts assert the owned window and original bounds. The lead reads wrapping, clipping and control alignment from this native capture.
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
- `settings-sidebar-before.json`, `settings-sidebar-dragged.json` and `settings-sidebar-reset.json` show 240 → 280 → 240 through the actual band; the two PNGs show the changed geometry and restored seam.
- The glass dump reports `look.glass_coverage`; hovered and selected sidebar rows paint opaque plates, and the forced Increase Contrast capture keeps helper text distinct from body text.
- `drive.log` records commands, replies and exits; the text request while Settings is focused exits 1 instead of editing a hidden composer.

### Evidence contract

```json
{
  "required": {
    "settings": [
      "settings-first.json",
      "settings-minimum.png",
      "settings-minimum-placement.json",
      "settings-minimum-restored.json",
      "settings-ask-first.json",
      "settings-relaunch.json",
      "settings.png",
      "settings-existing.json",
      "settings-new.json",
      "settings-command-error.png",
      "settings-command-error.json",
      "settings-sidebar-before.json",
      "settings-sidebar-dragged.json",
      "settings-sidebar-reset.json",
      "settings-sidebar-dragged.png",
      "settings-sidebar-reset.png"
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
      "settings-translucent-close-sheet.png",
      "settings-translucent-quit-sheet.png",
      "settings-translucent-close-sheet-sidebar-dim-native-read.json",
      "settings-translucent-quit-sheet-sidebar-dim-native-read.json",
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
    ],
    "settings-smoke": [
      "settings-smoke.png",
      "settings-smoke-native-read.json",
      "settings-smoke-freshness.json"
    ],
    "settings-glass-smoke": [
      "glass-smoke-pairs-native-read.json",
      "glass-smoke-solid-coarse.png",
      "glass-smoke-solid-inverse.png",
      "glass-smoke-translucent-coarse.png",
      "glass-smoke-translucent-inverse.png"
    ]
  },
  "pairs": {}
}
```

## Gotchas

Settings retains all three defaults.
A provider applies its offered modes when preparing a new composer; see [stored Grok modes](permission-matrix.md#stored-grok-modes) for compatibility.
The zero-thread default and offered-menu proof is in [new-thread](new-thread.md).

Settings is not a second native window.
The driver dispatches through the same OpenSettings action as the menu, but does not prove a native menu click.
The helper waits for provider groups to finish before relaunch; it does not add provider reaping.
The native drag action sends its pointer sequence in one dispatch. It cannot drive a platform release outside the window or loss of activation midway without another native input surface; the focused `resize_drag_ends_on_outside_release_and_deactivation` test covers both exits for sidebar and drawer, including unchanged geometry after reactivation and further movement.
The default is stored, not a process-local preference.
The run-owned git shim answers only the branch read, only while `branch-on` exists, and disappears with DATA during Cleanup.
Update Status from candidate acceptance.
