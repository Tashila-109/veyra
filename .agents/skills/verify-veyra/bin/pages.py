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

def evidence_contract(repo):
    """Each page owns its unchanged required files and screenshot pairs."""
    import json
    required, pairs = {}, {}
    for path in sorted((pathlib.Path(repo)/SKILL/'features').glob('*.md')):
        if path.name == 'README.md': continue
        match = re.search(r'^### Evidence contract\n\n```json\n(.*?)\n```',path.read_text(),re.S|re.M)
        assert match, 'missing evidence contract: '+path.name
        contract = json.loads(match[1])
        assert set(contract) == {'required','pairs'}, path.name
        for destination,key in [(required,'required'),(pairs,'pairs')]:
            for page,entries in contract[key].items():
                assert page == path.stem or page.startswith(path.stem+'-'), (path.name,page)
                assert page not in destination, 'duplicate evidence token: '+page
                destination[page] = entries
    assert pairs.keys() <= required.keys(), 'screenshot pairs lack a required-file token'
    return required,pairs

def rule_lists(text):
    """The glob lists of SKILL.md's Selection block, their one owner; an indented row continues the list above it."""
    section = re.search(r'^## Selection\n(.*?)(?=^## |\Z)',text,re.S|re.M)
    block = re.search(r'^```text\n(.*?)^```',section[1] if section else '',re.S|re.M)
    assert block, 'SKILL.md has no Selection block of glob lists'
    lists, name = {}, None
    for row in block[1].splitlines():
        if row[:1].isspace() and name: lists[name] += row.split()
        elif row.strip():
            name,*globs = row.split()
            assert name not in lists, 'Selection block repeats the list '+name
            lists[name] = globs
    missing = {'shared','no-page','codex','grok','provider','release'}-set(lists)
    assert not missing, 'Selection block lacks '+', '.join(sorted(missing))
    return lists

LISTS = rule_lists((pathlib.Path(__file__).resolve().parent.parent/'SKILL.md').read_text())
SHARED, NO_PAGE, CODEX, GROK, PROVIDER_SHARED, RELEASE = (LISTS[name] for name in
    ['shared','no-page','codex','grok','provider','release'])
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

def rust_code(data):
    """Mask comments and strings without moving line or brace positions (including nested block comments)."""
    text = (data or b'').decode()
    pattern = re.compile(r'//[^\n]*|/\*|r(\#*)"|"(?:\\.|[^"\\])*"|\'(?:\\.|[^\'\\])\'', re.S)
    chars, end = list(text), 0
    while match := pattern.search(text,end):
        stop = match.end()
        if match[0] == '/*':
            depth = 1
            while depth and stop < len(text):
                token = re.search(r'/\*|\*/',text[stop:])
                if not token: stop = len(text); break
                depth += 1 if token[0] == '/*' else -1
                stop += token.end()
        elif match[0].startswith('r'):
            closing = '"'+match[1]
            index = text.find(closing,stop)
            stop = len(text) if index < 0 else index+len(closing)
        for index in range(match.start(),stop):
            if chars[index] != '\n': chars[index] = ' '
        end = stop
    return ''.join(chars)

def test_ranges(data):
    """Line intervals of complete cfg(test) items, including their attributes, in this side of a Rust diff."""
    code, ranges = rust_code(data), []
    for match in re.finditer(r'#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]',code):
        # Attributes can contain braces. Skip them before finding the item's body or semicolon.
        start, cursor = match.start(), match.end()
        while more := re.match(r'\s*#\s*\[',code[cursor:]):
            cursor += more.end()
            depth = 1
            while depth and cursor < len(code):
                depth += (code[cursor]=='[')-(code[cursor]==']'); cursor += 1
        item = re.search(r'[;{]',code[cursor:])
        if item is None: continue
        stop = cursor+item.end()
        if item[0] == '{':
            depth = 1
            while depth and stop < len(code):
                depth += (code[stop]=='{')-(code[stop]=='}'); stop += 1
            if depth: continue  # malformed input is production, never an exemption
        ranges.append((code.count('\n',0,start)+1,code.count('\n',0,stop)+1))
    return ranges

def test_file(repo, rev, path, seen=None):
    """Follow a file's mod declaration, rather than exempting a production file containing some tests."""
    if re.search(r'(?:^|/)tests/',path): return True
    if not path.endswith('.rs'): return False
    seen = set() if seen is None else seen
    if path in seen: return False
    seen.add(path)
    p = pathlib.PurePosixPath(path)
    parent = p.parent
    name = p.stem
    if name == 'mod': name, parent = parent.name, parent.parent
    candidates = [parent/'mod.rs',parent/'main.rs',parent/'lib.rs',parent.with_suffix('.rs')]
    for candidate in dict.fromkeys(str(value) for value in candidates if str(value)!=path):
        body = read(repo,rev,candidate)
        if body is None: continue
        code = rust_code(body)
        for declaration in re.finditer(r'\bmod\s+'+re.escape(name)+r'\s*;',code):
            number = code.count('\n',0,declaration.start())+1
            if any(a<=number<=b for a,b in test_ranges(body)): return True
            if test_file(repo,rev,candidate,seen): return True
    return False

