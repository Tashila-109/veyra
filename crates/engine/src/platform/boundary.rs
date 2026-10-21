use std::{fs, io, path::Path};

const WORDS: &[&str] = &["libc"];
const NEEDLES: &[&str] = &[
    "UnixListener",
    "UnixStream",
    "UnixDatagram",
    "std::os::unix",
    "std::os::windows",
    "unix::",
    "windows::",
    "killpg",
    "process_group(",
    "target_os",
    "(unix",
    "(windows",
    "Application Support",
    "Command::new(\"ps\")",
    "Command::new(\"open\")",
    "Command::new(\"kill\")",
    "\"cmd-",
];

#[derive(Clone, Copy)]
enum SourceKind {
    Comment,
    Literal,
}

struct SourceSpan {
    start: usize,
    end: usize,
    kind: SourceKind,
}

fn source_spans(source: &str) -> Vec<SourceSpan> {
    let bytes = source.as_bytes();
    let mut spans = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let start = index;
        let rest = &bytes[index..];
        let kind = if rest.starts_with(b"//") {
            index += rest
                .iter()
                .position(|&byte| byte == b'\n')
                .unwrap_or(rest.len());
            Some(SourceKind::Comment)
        } else if rest.starts_with(b"/*") {
            index += 2;
            let mut depth = 1;
            while index < bytes.len() && depth > 0 {
                if bytes[index..].starts_with(b"/*") {
                    depth += 1;
                    index += 2;
                } else if bytes[index..].starts_with(b"*/") {
                    depth -= 1;
                    index += 2;
                } else {
                    index += 1;
                }
            }
            Some(SourceKind::Comment)
        } else {
            let prefix = usize::from(matches!(bytes[index], b'b' | b'c'));
            let quote = index + prefix;
            let raw = quote + usize::from(bytes.get(quote) == Some(&b'r'));
            let mut opening = raw;
            while bytes.get(opening) == Some(&b'#') {
                opening += 1;
            }
            if raw > quote && bytes.get(opening) == Some(&b'"') {
                let closing = format!("\"{}", "#".repeat(opening - raw));
                index = source[opening + 1..]
                    .find(&closing)
                    .map_or(bytes.len(), |offset| opening + 1 + offset + closing.len());
                Some(SourceKind::Literal)
            } else if bytes.get(quote) == Some(&b'"') {
                index = quote + 1;
                while index < bytes.len() {
                    match bytes[index] {
                        b'\\' => index = (index + 2).min(bytes.len()),
                        b'"' => {
                            index += 1;
                            break;
                        }
                        _ => index += 1,
                    }
                }
                Some(SourceKind::Literal)
            } else if bytes.get(quote) == Some(&b'\'') {
                let value = quote + 1;
                let end = if bytes.get(value) == Some(&b'\\') {
                    match bytes.get(value + 1) {
                        Some(b'u') => source[value..].find('}').map(|offset| value + offset + 1),
                        Some(b'x') => Some(value + 4),
                        Some(_) => Some(value + 2),
                        None => None,
                    }
                } else {
                    source[value..]
                        .chars()
                        .next()
                        .map(|value_char| value + value_char.len_utf8())
                };
                if let Some(end) = end.filter(|&end| bytes.get(end) == Some(&b'\'')) {
                    index = end + 1;
                    Some(SourceKind::Literal)
                } else {
                    index += 1;
                    None
                }
            } else {
                index += 1;
                None
            }
        };
        if let Some(kind) = kind {
            spans.push(SourceSpan {
                start,
                end: index,
                kind,
            });
        }
    }
    spans
}

fn blank_source(source: &str, literals: bool) -> String {
    let mut bytes = source.as_bytes().to_vec();
    for span in source_spans(source) {
        if literals || matches!(span.kind, SourceKind::Comment) {
            for byte in &mut bytes[span.start..span.end] {
                if !matches!(*byte, b'\n' | b'\r') {
                    *byte = b' ';
                }
            }
        }
    }
    String::from_utf8(bytes).expect("source blanking preserves UTF-8")
}

fn blank_comments(source: &str) -> String {
    blank_source(source, false)
}

fn matches(source: &str) -> Vec<(usize, &'static str)> {
    let source = blank_comments(source);
    let identifier = |c: char| c.is_alphanumeric() || c == '_';
    let mut found = Vec::new();
    for (index, line) in source.lines().enumerate() {
        for &word in WORDS {
            if line.match_indices(word).any(|(start, _)| {
                !line[..start].chars().next_back().is_some_and(identifier)
                    && !line[start + word.len()..]
                        .chars()
                        .next()
                        .is_some_and(identifier)
            }) {
                found.push((index + 1, word));
            }
        }
        for &needle in NEEDLES {
            if line.contains(needle) {
                found.push((index + 1, needle));
            }
        }
    }
    found
}

