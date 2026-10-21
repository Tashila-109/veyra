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
    return re.fullmatch(r'crates/[^/]+/tests/.+\.rs|crates/engine/src/tests/.+\.rs', name) is not None

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

def tests(source):
    hidden = mask(source)
    declarations = re.compile(r'(?m)^[ \t]*(?:pub(?:\([^\n]*\))?\s+)?(?:async\s+)?fn\s+(\w+)\s*\(')
    result = {}
    for match in declarations.finditer(hidden):
        attributes = re.search(r'((?:#\[[^\]]*\]\s*)+)$',hidden[:match.start()])
        if not attributes or not re.search(r'#\[(?:[\w]+::)*test(?:\s*\([^\]]*\))?\s*\]', attributes[0]):
            continue
        opening = hidden.find('{', match.end())
        assert opening >= 0, match[1]
        depth, end = 1, opening+1
        while depth:
            assert end < len(hidden), match[1]
            depth += (hidden[end] == '{') - (hidden[end] == '}')
            end += 1
        result[match[1]] = (attributes.start(), end)
    return result

def changed_tests(before, after):
    previous = tests(before)
    return sorted(name for name,(start,end) in tests(after).items()
        if name not in previous or canonical(before[slice(*previous[name])]) != canonical(after[start:end]))

def verdict(test, passed, ran, expected):
    if test in expected:
        return (passed and ran, 'EXPECTED PASS: '+test+'; '+expected[test] if passed and ran
            else 'FAIL stale expected-pass entry did not pass: '+test)
    if passed:
        return False, 'FAIL test passes on parent: '+test if ran else 'FAIL test filter executed no named test: '+test
    return True, 'RED test failed or could not compile on parent: '+test

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
    assert not verdict('planted',True,True,{})[0]
    assert verdict('planted',True,True,{'planted':'formatting only'})[0]
    assert not verdict('planted',False,False,{'planted':'formatting only'})[0]
    print('PASS self-test: wrapped-only test skipped; token/string changes selected; planted pass rejected; listed pass accepted; stale listed failure rejected', flush=True)

self_test()
if sys.argv[1:] == ['--self-test']:
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
names = subprocess.check_output(['git','diff','--name-only','--diff-filter=AM',base,'--','crates'],text=True).splitlines()
names += subprocess.check_output(['git','ls-files','--others','--exclude-standard','--','crates'],text=True).splitlines()
files = sorted({name for name in names if test_file(name)})
selected = []
for name in files:
    old = subprocess.run(['git','show',base+':'+name],capture_output=True,text=True)
    selected.extend((name,test) for test in changed_tests(old.stdout if old.returncode == 0 else '', pathlib.Path(name).read_text()))
stale = expected.keys() - {test for _,test in selected}
if stale:
    sys.exit('FAIL stale expected-pass entries not in diff: '+', '.join(sorted(stale)))
if not selected:
    print('PASS no added or changed tests in test-only files; source-file tests need manual red receipts')
    sys.exit(0)
with tempfile.TemporaryDirectory(prefix='veyra-speed-red-parent-',dir='/tmp') as folder:
    root = pathlib.Path(folder)
    archive = root/'base.tar'
    with archive.open('wb') as output:
        subprocess.run(['git','archive',base],stdout=output,check=True)
    with tarfile.open(archive) as source:
        source.extractall(root,filter='data')
    archive.unlink()
    for name in files:
        target = root/name
        target.parent.mkdir(parents=True,exist_ok=True)
        shutil.copyfile(name,target)
    env = os.environ.copy()
    env['CARGO_TARGET_DIR'] = str(root/'target')
    failed = False
    for name,test in selected:
        print(f'BASE={base} file={name} test={test} isolated_target={env["CARGO_TARGET_DIR"]}',flush=True)
        result = subprocess.run(['cargo','test','--workspace','--',test,'--nocapture'],cwd=root,env=env,capture_output=True,text=True)
        output = result.stdout+result.stderr
        ran = re.search(r'(?m)^test (?:\S*::)?'+re.escape(test)+r' \.\.\. ok$',output) is not None
        ok,message = verdict(test,result.returncode == 0,ran,expected)
        print(output,flush=True)
        print(message,flush=True)
        failed |= not ok
    if failed: sys.exit(1)
print('PASS selected tests were red or explicitly expected to pass; isolated tree removed')
PY
