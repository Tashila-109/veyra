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

#[derive(Clone, Copy, PartialEq)]
pub(super) enum PaletteEligibility {
    Command,
    RequestAnswer,
}
pub struct Binding {
    pub action: &'static str,
    pub chord: &'static str,
    pub context: Option<&'static str>,
    pub label: &'static str,
    pub(super) palette: PaletteEligibility,
    pub(super) build: fn() -> Box<dyn Action>,
}
macro_rules! binding {
    ($action:ident, $label:literal, $palette:ident, $chord:literal, $context:expr, $field:ident: $value:expr) => {
        Binding {
            action: stringify!($action),
            label: $label,
            palette: PaletteEligibility::$palette,
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
    ($action:ident, $label:literal, $palette:ident, $chord:literal, $context:expr) => {
        Binding {
            action: stringify!($action),
            label: $label,
            palette: PaletteEligibility::$palette,
            chord: $chord,
            context: $context,
            build: || Box::new($action::default()),
        }
    };
}
pub fn bindings() -> Vec<Binding> {
    vec![
        binding!(Quit, "Quit Veyra", Command, "secondary-q", None),
        binding!(
            OpenPalette,
            "Command Palette…",
            Command,
            "secondary-k",
            Some("Veyra")
        ),
        binding!(CopyNotice, "Copy notice", Command, "", Some("Veyra")),
        binding!(
            CopyCodeBlock,
            "Copy code block",
            Command,
            "",
            Some("VeyraReply")
        ),
        binding!(
            ToggleCodeWrap,
            "Toggle code wrapping",
            Command,
            "",
            Some("VeyraReply")
        ),
        binding!(
            CopyReply,
            "Copy final reply",
            Command,
            "",
            Some("VeyraReply")
        ),
        binding!(
            DismissToast,
            "Dismiss attention toast",
            Command,
            "",
            Some("VeyraToast")
        ),
        binding!(
            ToggleShellDrawer,
            "Toggle shell drawer",
            Command,
            "secondary-j",
            None
        ),
        binding!(
            NewShellTab,
            "New shell tab",
            Command,
            "secondary-t",
            Some("VeyraTerminal")
        ),
        binding!(
            CloseShellTab,
            "Close shell tab",
            Command,
            "secondary-w",
            Some("VeyraTerminal")
        ),
        binding!(
            NextShellTab,
            "Next shell tab",
            Command,
            "secondary-shift-]",
            Some("VeyraTerminal")
        ),
        binding!(
            PreviousShellTab,
            "Previous shell tab",
            Command,
            "secondary-shift-[",
            Some("VeyraTerminal")
        ),
        binding!(
            ConfirmCloseShellTab,
            "Confirm close shell tab",
            Command,
            "secondary-enter",
            Some("VeyraShellClose")
        ),
        binding!(
            CancelCloseShellTab,
            "Cancel close shell tab",
            Command,
            "escape",
            Some("VeyraShellClose")
        ),
        binding!(
            RestartShellTab,
            "Restart shell tab",
            Command,
            "enter",
            Some("VeyraTerminalEnded")
        ),
        binding!(
            PasteShell,
            "Paste into shell",
            Command,
            "secondary-v",
            Some("VeyraTerminal")
        ),
        binding!(
            CopyShell,
            "Copy shell selection",
            Command,
            "secondary-c",
            Some("VeyraTerminal")
        ),
        binding!(
            ScrollShellHistoryUp,
            "Scroll shell history up",
            Command,
            "shift-pageup",
            Some("VeyraTerminal")
        ),
        binding!(
            ScrollShellHistoryDown,
            "Scroll shell history down",
            Command,
            "shift-pagedown",
            Some("VeyraTerminal")
        ),
        binding!(
            ConfirmQuit,
            "Confirm quit",
            Command,
            "secondary-enter",
            Some("VeyraQuit")
        ),
        binding!(
            CancelQuit,
            "Cancel quit",
            Command,
            "escape",
            Some("VeyraQuit")
        ),
        binding!(
            OpenSettings,
            "Settings…",
            Command,
            "secondary-,",
            Some("Veyra")
        ),
        binding!(
            AddProject,
            "Add project…",
            Command,
            "secondary-shift-o",
            Some("Veyra")
        ),
        binding!(
            NewThread,
            "New thread",
            Command,
            "secondary-n",
            Some("Veyra")
        ),
        binding!(
            PickProvider,
            "Next provider",
            Command,
            "secondary-alt-p",
            Some("Veyra")
        ),
        binding!(
            OpenProviderPicker,
            "Choose provider…",
            Command,
            "secondary-alt-shift-p",
            Some("Veyra")
        ),
        binding!(
            PickModel,
            "Next model",
            Command,
            "secondary-alt-m",
            Some("Veyra")
        ),
        binding!(
            PickEffort,
            "Next reasoning effort",
            Command,
            "secondary-alt-e",
            Some("Veyra")
        ),
        binding!(
            PickPermission,
            "Next permission mode",
            Command,
            "secondary-alt-a",
            Some("Veyra")
        ),
        binding!(
            Send,
            "Send message",
            Command,
            "secondary-enter",
            Some("Veyra")
        ),
        binding!(Stop, "Stop turn", Command, "ctrl-c", Some("VeyraComposer")),
        binding!(
            Continue,
            "Continue thread",
            Command,
            "secondary-shift-enter",
            Some("Veyra")
        ),
        binding!(
            DiscardQueued,
            "Discard queued messages",
            Command,
            "secondary-shift-backspace",
            Some("Veyra")
        ),
        binding!(
            ActivateThreadRow,
            "Open focused thread",
            Command,
            "enter",
            Some("VeyraSidebarRow")
        ),
        binding!(
            ActivateThreadRow,
            "Open focused thread",
            Command,
            "space",
            Some("VeyraSidebarRow")
        ),
        binding!(
            OpenThreadMenu,
            "Thread actions…",
            Command,
            "secondary-alt-shift-m",
            Some("Veyra")
        ),
        binding!(
            Archive,
            "Archive thread",
            Command,
            "secondary-shift-a",
            Some("Veyra")
        ),
        binding!(
            ArchiveKeepChildren,
            "Archive, keep child threads",
            Command,
            "secondary-alt-shift-a",
            Some("Veyra")
        ),
        binding!(
            StopThread,
            "Stop thread",
            Command,
            "secondary-alt-shift-s",
            Some("Veyra")
        ),
        binding!(
            NextNeedsYou,
            "Next thread needing attention",
            Command,
            "secondary-shift-j",
            Some("Veyra")
        ),
        binding!(
            AllowOnce,
            "Allow once",
            RequestAnswer,
            "secondary-enter",
            Some("VeyraApproval")
        ),
        binding!(
            AllowInThread,
            "Allow in this thread",
            RequestAnswer,
            "secondary-alt-enter",
            Some("VeyraApproval")
        ),
        binding!(
            Deny,
            "Deny",
            RequestAnswer,
            "secondary-.",
            Some("VeyraApproval")
        ),
        binding!(
            FocusDock,
            "Focus request",
            Command,
            "secondary-shift-y",
            Some("Veyra")
        ),
        binding!(
            FocusComposer,
            "Focus composer",
            Command,
            "escape",
            Some("VeyraDock")
        ),
        binding!(
            MoveOptionUp,
            "Previous question option",
            Command,
            "up",
            Some("VeyraDock")
        ),
        binding!(
            MoveOptionDown,
            "Next question option",
            Command,
            "down",
            Some("VeyraDock")
        ),
        binding!(
            AnswerFocusedOption,
            "Answer focused option",
            RequestAnswer,
            "enter",
            Some("VeyraDock")
        ),
        binding!(
            ScrollTranscriptUp,
            "Scroll transcript up",
            Command,
            "pageup",
            Some("Veyra")
        ),
        binding!(
            ScrollTranscriptDown,
            "Scroll transcript down",
            Command,
            "pagedown",
            Some("Veyra")
        ),
        binding!(
            ScrollTranscriptEnd,
            "Latest reply",
            Command,
            "secondary-end",
            Some("Veyra")
        ),
        binding!(
            RecheckProviders,
            "Check providers again",
            Command,
            "secondary-alt-r",
            Some("Veyra")
        ),
        binding!(
            SetDefaultMode,
            "Next default permission mode",
            Command,
            "secondary-alt-d",
            Some("Veyra")
        ),
        binding!(
            SetWindowBackground,
            "Toggle window background",
            Command,
            "secondary-alt-b",
            Some("Veyra")
        ),
        binding!(
            SelectThread,
            "Next thread",
            Command,
            "secondary-alt-1",
            Some("Veyra")
        ),
        binding!(
            SelectProject,
            "Next project",
            Command,
            "secondary-alt-shift-1",
            Some("Veyra")
        ),
        binding!(
            ToggleItem,
            "Toggle tool or reasoning",
            Command,
            "secondary-shift-i",
            Some("Veyra")
        ),
        binding!(
            Retry,
            "Retry last failed turn",
            Command,
            "secondary-r",
            Some("Veyra")
        ),
        binding!(
            OpenTerminal,
            "Open shell",
            Command,
            "secondary-alt-t",
            Some("Veyra")
        ),
        binding!(
            NarrowSidebar,
            "Narrow sidebar",
            Command,
            "secondary-alt-left",
            Some("Veyra")
        ),
        binding!(
            WidenSidebar,
            "Widen sidebar",
            Command,
            "secondary-alt-right",
            Some("Veyra")
        ),
        binding!(
            ResetSidebar,
            "Reset sidebar width",
            Command,
            "secondary-alt-0",
            Some("Veyra")
        ),
        binding!(
            DismissNotice,
            "Dismiss notice",
            Command,
            "escape",
            Some("Veyra")
        ),
        binding!(AnswerQuestion, "Answer question", RequestAnswer, "1", Some("VeyraDock"), option: 0),
        binding!(AnswerQuestion, "Answer question", RequestAnswer, "2", Some("VeyraDock"), option: 1),
        binding!(AnswerQuestion, "Answer question", RequestAnswer, "3", Some("VeyraDock"), option: 2),
        binding!(AnswerQuestion, "Answer question", RequestAnswer, "4", Some("VeyraDock"), option: 3),
        binding!(AnswerQuestion, "Answer question", RequestAnswer, "5", Some("VeyraDock"), option: 4),
        binding!(AnswerQuestion, "Answer question", RequestAnswer, "6", Some("VeyraDock"), option: 5),
        binding!(AnswerQuestion, "Answer question", RequestAnswer, "7", Some("VeyraDock"), option: 6),
        binding!(AnswerQuestion, "Answer question", RequestAnswer, "8", Some("VeyraDock"), option: 7),
        binding!(AnswerQuestion, "Answer question", RequestAnswer, "9", Some("VeyraDock"), option: 8),
        binding!(ChooseAsyncOption, "Answer async question", RequestAnswer, "secondary-1", Some("Veyra"), option: 0),
        binding!(ChooseAsyncOption, "Answer async question", RequestAnswer, "secondary-2", Some("Veyra"), option: 1),
        binding!(ChooseAsyncOption, "Answer async question", RequestAnswer, "secondary-3", Some("Veyra"), option: 2),
        binding!(ChooseAsyncOption, "Answer async question", RequestAnswer, "secondary-4", Some("Veyra"), option: 3),
        binding!(ChooseAsyncOption, "Answer async question", RequestAnswer, "secondary-5", Some("Veyra"), option: 4),
        binding!(ChooseAsyncOption, "Answer async question", RequestAnswer, "secondary-6", Some("Veyra"), option: 5),
        binding!(ChooseAsyncOption, "Answer async question", RequestAnswer, "secondary-7", Some("Veyra"), option: 6),
        binding!(ChooseAsyncOption, "Answer async question", RequestAnswer, "secondary-8", Some("Veyra"), option: 7),
        binding!(ChooseAsyncOption, "Answer async question", RequestAnswer, "secondary-9", Some("Veyra"), option: 8),
    ]
}

pub(super) fn format_chord(chord: &str) -> Option<String> {
    (!chord.is_empty()).then(|| {
        chord
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
pub fn shortcut<A: Action + Default>() -> Option<String> {
    bindings()
        .iter()
        .find(|binding| {
            !binding.chord.is_empty() && (binding.build)().name() == A::default().name()
        })
        .and_then(|binding| format_chord(binding.chord))
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
    pub(super) fn palette_label(&self) -> Option<&'static str> {
        match self.palette {
            PaletteEligibility::Command => Some(self.label),
            PaletteEligibility::RequestAnswer => None,
        }
    }
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
    fn palette_metadata_is_explicit_for_every_binding() {
        let bindings = bindings();
        assert!(bindings.iter().all(|binding| !binding.label.is_empty()));
        let answers: Vec<_> = bindings
            .iter()
            .filter(|binding| binding.palette == super::PaletteEligibility::RequestAnswer)
            .collect();
        assert!(!answers.is_empty());
        assert!(
            answers
                .iter()
                .all(|binding| binding.palette_label().is_none())
        );
    }
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
