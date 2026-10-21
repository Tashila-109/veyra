#!/bin/bash
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
set -eu
cd "$(dirname "$0")/.."
python3 - "$@" <<'PY'
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile

def test_file(name):
    return re.fullmatch(r'crates/[^/]+/tests/.+|crates/engine/src/tests/.+', name) is not None

def mask(source):
    pattern = re.compile(r'''//[^\n]*|/\*[\s\S]*?\*/|(?:br|cr|r)(?P<hash>\#*)"[\s\S]*?"(?P=hash)|(?:b|c)?"(?:\\[\s\S]|[^"\\])*"|b?'(?:\\.|[^'\\])' ''',re.X)
    return pattern.sub(lambda match: ''.join('\n' if char == '\n' else ' ' for char in match[0]), source)

def canonical(source):
    literals = re.compile(r'''//[^\n]*|/\*[\s\S]*?\*/|(?:br|cr|r)(?P<hash>\#*)"[\s\S]*?"(?P=hash)|(?:b|c)?"(?:\\[\s\S]|[^"\\])*"|b?'(?:\\.|[^'\\])' ''',re.X)
    result, end = [], 0
    for match in literals.finditer(source):
        result.extend(re.findall(r'\w+|::|->|=>|\.\.=|\.\.\.|\.\.|==|!=|<=|>=|&&|\|\||<<|>>|[^\s]', source[end:match.start()]))
        literal = match[0]
        if not literal.startswith(('//','/*')):
            if literal.startswith(("'", "b'")):
                result.append(('char',literal))
                end = match.end()
                continue
            raw = re.fullmatch(r'(br|cr|r)(\#*)"([\s\S]*)"\2',literal)
            if raw:
                prefix = raw[1].removesuffix('r')
                value = raw[3]
            else:
                prefix = literal[:literal.index('"')]
                body = re.sub(r'\\\r?\n\s*', '', literal[literal.index('"')+1:-1])
                def escape(match):
                    code = match[0][1:]
                    if code.startswith('u{'): return chr(int(code[2:-1].replace('_',''),16))
                    if code.startswith('x'): return chr(int(code[1:],16))
                    return {'n':'\n','r':'\r','t':'\t','0':'\0','\\':'\\','"':'"',"'":"'"}[code]
                value = re.sub(r'\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)',escape,body)
            result.append((prefix,value))
        end = match.end()
    result.extend(re.findall(r'\w+|::|->|=>|\.\.=|\.\.|==|!=|<=|>=|&&|\|\||<<|>>|[^\s]', source[end:]))
    return result

def closing_brace(hidden, opening):
    depth, end = 1, opening+1
    while depth:
        assert end < len(hidden), 'unclosed Rust block'
        depth += (hidden[end] == '{') - (hidden[end] == '}')
        end += 1
    return end

def module_scopes(hidden):
    return [(match[1], match.end()-1, closing_brace(hidden, match.end()-1))
        for match in re.finditer(r'\bmod\s+(\w+)\s*\{', hidden)]

def enclosing_modules(scopes, position):
    return tuple(name for name,start,end in scopes if start < position < end)

def tests(source):
    hidden = mask(source)
    scopes = module_scopes(hidden)
    declarations = re.compile(r'\b(?:pub(?:\([^\n]*\))?\s+)?(?:async\s+)?fn\s+(\w+)\s*\(')
    result = {}
    for match in declarations.finditer(hidden):
        attributes = re.search(r'((?:#\[[^\]]*\]\s*)+)$',hidden[:match.start()])
        if not attributes or not re.search(r'#\[(?:[\w]+::)*test(?:\s*\([^\]]*\))?\s*\]', attributes[0]):
            continue
        opening = hidden.find('{', match.end())
        assert opening >= 0, match[1]
        name = '::'.join((*enclosing_modules(scopes, match.start()), match[1]))
        assert name not in result, 'duplicate test: '+name
        ignored = re.search(r'#\[ignore\b', attributes[0]) is not None
        result[name] = (attributes.start(), closing_brace(hidden, opening), ignored)
    return result

