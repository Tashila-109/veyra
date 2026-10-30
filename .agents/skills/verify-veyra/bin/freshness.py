"""Capture freshness is a rendered frame plus a newly written image, independent of its pixels."""
import hashlib
from orchestration import SafetyStop

def retry_capture(operation, discard, attempts=3):
    """Every failed attempt discards its image, including the final failure."""
    def remove():
        try: discard()
        except Exception as error: raise SafetyStop('capture discard failed; privacy cannot be enforced') from error
    for attempt in range(attempts):
        try: return operation()
        except SafetyStop:
            remove()
            raise
        except Exception:
            remove()
            if attempt+1 == attempts: raise

def record(path, started_ns, pid, repaint, previous=None):
    metadata = path.stat()
    assert metadata.st_size > 0 and metadata.st_mtime_ns >= started_ns, 'old image file reused'
    if previous and previous['pid'] == pid:
        assert repaint > previous['repaints'], 'stale screenshot: window has not repainted since previous capture'
    return {'pid':pid,'repaints':repaint,'mtime_ns':metadata.st_mtime_ns,
        'sha256':hashlib.sha256(path.read_bytes()).hexdigest(),'image':path.name}
