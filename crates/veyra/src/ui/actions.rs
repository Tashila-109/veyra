use gpui_kit::*;

actions!(
    veyra,
    [
        Quit,
        NewThread,
        Send,
        OpenSettings,
        RecheckProviders,
        Retry,
        OpenTerminal,
        NarrowSidebar,
        WidenSidebar,
        ResetSidebar,
        DismissNotice
    ]
);

macro_rules! choice_action {
    ($name:ident, $field:ident) => {
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
            pub $field: Option<String>,
        }
    };
}
choice_action!(AddProject, path);
choice_action!(PickProvider, provider);
choice_action!(PickModel, model);
choice_action!(PickEffort, effort);
choice_action!(PickPermission, mode);
choice_action!(SetDefaultMode, mode);
choice_action!(SelectThread, thread);
choice_action!(SelectProject, project);
choice_action!(ToggleItem, item);

#[derive(Clone, PartialEq, serde::Deserialize, ::gpui::private::schemars::JsonSchema, Action)]
#[schemars(crate = "::gpui::private::schemars")]
#[action(namespace = veyra)]
pub struct SetSidebarWidth {
    pub width: f32,
}

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("secondary-q", Quit, None),
        KeyBinding::new("secondary-,", OpenSettings, Some("Veyra")),
        KeyBinding::new("secondary-shift-o", AddProject::default(), Some("Veyra")),
        KeyBinding::new("secondary-n", NewThread, Some("Veyra")),
        KeyBinding::new("secondary-alt-p", PickProvider::default(), Some("Veyra")),
        KeyBinding::new("secondary-alt-m", PickModel::default(), Some("Veyra")),
        KeyBinding::new("secondary-alt-e", PickEffort::default(), Some("Veyra")),
        KeyBinding::new("secondary-alt-a", PickPermission::default(), Some("Veyra")),
        KeyBinding::new("secondary-enter", Send, Some("Veyra")),
        KeyBinding::new("secondary-alt-r", RecheckProviders, Some("Veyra")),
        KeyBinding::new("secondary-alt-d", SetDefaultMode::default(), Some("Veyra")),
        KeyBinding::new("secondary-alt-1", SelectThread::default(), Some("Veyra")),
        KeyBinding::new(
            "secondary-alt-shift-1",
            SelectProject::default(),
            Some("Veyra"),
        ),
        KeyBinding::new("secondary-shift-i", ToggleItem::default(), Some("Veyra")),
        KeyBinding::new("secondary-r", Retry, Some("Veyra")),
        KeyBinding::new("secondary-alt-t", OpenTerminal, Some("Veyra")),
        KeyBinding::new("secondary-alt-left", NarrowSidebar, Some("Veyra")),
        KeyBinding::new("secondary-alt-right", WidenSidebar, Some("Veyra")),
        KeyBinding::new("secondary-alt-0", ResetSidebar, Some("Veyra")),
        KeyBinding::new("escape", DismissNotice, Some("Veyra")),
    ]);
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
