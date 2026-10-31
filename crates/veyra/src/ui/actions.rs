use gpui_kit::*;
use veyra_engine::{
    domain::{ItemId, PermissionMode, ProjectId, RequestId, ThreadId, TurnId, WindowBackground},
    provider::ProviderKind,
};

#[cfg(debug_assertions)]
actions!(
    veyra,
    [
        ActivateWindow,
        DebugFocusShellDrawer,
        DebugDoubleClickSidebar,
        DebugDoubleClickShellDrawer
    ]
);

actions!(
    veyra,
    [
        Quit,
        OpenPalette,
        CopyNotice,
        ToggleShellDrawer,
        NewShellTab,
        CloseShellTab,
        NextShellTab,
        PreviousShellTab,
        ConfirmCloseShellTab,
        CancelCloseShellTab,
        RestartShellTab,
        PasteShell,
        CopyShell,
        ScrollShellHistoryUp,
        ScrollShellHistoryDown,
        ConfirmQuit,
        CancelQuit,
        Continue,
        DiscardQueued,
        NextNeedsYou,
        FocusDock,
        MoveOptionUp,
        MoveOptionDown,
        AnswerFocusedOption,
        FocusComposer,
        ScrollTranscriptUp,
        ScrollTranscriptDown,
        ScrollTranscriptEnd,
        NewThread,
        ActivateThreadRow,
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
choice_action!(PickProvider, provider, ProviderKind);
choice_action!(PickModel, model, String);
choice_action!(PickEffort, effort, String);
choice_action!(PickPermission, mode, PermissionMode);
choice_action!(SetDefaultMode, mode, PermissionMode);
choice_action!(SetWindowBackground, background, WindowBackground);
choice_action!(SelectThread, thread, ThreadId);
choice_action!(DismissToast, root, ThreadId);
choice_action!(ToggleCodeWrap, item, ItemId);
choice_action!(CopyReply, item, ItemId);
#[derive(
    Clone, Default, PartialEq, serde::Deserialize, ::gpui::private::schemars::JsonSchema, Action,
)]
#[serde(default)]
#[schemars(crate = "::gpui::private::schemars")]
#[action(namespace = veyra)]
pub struct CopyCodeBlock {
    #[schemars(with = "Option<String>")]
    pub item: Option<ItemId>,
    pub start: usize,
}
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
#[derive(
    Clone, Default, PartialEq, serde::Deserialize, ::gpui::private::schemars::JsonSchema, Action,
)]
#[serde(default)]
#[schemars(crate = "::gpui::private::schemars")]
#[action(namespace = veyra)]
pub struct DebugDisplayOptions {
    pub reduce_transparency: bool,
    pub increase_contrast: bool,
    pub reduce_motion: bool,
}

#[cfg(debug_assertions)]
#[derive(
    Clone, Default, PartialEq, serde::Deserialize, ::gpui::private::schemars::JsonSchema, Action,
)]
#[serde(default)]
#[schemars(crate = "::gpui::private::schemars")]
#[action(namespace = veyra)]
pub struct DebugComposerWidth {
    pub width: Option<f32>,
}

