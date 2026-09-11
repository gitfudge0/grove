//! Transient theme state shared with terminal rendering; independent of a picker view.
use gpui::App;
use grove_core::theme::Theme;

#[derive(Clone, Default)]
pub struct ThemePreview {
    pub project: Option<(String, Option<Theme>)>,
    pub app: Option<Theme>,
}
impl gpui::Global for ThemePreview {}

impl ThemePreview {
    pub fn for_project(cx: &App, project_name: &str) -> Option<Option<Theme>> {
        let preview = cx.try_global::<Self>()?;
        let (name, theme) = preview.project.as_ref()?;
        (name == project_name).then(|| theme.clone())
    }

    pub fn set(cx: &mut App, preview: Self) {
        cx.set_global(preview);
        cx.refresh_windows();
    }

    pub fn clear(cx: &mut App) {
        Self::set(cx, Self::default());
    }
}
