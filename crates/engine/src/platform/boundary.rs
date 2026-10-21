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

fn blank_comments(source: &str) -> String {
    let mut bytes = source.as_bytes().to_vec();
    let mut index = 0;
    let mut block_depth = 0;
    let mut line_comment = false;
    while index < bytes.len() {
        let pair = source.as_bytes().get(index..index + 2);
        let width = if line_comment {
            if bytes[index] == b'\n' {
                line_comment = false;
            }
            1
        } else if pair == Some(b"/*") {
            block_depth += 1;
            2
        } else if block_depth > 0 && pair == Some(b"*/") {
            block_depth -= 1;
            bytes[index..index + 2].fill(b' ');
            index += 2;
            continue;
        } else if block_depth == 0 && pair == Some(b"//") {
            line_comment = true;
            2
        } else {
            1
        };
        if line_comment || block_depth > 0 {
            for byte in &mut bytes[index..index + width] {
                if *byte != b'\n' && *byte != b'\r' {
                    *byte = b' ';
                }
            }
        }
        index += width;
    }
    String::from_utf8(bytes).expect("comment blanking preserves UTF-8")
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

const PLANNING_ID_ALLOWLIST: &[(&str, &str, &str)] = &[];

fn planning_id_violations(path: &str, source: &str) -> Vec<String> {
    let pattern = regex::Regex::new(
        r"\b([M][0-8](s[0-9])?|SC[0-9]+|SF[0-9]+|wave ?(0|zero)|[Ll]ane [0-9]|criterion [0-9]+|veyra-m[0-9])\b",
    )
    .unwrap();
    source
        .lines()
        .enumerate()
        .flat_map(|(index, line)| {
            pattern.find_iter(line).filter_map(move |found| {
                let kept = PLANNING_ID_ALLOWLIST.iter().any(|&(file, text, reason)| {
                    assert!(!reason.is_empty());
                    file == path && text == line
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
        ["veyra-", "m8"],
    ] {
        let planted = parts.concat();
        assert_eq!(planning_id_violations("planted", &planted).len(), 1);
    }
    assert!(planning_id_violations("clean", "SM0 SCHEMA SFoo waveform lane width").is_empty());
    assert_eq!(
        planning_id_violations("planted", &["clean\n", "M", "3"].concat())[0],
        ["planted:2: ", "M", "3"].concat()
    );
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
    let source = blank_comments(source);
    let calls = regex::Regex::new(
        r##"\b(?:ChildEnv\s*::\s*load\s*\(|Command\s*::\s*new\s*\(\s*(?:r#*)?"(?:claude|codex|grok)")"##,
    )
    .unwrap();
    let functions = regex::Regex::new(r"\bfn\s+([A-Za-z_][A-Za-z_0-9]*)\s*\(").unwrap();
    let mut kept = std::collections::BTreeSet::new();
    calls
        .find_iter(&source)
        .filter_map(|call| {
            let function = functions.captures_iter(&source[..call.start()]).last();
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
