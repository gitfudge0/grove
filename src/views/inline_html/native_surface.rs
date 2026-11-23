//! Native HTML card rendering and platform surface lifetime.

use super::{
    c, document_preferences_script, expanded, merge_exclusions, oversized_card, recovery_path, rpx,
    source_view, status, transcript_action, Cell, Disableable, GlobalElementId, HtmlArtifact,
    HtmlUi, LayoutId, LoadStatus, PathBuf, Rc, Selectable, StatusKind, Surface, SurfaceLease,
    ARTIFACT_IDENTITY_BASIS, INLINE_HEIGHT, REPORT_MAX_HEIGHT,
};
use crate::views::tokens::{
    CONTROL_H, ICON_MD, ICON_SM, RADIUS_GROUP, SPACE_2XL, SPACE_LG, SPACE_SM, TEXT_MICRO,
    TEXT_SMALL,
};
use gpui::prelude::{
    FluentBuilder, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled,
};
use gpui::{div, App, Bounds, Context, ElementId, EntityId, Pixels, Window};

pub(super) fn recovery_action(
    id: &'static str,
    label: &'static str,
    glyph: &'static str,
    window: &Window,
    cx: &App,
) -> gpui_component::button::Button {
    transcript_action(id, label, false, window, cx).icon(
        gpui_component::Icon::default()
            .path(format!("icons/{glyph}.svg"))
            .size(rpx(ICON_SM)),
    )
}

/// Icon treatment for document toolbar actions, retaining shared button behavior.
fn document_action(
    id: &'static str,
    label: &'static str,
    glyph: &'static str,
    window: &Window,
    cx: &App,
) -> gpui_component::button::Button {
    transcript_action(id, label, false, window, cx)
        .icon_only()
        .tooltip(label)
        .w(rpx(CONTROL_H))
        .px_0()
        .child(crate::icons::action_icon(glyph, ICON_MD))
}