fn scan(path: &Path, skipped: &Path, violations: &mut Vec<String>) -> io::Result<()> {
    if path == skipped {
        return Ok(());
    }
    if path.is_dir() {
        let mut entries = fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.path());
        for entry in entries {
            scan(&entry.path(), skipped, violations)?;
        }
    } else if path.extension().is_some_and(|extension| extension == "rs") {
        for (line, needle) in matches(&fs::read_to_string(path)?) {
            violations.push(format!("{}:{line}: {needle}", path.display()));
        }
    }
    Ok(())
}

#[test]
fn platform_boundary() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut violations = Vec::new();
    scan(crates, &crates.join("engine/src/platform"), &mut violations).unwrap();
    assert!(
        violations.is_empty(),
        "macOS calls belong in crates/engine/src/platform/:\n{}",
        violations.join("\n")
    );
}

#[test]
fn platform_boundary_self_test() {
    assert_eq!(
        matches("use tokio::net::{UnixListener, UnixStream};"),
        vec![(1, "UnixListener"), (1, "UnixStream")]
    );
    assert_eq!(
        matches("use libc as c;\nc::killpg(group, 9);"),
        vec![(1, "libc"), (2, "killpg")]
    );
    assert_eq!(matches("#[cfg(any(unix, windows))]"), vec![(1, "(unix")]);
    assert_eq!(
        matches("use std::os::{unix::fs::PermissionsExt};"),
        vec![(1, "unix::")]
    );
    assert_eq!(
        matches("Command::new(\"kill\")"),
        vec![(1, "Command::new(\"kill\")")]
    );
    assert_eq!(matches("// do not call libc::killpg"), vec![]);
    assert_eq!(matches("/* std::os::unix */"), vec![]);
    assert_eq!(matches("use std::os::fd::AsRawFd;"), vec![]);
    assert_eq!(matches("use glibc;"), vec![]);
    assert_eq!(
        matches("/* libc\n/* UnixStream */\nstd::os::unix */\nuse libc;"),
        vec![(4, "libc")]
    );
}

