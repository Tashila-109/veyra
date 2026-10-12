# Child threads

Status: not run, 2026-10-12, 833bbce, child controls, quiet time and timing probes

## Sub-features

- A production root relay calls start_thread, adding a child below its root on the sidebar guide.
- The root shows Started N agents, with the singular Started 1 agent, each child's glyph and status, Open and Stop while that child's shared turn target exists.
- The child opens independently, with its Task from Claude Code card and Task from the root title label.
- The breadcrumb links back to an unarchived root, and a parent request adds Claude Code needs you.
- Child and parent StopThread preserve a held wake until Continue; accepted wakes label Reports from N child threads and Report delivered.
- Archive hides a family; ArchiveKeepChildren keeps running children as top-level rows with an unlinked breadcrumb.
- Quiet for N min appears after ten silent minutes without changing Working, including an unselected child.
- Assistant soft line breaks, emphasis, lists and code remain visible.

## How to get to it (user POV)

Ask Claude Code in a root thread to start a child only when the task needs one.
Open the child from its row or the parent's card, and use the root breadcrumb to return.
Use Stop on a family card row to stop that child, then read the report before Continue on the root.
The row menu offers Stop thread and Archive, and a root with children also offers Archive, keep child threads.
OpenThreadMenu is ⌘⌥⇧M, Archive is ⌘⇧A, and ArchiveKeepChildren is ⌘⌥⇧A.
The breadcrumb and card Open use SelectThread; card Stop uses StopThread.

## Driving it with veyra ctl

Run Launch, Doctor, new-thread and restart-recovery first.
This fake lane uses the real relay, socket authentication and actor tool path.
The quiet check waits for the actual ten-minute clock while the child is unselected.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
set -eu
RUN=${RUN:?export the RUN that Launch printed}
PROOF=target/verify-veyra/$RUN
PYTHONPATH="$PROOF" python3 - <<'PY'
from driver import *
identity()
assert LANE == 'fake'
poll('multi-initial-idle',lambda d:sql('select count(*) from threads where live_pgid is not null')=='0')
restart('multi-fresh-lanes')
poll('multi-ready',lambda d:d['providers'][0]['readiness']=='ready')
root,child,task = create_family('report family')
working = poll('multi-tree-working',lambda d:d['transcript']['family_card'] is not None
    and d['transcript']['family_card']['rows'][0]['status']=='Working'
    and d['transcript']['family_card']['rows'][0]['stop_action']=='veyra::StopThread')
