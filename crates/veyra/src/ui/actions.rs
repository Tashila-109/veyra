use gpui_kit::*;
use veyra_engine::{
    domain::{ItemId, PermissionMode, ProjectId, RequestId, ThreadId, TurnId},
    provider::ProviderKind,
};

#[cfg(debug_assertions)]
actions!(veyra, [ActivateWindow]);

actions!(
    veyra,
    [
        Quit,
        ConfirmQuit,
        CancelQuit,
        Continue,
        DiscardQueued,
        NextNeedsYou,
        FocusDock,
        FocusComposer,
        ScrollTranscriptUp,
        ScrollTranscriptDown,
        ScrollTranscriptEnd,
        NewThread,
        Send,
        OpenSettings,
        RecheckProviders,
        OpenProviderPicker,
        Retry,
        OpenTerminal,
        NarrowSidebar,
        WidenSidebar,
        ResetSidebar,
        DismissNotice
    ]
);

macro_rules! choice_action {
    ($name:ident, $field:ident, $kind:ty) => {
        #[derive(
            Clone,
            Default,
            PartialEq,
            serde::Deserialize,
            ::gpui::private::schemars::JsonSchema,
            Action,
        )]
        #[serde(default)]
        #[schemars(crate = "::gpui::private::schemars")]
        #[action(namespace = veyra)]
        pub struct $name {
            #[schemars(with = "Option<String>")]
            pub $field: Option<$kind>,
        }
    };
}
#[derive(Clone, Copy, PartialEq, serde::Deserialize, ::gpui::private::schemars::JsonSchema)]
#[schemars(crate = "::gpui::private::schemars")]
pub struct StopTarget {
    #[schemars(with = "String")]
    pub thread: ThreadId,
    #[schemars(with = "String")]
    pub turn: TurnId,
}
#[derive(
    Clone, Default, PartialEq, serde::Deserialize, ::gpui::private::schemars::JsonSchema, Action,
)]
#[serde(default)]
#[schemars(crate = "::gpui::private::schemars")]
#[action(namespace = veyra)]
pub struct Stop {
    pub target: Option<StopTarget>,
}
choice_action!(AddProject, path, std::path::PathBuf);
#[derive(
    Clone, Default, PartialEq, serde::Deserialize, ::gpui::private::schemars::JsonSchema, Action,
)]
#[serde(default)]
#[schemars(crate = "::gpui::private::schemars")]
#[action(namespace = veyra)]
pub struct SwitchView {
    pub terminal: Option<bool>,
}
choice_action!(PickProvider, provider, ProviderKind);
choice_action!(PickModel, model, String);
choice_action!(PickEffort, effort, String);
choice_action!(PickPermission, mode, PermissionMode);
choice_action!(SetDefaultMode, mode, PermissionMode);
choice_action!(SelectThread, thread, ThreadId);
choice_action!(StopThread, thread, ThreadId);
choice_action!(OpenThreadMenu, thread, ThreadId);
choice_action!(Archive, thread, ThreadId);
choice_action!(ArchiveKeepChildren, thread, ThreadId);
choice_action!(AllowOnce, request, RequestId);
choice_action!(AllowInThread, request, RequestId);
choice_action!(Deny, request, RequestId);

#[derive(
    Clone, Default, PartialEq, serde::Deserialize, ::gpui::private::schemars::JsonSchema, Action,
)]
#[serde(default)]
#[schemars(crate = "::gpui::private::schemars")]
#[action(namespace = veyra)]
pub struct AnswerQuestion {
    #[schemars(with = "Option<String>")]
    pub request: Option<RequestId>,
    pub question: Option<usize>,
    pub option: Option<usize>,
}
#[derive(
    Clone, Default, PartialEq, serde::Deserialize, ::gpui::private::schemars::JsonSchema, Action,
)]
#[serde(default)]
#[schemars(crate = "::gpui::private::schemars")]
#[action(namespace = veyra)]
pub struct ChooseAsyncOption {
    #[schemars(with = "Option<String>")]
    pub item: Option<ItemId>,
    pub question: Option<usize>,
    pub option: Option<usize>,
}
choice_action!(SelectProject, project, ProjectId);
choice_action!(ToggleItem, item, ItemId);
#[cfg(debug_assertions)]
choice_action!(DebugNotificationResponse, tag, String);

#[derive(Clone, PartialEq, serde::Deserialize, ::gpui::private::schemars::JsonSchema, Action)]
#[schemars(crate = "::gpui::private::schemars")]
#[action(namespace = veyra)]
pub struct SetSidebarWidth {
    pub width: f32,
}
#[cfg(debug_assertions)]
#[derive(Clone, PartialEq, serde::Deserialize, ::gpui::private::schemars::JsonSchema, Action)]
#[schemars(crate = "::gpui::private::schemars")]
#[action(namespace = veyra)]
pub struct DebugStallUi {
    pub milliseconds: u64,
}