def module_files(path, prefix=(), crate_root=True, seen=None):
    seen = set() if seen is None else seen
    path = path.resolve()
    if path in seen:
        return {}
    seen.add(path)
    result = {path: prefix}
    source = path.read_text()
    hidden = mask(source)
    scopes = module_scopes(hidden)
    base = path.parent if crate_root or path.name == 'mod.rs' else path.with_suffix('')
    for match in re.finditer(r'\bmod\s+(\w+)\s*;', hidden):
        parents = enclosing_modules(scopes, match.start())
        attributes = re.search(r'((?:#\[[^\]]*\]\s*)+)$',hidden[:match.start()])
        explicit = re.search(r'#\[path\s*=\s*"([^"]+)"\]',source[attributes.start():match.start()]) if attributes else None
        candidates = [path.parent/explicit[1]] if explicit else [
            base.joinpath(*parents, match[1]+'.rs'), base.joinpath(*parents, match[1], 'mod.rs')]
        for child in candidates:
            if child.is_file():
                result.update(module_files(child, (*prefix, *parents, match[1]), False, seen))
                break
    return result

def cargo_targets(root, files):
    metadata = subprocess.check_output(['cargo','metadata','--no-deps','--format-version','1'],cwd=root,text=True)
    import json
    found = {name: [] for name in files}
    for package in json.loads(metadata)['packages']:
        for target in package['targets']:
            kind = next((kind for kind in ['test','lib','bin'] if kind in target['kind']), None)
            if kind is None:
                continue
            modules = module_files(pathlib.Path(target['src_path']))
            for name in files:
                prefix = modules.get((root/name).resolve())
                if prefix is not None:
                    found[name].append((package['name'], kind, target['name'], prefix))
    for name in files:
        assert found[name], 'Cannot resolve the Cargo test target for '+name
    return found

def cargo_command(package, kind, target, test):
    selection = ['--lib'] if kind == 'lib' else ['--'+kind, target]
    return ['cargo','test','-p',package,*selection,'--',test,'--exact','--nocapture']

def copy_test_files(source, root, files):
    for name in files:
        target = root/name
        target.parent.mkdir(parents=True,exist_ok=True)
        shutil.copyfile(source/name,target)

def changed_tests(before, after):
    previous = tests(before)
    return sorted(name for name,(start,end,_) in tests(after).items()
        if name not in previous or canonical(before[slice(*previous[name][:2])]) != canonical(after[start:end]))

def verdict(test, code, output, expected):
    results = re.findall(r'(?m)^test '+re.escape(test)+r' \.\.\. (ok|FAILED|ignored)\b', output)
    passed = 'ok' in results
    if test in expected:
        return (passed, 'EXPECTED PASS: '+test+'; '+expected[test] if passed
            else 'FAIL stale expected-pass entry did not pass: '+test)
    if passed:
        return False, 'FAIL test passes on parent: '+test
    if results == ['FAILED'] or (not results and code != 0
        and re.search(r'(?m)^error: could not compile ', output)):
        return True, 'RED target test failed or its Cargo target could not compile: '+test
    return False, 'FAIL no conclusive target result: '+test

