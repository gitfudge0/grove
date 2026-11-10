//! Compact completion prose and independently actionable artifacts, shared by providers.
use super::{components::inline_disclosure, rpx, tokens::*};
use crate::{icons::icon, theme as c};
use gpui::{div, prelude::*, AnyElement, App, FocusHandle};
use gpui_component::ElementExt;
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
    match url.scheme() {
        "http" | "https" | "mailto" => Some(url.to_string()),
        "file" => {
            let path = url.to_file_path().ok()?;
            let extension = path.extension()?.to_str()?.to_ascii_lowercase();
            // Source files and editor line references are not external artifacts.
            // Keep the shelf limited to documents browsers or desktop viewers open.
            (matches!(
                extension.as_str(),
                "html"
                    | "htm"
                    | "pdf"
                    | "txt"
                    | "md"
                    | "csv"
                    | "rtf"
                    | "doc"
                    | "docx"
                    | "odt"
                    | "xls"
                    | "xlsx"
                    | "ods"
                    | "ppt"
                    | "pptx"
                    | "odp"
                    | "png"
                    | "jpg"
                    | "jpeg"
                    | "gif"
                    | "webp"
                    | "svg"
                    | "bmp"
                    | "tif"
                    | "tiff"
                    | "mp3"
                    | "wav"
                    | "ogg"
                    | "flac"
                    | "m4a"
                    | "mp4"
                    | "webm"
                    | "mov"
                    | "m4v"
            ) && path.is_file())
            .then(|| url.to_string())
        }
        _ => None,
    }
}
impl ReviewSummary {
    pub fn parse(source: &str, cwd: &Path) -> Self {
        Self::parse_bounded(source, cwd, 240)
    }
    fn parse_bounded(source: &str, cwd: &Path, limit: usize) -> Self {
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
        fn visit(
            node: &Node,
            cwd: &Path,
            definitions: &HashMap<String, String>,
            out: &mut ReviewSummary,
            limit: usize,
        ) {
            if let Node::Paragraph(_) = node {
                if out.text.is_empty() && prose(node, false).chars().any(char::is_alphanumeric) {
                    out.text = bounded(&prose(node, true), limit);
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
                    visit(child, cwd, definitions, out, limit);
                }
            }
        }
        let Ok(root) = markdown::to_mdast(source, &markdown::ParseOptions::gfm()) else {
            return Self::default();
        };
        let mut definitions = HashMap::new();
        definitions_in(&root, &mut definitions);
        let mut result = Self::default();
        visit(&root, cwd, &definitions, &mut result, limit);
        result
    }
    pub fn from_snapshot(state: &grove_core::codex::Snapshot) -> Option<Self> {
        state.review_completion()?;
        let receipt = state.saved.session_review.pending.as_ref()?;
        let period = state.saved.work_periods.get(receipt.period)?;
        let items = state.saved.items.get(
            state
                .saved
                .session_review
                .reviewed_through
                .min(period.start_item)..period.end_item?,
        )?;
        if !state.saved.session_review.summary.is_empty() {
            let mut summary = Self::parse_bounded(
                &state.saved.session_review.summary,
                Path::new(&state.saved.cwd),
                usize::MAX,
            );
            // Keep artifacts from all unreviewed responses, independently of the brief.
            for item in items
                .iter()
                .filter(|item| item["type"] == "agentMessage" && item["phase"] != "commentary")
            {
                for link in Self::parse(
                    &grove_core::codex::item_text(item),
                    Path::new(&state.saved.cwd),
                )
                .links
                {
                    if !summary
                        .links
                        .iter()
                        .any(|existing| existing.url == link.url)
                    {
                        summary.links.push(link);
                    }
                }
            }
            return Some(summary);
        }
        let text = items.iter().rev().find(|item| {
            item["type"] == "agentMessage"
                && item["phase"] != "commentary"
                && !grove_core::codex::item_text(item).trim().is_empty()
        })?;
        let summary = Self::parse_bounded(
            &grove_core::codex::item_text(text),
            Path::new(&state.saved.cwd),
            usize::MAX,
        );
        (!summary.text.is_empty() || !summary.links.is_empty()).then_some(summary)
    }
    pub fn render(
        &self,
        key: &str,
        focus: &mut HashMap<String, FocusHandle>,
        disclosure: &mut Option<gpui::Entity<SummaryDisclosure>>,
        open_link: impl Fn(&str, &mut gpui::Window, &mut App) + 'static,
        cx: &mut App,
    ) -> AnyElement {
        let open_link = std::rc::Rc::new(open_link);
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
            let click_link = open_link.clone();
            let keyboard_link = open_link.clone();
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
                        click_link(&url, window, cx);
                    })
                    .on_key_down(move |event, window, cx| {
                        if !event.is_held
                            && matches!(event.keystroke.key.as_str(), "enter" | "space")
                        {
                            window.prevent_default();
                            cx.stop_propagation();
                            keyboard_link(&key_url, window, cx);
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
                    disclosure
                        .get_or_insert_with(|| {
                            cx.new(|cx| SummaryDisclosure {
                                text: self.text.clone(),
                                expanded: false,
                                focus: cx.focus_handle(),
                                measured: 0.,
                                from: 0.,
                                painted: std::rc::Rc::new(std::cell::Cell::new(0.)),
                                revision: 0,
                            })
                        })
                        .clone()
                        .update(cx, |view, cx| {
                            view.text.clone_from(&self.text);
                            cx.entity()
                        }),
                )
            })
            .when(!self.links.is_empty(), |el| el.child(links))
            .into_any_element()
    }
}

