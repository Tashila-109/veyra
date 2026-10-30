"""Dump assertions for the two idle focus scenes; 6d never falls back to the earlier surface."""
import copy
from orchestration import SafetyStop

def require_6d_dump(state):
    palette, toasts = state.get('palette'), state.get('toasts')
    if not isinstance(palette,dict) or type(palette.get('open')) is not bool or not isinstance(toasts,list):
        raise SafetyStop('6d idle scene unavailable: dump.palette and dump.toasts required')

def idle_variant(state, part, previous=None):
    if part != '6d': return None
    require_6d_dump(state)
    palette, toasts = state['palette'], state['toasts']
    assert palette['open'] is False, '6d idle scene requires palette closed'
    assert toasts and all(isinstance(toast,dict) and toast.get('kind') and toast.get('text') for toast in toasts), \
        '6d idle scene requires a toast at rest'
    value = {'palette_open':False,'toasts':copy.deepcopy(toasts)}
    assert previous is None or value == previous, '6d idle toast changed during measurement'
    return value

def assert_idle_state(state, focus, part, previous=None):
    variant = idle_variant(state,part,previous)
    assert state['focus']==focus and state['drawer']['open']
    assert not any(row.get('status')=='Working' for row in state['sidebar']['rows'])
    assert state['selected_thread']['status']=='Done'
    assert not state['drawer']['tabs'][0]['running']
    assert state['drawer']['terminal']['focused']==(focus=='drawer')
    assert state['look']['background']=='translucent'
    return variant

def require_part(state, part, side='candidate'):
    assert part in ['6a','6b','6c','6d'], 'guard requires explicit --part'
    modern = 'palette' in state or 'toasts' in state
    if side=='candidate' and modern:
        require_6d_dump(state)
        assert part=='6d', 'candidate tree requires --part 6d'
    elif part=='6d':
        require_6d_dump(state)
    elif side=='candidate':
        assert part=='6c', 'candidate tree requires --part 6c'
    return part

def parse_probe(argv):
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument('phase',choices=['stream','scenes'])
    parser.add_argument('side',choices=['baseline','candidate'])
    parser.add_argument('label')
    parser.add_argument('--scene',choices=['stream','idle','working'],default='stream')
    parser.add_argument('--part',choices=['6a','6b','6c','6d'],required=True)
    parser.add_argument('--idle-focus',choices=['drawer','composer'],default='composer')
    parser.add_argument('--motion',choices=['normal','reduced'],default='normal')
    return parser.parse_args(argv)

def measure_idle(window, part, focus):
    """Native and offline probe entries use this same before/sample/after loop."""
    before = window.read('scene-before')
    window.front('scene-before')
    scene = assert_idle_state(before,focus,part)
    started = window.clock.monotonic()
    observations = []
    while window.clock.monotonic()-started < 10:
        window.clock.sleep(min(.1,max(0,10-(window.clock.monotonic()-started))))
        observed = window.read('scene-sample')
        assert_idle_state(observed,focus,part,scene)
        observations.append({'seconds':window.clock.monotonic()-started,'repaints':observed['perf']['repaints'],
            'focus':observed['focus'],'window_active':observed['window_active'],
            'terminal_repaints':observed['drawer']['terminal'].get('repaints'),
            'terminal_frame_requests':[tab.get('frame_requests') for tab in observed['drawer']['tabs']]})
    after = window.read('scene-after')
    assert_idle_state(after,focus,part,scene)
    window.front('scene-after')
    seconds = window.clock.monotonic()-started
    return {'scene':'idle','seconds':seconds,'repaints':after['perf']['repaints']-before['perf']['repaints'],
        'repaints_per_second':(after['perf']['repaints']-before['perf']['repaints'])/seconds,
        'focus_before':before['focus'],'focus_after':after['focus'],'idle_scene':scene,'before':before,'after':after,'observations':observations,'measurement_complete':True}
