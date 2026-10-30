import time


def wait_for_active(action, dump, front, record, *, clock=time.monotonic, sleep=time.sleep, bound=5.0):
    started = clock()
    action()
    observations = []
    while True:
        active = dump()['window_active']
        elapsed = clock() - started
        observations.append({'seconds': elapsed, 'window_active': active})
        if active or elapsed >= bound:
            ownership = front()
            value = {'bound_seconds': bound, 'wait_seconds': elapsed, 'window_active': active,
                     'frontmost': ownership, 'observations': observations}
            record(value)
            if not active:
                owner = 'owned app remained frontmost' if ownership['frontmost_is_owned'] else 'frontmost app was not owned'
                raise AssertionError(f'window never active within {bound:.1f} s; {owner}; elapsed {elapsed:.3f} s')
            return value
        sleep(min(.05, max(0, bound - elapsed)))
