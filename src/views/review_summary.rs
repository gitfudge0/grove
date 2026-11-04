//! Compact completion prose and independently actionable artifacts, shared by providers.
use super::{components::inline_disclosure, rpx, tokens::*};
use crate::{icons::icon, theme as c};
use gpui::{div, prelude::*, AnyElement, App, FocusHandle};
use markdown::mdast::Node;
use std::{collections::HashMap, path::Path};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct ReviewSummary {
    pub text: String,
    pub links: Vec<ReviewLink>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ReviewLink {
    pub label: String,
    pub url: String,
}
fn bounded(text: &str, limit: usize) -> String {
    let plain = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if plain.chars().count() <= limit {
        return plain;
    }
    let prefix: String = plain.chars().take(limit - 1).collect();
    let end = prefix.rfind(' ').unwrap_or(prefix.len());
    format!("{}…", prefix[..end].trim_end())
}
fn prose(node: &Node, include_links: bool) -> String {
    match node {
        Node::Text(text) => text.value.clone(),
        Node::InlineCode(code) => code.value.clone(),
        Node::Break(_) => " ".into(),
        Node::Link(_) | Node::LinkReference(_) if !include_links => String::new(),
        Node::Image(_) | Node::ImageReference(_) | Node::Html(_) | Node::Code(_) => String::new(),
        _ => node
            .children()
            .map(|children| {
                children
                    .iter()
                    .map(|child| prose(child, include_links))
                    .collect()
            })
            .unwrap_or_default(),
    }
}
fn destination(target: &str, cwd: &Path) -> Option<String> {
    use gpui::http_client::Url;
    if target.starts_with("//") || target.starts_with('#') {
        return None;
    }
    let url = if target.starts_with('/') {
        Url::parse(&format!("file://{target}")).ok()?
    } else if let Ok(url) = Url::parse(target) {
        url
    } else {
        Url::from_directory_path(cwd).ok()?.join(target).ok()?
    };
    matches!(url.scheme(), "http" | "https" | "mailto" | "file").then(|| url.to_string())
}
impl ReviewSummary {
    pub fn parse(source: &str, cwd: &Path) -> Self {
        let Ok(root) = markdown::to_mdast(source, &markdown::ParseOptions::gfm()) else {
            return Self::default();
        };
        let mut definitions = HashMap::new();
        fn definitions_in(node: &Node, out: &mut HashMap<String, String>) {
            if let Node::Definition(def) = node {
                out.entry(def.identifier.clone())
                    .or_insert_with(|| def.url.clone());
            }
            if let Some(children) = node.children() {
                for child in children {
                    definitions_in(child, out);
                }
            }
        }
        definitions_in(&root, &mut definitions);
        let mut result = Self::default();
        fn visit(
            node: &Node,
            cwd: &Path,
            definitions: &HashMap<String, String>,
            out: &mut ReviewSummary,
        ) {
            if let Node::Paragraph(_) = node {
                if out.text.is_empty() && prose(node, false).chars().any(char::is_alphanumeric) {
                    out.text = bounded(&prose(node, true), 240);
                }
            }
            let target = match node {
                Node::Link(link) => Some(link.url.as_str()),
                Node::LinkReference(link) => definitions.get(&link.identifier).map(String::as_str),
                _ => None,
            };
            if let Some(url) = target.and_then(|target| destination(target, cwd)) {
                let label = bounded(&prose(node, true), 80);
                if !label.is_empty() && !out.links.iter().any(|link| link.url == url) {
                    out.links.push(ReviewLink { label, url });
                }
            }
            // Fenced examples and HTML are never executable artifact links.
            if let Some(children) = node.children() {
                for child in children {
                    visit(child, cwd, definitions, out);
                }
            }
        }
        visit(&root, cwd, &definitions, &mut result);
        result
    }
    pub fn from_snapshot(state: &grove_core::codex::Snapshot) -> Option<Self> {
        state.review_completion()?;
        let receipt = state.saved.session_review.pending.as_ref()?;
        let period = state.saved.work_periods.get(receipt.period)?;
        let items = state.saved.items.get(period.start_item..period.end_item?)?;
        let text = items.iter().rev().find(|item| {
            item["type"] == "agentMessage"
                && item["phase"] != "commentary"
                && !grove_core::codex::item_text(item).trim().is_empty()
        })?;
        let summary = Self::parse(
            &grove_core::codex::item_text(text),
            Path::new(&state.saved.cwd),
        );
        (!summary.text.is_empty() || !summary.links.is_empty()).then_some(summary)
    }
    pub fn render(
        &self,
        key: &str,
        focus: &mut HashMap<String, FocusHandle>,
        cx: &mut App,
    ) -> AnyElement {
        focus.retain(|identity, _| {
            self.links
                .iter()
                .any(|link| identity == &format!("{key}:{}", link.url))
        });
        let mut links = div()
            .flex()
            .flex_wrap()
            .gap_x(rpx(SPACE_3XL))
            .gap_y(rpx(SPACE_SM));
        for (index, link) in self.links.iter().enumerate() {
            let id = format!("{key}-link-{index}");
            let handle = focus
                .entry(format!("{key}:{}", link.url))
                .or_insert_with(|| cx.focus_handle())
                .clone();
            let click_focus = handle.clone();
            let url = link.url.clone();
            let key_url = url.clone();
            links = links.child(
                inline_disclosure(id.clone(), link.label.clone())
                    .debug_selector(move || id.clone())
                    .role(gpui::Role::Link)
                    .track_focus(&handle)
                    .tab_index(0)
                    .tab_stop(true)
                    .focus_visible(|style| style.bg(c::BG_HOVER()))
                    .w_auto()
                    .max_w_full()
                    .h_auto()
                    .min_w_0()
                    .justify_start()
                    .items_start()
                    .gap(rpx(SPACE_SM))
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::STATUS_INFO())
                    .child(
                        div()
                            .h(rpx(TEXT_BODY + SPACE_LG))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .child(icon("external-link", ICON_SM, c::STATUS_INFO())),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .whitespace_normal()
                            .child(link.label.clone()),
                    )
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        click_focus.focus(window, cx);
                        cx.open_url(&url);
                    })
                    .on_key_down(move |event, window, cx| {
                        if !event.is_held
                            && matches!(event.keystroke.key.as_str(), "enter" | "space")
                        {
                            window.prevent_default();
                            cx.stop_propagation();
                            cx.open_url(&key_url);
                        }
                    }),
            );
        }
        div()
            .id(gpui::SharedString::from(key.to_owned()))
            .debug_selector({
                let key = key.to_owned();
                move || key.clone()
            })
            .role(gpui::Role::Group)
            .aria_label("Review summary")
            .font_family(crate::fonts::UI_FAMILY)
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_SM))
            .text_size(rpx(TEXT_BODY))
            .line_height(rpx(TEXT_BODY + SPACE_LG))
            .child(
                div()
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::STATUS_SUCCESS())
                    .child("Review summary"),
            )
            .when(!self.text.is_empty(), |el| {
                el.child(
                    div()
                        .whitespace_normal()
                        .line_clamp(2)
                        .child(self.text.clone()),
                )
            })
            .when(!self.links.is_empty(), |el| el.child(links))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn review_summary_separates_multiple_artifacts_from_concise_prose() {
        let summary = ReviewSummary::parse("[View mock](</tmp/Long path/mock.html>)\n\n**Added review summaries.**\nThey stay visible with tasks collapsed.\n\n[Design notes](notes.md#checks) · [Website](https://example.com)\n\n[Duplicate](notes.md#checks)\n\n[Unsafe](javascript:alert)\n\n```md\n[Example](https://ignored.example)\n```", Path::new("/checkout"));
        assert_eq!(
            summary.text,
            "Added review summaries. They stay visible with tasks collapsed."
        );
        assert_eq!(
            summary.links,
            vec![
                ReviewLink {
                    label: "View mock".into(),
                    url: "file:///tmp/Long%20path/mock.html".into()
                },
                ReviewLink {
                    label: "Design notes".into(),
                    url: "file:///checkout/notes.md#checks".into()
                },
                ReviewLink {
                    label: "Website".into(),
                    url: "https://example.com/".into()
                },
            ]
        );
    }
    #[test]
    fn review_summary_handles_link_only_responses_references_and_unicode() {
        let summary = ReviewSummary::parse(
            "[View **mock**][artifact]\n\n[artifact]: /tmp/mock.html",
            Path::new("/checkout"),
        );
        assert!(summary.text.is_empty());
        assert_eq!(summary.links[0].label, "View mock");
        assert_eq!(summary.links[0].url, "file:///tmp/mock.html");
        let summary = ReviewSummary::parse(&"界 ".repeat(300), Path::new("/checkout"));
        assert!(summary.text.chars().count() <= 240);
        assert!(summary.text.ends_with('…'));
        assert_eq!(
            ReviewSummary::parse("", Path::new("/checkout")),
            ReviewSummary::default()
        );
    }
    struct Probe {
        summary: ReviewSummary,
        focus: HashMap<String, FocusHandle>,
    }
    impl gpui::Render for Probe {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            self.summary.render("review-test", &mut self.focus, cx)
        }
    }
    #[gpui::test]
    fn review_summary_links_support_pointer_keyboard_and_narrow_zoom(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, _| Probe {
            summary: ReviewSummary::parse("Added a review summary.\n\n[View the long mock artifact label](</tmp/mock file.html>) [Read notes](https://example.com/notes)", Path::new("/checkout")),
            focus: HashMap::new(),
        });
        for (width, rem) in [(800., 16.), (280., 20.)] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(500.)));
            cx.update(|window, cx| {
                window.set_rem_size(gpui::px(rem));
                let _ = window.draw(cx);
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            for index in 0..2 {
                let bounds = cx
                    .debug_bounds(if index == 0 {
                        "review-test-link-0"
                    } else {
                        "review-test-link-1"
                    })
                    .unwrap();
                assert!(bounds.right() <= gpui::px(width));
                cx.simulate_click(bounds.center(), gpui::Modifiers::default());
                assert_eq!(
                    cx.opened_url().as_deref(),
                    Some(if index == 0 {
                        "file:///tmp/mock%20file.html"
                    } else {
                        "https://example.com/notes"
                    })
                );
            }
            // Explicit focus of the first artifact exercises its own key route.
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.focus["review-test:file:///tmp/mock%20file.html"].focus(window, cx)
                })
            });
            cx.simulate_keystrokes("enter");
            assert_eq!(
                cx.opened_url().as_deref(),
                Some("file:///tmp/mock%20file.html")
            );
        }
    }
}
