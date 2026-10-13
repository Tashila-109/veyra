# Child threads

Status: passed, 2026-10-13, d2ff581, none

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
root,child,task = create_family('archive one child',child='running')
poll('multi-child-archive-started',lambda d:d['transcript']['family_card']['rows'][0]['status']=='Working')
action('multi-archive-child','Archive',{'thread':child})
held = poll('multi-child-archive-held',lambda d:d['selected_thread']['status']=='Held' and pending_wake(root))
assert held['selected_thread']['hold']=='child_report'
assert sql("select archived_at is not null from threads where id='"+child+"'")=='1'
assert sql("select archived_at is null from threads where id='"+root+"'")=='1'
assert held['transcript']['family_card']['rows'][0]['stop_action'] is None
capture('multi-child-archive-held')
action('multi-child-archive-continue','Continue')
family_delivered('multi-child-archive-delivered',root,task)
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

### Probe smoke measurements

This is one smoke per fixture, not the performance series.
Run it after the family drive with the fake CLI, before Doctor and Evidence.
The retained seed path under target is too long for control.sock, so copy its checkpointed database into the stopped app's short disposable DATA folder.
The source database remains available for later measurements.
The following section runs in a fresh shell with only RUN carried forward.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
set -eu
RUN=${RUN:?export the RUN that Launch printed}
PROOF=target/verify-veyra/$RUN
VEYRA_CLAUDE_BIN=/nonexistent/claude VEYRA_CODEX_BIN=/nonexistent/codex TMPDIR=/tmp cargo test -p veyra-engine --lib perf_snapshot_large -- --ignored --nocapture >"$PROOF/large-seed.txt" 2>&1
PYTHONPATH="$PROOF" python3 - <<'PYPROBE'
from driver import *
identity()
thread = create('probe-pause','Observe the halfway pause',scenario='stream-pause')
done = ctl('probe-pause-done','wait',thread,'Done')
assert done['perf']['visible']['applied_batches'] > 0
assert done['perf']['visible']['max_batch_gap_ms'] > 100
save('probe-pause-reading',done['perf'])
capture('probe-pause')
poll('probe-all-idle',lambda d:sql('select count(*) from threads where live_pgid is not null')=='0')
mode = ctl('probe-before-copy','dump')['settings']['default_mode']
stop('probe-before-copy')
seed = re.search(r'actor_ms=([0-9.]+) thread=([0-9a-f-]+) database=(.+)',(PROOF/'large-seed.txt').read_text())
assert seed
for suffix in ['-wal','-shm']: DB.with_name(DB.name+suffix).unlink(missing_ok=True)
shutil.copy2(pathlib.Path(seed[3]),DB)
start('probe-large')
action('probe-restore-default','SetDefaultMode',{'mode':mode})
poll('probe-default-restored',lambda d:d['settings']['default_mode']==mode)
thread = seed[2]
select('probe-large-open',thread)
opened = poll('probe-large-stamped',lambda d:len(d['transcript']['items'])==5000 and d['perf']['transcript_open_ms'] is not None)
save('probe-large-reading',{'items':len(opened['transcript']['items']),'actor_ms':float(seed[1]),
    'transcript_open_ms':opened['perf']['transcript_open_ms'],'source_database':seed[3]})
capture('probe-large')
print('PASS probe smokes; pause gap='+str(done['perf']['visible']['max_batch_gap_ms'])
    +'ms; large open='+str(opened['perf']['transcript_open_ms'])+'ms')