rows = [row for row in working['sidebar']['rows'] if row['kind']=='thread']
assert next(row for row in rows if row['id']==root)['row_status']=='Working'
assert next(row for row in rows if row['id']==child)['depth']==1
assert working['transcript']['family_card']['title']=='Started 1 agent'
assert working['transcript']['family_card']['rows'][0]['stop_action']=='veyra::StopThread'
assert [row['id'] for row in rows].index(child)==[row['id'] for row in rows].index(root)+1
assert sql("select parent_thread_id||'|'||root_thread_id||'|'||relationship from threads where id='"+child+"'")==root+'|'+root+'|subagent'
cli = int(sql("select live_pid from threads where id='"+root+"'"))
relay = subprocess.check_output(['pgrep','-P',str(cli),'-x','veyra'],text=True).split()
assert len(relay)==1
parent = subprocess.check_output(['ps','-o','ppid=','-p',relay[0]],text=True).strip()
assert parent==str(cli)
save('multi-peer',{'cli_pid':cli,'relay_pid':int(relay[0]),'relay_parent':int(parent)})
assert not list((DATA/'run').glob('*/mcp.json'))
capture('multi-tree-working')
opened = select('multi-child-task',child)
assert opened['selected_thread']['breadcrumb']=={'thread':root,'title':'report family','linked':True}
assert any(item['kind']=='task_card' and item['text']=='Task from Claude Code' for item in opened['transcript']['items'])
assert any(item['turn_label']=='Task from report family' for item in opened['transcript']['items'])
assert any(item['kind']=='user' and item['text'].startswith('Your last message is your report') and 'You cannot start child threads.' in item['text'] for item in opened['transcript']['items'])
poll('multi-visible-counted',lambda d:d['perf']['visible']['thread']==child and d['perf']['visible']['applied_batches']>0)
poll('multi-open-stamped',lambda d:d['perf']['transcript_open_ms'] is not None)
capture('multi-child-task')
family_delivered('multi-delivered',root,task)
select('multi-family-done',root)
completed = poll('multi-family-done',lambda d:d['transcript']['family_card']['rows'][0]['status']=='Done')
assert completed['transcript']['family_card']['rows'][0]['glyph']=='✓'
assert completed['transcript']['family_card']['rows'][0]['stop_action'] is None
assert completed['perf']['visible']=={'thread':root,'applied_batches':0,'max_batch_gap_ms':0.0}
assert any(item['turn_label']=='Reports from 1 child thread' for item in completed['transcript']['items'])
assert sql("select (select started_at from turns where id=tasks.wake_turn)>=(select ended_at from turns where thread_id=tasks.parent_thread_id and ordinal=1) from tasks where id='"+task+"'")=='1'
capture('multi-family-done')
select('multi-report-delivered',child)
poll('multi-report-delivered',lambda d:any(item['report_delivered'] for item in d['transcript']['items']))
capture('multi-report-delivered')
stage('ok-reply',slow=True)
ctl('multi-followup-text','text','A user follow-up after the delivered report')
ctl('multi-followup-send','keys','secondary-enter')
reset = poll('multi-turn-reset',lambda d:d['selected_thread']['status']=='Working')
assert reset['perf']['visible']=={'thread':child,'applied_batches':0,'max_batch_gap_ms':0.0}
ctl('multi-followup-done','wait',child,'Done')
followup = ctl('multi-followup-done','dump')
assert followup['perf']['visible']['applied_batches']>0
reported = [item for item in followup['transcript']['items'] if item['report_delivered']]
assert len(reported)==1 and reported[0]['text'].startswith('First report line')
assert not followup['transcript']['items'][-1]['report_delivered']
capture('multi-later-child-reply')
root,child,task = create_family('parent needs you',parent='attention',child='running')
poll('multi-parent-request',lambda d:d['selected_thread']['status']=='NeedsYou')
opened = select('multi-child-parent-needs',child)
assert opened['selected_thread']['parent_attention']=='Claude Code needs you'
capture('multi-child-parent-needs')
select('multi-breadcrumb-return',root)
action('multi-deny-parent','Deny')
poll('multi-parent-idle',lambda d:d['selected_thread']['status']=='Done')
action('multi-open-menu','OpenThreadMenu',{'thread':root})
menu = poll('multi-menu',lambda d:d['sidebar']['menu_thread']==root)
assert next(row for row in menu['sidebar']['rows'] if row.get('id')==root)['archive_keep_children']
capture('multi-menu')
action('multi-card-stop','StopThread',{'thread':child})
held = poll('multi-child-stop-held',lambda d:d['selected_thread']['status']=='Held' and pending_wake(root))
assert held['selected_thread']['hold']=='child_report'
assert held['transcript']['family_card']['rows'][0]['status']=='Interrupted'
assert held['transcript']['family_card']['rows'][0]['stop_action'] is None
capture('multi-child-stop-held')
action('multi-child-stop-continue','Continue')
family_delivered('multi-child-stop-delivered',root,task)
root,child,task = create_family('stop the parent',parent='running',child='running')
poll('multi-both-running',lambda d:d['selected_thread']['status']=='Working'
    and d['transcript']['family_card']['rows'][0]['status']=='Working')
action('multi-parent-stop','StopThread',{'thread':root})
held = poll('multi-parent-stop-held',lambda d:d['selected_thread']['status']=='Held' and pending_wake(root))
assert held['transcript']['family_card']['rows'][0]['status']=='Interrupted'
assert held['transcript']['family_card']['rows'][0]['stop_action'] is None
assert held['selected_thread']['hold']=='stopped'
capture('multi-parent-stop-held')
action('multi-parent-stop-continue','Continue')
family_delivered('multi-parent-stop-delivered',root,task)
root,child,task = create_family('archive the family',parent='running',child='running')
action('multi-archive','Archive',{'thread':root})
archived = poll('multi-archived-family',lambda d:all(row.get('id') not in [root,child] for row in d['sidebar']['rows']))
assert sql("select count(*) from threads where root_thread_id='"+root+"' and archived_at is not null")=='2'
capture('multi-archived-family')
root,child,task = create_family('keep the child',parent='running',child='running')
action('multi-archive-keep','ArchiveKeepChildren',{'thread':root})
poll('multi-kept-sidebar',lambda d:any(row.get('id')==child and row['depth']==0 for row in d['sidebar']['rows'])
    and not any(row.get('id')==root for row in d['sidebar']['rows']))