impl Render for HtmlArtifact {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let expanded = expanded(&cx.entity(), cx);
        if self.error.is_none() && (self.publication.is_none() || expanded) {
            if let Some(surface) = &self.surface {
                if let Err(error) = surface.native.prepare(window) {
                    self.error = Some(error);
                    self.surface = None;
                    self.poll = None;
                } else {
                    // Queued documents may remain offscreen indefinitely. Start
                    // the loading timeout only once a live window initializes it.
                    self.started.get_or_insert_with(std::time::Instant::now);
                }
            }
        }
        if self.reveal && !expanded {
            self.reveal = false;
            self.anchor.scroll_to(window, cx);
        }
        if let Some(limit) = self.size_limit.clone() {
            let path = limit.path.clone();
            let cwd = self
                .session
                .upgrade()
                .map(|session| session.read(cx).cwd.clone());
            let roots = self
                .session
                .upgrade()
                .map_or_else(Vec::new, |session| session.read(cx).roots.clone());
            let safe = cwd
                .as_ref()
                .and_then(|cwd| recovery_path(&path, cwd, &roots));
            let mut actions = Vec::new();
            if let (Some(path), Some(cwd)) = (safe, cwd) {
                if let Ok(url) = gpui::http_client::Url::from_file_path(&path) {
                    let browser_path = path.clone();
                    let browser_cwd = cwd.clone();
                    let browser_roots = roots.clone();
                    actions.push(
                        recovery_action(
                            "oversize-html-browser",
                            "Open in browser",
                            "external-link",
                            window,
                            cx,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if recovery_path(&browser_path, &browser_cwd, &browser_roots).is_none()
                            {
                                return;
                            }
                            let url = url.to_string();
                            this.recovery_error = None;
                            cx.spawn(async move |this, cx| {
                                if let Err(error) =
                                    crate::platform::inline_html::open_in_browser(url).await
                                {
                                    let _ = this.update(cx, |this, cx| {
                                        this.recovery_error =
                                            Some(format!("Couldn’t open in browser: {error}"));
                                        cx.notify();
                                    });
                                }
                            })
                            .detach();
                        }))
                        .into_any_element(),
                    );
                }
                actions.push(
                    recovery_action("oversize-html-reveal", "Reveal file", "folder", window, cx)
                        .on_click(move |_, _, cx| {
                            if let Some(path) = recovery_path(&path, &cwd, &roots) {
                                cx.reveal_path(&path);
                            }
                        })
                        .into_any_element(),
                );
            }
            actions.push(
                recovery_action(
                    "oversize-html-check-updates",
                    "Check for updates",
                    "refresh",
                    window,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| this.read_file(cx)))
                .into_any_element(),
            );
            if let Some(error) = &self.recovery_error {
                actions.push(
                    div()
                        .w_full()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FORM_ERROR())
                        .child(error.clone())
                        .into_any_element(),
                );
            }
            return div()
                .id("html-oversized-artifact")
                .w_full()
                .anchor_scroll((!expanded).then(|| self.anchor.clone()))
                .child(oversized_card(
                    "Preview too large",
                    &limit,
                    "Open the full document in your browser.",
                    actions,
                    cx,
                ))
                .into_any_element();
        }
        let report = self.report;
        if self.last_status == Some(LoadStatus::Ready) {
            let script = document_preferences_script(window, report, expanded, cx.reduce_motion());
            if script != self.report_theme {
                if let Some(surface) = &self.surface {
                    surface.native.report_theme(&script, report && !expanded);
                    self.report_theme = script;
                }
            }
        }
        let title = self
            .publication
            .as_ref()
            .map(|r| r.title.as_str())
            .or_else(|| {
                self.path
                    .as_ref()
                    .and_then(|path| path.file_name())
                    .and_then(|name| name.to_str())
            })
            .unwrap_or("HTML document")
            .to_owned();
        let mut header = super::super::components::content_header()
            .debug_selector(|| "html-document-toolbar".into())
            .when(expanded, |header| {
                header.child(
                    super::super::components::back_action(
                        "html-expand",
                        "Back to conversation",
                        window,
                        cx,
                    )
                    .with_focus_handle(self.return_focus.clone())
                    .on_click(cx.listener(Self::toggle_expand_click)),
                )
            })
            .when(!report || expanded, |header| {
                header
                    .p(rpx(SPACE_LG))
                    .border_b_1()
                    .border_color(c::BORDER())
            })
            .flex_wrap()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(rpx(TEXT_SMALL))
                    .when(!report || expanded, |label| label.child(title.clone())),
            )
            .child(
                document_action("html-preview", "Preview", "unified", window, cx)
                    .selected(!self.source)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.source = false;
                        cx.notify();
                    })),
            )
            .child(
                document_action("html-source", "Source", "terminal", window, cx)
                    .selected(self.source)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.source = true;
                        cx.notify();
                    })),
            );
        if self.version.pending.is_some() {
            header = header.child(
                document_action(
                    "html-new-version",
                    "Load new version",
                    "restore",
                    window,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    if this.version.adopt() {
                        this.load(cx);
                    }
                    cx.notify();
                })),
            );
        }
        if self.path.is_some() {
            header = header.child(
                document_action(
                    "html-check-version",
                    "Check for updates",
                    "refresh",
                    window,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| this.read_file(cx))),
            );
        }
        if report && self.path.is_none() {
            header = header.child(
                document_action("html-reload", "Reload preview", "refresh", window, cx).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.load(cx);
                        cx.notify();
                    }),
                ),
            );
        }
        if !expanded {
            header = header.child(
                document_action("html-expand", "Expand", "zen", window, cx)
                    .with_focus_handle(self.expand_focus.clone())
                    .on_click(cx.listener(Self::toggle_expand_click)),
            );
        }
        let browser_action = if self.publication.is_some() && !expanded {
            document_action(
                "html-open-browser",
                "Open in browser",
                "external-link",
                window,
                cx,
            )
        } else {
            recovery_action(
                "html-open-browser",
                "Open in browser",
                "external-link",
                window,
                cx,
            )
        };
        let browser_action = browser_action
            .debug_selector(|| "html-artifact-browser".into())
            .disabled(self.loading_file || self.publication_failed)
            .on_click(cx.listener(|this, _, _, cx| {
                if this.loading_file || this.publication_failed {
                    return;
                }
                let target = if let Some(path) = &this.path {
                    Ok(path.clone())
                } else {
                    // Keep exported documents available after the browser handoff.
                    (|| -> Result<PathBuf, String> {
                        use std::io::Write;
                        let mut file = tempfile::Builder::new()
                            .prefix("grove-document-")
                            .suffix(".html")
                            .tempfile()
                            .map_err(|e| e.to_string())?;
                        file.write_all(this.version.active.as_bytes())
                            .map_err(|e| e.to_string())?;
                        // Browser exports have no native host to signal the initial canvas frame.
                        file.write_all(
                        b"<script>window.dispatchEvent(new Event('grove:activate-view'));</script>",
                    )
                    .map_err(|e| e.to_string())?;
                        let (_, path) = file.keep().map_err(|e| e.to_string())?;
                        Ok(path)
                    })()
                };
                this.recovery_error = None;
                cx.spawn(async move |this, cx| {
                    let result = match target {
                        Ok(path) => match gpui::http_client::Url::from_file_path(path) {
                            Ok(url) => {
                                crate::platform::inline_html::open_in_browser(url.to_string()).await
                            }
                            Err(()) => Err("Invalid document path".to_owned()),
                        },
                        Err(error) => Err(error),
                    };
                    if let Err(error) = result {
                        let _ = this.update(cx, |this, cx| {
                            this.recovery_error =
                                Some(format!("Couldn’t open in browser: {error}"));
                            cx.notify();
                        });
                    }
                })
                .detach();
            }));
        if self.publication.is_some() && !expanded {
            return div()
                .id("html-publication-attachment")
                .debug_selector(|| "html-publication-attachment".into())
                .w_full()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(rpx(SPACE_SM))
                .py(rpx(SPACE_LG))
                .px(rpx(SPACE_2XL))
                .border_1()
                .border_color(c::BORDER())
                .rounded(rpx(RADIUS_GROUP))
                .anchor_scroll(Some(self.anchor.clone()))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(rpx(SPACE_LG))
                        .w_full()
                        .min_w_0()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(rpx(SPACE_LG))
                                .flex_grow(1.)
                                .flex_basis(rpx(ARTIFACT_IDENTITY_BASIS))
                                .min_w_0()
                                .child(crate::icons::icon("file", ICON_MD, c::FG_DIM()))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .child(
                                            div()
                                                .debug_selector(|| "html-artifact-title".into())
                                                .text_size(rpx(TEXT_SMALL))
                                                .line_clamp(2)
                                                .text_ellipsis()
                                                .child(title.clone()),
                                        )
                                        .child(
                                            div()
                                                .debug_selector(|| "html-artifact-format".into())
                                                .text_size(rpx(TEXT_MICRO))
                                                .text_color(c::FG_DIM())
                                                .child(format!(
                                                    "{} · {}",
                                                    if self.loading_file {
                                                        "Loading artifact…"
                                                    } else {
                                                        "HTML artifact"
                                                    },
                                                    title
                                                )),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_shrink_0()
                                .items_center()
                                .gap(rpx(SPACE_SM))
                                .child(
                                    recovery_action(
                                        "html-publication-open",
                                        "Open artifact",
                                        "zen",
                                        window,
                                        cx,
                                    )
                                    .debug_selector(|| "html-publication-open".into())
                                    .with_focus_handle(self.expand_focus.clone())
                                    .disabled(self.loading_file || self.publication_failed)
                                    .on_click(cx.listener(Self::toggle_expand_click)),
                                )
                                .child(browser_action),
                        ),
                )
                .when_some(
                    self.error.clone().filter(|_| self.publication_failed),
                    |card, error| {
                        card.child(
                            div()
                                .text_size(rpx(TEXT_SMALL))
                                .text_color(c::FORM_ERROR())
                                .child(error),
                        )
                    },
                )
                .when_some(self.recovery_error.clone(), |card, error| {
                    card.child(
                        div()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FORM_ERROR())
                            .child(error),
                    )
                })
                .into_any_element();
        }
        let version_notice = self.version.pending.is_some().then(|| div().p(rpx(SPACE_LG)).text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM()).child("A new version is available. Load new version resets this document’s edits, tab and scroll."));
        let mut body = div()
            .id("html-document-body")
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_hidden();
        if self.loading_file {
            body = body.child(
                status::label(
                    "html-reading-status",
                    StatusKind::Working,
                    "Reading HTML document…",
                    ICON_SM,
                    cx,
                )
                .p(rpx(SPACE_2XL)),
            );
        } else if self.source || self.error.is_some() {
            body = body.overflow_y_scroll();
            if let Some(error) = &self.error {
                body = body
                    .child(div().p(rpx(SPACE_LG)).child(status::label(
                        "html-preview-error-status",
                        StatusKind::Failed,
                        error.clone(),
                        ICON_SM,
                        cx,
                    )))
                    .when(!self.publication_failed, |body| {
                        body.child(
                            transcript_action("html-retry", "Retry preview", false, window, cx)
                                .icon(
                                    gpui_component::Icon::default()
                                        .path("icons/refresh.svg")
                                        .size(rpx(ICON_SM)),
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if this.publication.is_some() && this.version.active.is_empty()
                                    {
                                        this.read_publication(cx);
                                    } else if this.version.active.is_empty() && this.path.is_some()
                                    {
                                        this.read_file(cx);
                                    } else {
                                        this.load(cx);
                                    }
                                    cx.notify();
                                })),
                        )
                    });
            }
            if !self.version.markdown.is_empty() {
                body = body.child(source_view(&self.version.markdown, 0, window));
            }
        } else if let Some(surface) = &self.surface {
            if surface.native.status() == LoadStatus::Loading {
                body = body.child(
                    status::label(
                        "html-loading-status",
                        StatusKind::Working,
                        "Loading preview…",
                        ICON_SM,
                        cx,
                    )
                    .p(rpx(SPACE_2XL)),
                );
            }
            body = body.child(NativeElement {
                artifact: cx.entity_id(),
                surface: surface.clone(),
                paused: self.paused.clone(),
                integrated: report && !expanded,
            });
        }
        div()
            .id("html-artifact")
            .debug_selector(|| "html-artifact".into())
            .w_full()
            .min_w_0()
            .min_h_0()
            .when(expanded, gpui::Styled::h_full)
            .when(!expanded, |card| {
                if self.publication_failed {
                    return card.h_auto();
                }
                card.h(if report && !self.source && self.error.is_none() {
                    // Measured CSS pixels are logical pixels; the cap follows UI zoom.
                    let cap = rpx(if self.publication.is_some() {
                        INLINE_HEIGHT
                    } else {
                        REPORT_MAX_HEIGHT
                    })
                    .to_pixels(window.rem_size());
                    gpui::px(self.report_height).min(cap)
                        + rpx(CONTROL_H + SPACE_LG).to_pixels(window.rem_size())
                } else {
                    rpx(INLINE_HEIGHT).to_pixels(window.rem_size())
                })
            })
            .flex()
            .flex_col()
            .bg(if report && !expanded {
                c::BG()
            } else {
                c::BG_RAIL()
            })
            .when(!report || expanded, |card| {
                card.border_1()
                    .border_color(c::BORDER())
                    .rounded(rpx(RADIUS_GROUP))
            })
            .overflow_hidden()
            .anchor_scroll((!expanded).then(|| self.anchor.clone()))
            .when(expanded, |card| {
                card.child(header)
                    .when_some(version_notice, gpui::ParentElement::child)
            })
            .child(body)
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .p(rpx(SPACE_SM))
                    .gap(rpx(SPACE_SM))
                    .child(browser_action),
            )
            .when(expanded, |card| {
                card.track_focus(&self.expanded_focus)
                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                        if event.keystroke.key == "escape" {
                            this.toggle_expand(window, cx);
                            cx.stop_propagation();
                        }
                    }))
            })
            .when_some(self.recovery_error.clone(), |card, error| {
                card.child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child(error),
                )
            })
            .into_any_element()
    }
}
impl HtmlArtifact {
    fn toggle_expand_click(
        &mut self,
        _: &gpui::ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_expand(window, cx);
    }
}

