#!/usr/bin/env python3
"""Page selection and fingerprints for verify-veyra; SKILL.md, section Selection, states each rule.

    pages.py select BASE [HEAD]          tokens a change from BASE selects; no HEAD reads the working tree
    pages.py fresh HEAD [--versions F]   pages whose Status FINGERPRINT equals their fingerprint at HEAD
    pages.py print COMMIT                each page's fingerprint at COMMIT
    pages.py self-test                   every rule, in a scratch repository
"""
import hashlib, os, pathlib, re, shutil, subprocess, sys, tempfile

SKILL = '.agents/skills/verify-veyra'
LIVE = {'permission-matrix'}

def rule_lists(text):
    """The glob lists of SKILL.md's Selection block, their one owner; an indented row continues the list above it."""
    section = re.search(r'^## Selection\n(.*?)(?=^## |\Z)',text,re.S|re.M)
    block = re.search(r'^```text\n(.*?)^```',section[1] if section else '',re.S|re.M)
    assert block, 'SKILL.md has no Selection block of glob lists'
    lists, name = {}, None
    for row in block[1].splitlines():
        if row[:1].isspace() and name: lists[name] += row.split()
        elif row.strip(): name,*globs = row.split(); lists[name] = globs
    missing = {'shared','no-page','codex','grok','provider','release'}-set(lists)
    assert not missing, 'Selection block lacks '+', '.join(sorted(missing))
    return lists

LISTS = rule_lists((pathlib.Path(__file__).resolve().parent.parent/'SKILL.md').read_text())
SHARED, NO_PAGE, CODEX, GROK, PROVIDER_SHARED, RELEASE = (LISTS[name] for name in
    ['shared','no-page','codex','grok','provider','release'])
SUFFIX = r'[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*'
VERSION = r'(?<![\d.])\d+(?:\.\d+)+(?:-'+SUFFIX+r')?(?:\+'+SUFFIX+r')?(?![\d+-]|\.\d)'
SECTIONS = {'codex':re.compile(r'^### Codex$',re.M),'grok':re.compile(r'^### Grok$',re.M)}

def git(repo, *args, data=False):
    out = subprocess.run(['git','-c','core.quotepath=off',*args],cwd=repo,capture_output=True,check=True).stdout
    return out if data else out.decode()

def glob_regex(glob):
    parts, index = [], 0
    while index < len(glob):
        if glob.startswith('**',index): parts.append('.*'); index += 2
        elif glob[index] == '*': parts.append('[^/]*'); index += 1
        else: parts.append(re.escape(glob[index])); index += 1
    return re.compile(''.join(parts)+r'\Z')

def matches(path, globs):
    return any(glob_regex(glob).match(path) for glob in globs)

def read(repo, rev, path):
    """A file's bytes at rev, or in the working tree when rev is None; None when absent."""
    if rev is None:
        full = pathlib.Path(repo)/path
        return full.read_bytes() if full.is_file() else None
    try: return git(repo,'show',f'{rev}:{path}',data=True)
    except subprocess.CalledProcessError: return None

def without_status(data):
    lines = data.decode().splitlines(keepends=True)
    index = next((i for i,line in enumerate(lines) if line.startswith('Status:')),None)
    return ''.join(lines[:index]+lines[index+1:] if index is not None else lines).encode()

def line(text, prefix):
    return next((row[len(prefix):].strip() for row in text.splitlines() if row.startswith(prefix)),None)

def pages(repo, rev):
    """Each page name and its text at rev; README is the index, not a page."""
    if rev is None: names = [path.name for path in (pathlib.Path(repo)/SKILL/'features').glob('*.md')]
    else: names = [name.rsplit('/',1)[1] for name in git(repo,'ls-tree','--name-only',rev,SKILL+'/features/').split()]
    return {name[:-3]:read(repo,rev,f'{SKILL}/features/{name}').decode()
        for name in sorted(names) if name.endswith('.md') and name != 'README.md'}

def covers(text, fallback=None):
    found = line(text,'Covers:')
    return found.split() if found is not None else (fallback or [])

