"""Read-only CommitWatch, with its database path passed explicitly."""
import sqlite3,threading,time

class CommitWatch:
    def __init__(self,db,known_threads=(),every=.01):
        self.known=sorted(known_threads)
        self.every=every
        self.conn=sqlite3.connect(f'file:{db}?mode=ro',uri=True,isolation_level=None,check_same_thread=False)
        self.changes=[]
        self.stopped=threading.Event()
        self.thread=threading.Thread(target=self.run)
    def snapshot(self):
        ids=','.join("'"+value+"'" for value in self.known) or "''"
        row=self.conn.execute(f"""select (select count(*) from threads), (select count(*) from turns),
            (select count(*) from turns where state='ended'), (select count(*) from threads where live_pgid is not null),
            (select group_concat(id||':'||length(coalesce(json_extract(body_json,'$.AssistantText.text'),''))||':'||done,',')
             from items where kind='assistant_text' and thread_id not in ({ids}))""").fetchone()
        items={}
        for part in (row[4] or '').split(','):
            if part:
                key,length,done=part.rsplit(':',2)
                items[key]=(int(length),int(done))
        return dict(threads=row[0],turns=row[1],ended=row[2],live=row[3],items=items)
    def version(self): return self.conn.execute('pragma data_version').fetchone()[0]
    def run(self):
        version,state=self.version(),self.snapshot()
        while not self.stopped.is_set():
            time.sleep(self.every)
            now=self.version()
            if now==version: continue
            new=self.snapshot()
            same=all(new[key]==state[key] for key in ['threads','turns','ended','live'])
            grew=[key for key,(length,done) in new['items'].items() if key in state['items']
                and state['items'][key][1]==0 and done==0 and length>state['items'][key][0]]
            other=[key for key in new['items'] if key not in grew and new['items'][key]!=state['items'].get(key)]
            self.changes.append(dict(t=time.time(),kind='text_write' if same and grew and not other else 'fixed',**new))
            version,state=now,new
    def start(self):
        self.thread.start()
        return self
    def stop(self):
        time.sleep(.1)
        self.stopped.set()
        self.thread.join()
        self.conn.close()
        text=[change for change in self.changes if change['kind']=='text_write']
        return dict(observed_changes=len(self.changes),text_writes=len(text),
            text_write_gaps_s=[round(b['t']-a['t'],3) for a,b in zip(text,text[1:])],changes=self.changes)
