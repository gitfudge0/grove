//! Native, revision-bound reading layer. Authored Markdown remains available in full.
use super::*;

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
        let structured = !entries(&doc["claims"]).is_empty();
        let flow = if structured {
            Vec::new()
        } else {
            entries(&presentation["flow"])
        };
        if !flow.is_empty() {
            let mut row = div()
                .debug_selector(|| "native-plan-flow".into())
                .flex()
                .flex_col()
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
                        .min_w_0()
                        .max_w_full()
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
            if structured && key != "risks" {
                continue;
            }
            let list = entries(&presentation[key]);
            if !list.is_empty() {
                body = body.child(self.overview_list(key, label, &list, window, cx));
            }
        }
        let changes = entries(&presentation["revision_changes"]);
        if !structured && !changes.is_empty() {
            body = body.child(self.overview_list(
                "revision-changes",
                "Changed in this revision",
                &changes,
                window,
                cx,
            ));
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
}
