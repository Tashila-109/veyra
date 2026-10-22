use std::{collections::BTreeSet, fs, path::Path};

fn identifiers(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|s| !s.is_empty())
}
fn dump_keys(source: &str) -> BTreeSet<String> {
    let source = source.split("pub fn dump(").nth(1).expect("Shell::dump");
    let source = source
        .split("\n    #[cfg(debug_assertions)]\n    fn view_name")
        .next()
        .unwrap();
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
fn verify_nested_dump_names() {
    let skill =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.agents/skills/verify-veyra/SKILL.md");
    let map = fs::read_to_string(skill).unwrap();
    let mapped: BTreeSet<_> = identifiers(&map).collect();
    for name in [
        "follow",
        "gap",
        "top_item",
        "top_offset",
        "item_tops",
        "outcome",
        "wait_reason",
        "failed_last_line",
        "stop_visible",
        "discard_queued_visible",
        "dock",
        "request",
        "position",
        "count",
        "focused",
        "question_index",
        "posted",
        "open",
        "threads",
        "hold",
        "visible",
        "applied_batches",
        "max_batch_gap_ms",
        "transcript_open_ms",
        "drawer",
        "focus",
        "composer_caret",
        "active_tab",
        "tabs",
        "terminal",
        "grid",
        "columns",
        "rows",
        "flood",
        "pty_bytes",
        "elapsed_ms",
        "median_repaint_gap_ms",
        "max_repaint_gap_ms",
    ] {
        assert!(mapped.contains(name), "verify-veyra map is missing {name}");
    }
}
#[test]
fn verify_map_names() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let skill = root.join(".agents/skills/verify-veyra");
    let mut map = fs::read_to_string(skill.join("SKILL.md")).unwrap();
    let mut features = String::new();
    for entry in fs::read_dir(skill.join("features")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "md") {
            let page = fs::read_to_string(path).unwrap();
            features.push_str(&page);
            map.push_str(&page);
        }
    }
    let feature_names: BTreeSet<_> = identifiers(&features).collect();
    for tool in veyra_engine::tools::list()
        .as_array()
        .expect("engine tool list")
    {
        let name = tool["name"].as_str().expect("tool name");
        assert!(
            tool["description"]
                .as_str()
                .is_some_and(|text| !text.trim().is_empty()),
            "empty description for {name}"
        );
        assert!(
            feature_names.contains(name),
            "feature map is missing tool {name}"
        );
    }
    let mapped: BTreeSet<_> = identifiers(&map).collect();
    let actions = fs::read_to_string(root.join("crates/veyra/src/ui/actions.rs")).unwrap();
    let mut names = BTreeSet::new();
    for block in actions.split("actions!(").skip(1) {
        let list = block.split('[').nth(1).unwrap().split(']').next().unwrap();
        names.extend(identifiers(list).map(str::to_owned));
    }
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