#[cfg(debug_assertions)]
#[derive(Clone, PartialEq, serde::Deserialize, ::gpui::private::schemars::JsonSchema, Action)]
#[schemars(crate = "::gpui::private::schemars")]
#[action(namespace = veyra)]
pub struct DebugScrollTranscript {
    pub pixels: f32,
}

#[cfg(debug_assertions)]
#[derive(Clone, PartialEq, serde::Deserialize, ::gpui::private::schemars::JsonSchema, Action)]
#[schemars(crate = "::gpui::private::schemars")]
#[action(namespace = veyra)]
pub struct DebugScrollApproval {
    pub pixels: f32,
}

pub struct Binding {
    pub action: &'static str,
    pub chord: &'static str,
    pub context: Option<&'static str>,
    build: fn() -> Box<dyn Action>,
}
macro_rules! binding {
    ($action:ident, $chord:literal, $context:expr, $field:ident: $value:expr) => {
        Binding {
            action: stringify!($action),
            chord: $chord,
            context: $context,
            build: || {
                Box::new($action {
                    $field: Some($value),
                    ..Default::default()
                })
            },
        }
    };
    ($action:ident, $chord:literal, $context:expr) => {
        Binding {
            action: stringify!($action),
            chord: $chord,
            context: $context,
            build: || Box::new($action::default()),
        }
    };
}
pub fn bindings() -> Vec<Binding> {
    vec![
        binding!(Quit, "secondary-q", None),
        binding!(SwitchView, "secondary-shift-v", None),
        binding!(ConfirmQuit, "secondary-enter", Some("VeyraQuit")),
        binding!(CancelQuit, "escape", Some("VeyraQuit")),
        binding!(OpenSettings, "secondary-,", Some("Veyra")),
        binding!(AddProject, "secondary-shift-o", Some("Veyra")),
        binding!(NewThread, "secondary-n", Some("Veyra")),
        binding!(PickProvider, "secondary-alt-p", Some("Veyra")),
        binding!(OpenProviderPicker, "secondary-alt-shift-p", Some("Veyra")),
        binding!(PickModel, "secondary-alt-m", Some("Veyra")),
        binding!(PickEffort, "secondary-alt-e", Some("Veyra")),
        binding!(PickPermission, "secondary-alt-a", Some("Veyra")),
        binding!(Send, "secondary-enter", Some("Veyra")),
        binding!(Stop, "ctrl-c", Some("VeyraComposer")),
        binding!(Continue, "secondary-shift-enter", Some("Veyra")),
        binding!(DiscardQueued, "secondary-shift-backspace", Some("Veyra")),
        binding!(OpenThreadMenu, "secondary-alt-shift-m", Some("Veyra")),
        binding!(Archive, "secondary-shift-a", Some("Veyra")),
        binding!(ArchiveKeepChildren, "secondary-alt-shift-a", Some("Veyra")),
        binding!(StopThread, "secondary-alt-shift-s", Some("Veyra")),
        binding!(NextNeedsYou, "secondary-shift-j", Some("Veyra")),
        binding!(AllowOnce, "secondary-enter", Some("VeyraApproval")),
        binding!(AllowInThread, "secondary-alt-enter", Some("VeyraApproval")),
        binding!(Deny, "secondary-.", Some("VeyraApproval")),
        binding!(FocusDock, "secondary-shift-y", Some("Veyra")),
        binding!(FocusComposer, "escape", Some("VeyraDock")),
        binding!(ScrollTranscriptUp, "pageup", Some("Veyra")),
        binding!(ScrollTranscriptDown, "pagedown", Some("Veyra")),
        binding!(ScrollTranscriptEnd, "secondary-end", Some("Veyra")),
        binding!(RecheckProviders, "secondary-alt-r", Some("Veyra")),
        binding!(SetDefaultMode, "secondary-alt-d", Some("Veyra")),
        binding!(SelectThread, "secondary-alt-1", Some("Veyra")),
        binding!(SelectProject, "secondary-alt-shift-1", Some("Veyra")),
        binding!(ToggleItem, "secondary-shift-i", Some("Veyra")),
        binding!(Retry, "secondary-r", Some("Veyra")),
        binding!(OpenTerminal, "secondary-alt-t", Some("Veyra")),
        binding!(NarrowSidebar, "secondary-alt-left", Some("Veyra")),
        binding!(WidenSidebar, "secondary-alt-right", Some("Veyra")),
        binding!(ResetSidebar, "secondary-alt-0", Some("Veyra")),
        binding!(DismissNotice, "escape", Some("Veyra")),
        binding!(AnswerQuestion, "1", Some("VeyraDock"), option: 0),
        binding!(AnswerQuestion, "2", Some("VeyraDock"), option: 1),
        binding!(AnswerQuestion, "3", Some("VeyraDock"), option: 2),
        binding!(AnswerQuestion, "4", Some("VeyraDock"), option: 3),
        binding!(AnswerQuestion, "5", Some("VeyraDock"), option: 4),
        binding!(AnswerQuestion, "6", Some("VeyraDock"), option: 5),
        binding!(AnswerQuestion, "7", Some("VeyraDock"), option: 6),
        binding!(AnswerQuestion, "8", Some("VeyraDock"), option: 7),
        binding!(AnswerQuestion, "9", Some("VeyraDock"), option: 8),
        binding!(ChooseAsyncOption, "secondary-1", Some("Veyra"), option: 0),
        binding!(ChooseAsyncOption, "secondary-2", Some("Veyra"), option: 1),
        binding!(ChooseAsyncOption, "secondary-3", Some("Veyra"), option: 2),
        binding!(ChooseAsyncOption, "secondary-4", Some("Veyra"), option: 3),
        binding!(ChooseAsyncOption, "secondary-5", Some("Veyra"), option: 4),
        binding!(ChooseAsyncOption, "secondary-6", Some("Veyra"), option: 5),
        binding!(ChooseAsyncOption, "secondary-7", Some("Veyra"), option: 6),
        binding!(ChooseAsyncOption, "secondary-8", Some("Veyra"), option: 7),
        binding!(ChooseAsyncOption, "secondary-9", Some("Veyra"), option: 8),
    ]
}