def isolation_self_test():
    with tempfile.TemporaryDirectory(prefix='veyra-speed-parent-self-test-',dir='/tmp') as folder:
        root = pathlib.Path(folder)
        package = root/'crates/engine'
        (package/'src').mkdir(parents=True)
        (package/'tests').mkdir()
        (root/'Cargo.toml').write_text('[workspace]\nmembers=["crates/engine"]\nresolver="3"\n')
        (package/'Cargo.toml').write_text('[package]\nname="parent-probe"\nversion="0.0.0"\nedition="2024"\n')
        (package/'src/lib.rs').write_text('')
        passing = '#[test]\nfn restart() { assert!(true); }\n'
        sibling = '#[test]\nfn restart_recovers() { assert!(false); }\n'
        (package/'tests/matching.rs').write_text(passing+sibling)
        for case in ['substring sibling fails','other copied file fails compilation','passing target alone']:
            if case.startswith('other'):
                (package/'tests/broken.rs').write_text('fn broken(\n')
            if case.endswith('alone'):
                (package/'tests/matching.rs').write_text(passing)
                (package/'tests/broken.rs').unlink()
            command = cargo_command('parent-probe','test','matching','restart')
            result = subprocess.run(command,cwd=root,capture_output=True,text=True)
            output = result.stdout+result.stderr
            assert result.returncode == 0 and 'test restart ... ok' in output, (case,output)
            assert 'test restart_recovers ...' not in output
            assert not verdict('restart',result.returncode,output,{})[0], case
            print('PASS isolation self-test: '+case+' cannot hide passing target',flush=True)
        source = root/'candidate'
        fixture = 'crates/engine/tests/fixtures/new.txt'
        renamed = 'crates/engine/tests/renamed.rs'
        (source/fixture).parent.mkdir(parents=True)
        (source/fixture).write_bytes(b'new fixture\n')
        (source/renamed).write_text('mod nested {\n#[test]\nfn restart() { assert_eq!(include_str!("fixtures/new.txt"), "new fixture\\n"); }\n}\n')
        copy_test_files(source,root,[fixture,renamed])
        assert (root/fixture).read_bytes() == b'new fixture\n'
        assert test_file(renamed) and changed_tests('',(source/renamed).read_text()) == ['nested::restart']
        owner, = cargo_targets(root,{renamed})[renamed]
        result = subprocess.run(cargo_command(*owner[:3],'nested::restart'),cwd=root,capture_output=True,text=True)
        output = result.stdout+result.stderr
        assert result.returncode == 0 and 'test nested::restart ... ok' in output, output
        assert not verdict('nested::restart',result.returncode,output,{})[0]
        print('PASS isolation self-test: renamed nested test resolves exactly; new fixture is copied and used',flush=True)
        (package/'src/lib.rs').write_text('#[cfg(test)]\nmod tests;\n')
        (package/'src/tests').mkdir()
        (package/'src/tests/mod.rs').write_text('mod part;\n')
        unit = 'crates/engine/src/tests/part.rs'
        (root/unit).write_text(passing)
        owner, = cargo_targets(root,{unit})[unit]
        full_name = '::'.join((*owner[3],'restart'))
        assert full_name == 'tests::part::restart'
        result = subprocess.run(cargo_command(*owner[:3],full_name),cwd=root,capture_output=True,text=True)
        output = result.stdout+result.stderr
        assert result.returncode == 0 and 'test tests::part::restart ... ok' in output, output
        assert not verdict(full_name,result.returncode,output,{})[0]
        print('PASS isolation self-test: external unit modules resolve to their fully qualified test name',flush=True)
        for name in ['stop','awake']:
            (package/f'src/tests/{name}.rs').write_text('')
        modules = 'mod part;\nmod stop;\nmod awake;\n'
        (package/'src/tests/mod.rs').write_text(modules)
        (root/'scripts').mkdir()
        shutil.copyfile('scripts/red-on-parent.sh',root/'scripts/red-on-parent.sh')
        def git(*args):
            return subprocess.check_output(['git',*args],cwd=root,text=True,stderr=subprocess.STDOUT).strip()
        git('init','-q')
        git('config','user.name','Veyra Toy Probe')
        git('config','user.email','toy@example.invalid')
        git('config','commit.gpgsign','false')
        git('add','Cargo.toml','crates','scripts')
        git('commit','-qm','Seed toy repository')
        base = git('rev-parse','HEAD')
        (package/'src/lib.rs').write_text('pub fn new_only_api() {}\n#[cfg(test)]\nmod tests;\n')
        (package/'src/tests/stop.rs').write_text('#[test] fn review_probe_unit_passes() {}\n')
        (package/'src/tests/awake.rs').write_text('#[test] fn needs_new_api() { crate::new_only_api(); }\n')
        result = subprocess.run(['bash','scripts/red-on-parent.sh',base],cwd=root,capture_output=True,text=True)
        output = result.stdout+result.stderr
        assert result.returncode == 1, output
        assert 'FAIL test passes on parent: tests::stop::review_probe_unit_passes' in output, output
        assert 'RED target test failed or its Cargo target could not compile: tests::awake::needs_new_api' in output, output
        print('PASS isolation self-test: shared unit target passing file rejected; new-API sibling independently RED',flush=True)
        expected = root/'expected.txt'
        expected.write_text('tests::stop::review_probe_unit_passes planted passing control\n')
        result = subprocess.run(['bash','scripts/red-on-parent.sh',base,str(expected)],cwd=root,capture_output=True,text=True)
        assert result.returncode == 0 and 'EXPECTED PASS:' in result.stdout, result.stdout+result.stderr
        expected.write_text('absent stale control\n')
        result = subprocess.run(['bash','scripts/red-on-parent.sh',base,str(expected)],cwd=root,capture_output=True,text=True)
        assert result.returncode != 0 and 'stale expected-pass entries not in diff' in result.stderr, result.stdout+result.stderr
        print('PASS isolation self-test: listed passing test accepted; stale listed entry rejected',flush=True)
        (package/'src/lib.rs').write_text('#[cfg(test)]\nmod tests;\n')
        for name in ['stop','awake']:
            (package/f'src/tests/{name}.rs').write_text('')
        destination = 'crates/engine/tests/destination.rs'
        git('mv',renamed,destination)
        added = subprocess.check_output(diff_command(base),cwd=root,text=True).splitlines()
        assert destination in added and renamed not in added, added
        (root/fixture).write_bytes(b'changed fixture\n')
        (root/destination).write_text((root/destination).read_text().replace('new fixture','changed fixture'))
        result = subprocess.run(['bash','scripts/red-on-parent.sh',base],cwd=root,capture_output=True,text=True)
        assert result.returncode == 1 and 'test nested::restart ... ok' in result.stdout, result.stdout+result.stderr
        print('PASS isolation self-test: real git mv destination selected as added; changed fixture copied and used',flush=True)
        git('mv',destination,renamed)
        (root/renamed).write_text((root/renamed).read_text().replace('changed fixture','new fixture'))
        (root/fixture).write_bytes(b'new fixture\n')
        (package/'src/tests/stop.rs').write_text('#[test]\n#[ignore]\nfn ignored_unit() { crate::new_only_api(); }\n')
        (package/'tests/live_probe.rs').write_text('#[test]\n#[ignore = "operator only"]\nfn ignored_live() { crate::new_only_api(); }\n')
        result = subprocess.run(['bash','scripts/red-on-parent.sh',base],cwd=root,capture_output=True,text=True)
        assert result.returncode == 0, result.stdout+result.stderr
        assert result.stdout.count('MANUAL RED RECEIPT required: ignored test') == 2, result.stdout
        assert 'isolated_target=' not in result.stdout, result.stdout
        print('PASS isolation self-test: ignored unit and live tests require hand-written receipts without compiling or running',flush=True)


