use gpui_kit::component::Icon;
use gpui_kit::{
    AssetSource, Hsla, IntoElement, ParentElement, Pixels, Result, SharedString, Styled, div,
};
use std::borrow::Cow;
use veyra_engine::domain::Status;

pub(super) const WORKING_STEPS_PER_TURN: u32 = 12;

gpui_kit::assets::icon_assets!(
    ControlAssets,
    [
        FolderPlus,
        SquarePen,
        X,
        TextWrap,
        Shield,
        ShieldAlert,
        GitBranch,
        Archive,
        Plug,
        Brain,
        Wrench,
        CornerDownLeft,
        Command,
        Terminal,
    ]
);

const MARKS: [(&str, &[u8]); 13] = [
    (
        "marks/working.svg",
        include_bytes!("../../assets/marks/working.svg"),
    ),
    (
        "marks/needs-you.svg",
        include_bytes!("../../assets/marks/needs-you.svg"),
    ),
    (
        "marks/done.svg",
        include_bytes!("../../assets/marks/done.svg"),
    ),
    (
        "marks/failed.svg",
        include_bytes!("../../assets/marks/failed.svg"),
    ),
    (
        "marks/idle.svg",
        include_bytes!("../../assets/marks/idle.svg"),
    ),
    (
        "marks/held.svg",
        include_bytes!("../../assets/marks/held.svg"),
    ),
    (
        "marks/interrupted.svg",
        include_bytes!("../../assets/marks/interrupted.svg"),
    ),
    (
        "marks/child.svg",
        include_bytes!("../../assets/marks/child.svg"),
    ),
    (
        "marks/lineage.svg",
        include_bytes!("../../assets/marks/lineage.svg"),
    ),
    (
        "marks/claude.svg",
        include_bytes!("../../assets/marks/claude.svg"),
    ),
    (
        "marks/codex.svg",
        include_bytes!("../../assets/marks/codex.svg"),
    ),
    (
        "marks/grok.svg",
        include_bytes!("../../assets/marks/grok.svg"),
    ),
    (
        "marks/app.svg",
        include_bytes!("../../assets/marks/app.svg"),
    ),
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some((_, bytes)) = MARKS.iter().find(|(name, _)| *name == path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        if let Some(bytes) = ControlAssets.load(path)? {
            return Ok(Some(bytes));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(ControlAssets.list(path)?);
        paths.extend(
            MARKS
                .iter()
                .filter(|(name, _)| name.starts_with(path))
                .map(|(name, _)| (*name).into()),
        );
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

pub fn status_path(status: &Status) -> &'static str {
    match status {
        Status::Working { .. } => "marks/working.svg",
        Status::NeedsYou => "marks/needs-you.svg",
        Status::Done { .. } => "marks/done.svg",
        Status::Failed => "marks/failed.svg",
        Status::Idle => "marks/idle.svg",
        Status::Held => "marks/held.svg",
        Status::Interrupted => "marks/interrupted.svg",
    }
}

pub(super) fn status_mark(
    status: &Status,
    phase: u8,
    size: Pixels,
    color: Hsla,
) -> impl IntoElement {
    let mark = Icon::default()
        .path(status_path(status))
        .rotate(gpui_kit::radians(
            if matches!(status, Status::Working { .. }) {
                phase as f32 * std::f32::consts::TAU / WORKING_STEPS_PER_TURN as f32
            } else {
                0.
            },
        ))
        .text_color(color);
    div().size(size).flex_none().child(mark.size(size))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_paths_are_distinct() {
        use super::super::{
            sidebar::glyph_color,
            theme::{Appearance, Theme},
        };

        let paths = [
            (Status::Working { since: 0 }, "marks/working.svg", "accent"),
            (Status::NeedsYou, "marks/needs-you.svg", "warning"),
            (Status::Done { took: 0 }, "marks/done.svg", "success"),
            (Status::Failed, "marks/failed.svg", "danger"),
            (Status::Idle, "marks/idle.svg", "text_muted"),
            (Status::Held, "marks/held.svg", "text_muted"),
            (Status::Interrupted, "marks/interrupted.svg", "text_muted"),
        ];
        for (status, path, role) in &paths {
            assert_eq!(status_path(status), *path);
            for appearance in [Appearance::Dark, Appearance::Light] {
                let theme = Theme::new(appearance);
                let expected = match *role {
                    "accent" => theme.colors.accent,
                    "warning" => theme.colors.warning,
                    "success" => theme.colors.success,
                    "danger" => theme.colors.danger,
                    "text_muted" => theme.colors.text_muted,
                    _ => unreachable!(),
                };
                assert_eq!(
                    glyph_color(status, &theme),
                    expected,
                    "{status:?} {appearance:?}"
                );
            }
        }
        assert_eq!(
            paths
                .iter()
                .map(|(status, _, _)| status_path(status))
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            paths.len()
        );
    }

    #[test]
    fn icon_paths_resolve() {
        let source = include_str!("mod.rs");
        let assets: &dyn AssetSource = if source.contains(".with_assets(icons::Assets)") {
            &Assets
        } else {
            &gpui_kit::assets::Assets
        };
        for path in Assets.list("").unwrap() {
            assert!(
                assets.load(&path).unwrap().is_some(),
                "unresolved icon {path}"
            );
        }
        let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui");
        for entry in std::fs::read_dir(folder).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|extension| extension != "rs") {
                continue;
            }
            let source = std::fs::read_to_string(path).unwrap();
            for reference in source.split("IconName::").skip(1) {
                let variant: String = reference
                    .chars()
                    .take_while(|character| character.is_ascii_alphanumeric() || *character == '_')
                    .collect();
                if let Some(name) = gpui_kit::assets::IconName::ALL
                    .iter()
                    .find(|name| format!("{name:?}") == variant)
                {
                    let path = name.path();
                    assert!(
                        assets.load(&path).unwrap().is_some(),
                        "unresolved icon {path}"
                    );
                }
            }
            for literal in source.split('"').skip(1).step_by(2) {
                if (literal.starts_with("icons/") || literal.starts_with("marks/"))
                    && literal.ends_with(".svg")
                {
                    assert!(
                        assets.load(literal).unwrap().is_some(),
                        "unresolved icon {literal}"
                    );
                }
            }
        }
    }
}
