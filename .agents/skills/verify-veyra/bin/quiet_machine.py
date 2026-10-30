import datetime
import json
import math
import pathlib
import re
import subprocess
import time

LIMIT = 5.0
INTERVAL_SECONDS = 15
TIMEOUT_SECONDS = 20 * 60
SYSCTL = ['/usr/sbin/sysctl', '-n', 'vm.loadavg']


class QuietMachineHold(RuntimeError):
    pass


def stamp():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def save(path, receipt):
    path = pathlib.Path(path)
    temporary = path.with_suffix(path.suffix + '.tmp')
    temporary.write_text(json.dumps(receipt, indent=2) + '\n')
    temporary.replace(path)


def parse_load(output):
    match = re.fullmatch(r'\s*\{\s*([0-9]+(?:\.[0-9]+)?)\s+([0-9]+(?:\.[0-9]+)?)\s+([0-9]+(?:\.[0-9]+)?)\s*\}\s*', output)
    if match is None:
        raise ValueError('invalid vm.loadavg response')
    values = [float(item) for item in match.groups()]
    if not all(math.isfinite(item) for item in values):
        raise ValueError('non-finite vm.loadavg response')
    return values[0]


def wait_for_quiet(path, invocation, progress=None, *, query=None,
                   monotonic=None, sleep=None, deadline=None):
    query = query or subprocess.run
    monotonic = monotonic or time.monotonic
    sleep = sleep or time.sleep
    started = monotonic()
    budget=TIMEOUT_SECONDS if deadline is None else min(TIMEOUT_SECONDS,max(0,deadline-time.time()))
    receipt = {'state': 'waiting', 'finished': False, 'launched': False,
               'invocation': invocation, 'sysctl_invocation': SYSCTL,
               'started_at': stamp(), 'load_average_period_minutes': 1,
               'load_limit_exclusive': LIMIT, 'machine_cores': 12,
               'sample_interval_seconds': INTERVAL_SECONDS,
               'timeout_seconds': TIMEOUT_SECONDS, 'deadline_epoch':deadline,'effective_timeout_seconds':budget,'samples': [],
               'load_at_launch': None}

    def persist():
        receipt['elapsed_seconds'] = monotonic() - started
        save(path, receipt)
        if progress:
            progress(receipt)

    persist()
    try:
        while monotonic() - started < budget:
            sample_started = monotonic()
            sample = {'sampled_at': stamp(), 'elapsed_seconds': sample_started - started}
            receipt['samples'].append(sample)
            try:
                result = query(SYSCTL, capture_output=True, text=True,
                               timeout=min(5, budget - (sample_started - started)),
                               check=False)
                sample.update(exit=result.returncode, stdout=result.stdout, stderr=result.stderr)
                if result.returncode != 0:
                    raise ValueError('vm.loadavg query failed')
                sample['load_1m'] = parse_load(result.stdout)
            except Exception as error:
                sample['error'] = type(error).__name__ + ': ' + str(error)
                receipt['reason'] = 'quiet-machine sample failed; no native launch'
                raise QuietMachineHold(receipt['reason']) from error
            persist()
            if monotonic() - started >= budget or (deadline is not None and time.time()>=deadline):
                break
            if sample['load_1m'] < LIMIT:
                receipt.update(state='admitted', finished=True, admitted_at=stamp(),
                               load_at_launch=sample['load_1m'],
                               launch_value_sample_index=len(receipt['samples']) - 1)
                persist()
                return receipt
            remaining = budget - (monotonic() - started)
            delay = min(max(0, INTERVAL_SECONDS - (monotonic() - sample_started)), remaining)
            if delay > 0:
                sleep(delay)
        receipt['reason'] = ('GO cutoff reached; no native launch' if deadline is not None and time.time()>=deadline
            else 'one-minute load average did not drop below 5.0 within 20 minutes; no native launch')
        raise QuietMachineHold(receipt['reason'])
    except BaseException:
        receipt.update(state='held', finished=True, ended_at=stamp(), load_at_launch=None)
        receipt.setdefault('reason', 'quiet-machine wait interrupted; no native launch')
        persist()
        raise


def record_launch(path, receipt, child_pid):
    assert receipt['state'] == 'admitted' and receipt['load_at_launch'] < LIMIT
    receipt.update(launched=True, child_pid=child_pid, launched_at=stamp())
    save(path, receipt)