def diff_command(base):
    return ['git','diff','--no-renames','--name-only','--diff-filter=AM',base,'--','crates']

def self_test():
    before = '#[test]\nfn existing() { assert!(false); }\n'
    after = before+'\n#[tokio::test]\nasync fn planted() { assert!(true); }\n'
    assert changed_tests(before, after) == ['planted']
    assert changed_tests(before, before.replace('false','true')) == ['existing']
    assert changed_tests(before, before) == []
    assert changed_tests(before, before.replace('assert!','\n    assert!')) == []
    assert canonical('r#"a b"#') == canonical('"a b"')
    assert canonical('"a b"') != canonical('"ab"')
    assert canonical('"a\\\n   b"') == canonical('"ab"')
    assert canonical('left + right') != canonical('left - right')
    assert canonical('f(value)') != canonical('f(value,)')
    assert canonical("' '") != canonical("''")
    assert changed_tests(before, before.replace('assert!(false)', 'assert!(\n    false\n)')) == []
    assert test_file('crates/engine/tests/probe.rs')
    assert test_file('crates/engine/src/tests/probe.rs')
    assert not test_file('crates/engine/src/actor.rs')
    assert set(tests('#[test]\nfn quoted() { let text = r#"}"#; }\n')) == {'quoted'}
    assert not verdict('planted',0,'test planted ... ok',{})[0]
    assert verdict('planted',0,'test planted ... ok',{'planted':'formatting only'})[0]
    assert not verdict('planted',101,'test planted ... FAILED',{'planted':'formatting only'})[0]
    assert not verdict('restart',101,'test restart ... ok\ntest restart_recovers ... FAILED',{})[0]
    assert not verdict('restart',101,'unrelated infrastructure failure',{})[0]
    assert verdict('restart',101,'error: could not compile `parent-probe` (test \"matching\")',{})[0]
    assert set(tests('mod nested {\n#[test]\nfn check() {}\n}')) == {'nested::check'}
    print('PASS self-test: wrapped-only test skipped; token/string changes selected; planted pass rejected; listed pass accepted; stale listed failure rejected', flush=True)

