//! Shared semantic status presentation. Domain owners choose meaning; labels remain literal.
use super::{rpx, tokens::SPACE_SM};
use crate::{icons::icon, theme as c};
use gpui::{div, prelude::*, AnimationExt as _, AnyElement, App, Div, Hsla, SharedString};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusKind {
    Info,
    Working,
    Success,
    NeedsInput,
    Warning,
    Failed,
    Idle,
    Unknown,
    Pending,
    Canceled,
    Stopped,
    Running,
}
impl StatusKind {
    pub fn color(self) -> Hsla {
        match self {
            Self::Info | Self::Working | Self::Running => c::STATUS_INFO(),
            Self::Success => c::STATUS_SUCCESS(),
            Self::NeedsInput | Self::Warning => c::STATUS_WARNING(),
            Self::Failed => c::STATUS_ERROR(),
            _ => c::STATUS_NEUTRAL(),
        }
    }
    pub fn icon_name(self) -> &'static str {
        match self {
            Self::Info => "status-info",
            Self::Working | Self::Running => "status-working",
            Self::Success => "status-success",
            Self::NeedsInput => "status-attention",
            Self::Warning => "status-warning",
            Self::Failed => "status-failed",
            Self::Canceled => "status-canceled",
            Self::Stopped => "status-stopped",
            Self::Unknown => "status-unknown",
            Self::Idle => "status-idle",
            Self::Pending => "task-pending",
        }
    }
}
/// Explicit adapter for the existing session/child lifecycle vocabulary.
pub fn session_kind(label: &str) -> StatusKind {
    match label {
        "Working" | "Starting" => StatusKind::Working,
        "Running" => StatusKind::Running,
        "Done" => StatusKind::Success,
        "Needs you" | "Waiting" => StatusKind::NeedsInput,
        "Failed" | "Disconnected" => StatusKind::Failed,
        "Interrupted" | "Stopped" | "Exited" => StatusKind::Stopped,
        "Canceled" | "Cancelled" => StatusKind::Canceled,
        "Pending" => StatusKind::Pending,
        "Idle" => StatusKind::Idle,
        _ => StatusKind::Unknown,
    }
}
pub fn codex_kind(status: grove_core::codex::Status) -> StatusKind {
    use grove_core::codex::Status;
    match status {
        Status::Starting | Status::Working => StatusKind::Working,
        Status::Waiting => StatusKind::NeedsInput,
        Status::Done => StatusKind::Success,
        Status::Failed | Status::Disconnected => StatusKind::Failed,
        Status::Interrupted => StatusKind::Stopped,
        Status::Idle => StatusKind::Idle,
    }
}
/// Fixed-size icon slot; reduced motion never schedules animation.
pub fn mark(id: impl Into<SharedString>, kind: StatusKind, size: f32, cx: &App) -> AnyElement {
    let id = id.into();
    let selector = format!("status-mark-{id}");
    let slot = div()
        .id(SharedString::from(format!("session-spinner-icon-{id}")))
        .debug_selector(move || selector.clone())
        .size(rpx(size))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center();
    if matches!(kind, StatusKind::Working | StatusKind::Running) && !cx.reduce_motion() {
        slot.with_animation(
            SharedString::from(format!("session-spinner-{id}")),
            gpui::Animation::new(std::time::Duration::from_millis(1200)).repeat(),
            move |element, progress| {
                let frame =
                    (progress * crate::entities::animation_clock::SPINNER_FRAMES as f32) as u64;
                let id = id.clone();
                element
                    .debug_selector(move || format!("session-spinner-phase-{id}-{frame}"))
                    .child(
                        icon(kind.icon_name(), size, kind.color()).with_transformation(
                            gpui::Transformation::rotate(gpui::percentage(progress)),
                        ),
                    )
            },
        )
        .into_any_element()
    } else {
        slot.when(
            matches!(kind, StatusKind::Working | StatusKind::Running),
            |slot| slot.debug_selector(move || format!("session-spinner-phase-{id}-0")),
        )
        .child(icon(kind.icon_name(), size, kind.color()))
        .into_any_element()
    }
}
#[derive(gpui::IntoElement)]
struct FirstLineMark {
    mark: AnyElement,
    size: f32,
}

