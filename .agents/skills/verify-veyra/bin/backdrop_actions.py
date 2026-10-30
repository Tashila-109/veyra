"""Notification backdrop activation entry, shared by the feature recipe and raw-I/O tests."""
def activate(label,pid,executable,bundle):
    import driver as d
    def command(name,args):
        result=d.subprocess.run(args,capture_output=True,text=True)
        (d.PROOF/(name+'.stdout')).write_text(result.stdout)
        (d.PROOF/(name+'.stderr')).write_text(result.stderr)
        d.save(name+'-command',{'command':args,'exit':result.returncode})
        assert result.returncode==0, f'owned backdrop helper failed; see {name}.stderr'
        return result
    d.verify_owned_backdrop(pid,executable)
    reopened=d.reopen_owned_backdrop(pid,executable,lambda:command(label+'-open',['open',str(bundle)]))
    source=('import AppKit\n'+f'let pid: Int32 = {pid}\n'+
        'let end = Date().addingTimeInterval(5)\n'+
        'while NSWorkspace.shared.frontmostApplication?.processIdentifier != pid && Date() < end { '+
        'RunLoop.current.run(until: Date().addingTimeInterval(0.05)) }\n'+
        'guard NSWorkspace.shared.frontmostApplication?.processIdentifier == pid else { exit(2) }\nprint(pid)\n')
    result=command(label,['swift','-e',source])
    assert int(result.stdout.strip())==pid
    d.save(label,{'frontmost_pid':pid,'owned_backdrop_pid':pid})
    return reopened
