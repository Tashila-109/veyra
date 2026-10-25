use gpui_kit::component::Icon;
use gpui_kit::{
    AssetSource, Hsla, InteractiveElement, IntoElement, ParentElement, Pixels, Radians, Result,
    SharedString, Styled, div,
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

pub(super) struct StatusMark {
    path: &'static str,
    rotation: Radians,
    size: Pixels,
    color: Hsla,
}

impl StatusMark {
    fn path(mut self, path: &'static str) -> Self {
        self.path = path;
        self
    }

    fn rotate(mut self, rotation: Radians) -> Self {
        self.rotation = rotation;
        self
    }
}

impl IntoElement for StatusMark {
    type Element = <gpui_kit::Div as IntoElement>::Element;

    fn into_element(self) -> Self::Element {
        div()
            .size(self.size)
            .flex_none()
            .debug_selector(|| {
                format!(
                    "status-mark:{}:{:.6}:{:?}",
                    self.path, self.rotation.0, self.color
                )
            })
            .child(
                Icon::default()
                    .path(self.path)
                    .size(self.size)
                    .text_color(self.color)
                    .rotate(self.rotation),
            )
    }
}

pub(super) fn status_mark(status: &Status, phase: u8, size: Pixels, color: Hsla) -> StatusMark {
    StatusMark {
        path: "",
        rotation: gpui_kit::radians(0.),
        size,
        color,
    }
    .path(status_path(status))
    .rotate(gpui_kit::radians(
        if matches!(status, Status::Working { .. }) {
            phase as f32 * std::f32::consts::TAU / WORKING_STEPS_PER_TURN as f32
        } else {
            0.
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

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
