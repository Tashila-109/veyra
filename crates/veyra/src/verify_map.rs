use std::{collections::BTreeSet, fs, path::Path};

fn identifiers(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|s| !s.is_empty())
}
fn dump_keys(source: &str) -> BTreeSet<String> {
    let source = source.split("pub fn dump(").nth(1).expect("Shell::dump");
    let source = source.split("\nimpl Focusable").next().unwrap();
    let source = source.rsplit("json!({").next().expect("dump object");
    let mut keys = BTreeSet::new();
    let mut depth = 1;
    let mut chars = source.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            '"' => {
                let mut value = String::new();
                while let Some(c) = chars.next() {
                    if c == '\\' {
                        chars.next();
                        continue;
                    }
                    if c == '"' {
                        break;
                    }
                    value.push(c);
                }
                while chars.peek().is_some_and(|c| c.is_whitespace()) {
                    chars.next();
                }
                if depth == 1 && chars.peek() == Some(&':') {
                    keys.insert(value);
                }
            }
            _ => {}
        }
    }
    keys
}
#[test]
fn verify_map_names() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let skill = root.join(".agents/skills/verify-veyra");
    let mut map = fs::read_to_string(skill.join("SKILL.md")).unwrap();
    for entry in fs::read_dir(skill.join("features")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "md") {
            map.push_str(&fs::read_to_string(path).unwrap());
        }
    }
    let mapped: BTreeSet<_> = identifiers(&map).collect();
    let actions = fs::read_to_string(root.join("crates/veyra/src/ui/actions.rs")).unwrap();
    let list = actions
        .split("actions!(")
        .nth(1)
        .unwrap()
        .split('[')
        .nth(1)
        .unwrap()
        .split(']')
        .next()
        .unwrap();
    let mut names: BTreeSet<String> = identifiers(list).map(str::to_owned).collect();
    for part in actions.split("#[action(namespace = veyra)]").skip(1) {
        let name = part
            .split("struct ")
            .nth(1)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap();
        if !name.starts_with('$') {
            names.insert(name.to_owned());
        }
    }
    for part in actions.split("choice_action!(").skip(1) {
        names.insert(part.split(',').next().unwrap().trim().to_owned());
    }
    names.insert("settings".into());
    let control = fs::read_to_string(root.join("crates/veyra/src/control.rs")).unwrap();
    let parser = control
        .split("fn parse_args(")
        .nth(1)
        .unwrap()
        .split("pub fn client(")
        .next()
        .unwrap();
    for part in parser.split("command == \"").skip(1) {
        names.insert(part.split('"').next().unwrap().to_owned());
    }
    let ui = fs::read_to_string(root.join("crates/veyra/src/ui/mod.rs")).unwrap();
    names.extend(dump_keys(&ui));
    for part in control.split("dump[\"").skip(1) {
        names.insert(part.split('"').next().unwrap().to_owned());
    }
    let missing: Vec<_> = names
        .iter()
        .filter(|n| !mapped.contains(n.as_str()))
        .collect();
    assert!(
        missing.is_empty(),
        "verify-veyra map is missing names: {missing:?}"
    );
}