pub fn shortcut<A: Action + Default>() -> Option<String> {
    bindings()
        .iter()
        .find(|binding| (binding.build)().name() == A::default().name())
        .map(|binding| {
            binding
                .chord
                .replace("secondary-", "⌘")
                .replace("ctrl-", "⌃")
                .replace("alt-", "⌥")
                .replace("shift-", "⇧")
                .replace("⇧v", "⇧V")
                .replace("enter", "↵")
                .replace("backspace", "⌫")
                .replace("escape", "Esc")
        })
}
pub fn shortcut_chip<A: Action + Default>(cx: &App) -> impl IntoElement {
    let t = cx.global::<super::theme::Theme>();
    div()
        .debug_selector(|| A::default().name().into())
        .text_size(t.text.meta.size)
        .line_height(t.text.meta.line_height)
        .text_color(t.colors.text_muted)
        .pl(t.space.0[1])
        .pr(t.space.0[1])
        .rounded(t.radius.inline)
        .border(t.layout.border)
        .border_color(t.colors.border)
        .children(shortcut::<A>())
}
pub fn init(cx: &mut App) {
    cx.bind_keys(bindings().into_iter().map(|binding| {
        let key = Keystroke::parse(binding.chord).expect("valid window binding");
        let mut context = binding
            .context
            .map(|context| KeyBindingContextPredicate::parse(context).unwrap());
        if !key.modifiers.platform
            || (matches!(key.key.as_str(), "left" | "right")
                && !key.modifiers.shift
                && !key.modifiers.alt
                && !key.modifiers.control)
        {
            let outside_terminal = KeyBindingContextPredicate::Not(Box::new(
                KeyBindingContextPredicate::Identifier("VeyraTerminal".into()),
            ));
            context = Some(match context {
                Some(context) => {
                    KeyBindingContextPredicate::And(Box::new(context), Box::new(outside_terminal))
                }
                None => outside_terminal,
            });
        }
        KeyBinding::load(
            binding.chord,
            (binding.build)(),
            context.map(Into::into),
            false,
            None,
            &DummyKeyboardMapper,
        )
        .unwrap()
    }));
    cx.set_menus(vec![Menu {
        name: "Veyra".into(),
        disabled: false,
        items: vec![
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::separator(),
            MenuItem::action("Quit Veyra", Quit),
        ],
    }]);
}

#[cfg(test)]
mod tests {
    #[cfg(debug_assertions)]
    use super::ActivateWindow;
    use super::{AllowOnce, Deny, Stop, bindings, shortcut};
    use gpui_kit::Keystroke;
    #[test]
    fn shortcut_chips_match_bindings() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui");
        for entry in std::fs::read_dir(root).unwrap() {
            let source = std::fs::read_to_string(entry.unwrap().path()).unwrap();
            let source = source.split("#[cfg(test)]").next().unwrap();
            for call in source.split("shortcut_chip::<").skip(1) {
                let action = call.split('>').next().unwrap();
                let binding = bindings()
                    .into_iter()
                    .find(|binding| binding.action == action)
                    .unwrap_or_else(|| panic!("unbound rendered chip: {action}"));
                assert_eq!((binding.build)().name(), format!("veyra::{action}"));
                assert!(Keystroke::parse(binding.chord).is_ok());
            }
        }
        assert_eq!(shortcut::<AllowOnce>(), Some("⌘↵".into()));
        assert_eq!(shortcut::<Deny>(), Some("⌘.".into()));
        assert_eq!(shortcut::<Stop>(), Some("⌃c".into()));
        #[cfg(debug_assertions)]
        assert_eq!(shortcut::<ActivateWindow>(), None);
    }
}
