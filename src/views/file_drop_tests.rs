//! Exercise platform file-drop dispatch through the actual shared input element.
use super::{components, rpx, tokens::*};
use gpui::{
    div, point, prelude::*, px, Context, Entity, ExternalPaths, FileDropEvent, IntoElement, Pixels,
    Point, Render, TestAppContext, VisualTestContext, Window,
};
use gpui_component::input::{Input, InputState};

struct DropFixture {
    input: Entity<InputState>,
    disabled: bool,
}
impl Render for DropFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(rpx(MODAL_W_MD)).h(rpx(TEXTAREA_H)).child(
            Input::new(&self.input)
                .file_drop_overlay(components::file_drop_overlay())
                .aria_label("File drop test")
                .appearance(false)
                .bordered(false)
                .focus_bordered(false)
                .disabled(self.disabled)
                .p_0(),
        )
    }
}
fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
}
fn drop_at(
    cx: &mut VisualTestContext,
    paths: Vec<std::path::PathBuf>,
    position: Point<Pixels>,
    enabled: bool,
) {
    assert!(cx.debug_bounds("input-file-drop-label").is_none());
    cx.simulate_event(FileDropEvent::Entered {
        position,
        paths: ExternalPaths(paths.into()),
    });
    draw(cx);
    cx.simulate_event(FileDropEvent::Pending { position });
    draw(cx);
    // The child is painted only when the overlay resolves to visible.
    assert_eq!(cx.debug_bounds("input-file-drop-label").is_some(), enabled);
    cx.simulate_event(FileDropEvent::Submit { position });
    draw(cx);
}
#[gpui::test]
fn file_drop_plain_inputs_keep_selection_text_support_undo_and_reject_disabled(
    cx: &mut TestAppContext,
) {
    for multiline in [false, true] {
        cx.update(gpui_component::init);
        let mut fixture = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .multi_line(multiline)
                    .default_value("Keep me")
            });
            let view = cx.new(|_| DropFixture {
                input,
                disabled: false,
            });
            fixture = Some(view.clone());
            gpui_component::Root::new(view, window, cx).bordered(false)
        });
        let view = fixture.unwrap();
        draw(cx);
        cx.update(|_, cx| {
            view.read(cx)
                .input
                .clone()
                .update(cx, |input, cx| input.set_selected_range(0..7, cx))
        });
        cx.simulate_keystrokes("left");
        draw(cx);
        let position = point(px(50.), px(10.));
        drop_at(
            cx,
            vec!["/tmp/a.txt".into(), "/tmp/long name.txt".into()],
            position,
            true,
        );
        let actual = view.read_with(cx, |view, cx| view.input.read(cx).value().to_string());
        assert!(actual.contains("Keep me"), "{actual}");
        assert!(
            actual.contains(if multiline {
                "/tmp/a.txt\n/tmp/long name.txt"
            } else {
                "/tmp/a.txt /tmp/long name.txt"
            }),
            "{actual}"
        );
        cx.update(|window, cx| window.dispatch_action(Box::new(gpui_component::input::Undo), cx));
        draw(cx);
        assert_eq!(
            view.read_with(cx, |view, cx| view.input.read(cx).value().to_string()),
            "Keep me"
        );
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.disabled = true;
                cx.notify()
            })
        });
        draw(cx);
        drop_at(cx, vec!["/tmp/rejected.txt".into()], position, false);
        assert_eq!(
            view.read_with(cx, |view, cx| view.input.read(cx).value().to_string()),
            "Keep me"
        );
    }
}

/// Isolated real Linux window, bundled fonts and renderer; never loads user config.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "opens an isolated X11 QA window; set GROVE_FILE_DROP_CAPTURE_OUTPUT"]
fn file_drop_native_capture() {
    use std::time::Duration;
    let output = std::path::PathBuf::from(std::env::var("GROVE_FILE_DROP_CAPTURE_OUTPUT").unwrap());
    fs_err::create_dir_all(&output).unwrap();
    gpui_platform::application()
        .with_assets(crate::assets::Assets)
        .run(move |cx| {
            gpui_component::init(cx);
            crate::fonts::register(cx).unwrap();
            let window = cx
                .open_window(
                    gpui::WindowOptions {
                        window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                            point(px(40.), px(40.)),
                            gpui::size(px(560.), px(200.)),
                        ))),
                        ..Default::default()
                    },
                    |window, cx| {
                        window.set_window_title("Grove file drop QA");
                        let input = cx.new(|cx| {
                            InputState::new(window, cx)
                                .multi_line(true)
                                .default_value("Existing draft stays intact")
                        });
                        let fixture = cx.new(|_| DropFixture {
                            input,
                            disabled: false,
                        });
                        cx.new(|cx| gpui_component::Root::new(fixture, window, cx))
                    },
                )
                .unwrap();
            cx.activate(true);
            cx.spawn(async move |cx| {
                for (theme, rem) in [
                    ("grove-dark", 16.),
                    ("grove-light", 16.),
                    ("grove-dark", 24.),
                ] {
                    cx.update(|cx| {
                        grove_core::theme::set_by_name(theme);
                        crate::theme::sync_component_theme(cx);
                        gpui_component::Theme::global_mut(cx).font_size = px(rem);
                    });
                    let mut rest = None;
                    for (state, dragging) in [("rest", false), ("drag", true), ("exit", false)] {
                        cx.update_window(window.into(), |_, window, cx| {
                            window.set_rem_size(px(rem));
                            let event = if dragging {
                                FileDropEvent::Entered {
                                    position: point(px(50.), px(5.)),
                                    paths: ExternalPaths(
                                        vec!["/tmp/design notes.md".into()].into(),
                                    ),
                                }
                            } else {
                                FileDropEvent::Exited
                            };
                            window.dispatch_event(gpui::PlatformInput::FileDrop(event), cx);
                            window.refresh();
                        })
                        .unwrap();
                        cx.background_executor()
                            .timer(Duration::from_millis(200))
                            .await;
                        if dragging {
                            cx.update_window(window.into(), |_, window, cx| {
                                window.dispatch_event(
                                    gpui::PlatformInput::FileDrop(FileDropEvent::Pending {
                                        position: point(px(50.), px(5.)),
                                    }),
                                    cx,
                                );
                                window.refresh();
                            })
                            .unwrap();
                            cx.background_executor()
                                .timer(Duration::from_millis(120))
                                .await;
                        }
                        let target = output.join(format!("{theme}-{rem}-{state}.png"));
                        assert!(std::process::Command::new("import")
                            .args(["-window", "Grove file drop QA"])
                            .arg(&target)
                            .status()
                            .unwrap()
                            .success());
                        let frame = image::open(&target).unwrap().to_rgba8();
                        if state == "rest" {
                            rest = Some(frame);
                        } else if dragging {
                            assert_ne!(
                                rest.as_ref().unwrap(),
                                &frame,
                                "drop feedback must be painted"
                            );
                        } else {
                            assert_eq!(
                                rest.as_ref().unwrap(),
                                &frame,
                                "drag exit must restore the draft"
                            );
                        }
                    }
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
}