pub(super) fn surface_is_selected(artifact: EntityId, expanded: Option<EntityId>) -> bool {
    expanded.is_none_or(|selected| selected == artifact)
}

pub(super) struct NativeElement {
    pub(super) integrated: bool,
    pub(super) artifact: EntityId,
    pub(super) surface: Rc<Surface>,
    pub(super) paused: Rc<Cell<bool>>,
}
impl IntoElement for NativeElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl gpui::Element for NativeElement {
    type RequestLayoutState = ();
    type PrepaintState = Bounds<Pixels>;
    fn id(&self) -> Option<ElementId> {
        Some(ElementId::NamedInteger(
            "html-native-surface".into(),
            self.artifact.as_u64(),
        ))
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = gpui::Style::default();
        style.size.width = gpui::relative(1.).into();
        style.size.height = gpui::relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        (): &mut (),
        window: &mut Window,
        _: &mut App,
    ) -> Bounds<Pixels> {
        bounds.intersect(&window.content_mask().bounds)
    }
    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        (): &mut (),
        clip: &mut Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let selected = cx
            .try_global::<HtmlUi>()
            .and_then(|ui| ui.expanded.as_ref())
            .map(|entity| entity.entity_id());
        let blocked = super::super::mermaid::is_expanded(window, cx)
            || !surface_is_selected(self.artifact, selected)
            || (selected.is_none() && super::super::native_plan::blocks_inline_html(cx))
            || self.paused.get()
            || cx.try_global::<HtmlUi>().is_some_and(|ui| ui.blocked)
            || gpui_component::Root::try_read(window, cx)
                .is_some_and(|root| root.native_surface_blocked(cx));
        if blocked
            || self.surface.native.status() != LoadStatus::Ready
            || clip.size.width <= Pixels::ZERO
            || clip.size.height <= Pixels::ZERO
        {
            self.surface.native.hide();
            return;
        }
        let generation = self.surface.generation.get().wrapping_add(1);
        self.surface.generation.set(generation);
        self.surface.native.show(bounds, *clip, window);
        if !self.surface.initialized.replace(true) {
            self.surface.native.report_theme(
                "window.dispatchEvent(new Event('grove:activate-view'));",
                self.integrated,
            );
        }
        let panel = cx.try_global::<HtmlUi>().and_then(|ui| ui.exclusion);
        let tooltip = gpui_component::Root::try_read(window, cx)
            .and_then(|root| root.native_surface_exclusion(cx));
        self.surface.native.exclude(
            bounds,
            *clip,
            merge_exclusions(panel, tooltip, *clip),
            window,
        );
        let Some(id) = id else {
            self.surface.native.hide();
            return;
        };
        // Element arena objects are cleared AFTER presenting the current frame.
        // Keep the lease in GPUI's frame-owned state, not on the temporary
        // element or its request/prepaint data. It survives arena.clear and is
        // dropped only when a following frame omits this keyed surface.
        window.with_element_state(id, |previous: Option<SurfaceLease>, _| {
            drop(previous); // generation advanced above; cannot hide the new paint
            (
                (),
                SurfaceLease {
                    surface: self.surface.clone(),
                    generation,
                },
            )
        });
    }
}