self_test()
if sys.argv[1:] == ['--self-test']:
    isolation_self_test()
    sys.exit(0)
if len(sys.argv) not in (2,3):
    sys.exit('usage: scripts/red-on-parent.sh BASE [EXPECT_PASS_FILE] | --self-test')
expected = {}
if len(sys.argv) == 3:
    for line in pathlib.Path(sys.argv[2]).read_text().splitlines():
        if not line.strip(): continue
        name,reason = line.split(maxsplit=1)
        assert name not in expected and reason.strip(), 'Expected-pass entries need unique names and reasons'
        expected[name] = reason
base = subprocess.check_output(['git','rev-parse','--verify',sys.argv[1]+'^{commit}'],text=True).strip()
names = subprocess.check_output(diff_command(base),text=True).splitlines()
names += subprocess.check_output(['git','ls-files','--others','--exclude-standard','--','crates'],text=True).splitlines()
files = sorted({name for name in names if test_file(name)})
selected = []
for name in files:
    if not name.endswith('.rs'): continue
    old = subprocess.run(['git','show',base+':'+name],capture_output=True,text=True)
    after = pathlib.Path(name).read_text()
    for test in changed_tests(old.stdout if old.returncode == 0 else '', after):
        if tests(after)[test][2]:
            print(f'MANUAL RED RECEIPT required: ignored test {name}::{test}',flush=True)
        else:
            selected.append((name,test))
if not selected:
    if expected:
        sys.exit('FAIL stale expected-pass entries not in diff: '+', '.join(sorted(expected)))
    print('PASS no added or changed tests in test-only files; source-file tests need manual red receipts')
    sys.exit(0)
selected_targets = cargo_targets(pathlib.Path.cwd(),{name for name,_ in selected})
selected = [(name,'::'.join((*prefix,test)),package,kind,target)
    for name,test in selected for package,kind,target,prefix in selected_targets[name]]
stale = expected.keys() - {test for _,test,*_ in selected}
if stale:
    sys.exit('FAIL stale expected-pass entries not in diff: '+', '.join(sorted(stale)))
failed = False
fixtures = [name for name in files if not name.endswith('.rs')]
for name in sorted({name for name,*_ in selected}):
    with tempfile.TemporaryDirectory(prefix='veyra-speed-red-parent-',dir='/tmp') as folder:
        root = pathlib.Path(folder)
        archive = root/'base.tar'
        with archive.open('wb') as output:
            subprocess.run(['git','archive',base],stdout=output,check=True)
        with tarfile.open(archive) as source:
            source.extractall(root,filter='data')
        archive.unlink()
        copy_test_files(pathlib.Path.cwd(),root,[name,*fixtures])
        env = os.environ.copy()
        env['CARGO_TARGET_DIR'] = str(root/'target')
        for file,full_name,package,kind,target in selected:
            if file != name: continue
            print(f'BASE={base} file={name} target={package}/{kind}/{target} test={full_name} isolated_target={env["CARGO_TARGET_DIR"]}',flush=True)
            result = subprocess.run(cargo_command(package,kind,target,full_name),cwd=root,env=env,capture_output=True,text=True)
            output = result.stdout+result.stderr
            ok,message = verdict(full_name,result.returncode,output,expected)
            print(output,flush=True)
            print(message,flush=True)
            failed |= not ok
if failed: sys.exit(1)
print('PASS selected tests were red or explicitly expected to pass; isolated file trees removed')
PY
