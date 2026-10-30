"""Guarded Solid/Translucent coarse/inverse pairs for a production glass hunk only."""
def drive():
    import driver as d
    import native_reads
    import threading
    d.prepare()
    saved = d.ctl('glass-smoke-before','dump')['look']
    pid = d.identity()
    window = int((d.PROOF/'window-id').read_text())
    rectangle = (24,60,960,740)
    before = d.owned_window_record('glass-smoke-placement-before')
    assert before is not None, 'glass smoke fixed placement requires yabai'
    tool = d.shutil.which('yabai')
    labels = []
    backdrop = None
    executable = d.DATA/'glass-smoke-backdrop'
    d.shutil.copy2(d.PROOF/'backdrop',executable)
    try:
        for operation,value in [('--resize','abs:960:740'),('--move','abs:24:60')]:
            d.owned_window_record('glass-smoke-placement-check')
            d.subprocess.run([tool,'-m','window',str(window),operation,value],check=True,capture_output=True)
        placed = d.owned_window_record('glass-smoke-placement-after')
        assert placed['frame']=={'x':24,'y':60,'w':960,'h':740}, 'glass smoke rectangle changed'
        d.action('glass-smoke-settings','OpenSettings')
        d.action('glass-smoke-options','DebugDisplayOptions',{'reduce_motion':True,'reduce_transparency':False,'increase_contrast':False})
        for inverse in [False,True]:
            label = 'inverse' if inverse else 'coarse'
            log = d.PROOF/('glass-smoke-'+label+'-backdrop.stdout')
            backdrop = d.subprocess.Popen([str(executable),'--coarse',*(['--invert'] if inverse else [])],
                stdout=log.open('w'),stderr=(d.PROOF/'glass-smoke-backdrop.stderr').open('a'))
            threading.Thread(target=lambda child=backdrop:child.wait(),daemon=True).start()
            with (d.PROOF/'backdrop-owned-pids').open('a') as output: output.write(str(backdrop.pid)+'\n')
            (d.PROOF/'notifications-backdrop-pid').write_text(str(backdrop.pid)+'\n')
            d.save('notifications-backdrop-ownership',{'pid':backdrop.pid,'executable':str(executable)})
            end = d.time.monotonic()+10
            while 'backdrop ready pid=' not in log.read_text():
                assert backdrop.poll() is None and d.time.monotonic()<end, 'owned backdrop unavailable'
                d.time.sleep(.05)
            rows = [d.json.loads(line.split('backdrop window ',1)[1]) for line in log.read_text().splitlines()
                if line.startswith('backdrop window ')]
            assert rows and all(row['pid']==backdrop.pid for row in rows)
            background = {'pid':backdrop.pid,'windows':[row['window'] for row in rows]}
            d.save('backdrop-windows',background)
            d.verify_owned_backdrop(backdrop.pid,executable)
            for mode in ['solid','translucent']:
                name = 'glass-smoke-'+mode+'-'+label
                d.action(name,'SetWindowBackground',{'background':mode})
                d.action(name+'-activate','ActivateWindow')
                d.poll(name+'-applied',lambda state:state['look']['background']==mode and state['window_active'])
                pre = d.region_snapshot(pid,window,background,rectangle)
                d.capture_content(name)
                post = d.region_snapshot(pid,window,background,rectangle)
                assert pre == post, 'glass smoke guards changed'
                labels.append(name)
            d.stop_owned_backdrop(backdrop.pid,executable)
            backdrop = None
        native_reads.request(d.PROOF,'glass-smoke-pairs',['read blur response to backdrop inversion in both modes'],labels)
        d.page_done('settings-glass-smoke')
    finally:
        if backdrop is not None: d.stop_owned_backdrop(backdrop.pid,executable)
        d.action('glass-smoke-options-restore','DebugDisplayOptions',{key:saved[key] for key in
            ['reduce_motion','reduce_transparency','increase_contrast']})
        d.action('glass-smoke-background-restore','SetWindowBackground',{'background':'solid'})
        d.poll('glass-smoke-restored',lambda state:state['look']['background']=='solid')
        for operation,value in [('--resize',f"abs:{before['frame']['w']}:{before['frame']['h']}"),
                ('--move',f"abs:{before['frame']['x']}:{before['frame']['y']}")]:
            d.owned_window_record('glass-smoke-restore-check')
            d.subprocess.run([tool,'-m','window',str(window),operation,value],check=True,capture_output=True)
        assert d.owned_window_record('glass-smoke-restored-window')['frame']==before['frame'], 'glass smoke restore failed'
        d.prepare()