PYPROBE
```

The named reading JSON files record the two window measurements, and large-seed.txt records the actor measurement and retained source path.
The 5,000-item dump exercises the client's 4 MiB reply limit; the command limit remains 64 KiB.
The original family drive's dumps, screenshots and SQL receipts remain in PROOF after the disposable database is replaced.

### Codex

Run this fake-only section after the existing drive and before Evidence.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
set -eu
RUN=${RUN:?export the RUN that Launch printed}
PROOF=target/verify-veyra/$RUN
PYTHONPATH="$PROOF" python3 - <<'PYCODEX'
from driver import *
identity()
assert LANE == 'fake'
thread = create_codex('codex-relay',scenario='root-tool-call')
done = ctl('codex-relay-wait','wait',thread,'Done')
save('codex-relay-done',done)
assert any(item['kind']=='tool' and 'codex' in item['text'] and 'remaining_fan_out' in item['text'] for item in done['transcript']['items'])
assert 'remaining_fan_out' in sql("select body_json from items where thread_id='"+thread+"' and kind='tool'")
log = (DATA/'logs/veyra.log').read_text()
lines = [line for line in log.splitlines() if thread in line and ('MCP connection accepted' in line or 'list_providers' in line)]
assert any('MCP connection accepted' in line for line in lines)
assert any('list_providers' in line for line in lines)
(PROOF/'codex-relay-log.txt').write_text('\n'.join(lines)+'\n')
capture('codex-relay')
poll('codex-relay-idle',lambda d:sql('select count(*) from threads where live_pgid is not null')=='0')
restart('codex-family-fresh-lanes')
poll('codex-family-providers-ready',lambda d:all(d['providers'][i]['readiness']=='ready' for i in [0,1]))
parent = [json.loads(line) for line in (CODEX_SOURCE/'claude-parent-codex-child.jsonl').read_text().splitlines()]
child = codex_records('child-policy-full-access',root=False)
for record in child:
    method = record.get('line',{}).get('method')
    if method=='item/agentMessage/delta': record['after_ms']=2000
    if method=='turn/completed': record['after_ms']=1000
for lane,records in [(1,parent),(2,child)]:
    (FIXTURES/('t'+str(lane)+'-s1.jsonl')).write_text('\n'.join(json.dumps(r) for r in records)+'\n')
action('codex-family-default','SetDefaultMode',{'mode':'full_access'})
action('codex-family-new','NewThread')
action('codex-family-parent-provider','PickProvider',{'provider':'claude'})
ctl('codex-family-prompt','text','Start a Codex child')
ctl('codex-family-send','keys','secondary-enter')
root = poll('codex-family-root',lambda d:d['selected_thread'] is not None and d['selected_thread']['title']=='Start a Codex child')['selected_thread']['id']
poll('codex-family-task',lambda d:any(task['parent']==root for task in d['tasks']))
child,task = sql("select child_thread_id||'|'||id from tasks where parent_thread_id='"+root+"'").split('|')
working = poll('codex-family-working',lambda d:d['transcript']['family_card'] is not None and d['transcript']['family_card']['rows'][0]['status']=='Working')
assert working['transcript']['family_card']['title']=='Started 1 agent'
opened = select('codex-child-task',child)
assert opened['composer']['provider']=='codex' and opened['selected_thread']['config']['mode']=='full_access'
assert any(item['kind']=='task_card' and item['text']=='Task from Claude Code' for item in opened['transcript']['items'])
assert opened['selected_thread']['breadcrumb']=={'thread':root,'title':'Start a Codex child','linked':True}
assert sql("select provider from threads where id='"+child+"'")=='codex'
capture('codex-child-task')
family_delivered('codex-family-delivered',root,task)
select('codex-family-done',root)
done = poll('codex-family-done',lambda d:d['transcript']['family_card']['rows'][0]['status']=='Done')
assert any(item['turn_label']=='Reports from 1 child thread' for item in done['transcript']['items'])
assert sql("select summary||'|'||delivery from tasks where id='"+task+"'")=='ok|delivered'
capture('codex-family-done')
poll('codex-family-idle',lambda d:sql('select count(*) from threads where live_pgid is not null')=='0')
action('codex-family-restore-default','SetDefaultMode',{'mode':'ask_first'})
action('codex-family-return-new','NewThread')
action('codex-family-return-claude','PickProvider',{'provider':'claude'})
page_done('multi-agent-codex')
print('PASS Codex root production relay list_providers and Claude root/Codex child card and delivered wake')
PYCODEX
```

## Observable success

The Codex root section saves the production relay result in `transcript.items`, SQLite tool rows and `codex-relay-log.txt`.
That log contains the root thread's accepted connection and `list_providers` call.
The Claude-root/Codex-child section checks `composer.provider`, the child's `selected_thread.config.mode`, `breadcrumb`, TaskCard and family row.
SQLite records provider `codex`, summary `ok` and delivered task state; the PNGs show the child card and the completed family.

The top-level `tasks` dump key adds task state and Pending or Delivered, while `sidebar`, `selected_thread` and `transcript` contain the lineage and rendered controls.
The production relay's saved pid and parent match the root's live_pid; SQLite and the log show a real start_thread call.
The tools `list_providers`, `start_thread`, `get_task`, `message_thread` and `stop_thread` have engine-owned descriptions, checked by verify_map_names.
Both pages name `Archive` and `ArchiveKeepChildren`, the two actions behind Archive and Archive, keep child threads.
Working and Done family screenshots show the same child row change; the task card and final report screenshot show turn_label and report_delivered.
The held screenshots and SQLite show a pending wake, and Continue changes its delivery to delivered.
The archive screenshots and queries prove hidden families, kept top-level children and a single-child archive holding its unarchived parent for the report.
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