#[cfg(debug_assertions)]
#[derive(Clone, PartialEq, serde::Deserialize, ::gpui::private::schemars::JsonSchema, Action)]
#[schemars(crate = "::gpui::private::schemars")]
#[action(namespace = veyra)]
pub struct DebugAgeQuiet {
    #[schemars(with = "String")]
    pub thread: ThreadId,
    pub minutes: u64,
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
    pub label: Option<&'static str>,
    pub(super) build: fn() -> Box<dyn Action>,
}
macro_rules! binding {
    ($action:ident, $chord:literal, $context:expr, $field:ident: $value:expr) => {
        Binding {
            action: stringify!($action),
            label: palette_label(stringify!($action)),
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
            label: palette_label(stringify!($action)),
            chord: $chord,
            context: $context,
            build: || Box::new($action::default()),
        }
    };
}
pub fn bindings() -> Vec<Binding> {
    vec![
        binding!(Quit, "secondary-q", None),
        binding!(OpenPalette, "secondary-k", Some("Veyra")),
        binding!(CopyNotice, "", Some("Veyra")),
        binding!(CopyCodeBlock, "", Some("VeyraReply")),
        binding!(ToggleCodeWrap, "", Some("VeyraReply")),
        binding!(CopyReply, "", Some("VeyraReply")),
        binding!(DismissToast, "", Some("VeyraToast")),
        binding!(ToggleShellDrawer, "secondary-j", None),
        binding!(NewShellTab, "secondary-t", Some("VeyraTerminal")),
        binding!(CloseShellTab, "secondary-w", Some("VeyraTerminal")),
        binding!(NextShellTab, "secondary-shift-]", Some("VeyraTerminal")),
        binding!(PreviousShellTab, "secondary-shift-[", Some("VeyraTerminal")),
        binding!(
            ConfirmCloseShellTab,
            "secondary-enter",
            Some("VeyraShellClose")
        ),
        binding!(CancelCloseShellTab, "escape", Some("VeyraShellClose")),
        binding!(RestartShellTab, "enter", Some("VeyraTerminalEnded")),
        binding!(PasteShell, "secondary-v", Some("VeyraTerminal")),
        binding!(CopyShell, "secondary-c", Some("VeyraTerminal")),
        binding!(ScrollShellHistoryUp, "shift-pageup", Some("VeyraTerminal")),
        binding!(
            ScrollShellHistoryDown,
            "shift-pagedown",
            Some("VeyraTerminal")
        ),
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
        binding!(ActivateThreadRow, "enter", Some("VeyraSidebarRow")),
        binding!(ActivateThreadRow, "space", Some("VeyraSidebarRow")),
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
        binding!(MoveOptionUp, "up", Some("VeyraDock")),
        binding!(MoveOptionDown, "down", Some("VeyraDock")),
        binding!(AnswerFocusedOption, "enter", Some("VeyraDock")),
        binding!(ScrollTranscriptUp, "pageup", Some("Veyra")),
        binding!(ScrollTranscriptDown, "pagedown", Some("Veyra")),
        binding!(ScrollTranscriptEnd, "secondary-end", Some("Veyra")),
        binding!(RecheckProviders, "secondary-alt-r", Some("Veyra")),
        binding!(SetDefaultMode, "secondary-alt-d", Some("Veyra")),
        binding!(SetWindowBackground, "secondary-alt-b", Some("Veyra")),
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
        .find(|binding| {
            !binding.chord.is_empty() && (binding.build)().name() == A::default().name()
        })
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
        .text_size(t.text.micro.size)
        .line_height(t.text.micro.line_height)
        .pl(t.space.0[1])
        .pr(t.space.0[1])
        .rounded(t.radius.inline)
        .border(t.layout.border)
        .border_color(t.colors.border)
        .children(shortcut::<A>())
}
impl Binding {
    pub(super) fn predicate(&self) -> Option<KeyBindingContextPredicate> {
        let binding = self;
        let key = (!binding.chord.is_empty())
            .then(|| Keystroke::parse(binding.chord).expect("valid window binding"));
        let mut context = binding
            .context
            .map(|context| KeyBindingContextPredicate::parse(context).unwrap());
        if !matches!(
            binding.context,
            Some("VeyraTerminal" | "VeyraTerminalEnded")
        ) && key.as_ref().is_some_and(|key| {
            !key.modifiers.platform
                || (matches!(key.key.as_str(), "left" | "right")
                    && !key.modifiers.shift
                    && !key.modifiers.alt
                    && !key.modifiers.control)
        }) {
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
        context
    }
    pub(super) fn matches_context(&self, contexts: &[KeyContext]) -> bool {
        self.predicate().is_none_or(|predicate| {
            (1..=contexts.len()).any(|end| predicate.eval(&contexts[..end]))
        })
    }
}
fn palette_label(action: &str) -> Option<&'static str> {
    Some(match action {
        "Quit" => "Quit Veyra",
        "OpenPalette" => "Command Palette…",
        "CopyNotice" => "Copy notice",
        "CopyCodeBlock" => "Copy code block",
        "ToggleCodeWrap" => "Toggle code wrapping",
        "CopyReply" => "Copy final reply",
        "DismissToast" => "Dismiss attention toast",
        "ToggleShellDrawer" => "Toggle shell drawer",
        "NewShellTab" => "New shell tab",
        "CloseShellTab" => "Close shell tab",
        "NextShellTab" => "Next shell tab",
        "PreviousShellTab" => "Previous shell tab",
        "RestartShellTab" => "Restart shell tab",
        "PasteShell" => "Paste into shell",
        "CopyShell" => "Copy shell selection",
        "ScrollShellHistoryUp" => "Scroll shell history up",
        "ScrollShellHistoryDown" => "Scroll shell history down",
        "ConfirmCloseShellTab" => "Confirm close shell tab",
        "CancelCloseShellTab" => "Cancel close shell tab",
        "ConfirmQuit" => "Confirm quit",
        "CancelQuit" => "Cancel quit",
        "OpenSettings" => "Settings…",
        "AddProject" => "Add project…",
        "NewThread" => "New thread",
        "PickProvider" => "Next provider",
        "OpenProviderPicker" => "Choose provider…",
        "PickModel" => "Next model",
        "PickEffort" => "Next reasoning effort",
        "PickPermission" => "Next permission mode",
        "Send" => "Send message",
        "Stop" => "Stop turn",
        "Continue" => "Continue thread",
        "DiscardQueued" => "Discard queued messages",
        "ActivateThreadRow" => "Open focused thread",
        "OpenThreadMenu" => "Thread actions…",
        "Archive" => "Archive thread",
        "ArchiveKeepChildren" => "Archive, keep child threads",
        "StopThread" => "Stop thread",
        "NextNeedsYou" => "Next thread needing attention",
        "FocusDock" => "Focus request",
        "FocusComposer" => "Focus composer",
        "MoveOptionUp" => "Previous question option",
        "MoveOptionDown" => "Next question option",
        "ScrollTranscriptUp" => "Scroll transcript up",
        "ScrollTranscriptDown" => "Scroll transcript down",
        "ScrollTranscriptEnd" => "Latest reply",
        "RecheckProviders" => "Check providers again",
        "SetDefaultMode" => "Next default permission mode",
        "SetWindowBackground" => "Toggle window background",
        "SelectThread" => "Next thread",
        "SelectProject" => "Next project",
        "ToggleItem" => "Toggle tool or reasoning",
        "Retry" => "Retry last failed turn",
        "OpenTerminal" => "Open shell",
        "NarrowSidebar" => "Narrow sidebar",
        "WidenSidebar" => "Widen sidebar",
        "ResetSidebar" => "Reset sidebar width",
        "DismissNotice" => "Dismiss notice",
        _ => return None,
    })
}
pub(super) fn answers_request(action: &str) -> bool {
    matches!(
        action,
        "AllowOnce"
            | "AllowInThread"
            | "Deny"
            | "AnswerQuestion"
            | "ChooseAsyncOption"
            | "AnswerFocusedOption"
    )
}

pub fn init(cx: &mut App) {
    cx.bind_keys(
        bindings()
            .into_iter()
            .filter(|binding| !binding.chord.is_empty())
            .map(|binding| {
                let context = binding.predicate();
                KeyBinding::load(
                    binding.chord,
                    (binding.build)(),
                    context.map(Into::into),
                    false,
                    None,
                    &DummyKeyboardMapper,
                )
                .unwrap()
            }),
    );
    cx.set_menus(vec![Menu {
        name: "Veyra".into(),
        disabled: false,
        items: vec![
            MenuItem::action("New Thread", NewThread),
            MenuItem::action("Add Project…", AddProject::default()),
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::action("Command Palette…", OpenPalette),
            MenuItem::action("Toggle Window Background", SetWindowBackground::default()),
            MenuItem::separator(),
            MenuItem::action("Quit Veyra", Quit),
        ],
    }]);
}

#[cfg(debug_assertions)]
#[derive(Clone, PartialEq, serde::Deserialize, ::gpui::private::schemars::JsonSchema, Action)]
#[schemars(crate = "::gpui::private::schemars")]
#[action(namespace = veyra)]
pub struct DebugDragShellDrawer {
    pub pixels: f32,
}

#[cfg(debug_assertions)]
#[derive(Clone, PartialEq, serde::Deserialize, ::gpui::private::schemars::JsonSchema, Action)]
#[schemars(crate = "::gpui::private::schemars")]
#[action(namespace = veyra)]
pub struct DebugDragSidebar {
    pub pixels: f32,
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
