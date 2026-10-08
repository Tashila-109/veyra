use gpui_kit::*;

actions!(
    veyra,
    [
        Quit,
        AddProject,
        NewThread,
        PickProvider,
        PickModel,
        PickEffort,
        PickPermission,
        Send,
        NarrowSidebar,
        WidenSidebar,
        ResetSidebar,
        DismissNotice
    ]
);

#[derive(Clone, PartialEq, serde::Deserialize, ::gpui::private::schemars::JsonSchema, Action)]
#[schemars(crate = "::gpui::private::schemars")]
#[action(namespace = veyra)]
pub struct SetSidebarWidth {
    pub width: f32,
}

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("secondary-q", Quit, None),
        KeyBinding::new("secondary-shift-o", AddProject, Some("Veyra")),
        KeyBinding::new("secondary-n", NewThread, Some("Veyra")),
        KeyBinding::new("secondary-alt-p", PickProvider, Some("Veyra")),
        KeyBinding::new("secondary-alt-m", PickModel, Some("Veyra")),
        KeyBinding::new("secondary-alt-e", PickEffort, Some("Veyra")),
        KeyBinding::new("secondary-alt-a", PickPermission, Some("Veyra")),
        KeyBinding::new("secondary-enter", Send, Some("Veyra")),
        KeyBinding::new("secondary-alt-left", NarrowSidebar, Some("Veyra")),
        KeyBinding::new("secondary-alt-right", WidenSidebar, Some("Veyra")),
        KeyBinding::new("secondary-alt-0", ResetSidebar, Some("Veyra")),
        KeyBinding::new("escape", DismissNotice, Some("Veyra")),
    ]);
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.set_menus(vec![Menu {
        name: "Veyra".into(),
        disabled: false,
        items: vec![MenuItem::action("Quit Veyra", Quit)],
    }]);
}