def production_hunks(repo, base, head, path):
    """Classify hunks on both sides first. Each remaining hunk selects by its whole file."""
    before, after = read(repo,base,path), read(repo,head,path)
    if before is None and after is None: return []
    diff = git(repo,'diff','--no-ext-diff','--no-color','--no-renames','-U0',base,
        *([head] if head else []),'--',path)
    if not diff and before is None:  # untracked file
        diff = '@@ -0,0 +1,%d @@\n'%len(after.splitlines())+'\n'.join('+'+row for row in after.decode().splitlines())
    chunks = re.split(r'(?=^@@ )',diff,flags=re.M)[1:]
    ranges = [test_ranges(before),test_ranges(after)] if path.endswith('.rs') else [[],[]]
    whole = [test_file(repo,base,path) if before is not None else True,
        test_file(repo,head,path) if after is not None else True]
    production = []
    for chunk in chunks:
        header = re.match(r'@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@',chunk)
        assert header, 'unparsed diff hunk: '+path
        sides = [(int(header[1]),int(header[2] or 1)),(int(header[3]),int(header[4] or 1))]
        exempt = path.endswith('.rs') and all(whole[index] or count==0 or
            all(any(a<=line<=b for a,b in ranges[index]) for line in range(start,start+count))
            for index,(start,count) in enumerate(sides))
        changed_rows = [row[1:].strip() for row in chunk.splitlines()[1:] if row[:1] in '+-']
        # Adding/removing only the guard changes whether an existing item is production.
        if any(re.fullmatch(r'#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]',row) for row in changed_rows) \
                and all(row.startswith('#[') for row in changed_rows): exempt = False
        if not exempt: production.append(chunk)
    return production

def class_c_change(repo, base, head, path, property):
    if not matches(path,LISTS[property]) or not production_hunks(repo,base,head,path): return False
    if path not in ['Cargo.toml','Cargo.lock']: return True
    # Only the gpui pins own native properties; a Cargo profile or test dependency does not.
    import tomllib
    def pins(rev):
        data = tomllib.loads((read(repo,rev,path) or b'').decode())
        names = {'gpui-kit','gpui-pre','gpui-pre-platform'}
        if path == 'Cargo.lock': return [entry for entry in data.get('package',[]) if entry['name'] in names]
        return {key:value for key,value in data.get('workspace',{}).get('dependencies',{}).items() if key in names}
    return pins(base) != pins(head)

def release_change(repo, base, head, files):
    if any(matches(name,RELEASE) for name in files): return True
    diff = '\n'.join(hunk for path in files if path.endswith('.rs')
        for hunk in production_hunks(repo,base,head,path))
    if any(row[:1] in '+-' and not row.startswith(('+++','---')) and 'cfg(debug_assertions)' in row
            for row in diff.splitlines()): return True
    def profiles(data):
        kept, inside = [], False
        for row in (data or b'').decode().splitlines():
            if row.startswith('['): inside = row.startswith('[profile')
            if inside: kept.append(row)
        return kept
    return 'Cargo.toml' in files and profiles(read(repo,base,'Cargo.toml')) != profiles(read(repo,head,'Cargo.toml'))

def select(repo, base, head=None):
    files = [path for path in changed(repo,base,head) if production_hunks(repo,base,head,path)]
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
            recorded = recorded_version(text,provider)
            if recorded and recorded in known.get(provider.lower(),'').split():
                rows.append(f'{name}/{provider.lower()} {status[0]}')
    return rows

def recorded_version(text, provider):
    """The whitespace token after the page's first PROVIDER mention that starts with a digit, less one trailing
    comma, semicolon or full stop of its sentence; fresh() compares it exactly with versions.txt's tokens."""
    words = text.split()
    for word,after in zip(words,words[1:]):
        if word == provider and after[:1].isdigit(): return after[:-1] if after[-1] in ',;.' else after
    return None