kept = select('multi-kept-child',child)
assert kept['selected_thread']['status']=='Working'
assert kept['selected_thread']['breadcrumb']=={'thread':root,'title':'keep the child','linked':False}
assert sql("select archived_at is null from threads where id='"+child+"'")=='1'
capture('multi-kept-child')
action('multi-stop-kept','StopThread',{'thread':child})
poll('multi-kept-pending',lambda d:pending_wake(root))
assert sql("select state from turns where id=(select wake_turn from tasks where id='"+task+"')")=='queued'
root,child,task = create_family('quiet child',child='running')
poll('multi-quiet-started',lambda d:d['transcript']['family_card']['rows'][0]['status']=='Working')
action('multi-unselect-quiet','NewThread')
end = time.monotonic()+660
while True:
    quiet = ctl('multi-quiet-clock','dump')
    row = next(row for row in quiet['sidebar']['rows'] if row.get('id')==child)
    if row['quiet_minutes'] is not None: break
    assert time.monotonic()<end, 'quiet clock did not reach ten minutes'
    time.sleep(5)
assert quiet['selected_thread'] is None and row['status']=='Working' and row['quiet_minutes']>=10
assert row['title']=='File reporter'
save('multi-quiet-sidebar',quiet)
capture('multi-quiet-sidebar')
opened = select('multi-quiet-header',child)
assert opened['selected_thread']['status']=='Working'
assert opened['selected_thread']['title']=='File reporter'
assert 'Quiet for 10 min' in opened['selected_thread']['header_status']
capture('multi-quiet-header')
select('multi-quiet-parent',root)
action('multi-quiet-card-stop','StopThread',{'thread':child})
poll('multi-quiet-held',lambda d:d['selected_thread']['status']=='Held' and pending_wake(root))
action('multi-quiet-continue','Continue')
family_delivered('multi-quiet-delivered',root,task)
page_done('multi-agent')
print('PASS tree, breadcrumbs, cards, parent attention, held wakes, both archives, labels, delivery and quiet hint')
PY
```

## Observable success

The top-level `tasks` dump key adds task state and Pending or Delivered, while `sidebar`, `selected_thread` and `transcript` contain the lineage and rendered controls.
The production relay's saved pid and parent match the root's live_pid; SQLite and the log show a real start_thread call.
The tools `list_providers`, `start_thread`, `get_task`, `message_thread` and `stop_thread` have engine-owned descriptions, checked by verify_map_names.
Both pages name `Archive` and `ArchiveKeepChildren`, the two actions behind Archive and Archive, keep child threads.
Working and Done family screenshots show the same child row change; the task card and final report screenshot show turn_label and report_delivered.
The held screenshots and SQLite show a pending wake, and Continue changes its delivery to delivered.
The archive screenshots and queries prove hidden families and kept top-level children.
The quiet screenshots retain the child title beside its untruncated hint in the sidebar and header after the actual ten-minute interval.
The Done and Interrupted card rows retain Open and show no Stop.
The child task bubble shows the stored first-task guidance, and the later user reply keeps Report delivered on its original task report.
`perf.visible` holds the selected thread id, its applied text-batch count and its largest inter-batch gap in milliseconds.
Both the visible count and largest gap reset on a selection change and when that thread's turn starts; hidden streams do not affect them.
`perf.transcript_open_ms` is null until a transcript opens, measures its Snapshot request through the first render of the installed snapshot, and each open replaces it.
The visible-count, selection-reset and turn-reset dumps accompany the family drive; the transcript-open stamp is read after selection.

## Gotchas

The lane runs no installed provider.
Capture retains the screen-lock, frontmost-window and stale-frame guards.
A stopped child holds its parent's wake, while the agent's stop_thread tool wakes without a human hold.
An archived root keeps a pending wake forever; kept children retain their lineage and get no new tool authority.
The quiet hint neither nudges nor stops work.
Status is filled by the lead after candidate acceptance and its commit.
