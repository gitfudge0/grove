use gpui::{div, prelude::*, px, rems, Context, Render, TestAppContext, VisualTestContext, Window};
use gpui_component::text::TextView;

struct WrappingTableRoot {
    markdown: String,
    rem_size: f32,
}

impl Render for WrappingTableRoot {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        window.set_rem_size(px(self.rem_size));
        div()
            .debug_selector(|| "wrapping-table-viewport".into())
            .w(px(320.))
            .text_size(rems(0.875))
            .child(TextView::markdown("wrapping-table", self.markdown.clone()))
    }
}

#[gpui::test]
fn wrapping_markdown_tables_keep_last_column_reachable_at_narrow_width_and_zoom(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_component::init);
    for rem_size in [16., 24.] {
        for column_count in [2, 10] {
            let header = (0..column_count)
                .map(|ix| format!("Column {ix}"))
                .collect::<Vec<_>>()
                .join(" | ");
            let separator = vec!["---"; column_count].join(" | ");
            let cells = vec!["ordinary words wrap inside their column"; column_count].join(" | ");
            let markdown = format!("| {header} |\n| {separator} |\n| {cells} |");
            let (_, visual) =
                cx.add_window_view(move |_, _| WrappingTableRoot { markdown, rem_size });
            let visual: &mut VisualTestContext = visual;
            visual.run_until_parked();
            visual.update(|window, cx| {
                _ = window.draw(cx);
            });
            let viewport = visual.debug_bounds("wrapping-table-viewport").unwrap();
            let first_before = visual.debug_bounds("markdown-table-0-cell-0-0").unwrap();
            if column_count == 10 {
                let last_before = visual.debug_bounds("markdown-table-0-cell-0-9").unwrap();
                assert!(
                    last_before.right() > viewport.right(),
                    "wide columns retain their minimum widths before scrolling"
                );
            }
            visual.simulate_event(gpui::ScrollWheelEvent {
                position: viewport.origin + gpui::point(px(20.), px(20.)),
                delta: gpui::ScrollDelta::Pixels(gpui::point(px(-10_000.), px(0.))),
                ..Default::default()
            });
            visual.update(|window, cx| {
                _ = window.draw(cx);
            });
            let last = visual
                .debug_bounds(if column_count == 2 {
                    "markdown-table-0-cell-0-1"
                } else {
                    "markdown-table-0-cell-0-9"
                })
                .unwrap();
            assert!(
                last.left() >= viewport.left(),
                "last column starts within viewport"
            );
            assert!(
                last.right() <= viewport.right() + px(0.5),
                "last column is fully reachable at rem {rem_size}"
            );
            if column_count == 2 {
                let first_after = visual.debug_bounds("markdown-table-0-cell-0-0").unwrap();
                assert_eq!(
                    first_after.origin.x, first_before.origin.x,
                    "normal tables fit without horizontal scrolling"
                );
                let body = visual.debug_bounds("markdown-table-0-cell-1-0").unwrap();
                assert!(
                    body.size.height > px(rem_size * 2.),
                    "normal cell content still wraps"
                );
            }
        }
    }
}
