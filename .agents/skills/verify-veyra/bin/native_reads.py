"""Queue human capture reads. A queued read never masquerades as an automated paint verdict."""
import hashlib, json, pathlib

OWNERS = {
    'semantic_placeholders_and_input_clipping':'A: placeholder decisions and input bounds; no glyph raster claim',
    'composer_accent_and_unscrolled_header_rule':'B: active composer accent and unscrolled header band only',
    'semantic_approval_projection_and_control_bounds':'A: short command projection and decision bounds',
    'approval_command_plates_and_decision_controls':'B: short command wash, Deny/thread outlines, primary fill and warning ink',
    'option_cursor_paints_only_the_current_row':'B: current option ring, not native label/scene geometry',
    'settings_card_tracks_and_lit_thumbs':'B: Settings card/track/thumb cases, not sheets or title dimming',
    'status_marks_have_distinct_raster_shapes_and_working_steps':'B: shared glyph raster and 12 explicit phases, not host clocks',
    'family_status_marks_follow_child_status':'B: family status draw site, not native sidebar/header/count hosts',
}

def request(proof, name, checks, captures, state=None):
    proof = pathlib.Path(proof)
    row = {'status':'needs lead read','checks':checks,'captures':{
        label:hashlib.sha256((proof/(label+'.png')).read_bytes()).hexdigest() for label in captures},
        'state':state,'test_owners':OWNERS}
    (proof/(name+'-native-read.json')).write_text(json.dumps(row,indent=2)+'\n')
    return row

def capture_read(proof, name, state):
    checks = ['text, ink, clipping and order outside the admitted test cases',
        'header/status copy and ink; scrolled rule; provider readiness; transcript/failure/tool/family ordering',
        'sheet controls and trailing keycap ink; wrapped Send; residual MCP preview and approval inset',
        'sidebar/header/count mark hosts, clock scheduling and Reduce Motion; native chrome and display scale']
    # Focus retention does not imply an active native window. Read the inactive edge separately.
    if state['composer']['focused'] and state['window_active']:
        checks.append('active composer accent: composer_accent_and_unscrolled_header_rule owns admitted case')
    else: checks.append('inactive/unfocused composer border remains a native read')
    return request(proof,name,checks,[name],state)

def sidebar_dims(proof, before, after, state):
    return request(proof,after+'-sidebar-dim',['sidebar foreground and selected title dim under sheet; native only'],
        [before,after],state)

def option_cursor_paints(proof, before, after, state, labels):
    return request(proof,after+'-cursor',['native label geometry: '+repr(labels),
        'ring paint owned by option_cursor_paints_only_the_current_row'],[before,after],state)