def changed(repo, base, head):
    files = git(repo,'diff','--no-renames','--name-only',base,*([head] if head else [])).split('\n')
    if head is None: files += git(repo,'ls-files','--others','--exclude-standard').split('\n')
    return sorted({name for name in files if name})

def release_change(repo, base, head, files):
    if any(matches(name,RELEASE) for name in files): return True
    diff = git(repo,'diff','--no-ext-diff','--no-color','-U0',base,*([head] if head else []),'--','*.rs')
    if any(row[:1] in '+-' and not row.startswith(('+++','---')) and 'cfg(debug_assertions)' in row
            for row in diff.splitlines()): return True
    if head is None:
        for name in git(repo,'ls-files','--others','--exclude-standard').split():
            if name.endswith('.rs') and b'cfg(debug_assertions)' in (read(repo,None,name) or b''): return True
    def profiles(data):
        kept, inside = [], False
        for row in (data or b'').decode().splitlines():
            if row.startswith('['): inside = row.startswith('[profile')
            if inside: kept.append(row)
        return kept
    return 'Cargo.toml' in files and profiles(read(repo,base,'Cargo.toml')) != profiles(read(repo,head,'Cargo.toml'))

def select(repo, base, head=None):
    files = changed(repo,base,head)
    texts = pages(repo,head)
    every = set(texts)|{f'{name}:{kind}' for name in texts if name not in LIVE for kind,heading in SECTIONS.items()
        if heading.search(texts[name])}
    chosen = set()
    def page(name, kinds):
        for kind in kinds:
            if kind == 'main' or name in LIVE: chosen.add(name)
            elif SECTIONS[kind].search(texts[name]): chosen.add(f'{name}:{kind}')
    for name in files:
        if matches(name,SHARED): chosen |= every; continue
        if matches(name,NO_PAGE): continue
        own = re.fullmatch(re.escape(SKILL)+r'/features/(.+)\.md',name)
        if own and own[1] in texts:
            if without_status(read(repo,base,name) or b'') != without_status(read(repo,head,name) or b''):
                page(own[1],['main','codex','grok'])
            continue
        hits = [page_name for page_name,text in texts.items() if matches(name,covers(text))]
        if not hits: chosen |= every
        kinds = (['codex'] if matches(name,CODEX) else ['grok'] if matches(name,GROK)
            else ['main','codex','grok'] if matches(name,PROVIDER_SHARED) else ['main'])
        for hit in hits: page(hit,['main'] if hit in LIVE else kinds)
    if release_change(repo,base,head,files): chosen.add('release')
    return sorted(chosen)

def fingerprint(repo, commit, name, fallback_covers=None):
    """SHA-256 over sorted BLOB PATH lines for the page's Covers files and its own file without Status."""
    own = f'{SKILL}/features/{name}.md'
    text = read(repo,commit,own)
    globs = covers(text.decode(),fallback_covers)
    rows = []
    for entry in git(repo,'ls-tree','-r','-z','--full-tree',commit).split('\0'):
        if not entry: continue
        meta,path = entry.split('\t',1)
        if path == own:
            data = without_status(text)
            rows.append((path,hashlib.sha1(b'blob %d\0' % len(data)+data).hexdigest()))
        elif matches(path,globs): rows.append((path,meta.split()[2]))
    return hashlib.sha256(''.join(f'{blob} {path}\n' for path,blob in sorted(rows)).encode()).hexdigest()

def fresh(repo, head, versions=None):
    """A page is fresh while its fingerprint matches and nothing since its Status COMMIT selects it."""
    rows, since = [], {}
    for name,text in pages(repo,head).items():
        status = [field.strip() for field in (line(text,'Status:') or '').split(',')]
        if not re.fullmatch(r'[0-9a-f]{64}',status[-1]) or status[-1] != fingerprint(repo,head,name): continue
        if len(status) < 5: continue
        if status[2] not in since:
            try: since[status[2]] = select(repo,status[2],head)
            except subprocess.CalledProcessError: since[status[2]] = None
        if since[status[2]] is None or name in since[status[2]]: continue
        if name not in LIVE: rows.append(f'{name} {status[0]}'); continue
        if versions is None: continue
        known = dict(row.split(':',1) for row in pathlib.Path(versions).read_text().splitlines() if ':' in row)
        for provider in ['Claude','Codex','Grok']:
            cell = re.search(provider+' ('+VERSION+')',text)
            if cell and cell[1] in versions_in(known.get(provider.lower(),'')):
                rows.append(f'{name}/{provider.lower()} {status[0]}')
    return rows

