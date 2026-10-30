"""Failure telemetry shared by the driver and the section runner; only raw OS reads live here."""
import json, pathlib, subprocess, time

def observe(proof, step, kind, source='driver'):
    proof=pathlib.Path(proof)
    clock=proof/'input-idle-launch.json'
    errors=[]
    try: value=json.loads(clock.read_text()) if clock.exists() else {}
    except (OSError,ValueError) as error:
        value={};errors.append('launch clock: '+str(error))
    value.update(failure=step,kind=kind,failure_epoch=time.time(),observation_source=source,
        failure_idle_seconds=None,screen_locked=None,human_input=None)
    try:
        raw=subprocess.check_output(['ioreg','-c','IOHIDSystem'],text=True,timeout=5)
        value['failure_idle_seconds']=next(int(line.split()[-1])//1_000_000_000
            for line in raw.splitlines() if 'HIDIdleTime' in line)
    except Exception as error: errors.append('HID: '+str(error))
    try:
        value['screen_locked']='CGSSessionScreenIsLocked' in subprocess.check_output(['ioreg','-n','Root','-d1','-a'],text=True,timeout=5)
    except Exception as error: errors.append('lock: '+str(error))
    if 'launch_monotonic' in value:
        elapsed=time.monotonic()-value['launch_monotonic'];value['seconds_since_launch']=elapsed
        if value['failure_idle_seconds'] is not None: value['human_input']=value['failure_idle_seconds']<elapsed
    value['classification']='unknown' if value['human_input'] is None else 'human-input' if value['human_input'] else 'no-human-input'
    if errors: value['observation_errors']=errors
    try:
        proof.mkdir(parents=True,exist_ok=True)
        (proof/'focus-failure.json').write_text(json.dumps(value,indent=2)+'\n')
    except OSError as error:
        errors.append('observation receipt: '+str(error));value['human_input']=None;value['classification']='unknown'
    if errors: value['observation_errors']=errors
    return value