def self_test():
    """All fixtures are in memory. No init, index, commit, checkout or other Git write."""
    import difflib
    from unittest.mock import patch
    fixtures = {
        'base':{
            SKILL+'/features/alpha.md':b'# Alpha\nStatus: passed, date, sha, none\nCovers: src/alpha.rs crates/engine/src/provider/**\n### Codex\n',
            SKILL+'/features/beta.md':b'# Beta\nStatus: passed, date, sha, none\nCovers: src/beta.rs\n',
            SKILL+'/features/permission-matrix.md':b'# Live\nCovers: crates/engine/src/provider/**\n',
            'src/alpha.rs':b'fn alpha() {}\n','src/beta.rs':b'fn beta() {}\n',
            'crates/veyra/src/main.rs':b'#[cfg(test)]\nmod tests;\nmod ui;\n',
            'crates/veyra/src/tests.rs':b'#[cfg(debug_assertions)]\nfn old() {}\n',
            'crates/veyra/src/ui/mod.rs':b'fn live() {}\n#[cfg(test)]\nmod headless;\n#[cfg(test)]\nmod checks {\nfn old() {}\n}\n',
            'crates/veyra/src/ui/headless.rs':b'#[cfg(debug_assertions)]\nfn old() {}\n',
            'Cargo.toml':b'[workspace]\n[profile.dev]\nopt-level = 0\n'}}
    def fake_git(repo,*args,data=False):
        if args[0]=='show':
            rev,path=args[1].split(':',1)
            if path not in fixtures[rev]: raise subprocess.CalledProcessError(1,['git','show'])
            result=fixtures[rev][path]
        elif args[0]=='ls-files': result=b''
        elif args[0]=='ls-tree':
            rev=args[-1] if '-r' in args else args[-2]
            if '-r' in args:
                result=''.join('100644 blob '+hashlib.sha1(b'blob %d\0'%len(body)+body).hexdigest()+'\t'+path+'\0'
                    for path,body in sorted(fixtures[rev].items())).encode()
            else: result=('\n'.join(path for path in fixtures[rev] if path.startswith(SKILL+'/features/'))+'\n').encode()
        elif args[0]=='diff':
            before,after=fixtures['base'],fixtures['head']
            if '--name-only' in args:
                result='\n'.join(path for path in sorted(before.keys()|after.keys()) if before.get(path)!=after.get(path)).encode()
            else:
                path=args[-1]
                result=''.join(difflib.unified_diff(before.get(path,b'').decode().splitlines(True),
                    after.get(path,b'').decode().splitlines(True),fromfile=path,tofile=path,n=0)).encode()
        else: raise AssertionError(args)
        return result if data else result.decode()
    every=['alpha','alpha:codex','beta','permission-matrix']
    cases=[
        ('one page',{'src/beta.rs':b'fn changed() {}\n'},['beta']),
        ('uncovered',{'src/other.rs':b'fn other() {}\n'},every),
        ('shared helper',{SKILL+'/bin/helper.py':b'pass\n'},every),
        ('no page',{'ARCHITECTURE.md':b'doc\n'},[]),
        ('release file',{'crates/veyra/src/main.rs':b'fn main() {}\n'},every+['release']),
        ('production debug',{'src/beta.rs':b'#[cfg(debug_assertions)]\nfn changed() {}\n'},['beta','release']),
        ('test file through mod',{'crates/veyra/src/tests.rs':b'#[cfg(debug_assertions)]\nfn changed() {}\n'},[]),
        ('fd4adea..04ac4f3 fixture shape',{'crates/veyra/src/ui/headless.rs':b'#[cfg(debug_assertions)]\nfn repaired() {}\n',
            'crates/veyra/tests/headless.rs':b'#[cfg(debug_assertions)]\nfn changed() {}\n','ARCHITECTURE.md':b'doc\n'},[]),
        ('shared file test hunk',{'crates/veyra/src/ui/mod.rs':fixtures['base']['crates/veyra/src/ui/mod.rs'].replace(b'fn old()',b'fn new()')},[]),
        ('mixed shared file',{'crates/veyra/src/ui/mod.rs':fixtures['base']['crates/veyra/src/ui/mod.rs'].replace(b'fn live()',b'fn changed()').replace(b'fn old()',b'fn new()')},every),
        ('remove cfg test',{'crates/veyra/src/ui/mod.rs':fixtures['base']['crates/veyra/src/ui/mod.rs'].replace(b'#[cfg(test)]\nmod checks',b'mod checks')},every),
        ('crate integration tests',{'crates/example/tests/nested/test.rs':b'#[cfg(debug_assertions)]\nfn test() {}\n'},[]),
        ('provider',{'crates/engine/src/provider/codex.rs':b'fn codec() {}\n'},['alpha:codex','permission-matrix']),
        ('profile',{'Cargo.toml':b'[workspace]\n[profile.dev]\nopt-level = 1\n'},every+['release']),
    ]
    with patch(__name__+'.git',side_effect=fake_git):
        for label,change,expected in cases:
            fixtures['head']=dict(fixtures['base'],**change)
            actual=select(None,'base','head')
            assert actual==sorted(expected),(label,actual,expected)
        fixtures['head']=dict(fixtures['base'])
        own=SKILL+'/features/beta.md'
        fixtures['head'][own]=fixtures['base'][own].replace(b'Status: passed',b'Status: failed')
        assert select(None,'base','head')==[]
        assert fingerprint(None,'base','beta')==fingerprint(None,'head','beta')
        fixtures['head'][own]+=b'Extra recipe\n'
        assert select(None,'base','head')==['beta']
    repo=pathlib.Path(__file__).resolve().parents[4]
    # Exercise the real admitted test-only commit, without depending on the current uncommitted helper diff.
    if subprocess.run(['git','cat-file','-e','04ac4f3^{commit}'],cwd=repo,capture_output=True).returncode==0:
        assert select(repo,'fd4adea','04ac4f3')==[], 'admitted test-only diff selected native work'
    assert recorded_version('Claude 2.1.0,','Claude')=='2.1.0'
    print('PASS select-pages: test-hunk-first, mod ancestry, mixed production/test, debug-only release, '+
        'fd4adea..04ac4f3 fixture and real commit, provider/shared/uncovered/no-page/Status/fingerprint rules; no Git writes')

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
