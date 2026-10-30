"""Raw process/IPC/WindowServer/libproc fixtures for the actual native adapter and scene entries.

No driver, admission, scene, ownership or cleanup function is substituted. Unknown commands
remain denied by Fixture. All filesystem state and launches are synthetic, under the test root.
"""
import ctypes, errno, json, os, pathlib, plistlib, shutil, sys, tempfile
from types import SimpleNamespace
from unittest.mock import patch
from self_tests import Fixture
import orchestration as o

class Function:
    def __init__(self,call): self.call=call
    def __call__(self,*args): return self.call(*args)

class RawFixture(Fixture):
    def __init__(self,*args):
        super().__init__(*args)
        self.yabai=True;self.threads=[];self.selected=None;self.focus='composer';self.text=''
        self.look={'background':'solid','reduce_motion':True,'reduce_transparency':False,'increase_contrast':False}
        self.motion_payloads=[];self.corruption=None;self.launch_env=[];self.signals=[]
        self.backdrop_pid=99999998;self.backdrop_path=None;self.backdrop_alive=False
        self.path_reads=0;self.path_hook=None;self.keep_backdrop=False;self.on_signal=None;self.open_hook=None;self.opens=[]
        self.fail_half=None;self.half_failed=False;self.always_retry=False;self.half='';self.helper_reads=0;self.capture_events=[];self.capture_hook=None;
        self.raw_captures=0;self.lib_buffer=None;self.screen=(0,0,2000,1200);self.occlude_half=None;self.drawer_for_measure=False
    def __enter__(self):
        super().__enter__()
        self.stack.enter_context(patch.object(os,'getpgid',side_effect=self.getpgid))
        self.stack.enter_context(patch.object(os,'kill',side_effect=self.kill))
        self.stack.enter_context(patch.object(os,'waitpid',return_value=(self.pid,0)))
        self.stack.enter_context(patch.object(ctypes,'CDLL',side_effect=self.library))
        mkdtemp=tempfile.mkdtemp
        self.stack.enter_context(patch.object(tempfile,'mkdtemp',side_effect=lambda **kw:mkdtemp(prefix='native-work-',dir=self.root)))
        self.stack.enter_context(patch.dict(os.environ,VEYRA_FRAME_TMP=str(self.root)+'/native-data-'))
        for provider,name in [('claude','probe-initialize'),('codex','probe-ready'),('grok','probe-grok'),
                ('claude','long-reply'),('claude','ok-reply'),('tui','echo')]:
            p=self.repo/'crates/engine/tests/fixtures'/provider/(name+'.jsonl');p.parent.mkdir(parents=True,exist_ok=True);p.write_text('{}\n')
        return self
    def sync(self):
        d=sys.modules.get('driver')
        if d is not None:
            self.proof,self.data=d.PROOF,d.DATA
        return d
    def getpgid(self,pid):
        return pid+1 if self.corruption=='group' and self.motion_payloads and pid==self.pid else pid
    def kill(self,pid,number):
        self.signals.append((pid,number))
        if self.on_signal: self.on_signal(pid,number)
        if pid==self.backdrop_pid:
            if not self.backdrop_alive: raise ProcessLookupError(pid)
            if number and not self.keep_backdrop: self.backdrop_alive=False
        elif pid==self.pid:
            if not self.running: raise ProcessLookupError(pid)
            if number: self.running=False
        else: raise ProcessLookupError(pid)
    def library(self,name,*args,**kw):
        def path(pid,buffer,size):
            self.path_reads+=1
            value=self.path_hook(self.path_reads) if self.path_hook else self.backdrop_path if self.backdrop_alive else None
            if value is None: ctypes.set_errno(errno.ESRCH);return 0
            buffer.value=os.fsencode(str(value));return len(buffer.value)
        def windows(*args):
            x,y,w,h=self.frame.values()
            rows=[{'kCGWindowNumber':23,'kCGWindowOwnerPID':self.pid,'kCGWindowLayer':0,
                'kCGWindowBounds':{'X':x,'Y':y,'Width':w,'Height':h}},
                {'kCGWindowNumber':24,'kCGWindowOwnerPID':self.backdrop_pid,'kCGWindowLayer':0,
                'kCGWindowBounds':{'X':0,'Y':0,'Width':2000,'Height':1200}}]
            if self.half==self.occlude_half:
                rows.insert(0,{'kCGWindowNumber':25,'kCGWindowOwnerPID':99999997,'kCGWindowLayer':0,
                    'kCGWindowBounds':{'X':0,'Y':0,'Width':2000,'Height':421}})
            self.lib_buffer=ctypes.create_string_buffer(plistlib.dumps(rows));return 1
        def displays(limit,values,count):
            count._obj.value=1
            if values is not None: values[0]=1
            return 0
        return SimpleNamespace(sysctl=Function(lambda *args:0),proc_pidpath=Function(path),
            CGWindowListCopyWindowInfo=Function(windows),CFPropertyListCreateData=Function(lambda *args:1),
            CFDataGetLength=Function(lambda *args:len(self.lib_buffer.value)),
            CFDataGetBytePtr=Function(lambda *args:ctypes.addressof(self.lib_buffer)),CFRelease=Function(lambda *args:None),
            CGGetActiveDisplayList=Function(displays),CGDisplayBounds=Function(lambda *args:SimpleNamespace(x=0,y=0,width=2000,height=1200)))
    def dump(self):
        self.sync();state=super().dump()
        state.update(focus=self.focus,selected_thread=self.selected,composer_text=self.text)
        state['composer']['provider']='claude';state['sidebar']['rows']=list(self.threads)
        state['look']=dict(self.look)
        if self.drawer_for_measure: state['drawer']={'open':False,'tabs':[]}
        revision=self.proof/'build-rev'
        if revision.exists(): state['build_rev']=revision.read_text().strip()
        if self.corruption and self.motion_payloads:
            if self.corruption in ['pid','data_dir','build_rev']: state[self.corruption]=0 if self.corruption=='pid' else 'impostor'
            if self.corruption=='lock': (self.data/'veyra.lock').write_text('impostor')
        return state
    def command(self,argv,**options):
        argv=[str(v) for v in argv];name=pathlib.Path(argv[0]).name;self.sync()
        out=None
        if name=='bash':
            child=self.popen(argv,**options);stdout,stderr=child.communicate()
            if options.get('stdout'): options['stdout'].write(stdout+stderr)
            return SimpleNamespace(returncode=child.returncode,stdout=stdout,stderr=stderr)
        if name=='capture-window' and self.backdrop_alive:
            self.helper_reads+=1
            if self.helper_reads==2 and self.half==self.fail_half and (self.always_retry or not self.half_failed):
                self.half_failed=True
                if options.get('check'): raise subprocess.CalledProcessError(1,argv)
                return SimpleNamespace(returncode=1,stdout='',stderr='owned window is covered')
        if name=='veyra' and 'ctl' in argv:
            cmd=argv[argv.index('ctl')+1:]
            if cmd[0]=='action':
                action=cmd[1].split('::')[-1];data=json.loads(cmd[2]) if len(cmd)>2 else {}
                if action=='NewThread': self.selected=None;self.text='';self.focus='composer'
                if action=='ScrollTranscriptUp': self.focus='none'
                if action=='SetWindowBackground': self.look['background']=data['background']
                if action=='DebugDisplayOptions': self.look.update(data);self.motion_payloads.append(data)
                if action=='StopThread':
                    for row in self.threads:
                        if row['id']==data['thread']: row['status']='Done'
                out='{"ok":true}'
            elif cmd[0]=='text': self.text=cmd[1];out='{"ok":true}'
            elif cmd[:2]==['keys','secondary-enter']:
                row={'id':str(len(self.threads)+1),'status':'Working','kind':'thread'}
                self.threads.append(row);self.selected=row;self.text='';out='{"ok":true}'
            elif cmd[0]=='wait': out='{"ok":true}'
        elif name=='sqlite3':
            query=argv[-1]
            if query=='select count(*) from threads': out=str(len(self.threads))
            elif query=='select count(*) from projects': out='1'
        elif name=='visible-frame': out=json.dumps({'visible_frame':list(self.screen)})
        elif name=='front-app': out=json.dumps({'owned_pid':self.pid,'frontmost_is_owned':True})
        elif name=='ps': out='' if '-axo' in argv else 'Z'
        elif name=='ioreg' and 'Root' in argv and self.corruption=='screen' and self.motion_payloads: out='CGSSessionScreenIsLocked'
        elif name=='swift': out=str(self.backdrop_pid)
        elif name=='open':
            self.opens.append(argv)
            if self.open_hook: self.open_hook()
            out=''
        elif name=='yabai' and '--remove' in argv: out=''
        if out is not None:
            return SimpleNamespace(returncode=0,stdout=out if options.get('text') else out.encode(),stderr='' if options.get('text') else b'')
        if name=='screencapture':
            self.raw_captures+=1;self.capture_events.append((pathlib.Path(argv[-1]).name,self.raw_captures))
            if self.capture_hook: self.capture_hook()
        result=super().command(argv,**options)
        if name=='screencapture': pathlib.Path(argv[-1]).write_bytes(('raw capture '+str(self.raw_captures)).encode())
        return result
    def popen(self,argv,**options):
        name=pathlib.Path(str(argv[0])).name
        if name.startswith('perf-backdrop') or name in ['glass-smoke-backdrop','backdrop']:
            self.half='inverse' if '--invert' in argv else 'coarse';self.helper_reads=0
            assert pathlib.Path(argv[0]).read_bytes()==(self.tier.parent/'tools/backdrop').read_bytes(), 'backdrop executable differs from admitted helper'
            self.backdrop_alive=True;self.backdrop_path=pathlib.Path(argv[0]).resolve()
            options['stdout'].write('backdrop window '+json.dumps({'pid':self.backdrop_pid,'window':24})+'\nbackdrop ready pid='+str(self.backdrop_pid)+'\n');options['stdout'].flush()
            f=self
            class Child:
                pid=f.backdrop_pid
                def poll(self): return None if f.backdrop_alive else 0
                def wait(self,timeout=None): return 0
                def terminate(self): f.backdrop_alive=False
                def kill(self): f.backdrop_alive=False
            return Child()
        if str(argv[0])=='bash':
            self.calls.append(list(argv));f=self
            class Child:
                pid=f.pid
                def communicate(self,timeout=None):
                    try:
                        d=f.sync();d.cleanup_window_rules(f.proof,d.RUN)
                        from heredocs import HEREDOC
                        body=HEREDOC.search(f.runner.sections()[('SKILL','Cleanup')])['body']
                        with patch.object(sys,'argv',['cleanup',str(f.proof),'','']): exec(body,{'__name__':'__fixture_cleanup__'})
                        f.running=False;shutil.rmtree(f.data);self.returncode=0
                        return '',''
                    except BaseException:
                        import traceback
                        self.returncode=1;return '',traceback.format_exc()
            return Child()
        if name=='veyra':
            linked={p.name:p.resolve(strict=True) for p in self.proof.iterdir() if p.is_symlink()}
            assert linked=={name:(self.tier.parent/'tools'/name).resolve() for name in o.TOOLS}, 'native helper links differ from admitted catalog'
            d=self.sync();self.launch_env.append(dict(options['env']));self.app_launches+=1;self.running=True
            self.data.mkdir(exist_ok=True);(self.data/'veyra.lock').write_text(str(self.pid))
            (self.data/'logs').mkdir(exist_ok=True);(self.data/'veyra.db').write_text('raw SQL fixture')
            ready=f'veyra ready pid={self.pid} data_dir={self.data}\n'
            options['stderr'].write(ready);options['stderr'].flush();(self.data/'logs/veyra.log').write_text(ready)
            return SimpleNamespace(pid=self.pid,poll=lambda:None,returncode=None)
        return super().popen(argv,**options)
