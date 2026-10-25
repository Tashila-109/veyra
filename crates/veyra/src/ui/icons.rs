use gpui_kit::component::Icon;
use gpui_kit::{
    AssetSource, Hsla, IntoElement, ParentElement, Pixels, Radians, Result, SharedString, Styled,
    div,
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

trait MarkArgument: Copy {
    const APPLY: fn(Icon, Self) -> Icon;
    #[cfg(test)]
    fn call(self) -> MarkCall;
}

impl MarkArgument for &'static str {
    const APPLY: fn(Icon, Self) -> Icon = Icon::path;
    #[cfg(test)]
    fn call(self) -> MarkCall {
        MarkCall::Path(self)
    }
}

impl MarkArgument for Radians {
    const APPLY: fn(Icon, Self) -> Icon = Icon::rotate;
    #[cfg(test)]
    fn call(self) -> MarkCall {
        MarkCall::Rotation(self)
    }
}

impl MarkArgument for Hsla {
    const APPLY: fn(Icon, Self) -> Icon = Icon::text_color;
    #[cfg(test)]
    fn call(self) -> MarkCall {
        MarkCall::Color(self)
    }
}

fn paint_argument<T: MarkArgument>(mark: Icon, argument: T) -> Icon {
    #[cfg(test)]
    MARK_CALLS.with_borrow_mut(|calls| calls.push(argument.call()));
    (T::APPLY)(mark, argument)
}

pub(super) fn status_mark(
    status: &Status,
    phase: u8,
    size: Pixels,
    color: Hsla,
) -> impl IntoElement {
    let mark = paint_argument(Icon::default(), status_path(status));
    let mark = paint_argument(
        mark,
        gpui_kit::radians(if matches!(status, Status::Working { .. }) {
            phase as f32 * std::f32::consts::TAU / WORKING_STEPS_PER_TURN as f32
        } else {
            0.
        }),
    );
    let mark = paint_argument(mark, color);
    div().size(size).flex_none().child(mark.size(size))
}

#[cfg(test)]
enum MarkCall {
    Path(&'static str),
    Rotation(Radians),
    Color(Hsla),
}

#[cfg(test)]
thread_local! {
    static MARK_CALLS: std::cell::RefCell<Vec<MarkCall>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
pub(super) fn take_painted_marks() -> Vec<(&'static str, Radians, Hsla)> {
    let calls = MARK_CALLS.with_borrow_mut(std::mem::take);
    assert_eq!(calls.len() % 3, 0);
    calls
        .as_chunks::<3>()
        .0
        .iter()
        .map(|calls| match calls {
            [
                MarkCall::Path(path),
                MarkCall::Rotation(rotation),
                MarkCall::Color(color),
            ] => (*path, *rotation, *color),
            _ => panic!("a status mark must apply path, rotation and color once"),
        })
        .collect()
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