fn files_under(path: &Path) -> io::Result<Vec<std::path::PathBuf>> {
    if !path.is_dir() {
        return Ok(vec![path.to_owned()]);
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(path)? {
        files.extend(files_under(&entry?.path())?);
    }
    files.sort();
    Ok(files)
}

const SIGNED_FIXTURE: &str = "crates/engine/tests/fixtures/claude/interrupt-honoured.jsonl";
const SIGNATURE_NOISE: &str = "Base64 noise inside Claude's recorded thinking signature";
const PLANNING_ID_ALLOWLIST: &[(&str, &str, &str)] = &[
    (
        SIGNED_FIXTURE,
        concat!("M", "4Bsi5UtffmEyJeUTDvyx"),
        SIGNATURE_NOISE,
    ),
    (SIGNED_FIXTURE, concat!("m", "8tpax"), SIGNATURE_NOISE),
    (
        SIGNED_FIXTURE,
        concat!("m", "9NaGF6TwHhtfTW5x5ZtxmSbknxAhVwZXOfnryXE4uiUXtM"),
        SIGNATURE_NOISE,
    ),
];

fn planning_id_violations(path: &str, source: &str) -> Vec<String> {
    let pattern = regex::Regex::new(concat!(
        r"(?i)(M[0-9]+\w*|SC[0-9]+|SF[0-9]+|wave[ _-]?(0|zero)|",
        r"lane[ _-]?[0-9]+|criterion[ _-]?[0-9]+|veyra-m[0-9]+\w*|c[0-9]+)"
    ))
    .unwrap();
    source
        .lines()
        .enumerate()
        .flat_map(|(index, line)| {
            pattern.find_iter(line).filter_map(move |found| {
                let before = line[..found.start()].chars().next_back();
                let after = line[found.end()..].chars().next();
                let criterion_prefix = found.as_str().to_ascii_lowercase().starts_with('c')
                    && found.as_str()[1..]
                        .bytes()
                        .all(|byte| byte.is_ascii_digit());
                if before.is_some_and(char::is_alphanumeric)
                    || after.is_some_and(char::is_alphanumeric)
                    || (criterion_prefix && after != Some('_'))
                {
                    return None;
                }
                let kept = PLANNING_ID_ALLOWLIST.iter().any(|&(file, text, reason)| {
                    assert!(!reason.is_empty());
                    file == path && text == found.as_str()
                });
                (!kept).then(|| format!("{path}:{}: {}", index + 1, found.as_str()))
            })
        })
        .collect()
}

#[test]
fn planning_ids_absent() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let mut violations = Vec::new();
    for name in ["crates", "ARCHITECTURE.md", ".agents"] {
        for path in files_under(&root.join(name)).unwrap() {
            let bytes = fs::read(&path).unwrap();
            if let Ok(source) = std::str::from_utf8(&bytes) {
                let relative = path.strip_prefix(root).unwrap().to_str().unwrap();
                violations.extend(planning_id_violations(relative, source));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "planning ids:\n{}",
        violations.join("\n")
    );
}

#[test]
fn planning_ids_absent_self_test() {
    for parts in [
        ["M", "0"],
        ["M", "8s2"],
        ["SC", "123"],
        ["SF", "7"],
        ["wave", " zero"],
        ["wave", "0"],
        ["Lane", " 2"],
        ["lane", " 3"],
        ["criterion", " 12"],
        ["veyra-m", "8"],
    ] {
        let planted = parts.concat();
        assert_eq!(planning_id_violations("planted", &planted).len(), 1);
    }
    for parts in [
        ["verify_m", "3", "_nested_dump_names"],
        ["M", "3", "_INITIAL_TOKEN_CALLS"],
        ["VEYRA_M", "6", "_RELAY"],
        ["veyra-m", "6", "s1-scratch"],
        ["veyra-m", "7", "s1-scratch"],
        ["wave", "0", "_probe"],
        ["VEYRA_M", "8", "_SHELL_CHILD"],
        ["m", "8", "-lanes"],
        ["wave-", "zero", "-session"],
        ["c", "1", "_fan_out"],
        ["", "M", "3"],
        ["VERIFY_M", "3", "_NESTED_DUMP_NAMES"],
        ["Wave-", "ZeRo", "-session"],
        ["lane", "-", "2"],
        ["lane", "_", "2"],
        ["wave", "_zero", "_probe"],
        ["WAVE", "_", "0"],
        ["criterion", "_", "3"],
        ["veyra-m", "8", "r1-scratch"],
        ["VEYRA_M", "9", "_POLISH"],
        ["M", "123", "r2"],
        ["veyra-m", "8speed", "-scratch"],
        ["M", "8", "perf"],
        ["m", "8", "review"],
    ] {
        let planted = parts.concat();
        assert!(
            !planning_id_violations("planted", &planted).is_empty(),
            "{planted}"
        );
    }
    assert!(planning_id_violations("clean", "SM0 SCHEMA SFoo waveform lane width").is_empty());
    assert_eq!(
        planning_id_violations("planted", &["clean\n", "M", "3"].concat())[0],
        ["planted:2: ", "M", "3"].concat()
    );
    let ids = (3..=8)
        .map(|n| format!("{}{n}/", "M"))
        .collect::<String>()
        .repeat(4);
    let encoded = format!(r#"{{"signature":"{ids}"}}"#);
    assert_eq!(planning_id_violations("fixture.jsonl", &encoded).len(), 24);
    assert!(!PLANNING_ID_ALLOWLIST.is_empty());
    for &(file, kept, reason) in PLANNING_ID_ALLOWLIST {
        assert!(!reason.is_empty());
        let line = format!(r#"{{"signature":"AAAA+{kept}+AAAA"}}"#);
        assert!(planning_id_violations(file, &line).is_empty(), "{kept}");
        assert_eq!(planning_id_violations("other.jsonl", &line).len(), 1);
        let other = format!(r#"{{"signature":"AAAA+{kept}+{}{}+AAAA"}}"#, "M", 3);
        assert_eq!(planning_id_violations(file, &other).len(), 1);
    }
}

fn long_rust_lines(path: &str, source: &str) -> Vec<String> {
    source
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let columns = line.chars().count();
            (columns > 120).then(|| format!("{path}:{}: {columns} columns", index + 1))
        })
        .collect()
}

#[test]
fn rust_lines_within_120_columns() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let violations: Vec<_> = files_under(crates)
        .unwrap()
        .into_iter()
        .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
        .flat_map(|path| {
            long_rust_lines(path.to_str().unwrap(), &fs::read_to_string(&path).unwrap())
        })
        .collect();
    assert!(
        violations.is_empty(),
        "long Rust lines:\n{}",
        violations.join("\n")
    );
}

#[test]
fn rust_lines_within_120_columns_self_test() {
    assert!(long_rust_lines("clean", &"a".repeat(120)).is_empty());
    assert!(long_rust_lines("clean", &"λ".repeat(120)).is_empty());
    assert_eq!(
        long_rust_lines("planted", &format!("short\n{}", "x".repeat(121))),
        vec!["planted:2: 121 columns"]
    );
}

const HERMETIC_ALLOWLIST: &[(&str, &str, &str)] = &[(
    "engine/tests/replay_claude.rs",
    "check_child_env_snapshot",
    "The subprocess installs its own scripted login shell and invokes only /bin/sh.",
)];

fn hermetic_violations(path: &str, source: &str) -> Vec<String> {
    let path = Path::new(path);
    if path
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("live_")
        || path.ends_with("tests/common/live.rs")
    {
        return Vec::new();
    }
    let code = blank_source(source, true);
    let calls = regex::Regex::new(
        r"\b(?:ChildEnv\s*::\s*load|Command\s*::\s*new|Engine\s*::\s*(?:start|start_with_processes))\s*\(",
    )
    .unwrap();
    let functions = regex::Regex::new(r"\bfn\s+([A-Za-z_][A-Za-z_0-9]*)\s*\(").unwrap();
    let identifiers = regex::Regex::new(r"\b[A-Za-z_][A-Za-z_0-9]*\b").unwrap();
    let bindings = regex::Regex::new(
        r"\b(?:let|const|static)\s+(?:mut\s+)?([A-Za-z_][A-Za-z_0-9]*)\s*(?::[^=;]+)?=",
    )
    .unwrap();
    let provider_names =
        crate::provider::ProviderKind::ALL.map(crate::provider::ProviderKind::binary);
    let provider = |text: &str| {
        let text = text.to_ascii_lowercase();
        provider_names.iter().any(|name| text.contains(name))
    };
    let literals: Vec<_> = source_spans(source)
        .into_iter()
        .filter(|span| matches!(span.kind, SourceKind::Literal))
        .collect();
    let provider_variables: std::collections::BTreeSet<_> = bindings
        .captures_iter(&code)
        .filter(|binding| {
            let start = binding.get(0).unwrap().end();
            let end = code[start..]
                .find(';')
                .map_or(code.len(), |offset| start + offset);
            literals.iter().any(|span| {
                span.start >= start && span.end <= end && provider(&source[span.start..span.end])
            })
        })
        .map(|binding| binding[1].to_owned())
        .collect();
    let mut kept = std::collections::BTreeSet::new();
    calls
        .find_iter(&code)
        .filter(|call| {
            if !call.as_str().starts_with("Command") {
                return true;
            }
            let mut depth = 1;
            let end = code[call.end()..]
                .char_indices()
                .find_map(|(offset, character)| {
                    depth += i32::from(character == '(') - i32::from(character == ')');
                    (depth == 0).then_some(call.end() + offset)
                })
                .unwrap_or(code.len());
            literals.iter().any(|span| {
                span.start >= call.end()
                    && span.end <= end
                    && provider(&source[span.start..span.end])
            }) || identifiers.find_iter(&code[call.end()..end]).any(|name| {
                let before = code[call.end()..call.end() + name.start()].trim_end();
                let after = code[call.end() + name.end()..end].trim_start();
                let field = after.strip_prefix('.').is_some_and(|tail| {
                    identifiers
                        .find(tail)
                        .is_some_and(|field| !tail[field.end()..].trim_start().starts_with('('))
                });
                provider_variables.contains(name.as_str())
                    && !before.ends_with('.')
                    && !after.starts_with("::")
                    && !after.starts_with('!')
                    && !field
            })
        })
        .filter_map(|call| {
            let function = functions.captures_iter(&code[..call.start()]).last();
            let allowed =
                HERMETIC_ALLOWLIST
                    .iter()
                    .enumerate()
                    .find(|(_, (file, name, reason))| {
                        assert!(!reason.is_empty());
                        path == Path::new(file)
                            && function
                                .as_ref()
                                .is_some_and(|function| &function[1] == *name)
                            && call.as_str().starts_with("ChildEnv")
                    });
            if allowed.is_some_and(|(index, _)| kept.insert(index)) {
                return None;
            }
            let line = source[..call.start()]
                .bytes()
                .filter(|&byte| byte == b'\n')
                .count()
                + 1;
            Some(format!("{}:{line}: {}", path.display(), call.as_str()))
        })
        .collect()
}

#[test]
fn tests_stay_hermetic() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let violations: Vec<_> = files_under(crates)
        .unwrap()
        .into_iter()
        .filter(|path| {
            path.extension().is_some_and(|extension| extension == "rs")
                && path
                    .strip_prefix(crates)
                    .unwrap()
                    .components()
                    .nth(1)
                    .is_some_and(|component| component.as_os_str() == "tests")
        })
        .flat_map(|path| {
            hermetic_violations(
                path.strip_prefix(crates).unwrap().to_str().unwrap(),
                &fs::read_to_string(&path).unwrap(),
            )
        })
        .collect();
    assert!(
        violations.is_empty(),
        "host-dependent tests:\n{}",
        violations.join("\n")
    );
}

#[test]
fn tests_stay_hermetic_self_test() {
    for source in [
        "fn ordinary() { let marker = \"/*\";\nChildEnv::load(); }",
        "fn ordinary() { let url = \"https://example.test\"; ChildEnv::load(); }",
        "fn ordinary() { let url = \"https://example.test\"; Command::new(\"codex\"); }",
        "fn ordinary() { let marker = r#\"/*\"#; ChildEnv::load(); }",
        "fn ordinary() { let marker = b\"/*\"; ChildEnv::load(); }",
        "fn ordinary() { let marker = br#\"/*\"#; ChildEnv::load(); }",
        "fn ordinary() { let marker = '/'; let star = '*'; ChildEnv::load(); }",
        "fn ordinary() { Command::new(\"/opt/homebrew/bin/claude\"); }",
        "fn ordinary() { let cli = \"codex\"; Command::new(cli); }",
        "fn ordinary() { let cli = /* marker */ \"grok\"; Command::new(cli); }",
        "fn ordinary() { const CLI: &str = \"codex\"; Command::new(CLI); }",
        "fn ordinary() { static CLI: &str = \"claude\"; Command::new(CLI); }",
        "fn ordinary() { let cli = String::from(\"codex\"); Command::new(cli); }",
        "fn ordinary() { let cli = PathBuf::from(\"/opt/homebrew/bin/claude\"); Command::new(cli); }",
        "fn ordinary() { Engine::start(path); }",
        "fn ordinary() { Engine::start_with_processes(path, processes); }",
        "fn ordinary() { ChildEnv::load(); }",
        "fn ordinary() { ChildEnv :: load (); }",
        "fn ordinary() { std::process::Command::new(\"codex\"); }",
        "fn ordinary() { Command::new (\"claude\"); }",
        "fn ordinary() { Command::new(r\"grok\"); }",
    ] {
        assert_eq!(
            hermetic_violations("engine/tests/planted.rs", source).len(),
            1
        );
        assert!(hermetic_violations("engine/tests/live_probe.rs", source).is_empty());
        assert!(hermetic_violations("engine/tests/common/live.rs", source).is_empty());
    }
    for source in [
        r##"fn clean() { let text = r#"ChildEnv::load(); Command::new("codex");"#; }"##,
        r#"fn clean() { let slash = '/'; let quote = '\''; let byte = b'/'; }"#,
        r#"fn clean() { /* nested /* ChildEnv::load(); */ Engine::start(path); */ }"#,
        r#"fn clean() { let env = "codex"; Command::new(std::env::current_exe()); }"#,
        r#"fn clean() { let env = "codex"; Command::new(env!("CARGO_BIN_EXE_veyra-fake-cli")); }"#,
        r#"fn clean() { let env = "codex"; Command::new(env.claude_bin.as_ref().unwrap()); }"#,
    ] {
        assert!(hermetic_violations("engine/tests/clean.rs", source).is_empty());
    }
    let allowed = "fn check_child_env_snapshot() { ChildEnv::load(); }";
    assert!(hermetic_violations("engine/tests/replay_claude.rs", allowed).is_empty());
    assert_eq!(
        hermetic_violations("engine/tests/other.rs", allowed).len(),
        1
    );
    assert_eq!(
        hermetic_violations(
            "engine/tests/replay_claude.rs",
            &allowed.replace(";", "; ChildEnv::load();")
        )
        .len(),
        1
    );
    assert!(
        hermetic_violations(
            "engine/tests/clean.rs",
            "fn clean() { ChildEnv::isolated(); Command::new(\"/bin/sh\"); } // ChildEnv::load()"
        )
        .is_empty()
    );
}

#[test]
fn actor_no_blocking_io() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations = Vec::new();
    for name in ["actor.rs", "store.rs", "commands.rs"] {
        for (line, text) in blank_comments(&fs::read_to_string(root.join(name)).unwrap())
            .lines()
            .enumerate()
        {
            for needle in ["std::fs", "Command::new", "thread::sleep"] {
                if text.contains(needle) {
                    violations.push(format!("{name}:{}: {needle}", line + 1));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "blocking actor calls: {}",
        violations.join("\n")
    );
}
