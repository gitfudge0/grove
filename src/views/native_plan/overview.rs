//! Native, revision-bound reading layer. Authored Markdown remains available in full.
use super::*;

const FLOW_STEP_MIN_WIDTH: f32 = 150.0;

impl NativePlan {
    pub(super) fn overview(
        &self,
        doc: &Value,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let presentation = &doc["presentation"];
        let mut body = div()
            .debug_selector(|| "native-plan-overview".into())
            .flex()
            .flex_col()
            .gap(rpx(SPACE_2XL))
            .min_w_0()
            .w_full()
            .child(
                div()
                    .text_size(rpx(TEXT_TITLE))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(string(doc, "title")),
            )
            .child(self.text(
                "plan-outcome".into(),
                string(presentation, "outcome"),
                window,
                cx,
            ));
        let changes = entries(&presentation["revision_changes"]);
        if !changes.is_empty() {
            body = body.child(self.overview_list(
                "revision-changes",
                "Changed in this revision · authored summary",
                &changes,
                window,
                cx,
            ));
        }
        let flow = entries(&presentation["flow"]);
        if !flow.is_empty() {
            let mut row = div()
                .debug_selector(|| "native-plan-flow".into())
                .flex()
                .flex_wrap()
                .gap(rpx(SPACE_LG))
                .w_full()
                .min_w_0();
            for (index, step) in flow.iter().enumerate() {
                row = row.child(
                    div()
                        .debug_selector(move || format!("native-plan-flow-step-{index}"))
                        .flex()
                        .flex_col()
                        .gap(rpx(SPACE_SM))
                        .flex_1()
                        .min_w_0()
                        .flex_basis(rpx(FLOW_STEP_MIN_WIDTH))
                        .max_w_full()
                        .border_1()
                        .border_color(c::BORDER_SOFT())
                        .rounded(rpx(RADIUS_CONTROL))
                        .p(rpx(SPACE_LG))
                        .child(self.text(
                            format!("flow-title-{index}"),
                            format!("{}. {}", index + 1, string(step, "title")),
                            window,
                            cx,
                        ))
                        .child(div().text_color(c::FG_DIM()).child(self.text(
                            format!("flow-detail-{index}"),
                            string(step, "detail"),
                            window,
                            cx,
                        ))),
                );
            }
            body = body.child(row);
        }
        for (key, label) in [
            ("included", "Included"),
            ("deferred", "Deferred"),
            ("risks", "Risks"),
            ("success_criteria", "Success criteria"),
        ] {
            let list = entries(&presentation[key]);
            if !list.is_empty() {
                body = body.child(self.overview_list(key, label, &list, window, cx));
            }
        }
        body.into_any_element()
    }
    fn overview_list(
        &self,
        key: &str,
        label: &str,
        entries: &[Value],
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let debug_key = key.to_owned();
        let mut body = div()
            .debug_selector(move || format!("native-plan-overview-{debug_key}"))
            .flex()
            .flex_col()
            .min_w_0()
            .gap(rpx(SPACE_SM))
            .child(
                div()
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .child(label.to_owned()),
            );
        for (index, value) in entries.iter().enumerate() {
            body = body.child(self.text(
                format!("overview-{key}-{index}"),
                value.as_str().unwrap_or_default().to_owned(),
                window,
                cx,
            ));
        }
        body.into_any_element()
    }
    pub(super) fn detail_section(
        &self,
        index: usize,
        title: String,
        markdown: String,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.open_sections.contains(&index);
        let key = format!("section:{index}");
        let target = self.projection.borrow_mut().control(
            key,
            measured::Action::ToggleSection(index),
            true,
            cx,
        );
        let action = target.action.clone();
        let keyboard_action = action.clone();
        let label = format!(
            "{} · {}",
            if index == 0 { "Plan detail" } else { "Detail" },
            title
        );
        let header = div()
            .id(format!("plan-detail-{index}"))
            .debug_selector(move || format!("native-plan-detail-{index}"))
            .flex()
            .items_center()
            .gap(rpx(SPACE_SM))
            .min_w_0()
            .w_full()
            .py(rpx(SPACE_LG))
            .role(gpui::Role::Button)
            .aria_label(label.clone())
            .aria_expanded(open)
            .cursor_pointer()
            .text_color(c::FG_DIM())
            .hover(|s| s.text_color(c::FG()))
            .tab_index(0)
            .track_focus(&target.focus)
            .focus_visible(|s| s.bg(c::BG_HOVER()))
            .child(crate::icons::icon(
                if open { "chev-down" } else { "chev-right" },
                ICON_SM,
                c::FG_DIM(),
            ))
            .child(label)
            .on_click(cx.listener(move |this, _, window, cx| this.activate(&action, window, cx)))
            .on_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        this.activate(&keyboard_action, window, cx);
                        cx.stop_propagation();
                    }
                }),
            );
        let mut section = div()
            .min_w_0()
            .w_full()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(c::BORDER_SOFT())
            .child(measured::Observed::new(header.into_any_element(), target));
        if open {
            // Immediate disclosure preserves text selection and avoids moving focus into hidden content.
            section = section
                .child(
                    div()
                        .debug_selector(move || format!("native-plan-detail-content-{index}"))
                        .child(self.measured_text(
                            format!("plan-detail-text-{index}"),
                            markdown,
                            true,
                            window,
                            cx,
                        )),
                )
                .child(div().flex().child(self.discuss_action(
                    title,
                    format!("plan-detail-discuss-{index}"),
                    window,
                    cx,
                )));
        }
        section.into_any_element()
    }
}
