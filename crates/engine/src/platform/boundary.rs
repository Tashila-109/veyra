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
