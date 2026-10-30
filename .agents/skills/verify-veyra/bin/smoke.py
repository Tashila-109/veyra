"""Bounded capture scenes for selected pages. They prove only these scenes, never a full page."""
import re

def drained(d, threads):
    assert all(re.fullmatch(r'[0-9a-f-]{36}',thread) for thread in threads), 'invalid scene thread id'
    return all(d.sql("select live_pgid is null from threads where id='"+thread+"'")=='1' for thread in threads)

def family_stopped(state, root, child, task):
    parent=state['selected_thread']
    row=next((row for row in state['tasks'] if row['id']==task),{})
    settled=row.get('state',{}).get('Settled',{}) if isinstance(row.get('state'),dict) else {}
    outcome=settled.get('outcome')
    return bool(parent and parent['id']==root and parent['status']=='Held' and parent['hold']=='stopped'
        and row.get('parent')==root and row.get('child')==child
        and isinstance(outcome,dict) and 'Interrupted' in outcome and settled.get('delivery',{}).get('Pending',{}).get('wake'))

def drive(page):
    import driver as d
    d.prepare()
    threads = [];family=None
    try:
        if page == 'settings':
            d.action('smoke-settings','OpenSettings')
            d.poll('smoke-settings-open',lambda state:state['view']=='settings')
        elif page == 'approvals-questions':
            d.action('smoke-ask-first','SetDefaultMode',{'mode':'ask_first'})
            d.action('smoke-approval-new','NewThread')
            thread = d.create('smoke-approval','Review the fake command',scenario='approval-deny')
            threads.append(thread)
            d.poll('smoke-dock',lambda state:state['composer']['dock'] is not None)
        elif page == 'multi-agent':
            root,child,task = d.create_family('smoke-family',child='running')
            family=(root,child,task)
            threads += [root,child]
            d.select('smoke-family-selected',root)
            d.poll('smoke-family-card',lambda state:state['transcript']['family_card'] is not None)
        elif page != 'new-thread':
            scenario = 'long-reply' if page in ['stop-interrupt','restart-recovery'] else 'ok-reply'
            thread = d.create('smoke-'+page,'Smoke capture',scenario=scenario)
            threads.append(thread)
            d.ctl('smoke-ready','wait',thread,'Working' if scenario=='long-reply' else 'Done')
            if page == 'restart-recovery':
                d.action('smoke-quit-sheet','Quit')
                d.poll('smoke-quit-open',lambda state:state['quit_sheet']['open'])
            elif page == 'shell-drawer':
                d.drawer_fixture()
                d.ctl('smoke-drawer-open','keys','secondary-j')
                d.poll('smoke-shell-ready',lambda state:state['drawer'] and state['drawer']['terminal']
                    and 'VEYRA_TUI_READY' in ''.join(state['drawer']['terminal']['grid']))
        d.capture_content(page+'-smoke')
        d.page_done(page+'-smoke')
    finally:
        # Only fake requests are driven, but even those remain pending until an explicit scene decision.
        if page == 'restart-recovery':
            d.ctl('smoke-quit-cancel','keys','escape')
            d.poll('smoke-quit-closed',lambda state:not state['quit_sheet']['open'])
        try:
            for thread in threads: d.action('smoke-stop-'+thread,'StopThread',{'thread':thread})
            scope=[family[1]] if family else threads
            d.poll('smoke-provider-groups-ended',lambda state:(not family or family_stopped(state,*family)) and drained(d,scope))
        finally:
            d.owned_instance_pid()
            if page == 'shell-drawer':
                d.action('smoke-drawer-focus','DebugFocusShellDrawer')
                d.ctl('smoke-drawer-close','keys','secondary-w')
                d.poll('smoke-drawer-hidden',lambda state:not state['drawer']['open'])
            d.action('smoke-default-restore','SetDefaultMode',{'mode':'full_access'})
            d.prepare()
