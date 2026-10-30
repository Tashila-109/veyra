"""The frame probe's macOS process/window/clock/IPC boundary; checks stay on the shared probe path."""
import importlib, importlib.machinery, importlib.util, json, os, pathlib, secrets, shutil
import subprocess, threading, time
import orchestration as o
from commit_watch import CommitWatch
from frame_scene import require_6d_dump

class NativeWindow:
    clock = time
    def __init__(self):
        self.workspace=None
        self.backdrop=None
        self.watch=None
        self.d=None
        self.data=None
        self.data_created=False
        self.scratch=None
    def start(self,args,tools,baseline,result):
        import tempfile
        self.args,self.result=args,result
        self.original_cwd=pathlib.Path.cwd()
        os.environ.pop('VEYRA_VERIFY_RULE_INDEX',None)
        self.scratch=pathlib.Path(tempfile.mkdtemp(prefix='veyra-frame-work-',dir='/tmp'))
        self.run=secrets.token_hex(4)
        self.data=pathlib.Path('/tmp/veyra-frame-'+self.run);self.data.mkdir();self.data_created=True
        self.workspace=self.scratch/'workspace'
        (self.workspace/'target/debug').mkdir(parents=True)
        for name in ['crates','.agents']: (self.workspace/name).symlink_to(o.REPO/name)
        target=baseline if args.side=='baseline' else o.REPO/'target'
        for name in ['veyra','veyra-fake-cli']:
            (self.workspace/'target/debug'/name).symlink_to(target/'debug'/name)
        self.proof=self.workspace/'target/verify-veyra'/self.run
        (self.proof/'fixtures').mkdir(parents=True)
        (self.proof/'lane').write_text('fake\n');(self.proof/'pages-run').write_text('')
        (self.proof/'configured-default-mode').write_text('full_access\n')
        o.write(self.proof/'helper-ownership.json',{'run':self.run,'workspace':str(self.workspace),
            'scratch':str(self.scratch),'data':str(self.data),'proof':str(self.proof)})
        for name,path in o.helper_paths(tools).items(): (self.proof/name).symlink_to(path)
        for file,provider,source in [('probe.jsonl','claude','probe-initialize.jsonl'),
                ('probe-codex.jsonl','codex','probe-ready.jsonl'),('probe-grok.jsonl','grok','probe-grok.jsonl')]:
            shutil.copyfile(o.REPO/'crates/engine/tests/fixtures'/provider/source,self.proof/'fixtures'/file)
        for name in ['project','appcwd']: (self.data/name).mkdir()
        shell=self.data/'login-shell';shell.write_text('#!/bin/sh\nexec /bin/sh "$@"\n');shell.chmod(0o700)
        os.environ.update(RUN=self.run,VEYRA_VERIFY_TMP='/tmp/veyra-frame-')
        os.chdir(self.workspace)
        self.d=importlib.import_module('driver')
        self.d.ENV['SHELL']=str(shell);self.d.ENV.pop('VEYRA_RECORD',None)
        self.d.gate()
        result['gate']=json.loads((self.proof/'input-idle-launch.json').read_text())
        result['load_start']=subprocess.check_output(['sysctl','-n','vm.loadavg'],text=True).strip()
        self.d.make_git_shim()
        pid=self.d.start('launch',recording=False,baseline=baseline if args.side=='baseline' else None)
        threading.Thread(target=lambda:os.waitpid(pid,0),daemon=True).start()
        self.pid=pid
        from window_wait import wait_for_active
        def record(value):
            result['activation']=value
            result['activation_wait_seconds']=value['wait_seconds']
            o.write(self.proof/'activation-wait.json',value)
            if not value['frontmost']['frontmost_is_owned']: result['foreign_activation']={'step':'launch-window-wait',**value['frontmost']}
        try:
            wait_for_active(lambda:self.d.action('probe-activate','ActivateWindow'),
                lambda:self.d.ctl('probe-active','dump',log=False),lambda:self.frontmost('launch'),record)
        except AssertionError:
            self.d.observe_failure('probe-active','focus-timeout');raise
        self.d.poll('probe-login-ready',lambda state:state.get('login_path')!='pending')
        self.d.action('probe-solid','SetWindowBackground',{'background':'solid'})
        result.update(run=self.run,pid=pid,expected_rev=(self.proof/'build-rev').read_text().strip(),
            binary_sha256=o.sha(target/'debug/veyra'),fake_sha256=o.sha(target/'debug/veyra-fake-cli'))
    def frontmost(self,name):
        command=[str(self.proof/'front-app'),str(self.pid)]
        o.write(self.proof/(name+'-frontmost-invocation.json'),command)
        raw=json.loads(subprocess.check_output(command,text=True))
        value={key:raw[key] for key in ['owned_pid','frontmost_is_owned']}
        assert value['owned_pid']==self.pid and type(value['frontmost_is_owned']) is bool
        o.write(self.proof/(name+'-frontmost.json'),value)
        return value
    def read(self,name):
        if self.d.locked():
            self.d.observe_failure(name,'capture');raise AssertionError('screen is locked')
        state=self.d.ctl(name,'dump',log=False)
        assert os.getpgid(self.pid)==self.pid==int((self.proof/'pgid').read_text())
        assert (self.data/'veyra.lock').read_text().strip()==str(self.pid)
        assert state['pid']==self.pid and state['data_dir']==str(self.data) and state['build_rev']==self.result['expected_rev']
        if not state['window_active']:
            ownership=self.frontmost('inactive')
            if not ownership['frontmost_is_owned']: self.result['foreign_activation']={'step':name,**ownership}
            self.d.observe_failure(name,'focus-timeout');raise AssertionError('probe window became inactive')
        if (self.proof/'backdrop-windows.json').exists(): self.front(name)
        return state
    def front(self,name):
        if self.d.locked():
            self.d.observe_failure(name,'capture');raise AssertionError('screen is locked')
        command=[str(self.proof/'capture-window'),str(self.pid),(self.proof/'window-id').read_text().strip()]
        o.write(self.proof/(name+'-front-invocation.json'),command)
        checked=subprocess.run(command,capture_output=True,text=True)
        o.write(self.proof/(name+'-front.json'),{'exit':checked.returncode,'stderr':checked.stderr})
        if checked.returncode:
            self.result['cover_trip']='covered' in checked.stderr or 'intersecting' in checked.stderr
            ownership=self.frontmost('inactive')
            if not ownership['frontmost_is_owned']: self.result['foreign_activation']={'step':name,**ownership}
            self.d.observe_failure(name,'capture');raise AssertionError('frontmost guard failed: '+checked.stderr)
        try:
            value=self.d.region_snapshot(self.pid,int((self.proof/'window-id').read_text()),
                json.loads((self.proof/'backdrop-windows.json').read_text()),(24,60,960,740))
            o.write(self.proof/(name+'-region-guard.json'),value)
        except Exception:
            self.d.observe_failure(name,'capture');raise
    def prepare(self,args,backdrop):
        d=self.d
        d.prepare()
        executable=self.data/'perf-backdrop';shutil.copy2(backdrop,executable)
        log=self.proof/'backdrop.stdout'
        self.backdrop=subprocess.Popen([str(executable)],stdout=log.open('w'),stderr=(self.proof/'backdrop.stderr').open('w'))
        threading.Thread(target=self.backdrop.wait,daemon=True).start()
        (self.proof/'backdrop-owned-pids').write_text(str(self.backdrop.pid)+'\n')
        (self.proof/'notifications-backdrop-pid').write_text(str(self.backdrop.pid)+'\n')
        o.write(self.proof/'notifications-backdrop-ownership.json',{'pid':self.backdrop.pid,'executable':str(executable)})
        end=time.monotonic()+10
        while 'backdrop ready pid=' not in log.read_text():
            assert self.backdrop.poll() is None and time.monotonic()<end, 'owned backdrop unavailable'
            time.sleep(.05)
        rows=[json.loads(line.split('backdrop window ',1)[1]) for line in log.read_text().splitlines() if line.startswith('backdrop window ')]
        assert rows and all(row['pid']==self.backdrop.pid for row in rows)
        o.write(self.proof/'backdrop-windows.json',{'pid':self.backdrop.pid,'windows':[row['window'] for row in rows]})
        d.verify_owned_backdrop(self.backdrop.pid,executable)
        before=d.owned_window_record('probe-placement-before');assert before is not None, 'probe placement requires yabai'
        assert before['frame']['w']==960 and before['frame']['h']==740, 'owned probe size changed'
        tool=shutil.which('yabai');window=(self.proof/'window-id').read_text().strip()
        visible=json.loads((self.proof/'launch-normalized-window.json').read_text())['visible_frame']
        x,y,w,h=visible;assert x<=24 and y<=60 and 24+960<=x+w and 60+740<=y+h, 'probe rectangle outside display'
        for operation,value in [('--resize','abs:960:740'),('--move','abs:24:60')]:
            d.owned_window_record('probe-placement-check')
            subprocess.run([tool,'-m','window',window,operation,value],check=True,capture_output=True)
        end=time.monotonic()+5
        while True:
            after=d.owned_window_record('probe-placement-after')
            if after['frame']=={'x':24,'y':60,'w':960,'h':740}: break
            assert time.monotonic()<end, 'owned probe did not reach the proved rectangle'
            time.sleep(.05)
        self.result['probe_placement']={'before':before['frame'],'after':after['frame'],'visible_frame':visible,'rectangle':[24,60,960,740]}
        self.result['bounds']=subprocess.check_output([str(self.proof/'winid'),str(self.pid),'--bounds'],text=True).strip()
        d.action('probe-return-focus','ActivateWindow');d.poll('probe-active-again',lambda state:state['window_active'])
        self.front('prepared')
        if args.scene=='idle':
            d.drawer_fixture();thread=d.create('idle','Idle with an owned shell')
            d.ctl('idle-done','wait',thread,'Done')
            d.action('idle-translucent','SetWindowBackground',{'background':'translucent'})
            d.poll('idle-translucent-applied',lambda state:state['look']['background']=='translucent')
            d.ctl('idle-open-drawer','keys','secondary-j')
            d.poll('idle-shell-ready',lambda state:state['drawer']['terminal'] and
                'VEYRA_TUI_READY' in ''.join(state['drawer']['terminal']['grid']) and not state['drawer']['tabs'][0]['running'])
            if args.part=='6d':
                state=self.read('idle-capabilities');require_6d_dump(state)
                if state['palette']['open']: d.ctl('idle-close-palette','keys','escape')
                assert state['composer_text']==''
                d.action('idle-toast','Send')
                d.poll('idle-toast-present',lambda state:state['toasts'] and not state['palette']['open'])
            d.action('idle-focus','FocusComposer' if args.idle_focus=='composer' else 'DebugFocusShellDrawer')
            d.poll('idle-focus-ready',lambda state:state['focus']==args.idle_focus and
                state['drawer']['terminal']['focused']==(args.idle_focus=='drawer'))
            self.result.update(idle_variant='a' if args.idle_focus=='composer' else 'b',focus_target=args.idle_focus)
            time.sleep(.5)
    def stream(self):
        d=self.d
        records=[json.loads(line) for line in (d.SOURCE/'stream-2000.jsonl').read_text().splitlines()]
        expected=''.join(record['line']['event']['delta']['text'] for record in records if record.get('type')=='emit_inbound'
            and record['line'].get('event',{}).get('delta',{}).get('type')=='text_delta')
        assert len(expected)==2000
        d.stage('stream-2000');d.action('perf-new','NewThread');d.ctl('perf-text','text','stream 2000 deltas')
        before=self.read('perf-before');assert before['perf']['max_batch_gap_ms']==0
        self.watch=CommitWatch(d.DB).start()
        cpu0=self.cpu();started=time.monotonic();next_read=started+1;d.ctl('perf-send','keys','secondary-enter')
        thread=None
        while True:
            assert time.monotonic()-started<60, 'stream deadline'
            if time.monotonic()>=next_read:
                self.read('perf-active');next_read=time.monotonic()+1
            if thread is None:
                rows=d.sql('select id from threads').split();thread=rows[0] if rows else None
            elif d.sql("select count(*) from turns where thread_id='"+thread+"' and state='ended'")=='1': break
            time.sleep(.2)
        d.ctl('perf-done','wait',thread,'Done','30');after=self.read('perf-after');after_dump=time.time();cpu1=self.cpu()
        observed=self.watch.stop();self.watch=None;o.write(self.proof/'commit-watch.json',observed)
        items=[item for item in after['transcript']['items'] if item['kind']=='assistant']
        assert len(items)==1 and items[0]['text']==expected and items[0]['done'] and items[0]['delta_count']==2000
        assert d.sql("select json_extract(body_json,'$.AssistantText.text')||'|'||done from items where kind='assistant_text'")==expected+'|1'
        start,end,outcome=d.sql("select started_at||'|'||ended_at||'|'||outcome_json from turns").split('|')
        assert json.loads(outcome)=='Completed'
        seconds=(int(end)-int(start))/1000
        batches=after['perf']['applied_batches']-before['perf']['applied_batches']
        writes=sum(row['kind']=='text_write' and row['t']<=after_dump for row in observed['changes'])
        commits=after['perf']['commits']-before['perf']['commits']
        d.poll('stream-sessions-ended',lambda state:d.sql('select count(*) from threads where live_pgid is not null')=='0')
        return {'core_fraction':(cpu1-cpu0)/seconds,'turn_seconds':seconds,'cpu_seconds':cpu1-cpu0,
            'batches':batches,'batches_per_second':batches/seconds,'largest_gap_ms':after['perf']['max_batch_gap_ms'],
            'repaints':after['perf']['repaints']-before['perf']['repaints'],'commits':commits,
            'commits_text_writes':writes,'commits_fixed':commits-writes,'commits_10s':commits-writes+5,
            'exact_2000_deltas':True,'stored_text_exact':True}
    def cpu(self):
        row=subprocess.check_output(['ps','-o','pid=,pgid=,rss=,stat=,cputime=','-p',str(self.pid)],text=True).split()
        assert int(row[0])==int(row[1])==self.pid
        value=0.0
        for part in row[-1].split(':'): value=value*60+float(part)
        return value
    def working(self,motion):
        d=self.d;threads=[]
        fixture=d.fixture_source([json.loads(line) for line in (d.SOURCE/'long-reply.jsonl').read_text().splitlines()])
        for lane in range(1,6):
            for epoch in range(1,9): d.link_fixture(f't{lane}-s{epoch}.jsonl',fixture)
        for index in range(5):
            thread=d.create('working-'+str(index),'Working '+str(index),scenario=None);threads.append(thread)
            d.ctl('working-ready','wait',thread,'Working')
        d.action('working-nontext','ScrollTranscriptUp')
        flags={key:self.read('working-flags')['look'][key] for key in ['reduce_motion','reduce_transparency','increase_contrast']}
        flags['reduce_motion']=motion=='reduced';d.action('working-motion','DebugDisplayOptions',flags)
        d.poll('working-ready',lambda state:state['focus']=='none' and all(state['look'][key]==value for key,value in flags.items()))
        time.sleep(.5)
        before=self.read('working-before');self.assert_working(before,flags);self.front('working-before');started=time.monotonic();observations=[]
        while time.monotonic()-started<10:
            state=self.read('working-sample')
            self.assert_working(state,flags)
            observations.append({'seconds':time.monotonic()-started,'repaints':state['perf']['repaints'],'focus':state['focus'],'window_active':state['window_active']})
            time.sleep(.1)
        after=self.read('working-after');self.assert_working(after,flags);self.front('working-after');seconds=time.monotonic()-started
        for thread in threads: d.action('working-stop','StopThread',{'thread':thread})
        d.poll('working-sessions-ended',lambda state:d.sql('select count(*) from threads where live_pgid is not null')=='0')
        delta=after['perf']['repaints']-before['perf']['repaints']
        return {'scene':'working','seconds':seconds,'repaints':delta,'repaints_per_second':delta/seconds,
            'before':before,'after':after,'observations':observations,'measurement_complete':True,
            'display_options_override':flags,'macos_settings_changed':False}
    @staticmethod
    def assert_working(state,flags):
        assert state['focus']=='none' and state['look']['background']=='solid'
        assert sum(row.get('status')=='Working' for row in state['sidebar']['rows'])==5
        assert all(state['look'][key]==value for key,value in flags.items())
    def finish(self,result):
        if self.args.scene=='idle':
            before,after=result['before'],result['after']
            terminal_repaints=after['drawer']['terminal']['repaints']-before['drawer']['terminal']['repaints']
            frame_requests=after['drawer']['tabs'][0]['frame_requests']-before['drawer']['tabs'][0]['frame_requests']
            shell_pid=int(before['drawer']['tabs'][0]['pid'])
            self.d.action('idle-close-focus','DebugFocusShellDrawer')
            self.d.poll('idle-close-focused',lambda state:state['focus']=='drawer')
            self.d.ctl('idle-close-tab','keys','secondary-w')
            self.d.poll('idle-drawer-hidden',lambda state:not state['drawer']['open'])
            end=time.monotonic()+5
            while True:
                try: os.kill(shell_pid,0)
                except ProcessLookupError: break
                assert time.monotonic()<end, 'owned bare-prompt shell remained after close'
                time.sleep(.05)
            self.d.poll('idle-sessions-ended',lambda state:self.d.sql('select count(*) from threads where live_pgid is not null')=='0')
            return {'owned_shell_pid':shell_pid,'owned_shell_gone':True,'idle_rule_evaluated_by_supervisor':True,
                'terminal_repaints':terminal_repaints,'terminal_frame_requests':frame_requests}
        return {}
    def close(self,output):
        empty={'cleanup_exit':0,'data_removed':True,'scratch_removed':True,'recording_off':True,'owned_rule_cleanup':{'passed':True}}
        if self.workspace is None:
            if self.data_created: shutil.rmtree(self.data)
            if self.scratch is not None: shutil.rmtree(self.scratch)
            return empty
        if self.watch is not None:
            try: self.watch.stop()
            except Exception as error: self.result.update(passed=False,watch_error=str(error))
        if self.d is None:
            if self.data is not None: shutil.rmtree(self.data,ignore_errors=True)
            shutil.rmtree(self.scratch);os.chdir(self.original_cwd);return empty
        loader=importlib.machinery.SourceFileLoader('frame_cleanup',str(o.BIN/'run'))
        spec=importlib.util.spec_from_loader(loader.name,loader);runner=importlib.util.module_from_spec(spec);loader.exec_module(runner)
        env=dict(os.environ,PYTHONPATH=str(o.BIN),RUN=self.run,VEYRA_VERIFY_TMP='/tmp/veyra-frame-')
        code=0
        recording=not (self.proof/'rec-current').exists() and not list(self.proof.rglob('rec')) and not list(self.data.rglob('rec'))
        o.write(self.proof/'recording-absent.json',{'recording_directories':[str(path) for root in [self.proof,self.data] for path in root.rglob('rec')],
            'rec_current_exists':(self.proof/'rec-current').exists(),'values_recorded':False})
        try:
            idle=self.d.input_idle_seconds()
            clock=json.loads((self.proof/'input-idle-launch.json').read_text()) if (self.proof/'input-idle-launch.json').exists() else {}
            observed=json.loads((self.proof/'focus-failure.json').read_text()) if (self.proof/'focus-failure.json').exists() else None
            self.result.update(load_end=subprocess.check_output(['sysctl','-n','vm.loadavg'],text=True).strip(),
                human_input=bool(clock and idle<time.monotonic()-clock['launch_monotonic']),observed=observed,idle_end=idle)
        except Exception as error: self.result.update(passed=False,diagnostic_error=str(error))
        if (self.proof/'pid').exists():
            with (self.proof/'cleanup.log').open('w') as log:
                code=subprocess.run(['bash','-c',runner.sections()[('SKILL','Cleanup')]],cwd=self.workspace,env=env,
                    stdout=log,stderr=subprocess.STDOUT).returncode
        else:
            self.d.cleanup_window_rules(self.proof,self.run);shutil.rmtree(self.data)
        owned=json.loads(max(self.proof.glob('yabai-rule-cleanup-*.json'),key=lambda p:p.stat().st_mtime_ns).read_text()) \
            if (self.proof/'owned-yabai-labels').exists() else {'passed':True}
        assert owned['passed'], 'guard owned-rule cleanup failed'
        shutil.copytree(self.proof,output/'run')
        os.chdir(self.original_cwd)
        if code==0 and not self.data.exists(): shutil.rmtree(self.scratch)
        return {'cleanup_exit':code,'data_removed':not self.data.exists(),'scratch_removed':not self.scratch.exists(),
            'recording_off':recording,'owned_rule_cleanup':owned}
