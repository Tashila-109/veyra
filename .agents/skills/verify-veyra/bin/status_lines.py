"""Full-run status proposals are applied only after native reads, in the lead's docs-only change."""
import datetime, json, pathlib
import pages
from orchestration import write

def propose(folder, expected, attempts, status, names):
    write(pathlib.Path(folder)/'status-proposals.json',{'head':expected['head'],'full_run':True,
        'date':datetime.date.today().isoformat(),'pages':{
            name:('passed' if status=='DONE' else 'blocked' if status.startswith('BLOCKED') else 'failed')
            for name in names if name not in pages.LIVE},'native_reads_required':True,
        'outcome':status,'identity':expected})