def versions_in(text):
    """Every whole version token on a line: a dotted number with its -prerelease and +build suffixes."""
    return {match[0] for match in re.finditer(VERSION,text)}

def self_test():
    root = pathlib.Path(tempfile.mkdtemp(prefix='veyra-pages-'))
    env = dict(os.environ,GIT_CONFIG_GLOBAL='/dev/null',GIT_CONFIG_NOSYSTEM='1')
    def run(*args): subprocess.run(['git',*args],cwd=root,env=env,check=True,capture_output=True)
    def write(path, text):
        (root/path).parent.mkdir(parents=True,exist_ok=True)
        (root/path).write_text(text)
    def commit(message):
        run('add','-A')
        run('-c','user.name=self-test','-c','user.email=self-test@example.invalid','-c','commit.gpgsign=false',
            'commit','-q','-m',message)
        return git(root,'rev-parse','HEAD').strip()
    def page(name, status, globs, extra=''):
        write(f'{SKILL}/features/{name}.md',f'# {name}\n\nStatus: {status}\nCovers: {globs}\n\n## Driving\n{extra}')
    try:
        run('init','-q')
        write('Cargo.toml','[workspace]\n\n[profile.dev]\nopt-level = 0\n')
        for path in ['src/alpha.rs','src/beta.rs','src/other.rs','ARCHITECTURE.md','crates/veyra/src/main.rs',
                'crates/engine/src/provider/codex.rs','crates/engine/src/provider/session.rs',SKILL+'/SKILL.md',
                SKILL+'/features/README.md']:
            write(path,'one\n')
        page('alpha','passed, 2026-01-01, abc1234, none','src/alpha.rs crates/engine/src/provider/**','\n### Codex\n')
        page('beta','passed, 2026-01-01, abc1234, none','src/beta.rs '+SKILL+'/SKILL.md')
        page('permission-matrix','passed, 2026-01-01, abc1234, live cells with Claude 2.1.0, Codex 0.9.0',
            'crates/engine/src/provider/**','\n### Codex preserved live evidence\n')
        base = commit('base')
        every = ['alpha','alpha:codex','beta','permission-matrix']
        def case(label, change, expected):
            change()
            head = commit(label)
            assert select(root,base,head) == expected, (label,select(root,base,head),expected)
            run('reset','-q','--hard',base)
        case('one page',lambda: write('src/beta.rs','two\n'),['beta'])
        case('shared',lambda: write(SKILL+'/SKILL.md','two\n'),every)
        case('uncovered',lambda: write('src/other.rs','two\n'),every)
        case('no page',lambda: write('ARCHITECTURE.md','two\n'),[])
        case('status only',lambda: page('beta','failed, 2026-02-02, def5678, all','src/beta.rs '+SKILL+'/SKILL.md'),[])
        case('own page',lambda: page('beta','passed, 2026-01-01, abc1234, none','src/beta.rs '+SKILL+'/SKILL.md','More\n'),['beta'])
        case('live page',lambda: page('permission-matrix','passed, 2026-01-01, abc1234, live cells',
            'crates/engine/src/provider/**','\n### Codex preserved live evidence\nMore\n'),['permission-matrix'])
        case('codex',lambda: write('crates/engine/src/provider/codex.rs','two\n'),['alpha:codex','permission-matrix'])
        case('provider shared',lambda: write('crates/engine/src/provider/session.rs','two\n'),
            ['alpha','alpha:codex','permission-matrix'])
        case('release file',lambda: write('crates/veyra/src/main.rs','two\n'),every+['release'])
        case('debug line',lambda: write('src/beta.rs','#[cfg(debug_assertions)]\n'),['beta','release'])
        case('profile',lambda: write('Cargo.toml','[workspace]\n\n[profile.dev]\nopt-level = 1\n'),every+['release'])
        write('src/other.rs','two\n')
        assert select(root,base) == every, 'the working tree is not read'
        run('checkout','-q','--','src/other.rs')
        before = fingerprint(root,base,'beta')
        page('beta','passed, 2026-03-03, '+base[:7]+', none, '+before,'src/beta.rs '+SKILL+'/SKILL.md')
        stamped = commit('status with fingerprint')
        assert fingerprint(root,stamped,'beta') == before, 'a Status line changed the fingerprint'
        assert 'beta passed' in fresh(root,stamped), fresh(root,stamped)
        write('src/other.rs','two\n')
        assert 'beta passed' not in fresh(root,commit('uncovered change')), 'an uncovered change kept beta fresh'
        run('reset','-q','--hard',stamped)
        write('src/beta.rs','two\n')
        moved = commit('covered change')
        assert fingerprint(root,moved,'beta') != before, 'a covered change kept the fingerprint'
        assert 'beta passed' not in fresh(root,moved)
        write('src/alpha.rs','two\n')
        assert fingerprint(root,commit('uncovered by beta'),'beta') == fingerprint(root,moved,'beta')
        live = fingerprint(root,moved,'permission-matrix')
        page('permission-matrix','passed, 2026-01-01, '+moved[:7]+', live cells with Claude 2.1.0, Codex 0.9, '
            'Grok 1.0.62-beta.2, '+live,'crates/engine/src/provider/**','\n### Codex preserved live evidence\n')
        stamped = commit('live fingerprint')
        versions = root/'.git'/'versions.txt'
        same = 'claude: 2.1.0 (Claude Code)\ncodex: codex-cli 0.9\ngrok: grok 1.0.62-beta.2 (abc123) [stable]\n'
        def live_rows(head, lines):
            versions.write_text(lines)
            return [row for row in fresh(root,head,versions) if row.startswith('permission-matrix')]
        assert live_rows(stamped,same) == ['permission-matrix/claude passed','permission-matrix/codex passed',
            'permission-matrix/grok passed'], live_rows(stamped,same)
        assert live_rows(stamped,'claude: 2.1.0-alpha.1\ncodex: codex-cli 0.9.1\ngrok: grok 1.0.62\n') == [], \
            'a prerelease, a longer version or a dropped suffix kept a live row fresh'
        assert not [row for row in fresh(root,stamped) if row.startswith('permission-matrix')]
        write('Cargo.lock','two\n')
        assert live_rows(commit('shared change'),same) == [], 'a shared change kept the live rows fresh'
    finally:
        shutil.rmtree(root)
    print('PASS select-pages: one page, shared and uncovered with the live page, no-page, Status-only, own page, '
        'provider, release and working tree; fingerprint: Status-blind, covered change, uncovered change; fresh: '
        'pages, a selecting change since COMMIT, a shared change for the live rows and whole version tokens')

def main(argv):
    repo = pathlib.Path(__file__).resolve().parents[4]
    command = argv[0] if argv else ''
    if command == 'select' and len(argv) in (2,3): print('\n'.join(select(repo,*argv[1:])))
    elif command == 'fresh' and len(argv) in (2,4) and (len(argv) == 2 or argv[2] == '--versions'):
        print('\n'.join(fresh(repo,argv[1],argv[3] if len(argv) == 4 else None)))
    elif command == 'print' and len(argv) == 2:
        current = {name:covers(text) for name,text in pages(repo,None).items()}
        for name in pages(repo,argv[1]): print(name,fingerprint(repo,argv[1],name,current.get(name)))
    elif command == 'self-test' and len(argv) == 1: self_test()
    else:
        print(__doc__.strip(),file=sys.stderr)
        return 2

if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]) or 0)
