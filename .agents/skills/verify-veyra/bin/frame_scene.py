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
