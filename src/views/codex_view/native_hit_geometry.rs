/// Exercise the optimized geometry against the original per-character oracle
/// using actual CoreText shaping rather than GPUI's NoopTextSystem.
#[test]
#[cfg(target_os = "macos")]
#[ignore = "requires macOS native text system"]
fn native_text_hit_bounds_match_reference_with_unicode() {
    use gpui::AppContext as _;
    struct Empty;
    impl Render for Empty {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }
    let mut cx = gpui::HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        Arc::new(crate::assets::Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx).unwrap();
    });
    let window = cx
        .open_window(gpui::size(gpui::px(500.), gpui::px(800.)), |window, cx| {
            let child = cx.new(|_| Empty);
            cx.new(|cx| gpui_component::Root::new(child, window, cx).bordered(false))
        })
        .unwrap();
    let text = "naïve café 界🙂 e\u{301} 👩‍👩‍👧‍👦 office ffi\tשלום مرحبا words wrap safely here "
        .repeat(10)
        + "\n\nlast naïve line\n\t    indentation\n";
    let mut checked = 0;
    for family in [crate::fonts::UI_FAMILY, crate::fonts::MONO_FAMILY] {
        for width in [90., 170., 420.] {
            cx.update_window(window.into(), |_, window, cx| {
                let styled = gpui::StyledText::new(text.clone());
                let layout = styled.layout().clone();
                let mut element = div()
                    .w(gpui::px(width))
                    .font_family(family)
                    .text_size(gpui::px(14.))
                    .child(styled)
                    .into_any_element();
                element.layout_as_root(
                    gpui::size(
                        gpui::AvailableSpace::Definite(gpui::px(width)),
                        gpui::AvailableSpace::Definite(gpui::px(100_000.)),
                    ),
                    window,
                    cx,
                );
                element.prepaint_at(gpui::point(gpui::px(0.), gpui::px(0.)), window, cx);
                let line_height = layout.line_height();
                for row in [0., 1., 2.5, 7., 12.5, 20., 35., 60., 200.] {
                    for x in [0., 25.] {
                        let mask = gpui::Bounds::new(
                            gpui::point(gpui::px(x), line_height * row),
                            gpui::size(gpui::px(110.), line_height * 2.25),
                        );
                        gpui_component::text::verify_text_hit_bounds_against_reference(
                            &text, &layout, mask,
                        )
                        .unwrap_or_else(|error| {
                            panic!("{family} width={width} row={row} x={x}: {error}")
                        });
                        checked += 1;
                    }
                }
            })
            .unwrap();
        }
    }
    assert_eq!(checked, 108);
    cx.update_window(window.into(), |_, window, _| window.remove_window())
        .unwrap();
}
