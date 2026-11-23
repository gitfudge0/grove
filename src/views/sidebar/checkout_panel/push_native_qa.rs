use super::*;
use gpui::AppContext as _;

struct PushProbe(Instant);
impl gpui::Render for PushProbe {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(c::BG())
            .font_family(crate::fonts::UI_FAMILY)
            .child(
                project_field_well(false, false)
                    .w(rpx(CARD_WIDTH))
                    .h_auto()
                    .child(checkout_row(
                        "push-qa-branch",
                        "feat/new-sidebar-with-a-long-branch-name",
                        "git-branch",
                        window,
                        cx,
                    ))
                    .child(
                        checkout_loading_row(
                            "push-qa-busy",
                            "Pushing…",
                            "git-push",
                            true,
                            window,
                            cx,
                        )
                        .disabled(true),
                    )
                    .child(
                        checkout_cycle_row(
                            "action-qa-confirmed",
                            "",
                            "status-success",
                            false,
                            Some(super::super::action_feedback::Feedback {
                                token: 1,
                                label: "Tracked edits discarded".into(),
                                completed: Some(self.0),
                                collapse: false,
                            }),
                            window,
                            cx,
                        )
                        .disabled(true),
                    )
                    .child(checkout_row(
                        "push-qa-changes",
                        "Changes",
                        "unified",
                        window,
                        cx,
                    )),
            )
    }
}

#[test]
#[ignore = "opens an isolated native X11 push-animation window"]
fn push_animation_linux_capture() {
    gpui_platform::application()
        .with_assets(crate::assets::Assets)
        .run(|cx| {
            gpui_component::init(cx);
            crate::fonts::register(cx).unwrap();
            let mut probe_handle = None;
            let window = cx
                .open_window(
                    gpui::WindowOptions {
                        window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                            gpui::point(gpui::px(40.), gpui::px(40.)),
                            gpui::size(gpui::px(300.), gpui::px(220.)),
                        ))),
                        ..Default::default()
                    },
                    |window, cx| {
                        window.set_window_title("Grove push animation QA");
                        let probe = cx.new(|_| PushProbe(Instant::now()));
                        probe_handle = Some(probe.clone());
                        cx.new(|cx| gpui_component::Root::new(probe, window, cx))
                    },
                )
                .unwrap();
            let probe = probe_handle.unwrap();
            cx.activate(true);
            cx.spawn(async move |cx| {
                for (theme, rem, reduced) in [
                    ("grove-dark", 16., false),
                    ("grove-light", 16., false),
                    ("grove-dark", 20., true),
                ] {
                    cx.update(|cx| {
                        grove_core::theme::set_by_name(theme);
                        c::sync_component_theme(cx);
                        cx.set_reduce_motion(reduced);
                        gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
                        probe.update(cx, |probe, cx| { probe.0 = Instant::now(); cx.notify(); });
                    });
                    cx.update_window(window.into(), |_, window, _| {
                        window.set_rem_size(gpui::px(rem));
                        window.refresh();
                    })
                    .unwrap();
                    for frame in ["early", "later"] {
                        cx.background_executor().timer(Duration::from_millis(120)).await;
                        let output = format!("{}/.grove-artifacts/side-panel-implementation/native-{theme}-{rem}-{reduced}-{frame}.png", env!("CARGO_MANIFEST_DIR"));
                        fs_err::create_dir_all(std::path::Path::new(&output).parent().unwrap()).unwrap();
                        assert!(std::process::Command::new("import").args(["-window", "Grove push animation QA", &output]).status().unwrap().success());
                    }
                    cx.background_executor().timer(Duration::from_millis(450)).await;
                    let output = format!(
                        "{}/.grove-artifacts/push-motion/native-{theme}-{rem}-{reduced}.png",
                        env!("CARGO_MANIFEST_DIR")
                    );
                    fs_err::create_dir_all(std::path::Path::new(&output).parent().unwrap())
                        .unwrap();
                    assert!(std::process::Command::new("import")
                        .args(["-window", "Grove push animation QA", &output])
                        .status()
                        .unwrap()
                        .success());
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
}
