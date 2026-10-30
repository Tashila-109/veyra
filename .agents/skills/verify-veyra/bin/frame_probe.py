"""One native frame probe. Admission, part selection and sampling have one shared entry point."""
import os, pathlib, shutil, signal, sys
import orchestration as o
from frame_guard import authenticate_baseline
from frame_scene import parse_probe, require_part, measure_idle

def main(argv=None, window=None):
    args = parse_probe(sys.argv[1:] if argv is None else argv)
    baseline = pathlib.Path(os.environ['VEYRA_FRAME_BASELINE'])
    authenticate_baseline(baseline)
    expected = o.code_identity()
    tier = pathlib.Path(os.environ['VEYRA_VERIFY_TIER_S'])
    o.require_tier(tier,expected)
    o.require_go(os.environ['VEYRA_VERIFY_GO'],expected,float(os.environ['VEYRA_VERIFY_DEADLINE']),guard=True)
    tools = pathlib.Path(os.environ['VEYRA_FRAME_TOOLS']).resolve()
    assert tools == o.tier_tools(tier), 'probe tools differ from admitted tools'
    o.helper_paths(tools)
    output = pathlib.Path(os.environ['VEYRA_FRAME_GUARD_PROOF'])/args.phase/args.label
    output.mkdir(parents=True,exist_ok=False)
    o.write(output/'invocation.json',[sys.executable,str(o.BIN/'frame_probe.py'),*(sys.argv[1:] if argv is None else argv)])
    (output/'worker-pid').write_text(str(os.getpid())+'\n')
    result = {'phase':args.phase,'side':args.side,'label':args.label,'part':args.part,'passed':False,'measurement_started':False}
    handlers = {}
    if window is None:
        def interrupted(number, frame): raise KeyboardInterrupt('owned frame probe interrupted')
        for number in o.SIGNALS:
            handlers[number] = signal.signal(number,interrupted)
        from frame_native import NativeWindow
        window = NativeWindow()
    try:
        window.start(args,tools,baseline,result)
        initial = window.read('initial')
        require_part(initial,args.part,args.side)
        backdrop = output/'backdrop'
        shutil.copy2(tools/'backdrop',backdrop)
        window.prepare(args,backdrop)
        if args.scene=='idle':
            result.update(measure_idle(window,args.part,args.idle_focus))
        elif args.scene=='stream':
            result['measurement_started'] = True
            result.update(window.stream())
        else:
            result.update(window.working(args.motion))
        result.update(window.finish(result))
        result['passed'] = True
    except BaseException as error:
        result.update(error=str(error),cover_trip=result.get('cover_trip',False) or 'covered' in str(error))
        raise
    finally:
        previous = signal.pthread_sigmask(signal.SIG_BLOCK,o.SIGNALS)
        try:
            try:
                result.update(window.close(output))
                result['passed'] = result['passed'] and result['cleanup_exit']==0 and result['data_removed'] and result['scratch_removed']
            except BaseException as error:
                result.update(passed=False,cleanup_exit=1,data_removed=False,scratch_removed=False,
                    cleanup_error=str(error),owned_rule_cleanup={'passed':False})
                raise
            finally: o.write(output/'result.json',result)
        finally:
            for number,handler in handlers.items(): signal.signal(number,handler)
            signal.pthread_sigmask(signal.SIG_SETMASK,previous)
    return result

if __name__=='__main__':
    try: main()
    except BaseException:
        import traceback
        traceback.print_exc()
        raise SystemExit(1)
