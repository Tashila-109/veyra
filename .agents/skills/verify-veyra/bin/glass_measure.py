"""Settings glass measurement entry; uses the same guarded placement and capture path as the recipe."""
from driver import *

processes=[]
original_capture=capture_content

def place_glass_window(label, rectangle=(24,60,960,740)):
    tool = shutil.which('yabai')
    pid = owned_instance_pid()
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
    pid = owned_instance_pid()
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
        pid,window = owned_instance_pid(),int((PROOF/'window-id').read_text())
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

def drive_pair(labels,backdrop):
    def take_half(label,rectangle):
        nonlocal backdrop
        stop_backdrop(backdrop)
        backdrop,records=start_backdrop(label,label.endswith('-inverse'))
        save('backdrop-windows',records)
        return measure_capture(label,rectangle)
    measured_pair(labels,take_half)
    return backdrop