impl gpui::RenderOnce for FirstLineMark {
    fn render(self, window: &mut gpui::Window, _: &mut App) -> impl IntoElement {
        // Resolve the inherited line height during layout, so different text roles
        // and zoom levels center against the first line rather than the whole label.
        div()
            .w(rpx(self.size))
            .h(window
                .line_height()
                .max(rpx(self.size).to_pixels(window.rem_size())))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .child(self.mark)
    }
}

/// Plain semantic icon and matching text, with no badge background.
pub fn label(
    id: impl Into<SharedString>,
    kind: StatusKind,
    text: impl Into<SharedString>,
    size: f32,
    cx: &App,
) -> Div {
    let text = text.into();
    div()
        .flex()
        .items_start()
        .gap(rpx(SPACE_SM))
        .min_w_0()
        .text_color(kind.color())
        .child(FirstLineMark {
            mark: mark(id, kind, size, cx),
            size,
        })
        .child(div().min_w_0().child(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALIGNMENT_WIDTH: f32 = 180.;
    const ALIGNMENT_LINE_H: f32 = 22.;
    struct AlignmentProbe {
        rem: f32,
    }
    impl gpui::Render for AlignmentProbe {
        fn render(
            &mut self,
            window: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            window.set_rem_size(gpui::px(self.rem));
            div()
                .w(rpx(ALIGNMENT_WIDTH))
                .text_size(rpx(super::super::tokens::TEXT_BODY))
                .line_height(rpx(ALIGNMENT_LINE_H))
                .child(
                    label(
                        "alignment-probe",
                        StatusKind::Success,
                        "A diagnostic that wraps onto several lines and preserves its leading mark",
                        super::super::tokens::ICON_SM,
                        cx,
                    )
                    .debug_selector(|| "alignment-label".into()),
                )
        }
    }

    #[gpui::test]
    fn status_mark_centers_on_first_line_at_each_zoom(cx: &mut gpui::TestAppContext) {
        for rem in [crate::zoom::REM_BASE, 20.] {
            let (_, cx) = cx.add_window_view(|_, _| AlignmentProbe { rem });
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            let row = cx.debug_bounds("alignment-label").unwrap();
            let mark = cx.debug_bounds("status-mark-alignment-probe").unwrap();
            let scale = rem / crate::zoom::REM_BASE;
            assert!(
                f32::from(row.size.height) > ALIGNMENT_LINE_H * scale,
                "probe must exercise multiline text"
            );
            let expected = (ALIGNMENT_LINE_H - super::super::tokens::ICON_SM) * scale / 2.;
            assert!(
                (f32::from(mark.top() - row.top()) - expected).abs() <= 0.5,
                "mark must center on the first line within layout pixel rounding"
            );
            assert_eq!(f32::from(mark.size.width), 12. * scale);
        }
    }

    #[test]
    fn lifecycle_adapters_preserve_semantic_distinctions() {
        use grove_core::codex::Status;
        for (status, kind) in [
            (Status::Starting, StatusKind::Working),
            (Status::Working, StatusKind::Working),
            (Status::Waiting, StatusKind::NeedsInput),
            (Status::Done, StatusKind::Success),
            (Status::Failed, StatusKind::Failed),
            (Status::Disconnected, StatusKind::Failed),
            (Status::Interrupted, StatusKind::Stopped),
            (Status::Idle, StatusKind::Idle),
        ] {
            assert_eq!(codex_kind(status), kind);
        }
        assert_eq!(session_kind("Done").icon_name(), "status-success");
        assert_eq!(
            session_kind("unrecognized provider diagnostics"),
            StatusKind::Unknown
        );
        assert_ne!(
            StatusKind::Failed.icon_name(),
            StatusKind::Stopped.icon_name()
        );
        assert_ne!(
            StatusKind::Canceled.icon_name(),
            StatusKind::Stopped.icon_name()
        );
        assert_ne!(
            StatusKind::Unknown.icon_name(),
            StatusKind::Pending.icon_name()
        );
        assert_eq!(StatusKind::Success.color(), c::STATUS_SUCCESS());
        assert_eq!(StatusKind::Working.color(), c::STATUS_INFO());
    }
}

#[cfg(all(test, target_os = "macos"))]
mod native_snapshots {
    use super::*;
    use crate::views::tokens::{ICON_SM, SPACE_3XL, SPACE_LG, TEXT_SMALL};
    use gpui::{Context, Render, Window};
    const STATUS_GALLERY_HEIGHT: f32 = 960.;
    struct StatusGallery;
    impl Render for StatusGallery {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let mut panel = div()
                .w_full()
                .h_full()
                .p(rpx(SPACE_3XL))
                .flex()
                .flex_col()
                .gap(rpx(SPACE_LG))
                .bg(c::BG())
                .text_size(rpx(TEXT_SMALL))
                .font_family(crate::fonts::UI_FAMILY);
            for (index, (kind, text)) in [
                (StatusKind::Info, "Information"),
                (StatusKind::Working, "Working"), (StatusKind::Running, "Running"),
                (StatusKind::Success, "Done"), (StatusKind::NeedsInput, "Needs you"),
                (StatusKind::Warning, "Checks require attention"), (StatusKind::Failed, "Failed"),
                (StatusKind::Idle, "Idle"), (StatusKind::Unknown, "Unavailable"),
                (StatusKind::Pending, "Pending"), (StatusKind::Canceled, "Canceled"),
                (StatusKind::Stopped, "Stopped"),
                (StatusKind::Failed, "Literal provider diagnostic: the connection could not be restored. Your draft remains available.\nAdditional diagnostic details remain visible and wrap within this panel."),
            ].into_iter().enumerate() {
                panel = panel.child(label(format!("gallery-{index}"), kind, text, ICON_SM, cx));
            }
            // The action glyph must inherit both inverse rest and semantic
            // hover/focus foregrounds; actual interaction is checked in the app.
            for (text, fill, foreground) in [
                ("Delete worktree", c::RED(), c::BG()),
                ("Hover foreground", c::RED_WASH(), c::RED()),
                ("Focus foreground", c::RED_WASH(), c::RED()),
            ] {
                panel = panel.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(rpx(SPACE_SM))
                        .px(rpx(SPACE_LG))
                        .h(rpx(super::super::tokens::CONTROL_H))
                        .rounded(rpx(super::super::tokens::RADIUS_CONTROL))
                        .bg(fill)
                        .text_color(foreground)
                        .child(crate::icons::action_icon("trash", ICON_SM))
                        .child(text),
                );
            }
            panel
        }
    }
    #[test]
    #[ignore = "requires macOS Metal renderer; writes fixture screenshots to /tmp"]
    fn shared_status_native_snapshots() {
        let mut cx = gpui::HeadlessAppContext::with_platform(
            gpui_platform::current_platform(true).text_system(),
            std::sync::Arc::new(crate::assets::Assets),
            gpui_platform::current_headless_renderer,
        );
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::fonts::register(cx).unwrap();
            cx.set_reduce_motion(true);
        });
        for light in [false, true] {
            for rem in [16., 20.] {
                for width in [220., 340.] {
                    cx.update(|cx| {
                        grove_core::theme::set(
                            grove_core::theme::by_name(if light {
                                "grove-light"
                            } else {
                                "grove-dark"
                            })
                            .unwrap(),
                        );
                        c::sync_component_theme(cx);
                        gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
                    });
                    let window = cx
                        .open_window(
                            gpui::size(gpui::px(width), gpui::px(STATUS_GALLERY_HEIGHT)),
                            |window, cx| {
                                let gallery = cx.new(|_| StatusGallery);
                                cx.new(|cx| {
                                    gpui_component::Root::new(gallery, window, cx)
                                        .bordered(false)
                                        .bg(c::BG())
                                })
                            },
                        )
                        .unwrap();
                    cx.run_until_parked();
                    cx.update_window(window.into(), |_, window, cx| {
                        let _ = window.draw(cx);
                    })
                    .unwrap();
                    let path = format!(
                        "/tmp/grove-status-{}-{width:.0}-rem{rem:.0}.png",
                        if light { "light" } else { "dark" }
                    );
                    cx.capture_screenshot(window.into())
                        .unwrap()
                        .save(&path)
                        .unwrap();
                    eprintln!("Native shared status fixture: {path}");
                    cx.update_window(window.into(), |_, window, _| window.remove_window())
                        .unwrap();
                }
            }
        }
    }
}