/// Retained per session so redraws and task disclosures preserve the user's choice.
pub(super) struct SummaryDisclosure {
    text: String,
    expanded: bool,
    focus: FocusHandle,
    measured: f32,
    from: f32,
    painted: std::rc::Rc<std::cell::Cell<f32>>,
    revision: u64,
}
impl SummaryDisclosure {
    fn toggle(&mut self, cx: &mut gpui::Context<Self>) {
        self.from = self.painted.get();
        self.expanded = !self.expanded;
        self.revision = self.revision.wrapping_add(1);
        cx.notify();
    }
}
impl gpui::Render for SummaryDisclosure {
    fn render(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        use gpui::AnimationExt as _;
        let collapsed = rpx((TEXT_BODY + SPACE_LG) * 2.);
        let collapsed = f32::from(collapsed.to_pixels(window.rem_size()));
        let target = if self.expanded {
            self.measured.max(collapsed)
        } else {
            self.measured.min(collapsed)
        };
        let moving = !cx.reduce_motion() && self.revision > 0;
        let measure = cx.entity().downgrade();
        // Measure the full text even when its painted copy is clamped.
        let full = div()
            .w_full()
            .flex_shrink_0()
            .whitespace_normal()
            .child(self.text.clone())
            .on_prepaint(move |bounds, _, cx| {
                let _ = measure.update(cx, |this, cx| {
                    let height = f32::from(bounds.size.height);
                    if (this.measured - height).abs() > 0.5 {
                        this.measured = height;
                        cx.notify();
                    }
                });
            });
        let visible = div()
            .absolute()
            .top_0()
            .left_0()
            .w_full()
            .whitespace_normal()
            .child(self.text.clone());
        let visible = if !self.expanded && moving {
            visible
                .with_animation(
                    format!("summary-clamp-{}", self.revision),
                    gpui::Animation::new(std::time::Duration::from_millis(MOTION_BASE_MS)),
                    |el, t| el.when(t >= 1., |el| el.line_clamp(2).text_ellipsis()),
                )
                .into_any_element()
        } else {
            visible
                .when(!self.expanded, |el| el.line_clamp(2).text_ellipsis())
                .into_any_element()
        };
        let content = div()
            .w_full()
            .min_w_0()
            .relative()
            .child(full.opacity(0.))
            .child(visible);
        let from = self.from;
        let painted = self.painted.clone();
        let body = div().w_full().min_w_0().overflow_hidden().child(content);
        let body = if moving {
            body.with_animation(
                format!("summary-height-{}", self.revision),
                gpui::Animation::new(std::time::Duration::from_millis(MOTION_BASE_MS))
                    .with_easing(super::motion::ease_standard),
                move |el, t| {
                    let height = from + (target - from) * t;
                    painted.set(height);
                    // Restore the clamped copy at the collapsed endpoint.
                    el.max_h(gpui::px(height.max(0.)))
                },
            )
            .into_any_element()
        } else {
            self.painted.set(target);
            body.when(self.measured > 0., |el| el.max_h(gpui::px(target)))
                .into_any_element()
        };
        inline_disclosure(
            "summary-disclosure",
            if self.expanded {
                "Collapse review summary"
            } else {
                "Expand review summary"
            },
        )
        .debug_selector(|| "summary-disclosure".to_owned())
        .track_focus(&self.focus)
        .tab_index(0)
        .tab_stop(true)
        .aria_expanded(self.expanded)
        .focus_visible(|style| style.bg(c::BG_HOVER()))
        .w_full()
        .h_auto()
        .min_w_0()
        .justify_start()
        .text_color(c::FG())
        .items_start()
        .gap(rpx(SPACE_SM))
        .child(
            div()
                .h(rpx(TEXT_BODY + SPACE_LG))
                .flex_shrink_0()
                .flex()
                .items_center()
                .child(crate::icons::icon(
                    if self.expanded {
                        "chev-down"
                    } else {
                        "chev-right"
                    },
                    ICON_SM,
                    c::FG_DIM(),
                )),
        )
        .child(div().flex_1().min_w_0().child(body))
        .on_click(cx.listener(|this, _, window, cx| {
            cx.stop_propagation();
            this.focus.focus(window, cx);
            this.toggle(cx);
        }))
        .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
            if !event.is_held && matches!(event.keystroke.key.as_str(), "enter" | "space") {
                window.prevent_default();
                cx.stop_propagation();
                this.toggle(cx);
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn artifacts() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for name in ["mock.html", "mock file.html", "notes.md"] {
            fs_err::write(dir.path().join(name), "fixture").unwrap();
        }
        dir
    }
    #[test]
    fn review_summary_separates_multiple_artifacts_from_concise_prose() {
        let dir = artifacts();
        let source = "[View mock](</tmp/Long path/mock.html>)\n\n**Added review summaries.**\nThey stay visible with tasks collapsed.\n\n[Design notes](notes.md#checks) · [Website](https://example.com)\n\n[Duplicate](notes.md#checks)\n\n[Unsafe](javascript:alert)\n\n```md\n[Example](https://ignored.example)\n```".replace("/tmp/Long path/mock.html", &dir.path().join("mock.html").to_string_lossy());
        let summary = ReviewSummary::parse(&source, dir.path());
        assert_eq!(
            summary.text,
            "Added review summaries. They stay visible with tasks collapsed."
        );
        assert_eq!(
            summary.links,
            vec![
                ReviewLink {
                    label: "View mock".into(),
                    url: gpui::http_client::Url::from_file_path(dir.path().join("mock.html"))
                        .unwrap()
                        .to_string()
                },
                ReviewLink {
                    label: "Design notes".into(),
                    url: format!(
                        "{}#checks",
                        gpui::http_client::Url::from_file_path(dir.path().join("notes.md"))
                            .unwrap()
                    )
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
        let dir = artifacts();
        let summary = ReviewSummary::parse(
            "[View **mock**][artifact]\n\n[artifact]: mock.html",
            dir.path(),
        );
        assert!(summary.text.is_empty());
        assert_eq!(summary.links[0].label, "View mock");
        assert_eq!(
            summary.links[0].url,
            gpui::http_client::Url::from_file_path(dir.path().join("mock.html"))
                .unwrap()
                .to_string()
        );
        let summary = ReviewSummary::parse(&"界 ".repeat(300), Path::new("/checkout"));
        assert!(summary.text.chars().count() <= 240);
        assert!(summary.text.ends_with('…'));
        assert_eq!(
            ReviewSummary::parse("", Path::new("/checkout")),
            ReviewSummary::default()
        );
    }
    #[test]
    fn review_links_only_include_openable_artifacts_for_codex_and_claude() {
        let dir = artifacts();
        for name in [
            "install.sh",
            "ci.yml",
            "main.rs",
            "mock.html:250",
            "image.PNG",
            "report.pdf",
        ] {
            fs_err::write(dir.path().join(name), "fixture").unwrap();
        }
        fs_err::create_dir(dir.path().join("directory.html")).unwrap();
        let summary = ReviewSummary::parse(
            "Done. [script](install.sh) [workflow](ci.yml) [source](main.rs) [line](mock.html:250) [missing](missing.pdf) [dir](directory.html) [image](image.PNG) [pdf](report.pdf) [html](mock.html#review) [web](https://example.com) [mail](mailto:test@example.com) [unsafe](javascript:alert) [anchor](#review)",
            dir.path(),
        );
        assert_eq!(
            summary
                .links
                .iter()
                .map(|link| link.label.as_str())
                .collect::<Vec<_>>(),
            ["image", "pdf", "html", "web", "mail"]
        );
    }
    struct Probe {
        summary: ReviewSummary,
        focus: HashMap<String, FocusHandle>,
        disclosure: Option<gpui::Entity<SummaryDisclosure>>,
    }
    impl gpui::Render for Probe {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            self.summary.render(
                "review-test",
                &mut self.focus,
                &mut self.disclosure,
                |url, _, cx| cx.open_url(url),
                cx,
            )
        }
    }
    #[gpui::test]
    fn review_summary_links_support_pointer_keyboard_and_narrow_zoom(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, _| Probe {
            summary: ReviewSummary::parse("Added a review summary.\n\n[View the long mock artifact label](https://example.com/mock%20file.html) [Read notes](https://example.com/notes)", Path::new("/checkout")),
            focus: HashMap::new(),
            disclosure: None,
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
                        "https://example.com/mock%20file.html"
                    } else {
                        "https://example.com/notes"
                    })
                );
            }
            let summary_bounds = cx.debug_bounds("summary-disclosure").unwrap();
            cx.simulate_click(summary_bounds.center(), gpui::Modifiers::default());
            assert!(view.read_with(cx, |view, cx| view
                .disclosure
                .as_ref()
                .unwrap()
                .read(cx)
                .expanded));
            cx.simulate_keystrokes("space");
            assert!(!view.read_with(cx, |view, cx| view
                .disclosure
                .as_ref()
                .unwrap()
                .read(cx)
                .expanded));
            // Explicit focus of the first artifact exercises its own key route.
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.focus["review-test:https://example.com/mock%20file.html"]
                        .focus(window, cx);
                });
            });
            cx.simulate_keystrokes("enter");
            assert_eq!(
                cx.opened_url().as_deref(),
                Some("https://example.com/mock%20file.html")
            );
        }
    }
}
