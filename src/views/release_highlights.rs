//! Release-authored, opt-in highlights. Replays never alter first-run bookkeeping.
use super::{rpx, tokens::*};
use crate::{assets::Assets, theme as c};
use gpui::{
    div, img, prelude::*, App, Context, EventEmitter, FocusHandle, FontWeight, MouseButton,
    StyledImage, Window,
};
use gpui_component::Sizable;
use grove_core::release_highlights::{Manifest, MediaKind, ReleaseHighlights};

pub fn manifest() -> Option<Manifest> {
    let asset = Assets::get("highlights/manifest.json")?;
    Manifest::parse(&asset.data).ok()
}

pub fn selected(debug: bool) -> Option<ReleaseHighlights> {
    let debug = debug && cfg!(debug_assertions);
    let manifest = manifest()?;
    let version = if debug && cfg!(debug_assertions) {
        std::env::var("GROVE_HIGHLIGHTS_VERSION")
            .unwrap_or_else(|_| env!("CARGO_PKG_VERSION").into())
    } else {
        env!("CARGO_PKG_VERSION").into()
    };
    manifest
        .release(&version)
        .filter(|release| !release.slides.is_empty() && (debug || release.enabled))
        .cloned()
}

#[derive(Clone)]
pub struct Closed;

pub struct ReleaseCarousel {
    release: ReleaseHighlights,
    index: usize,
    playing: bool,
    focus: FocusHandle,
    return_focus: Option<FocusHandle>,
    media_error: Option<String>,
    #[cfg(test)]
    image_source_override: Option<gpui::ImageSource>,
}
impl EventEmitter<Closed> for ReleaseCarousel {}

impl ReleaseCarousel {
    pub fn new(release: ReleaseHighlights, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let return_focus = window.focused(cx);
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Self {
            release,
            index: 0,
            playing: false,
            focus,
            return_focus,
            media_error: None,
            #[cfg(test)]
            image_source_override: None,
        }
    }
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(focus) = self.return_focus.take() {
            focus.focus(window, cx);
        }
        cx.emit(Closed);
    }
    fn next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.index + 1 == self.release.slides.len() {
            self.close(window, cx);
        } else {
            self.index += 1;
            self.playing = false;
            self.media_error = None;
            cx.notify();
        }
    }
    fn back(&mut self, cx: &mut Context<Self>) {
        self.index = self.index.saturating_sub(1);
        self.playing = false;
        self.media_error = None;
        cx.notify();
    }
    fn key(&mut self, event: &gpui::KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => self.close(window, cx),
            "right" => self.next(window, cx),
            "left" => self.back(cx),
            "tab" => {
                for _ in 0..128 {
                    if event.keystroke.modifiers.shift {
                        window.focus_prev(cx);
                    } else {
                        window.focus_next(cx);
                    }
                    if self.focus.contains_focused(window, cx) {
                        break;
                    }
                }
                if !self.focus.contains_focused(window, cx) {
                    self.focus.focus(window, cx);
                }
            }
            "enter" | "space" => return,
            _ => {
                cx.stop_propagation();
                return;
            }
        }
        window.prevent_default();
        cx.stop_propagation();
    }
    fn play_video(&mut self, cx: &mut Context<Self>) {
        let Some(slide) = self.release.slides.get(self.index) else {
            return;
        };
        match materialize(&slide.media.src) {
            Ok(path) => {
                #[cfg(target_os = "macos")]
                let result = std::process::Command::new("open").arg(&path).spawn();
                #[cfg(target_os = "windows")]
                let result = std::process::Command::new("explorer.exe")
                    .arg(&path)
                    .spawn();
                #[cfg(not(any(target_os = "macos", target_os = "windows")))]
                let result = std::process::Command::new("xdg-open").arg(&path).spawn();
                if let Err(error) = result {
                    self.media_error = Some(error.to_string());
                    cx.notify();
                }
            }
            Err(error) => {
                self.media_error = Some(error.to_string());
                cx.notify();
            }
        }
    }
}

/// Only embedded media can leave the application; manifest paths are never URLs.
fn materialize(src: &str) -> anyhow::Result<std::path::PathBuf> {
    let path = std::path::Path::new(src);
    anyhow::ensure!(
        src.starts_with("highlights/")
            && path
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_))),
        "Invalid media path"
    );
    let asset = Assets::get(src).ok_or_else(|| anyhow::anyhow!("Media unavailable"))?;
    let cache = grove_core::storage::config_dir()?
        .join("cache/release-highlights")
        .join(
            path.parent()
                .ok_or_else(|| anyhow::anyhow!("Missing media directory"))?,
        );
    fs_err::create_dir_all(&cache)?;
    let filename = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("Missing media filename"))?;
    let destination = cache.join(filename);
    fs_err::write(&destination, &asset.data)?;
    Ok(destination)
}

fn button(
    id: &'static str,
    label: &'static str,
    window: &Window,
    cx: &App,
) -> gpui_component::button::Button {
    super::components::form_action(id, label, id == "highlights-next", window, cx)
}
fn unavailable() -> gpui::AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(c::FG_DIM())
        .child("Preview unavailable. You can still explore this update.")
        .into_any_element()
}
impl gpui::Focusable for ReleaseCarousel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Render for ReleaseCarousel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(slide) = self.release.slides.get(self.index) else {
            return div().into_any_element();
        };
        let scale = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
        let width = MODAL_W_XL
            .min((f32::from(window.viewport_size().width) / scale - SPACE_LG * 2.0).max(0.0));
        let height = (f32::from(window.viewport_size().height) / scale - SPACE_LG * 2.0).max(0.0);
        let animated = self.playing && !cx.reduce_motion();
        let source = match slide.media.kind {
            MediaKind::Image => Some(slide.media.src.clone()),
            MediaKind::Gif if animated => Some(slide.media.src.clone()),
            MediaKind::Gif | MediaKind::Video => slide.media.poster.clone(),
        };
        let media_height = (width * 9.0 / 16.0).min(height * 0.52);
        let mut media = div()
            .id("highlights-media")
            .debug_selector(|| "highlights-media".into())
            .w_full()
            .h(rpx(media_height))
            .flex_shrink_0()
            .overflow_hidden()
            .bg(c::BG())
            .relative()
            .aria_label(slide.media.alt.clone());
        media = media.child(match source {
            Some(source) if Assets::get(&source).is_some() => {
                let image_source = gpui::ImageSource::from(gpui::SharedString::from(source));
                #[cfg(test)]
                let image_source = self.image_source_override.clone().unwrap_or(image_source);
                let image_width = (width - 2.0).max(0.0);
                let (image_width, image_height, image_left, image_top) = match &slide.media.frame {
                    Some(frame) => {
                        let image_width = image_width * frame.zoom;
                        let image_height = image_width / frame.aspect_ratio;
                        (image_width, image_height,
                            -(image_width - (width - 2.0)).max(0.0) * frame.position_x,
                            -(image_height - media_height).max(0.0) * frame.position_y)
                    }
                    None => (image_width, media_height, 0.0, 0.0),
                };
                let image_radius = RADIUS_CHROME * slide.media.frame.as_ref().map_or(1.0, |frame| frame.zoom);
                let retry_source = image_source.clone();
                let carousel = cx.entity().downgrade();
                img(image_source).id(format!("highlights-image-{}-{}", self.index, animated)).debug_selector(|| "highlights-image".into()).absolute().left(rpx(image_left)).top(rpx(image_top)).w(rpx(image_width)).h(rpx(image_height)).rounded(rpx(image_radius)).object_fit(gpui::ObjectFit::Contain)
                    .with_fallback(move || {
                        let source = retry_source.clone(); let key_source = retry_source.clone(); let key_carousel = carousel.clone(); let carousel = carousel.clone();
                        div().size_full().flex().flex_col().gap(rpx(SPACE_LG)).items_center().justify_center().text_color(c::FG_DIM())
                            .child("Preview could not be decoded. The highlight is still available below.")
                            .child(div().id("highlights-retry").tab_index(0).aria_label("Retry media preview").child("Retry")
                                .on_click(move |_, _, cx| { source.remove_asset(cx); let _ = carousel.update(cx, |_, cx| cx.notify()); })
                                .on_key_down(move |event, window, cx| { if matches!(event.keystroke.key.as_str(), "enter" | "space") { key_source.remove_asset(cx); let _ = key_carousel.update(cx, |_, cx| cx.notify()); window.prevent_default(); cx.stop_propagation(); } }))
                            .into_any_element()
                    }).into_any_element()
            },
            _ => unavailable(),
        });
        if let Some(frame) = &slide.media.frame {
            if let Some([left, top, marker_width, marker_height]) = frame.highlight {
                let image_width = (width - 2.0).max(0.0) * frame.zoom;
                let image_height = image_width / frame.aspect_ratio;
                let image_left = -(image_width - (width - 2.0)).max(0.0) * frame.position_x;
                let image_top = -(image_height - media_height).max(0.0) * frame.position_y;
                media = media.child(
                    div()
                        .debug_selector(|| "highlights-image-focus".into())
                        .absolute()
                        .left(rpx(image_left + image_width * left))
                        .top(rpx(image_top + image_height * top))
                        .w(rpx(image_width * marker_width))
                        .h(rpx(image_height * marker_height))
                        .rounded(rpx(RADIUS_CONTROL))
                        .border_2()
                        .border_color(c::FG()),
                );
            }
        }
        match slide.media.kind {
            MediaKind::Gif if !cx.reduce_motion() => {
                media = media.child(
                    div()
                        .absolute()
                        .bottom(rpx(SPACE_LG))
                        .right(rpx(SPACE_LG))
                        .child(
                            button(
                                "highlights-gif",
                                if animated {
                                    "Pause animation"
                                } else {
                                    "Play animation"
                                },
                                window,
                                cx,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.playing = !this.playing;
                                cx.notify();
                            })),
                        ),
                );
            }
            MediaKind::Video => {
                media = media.child(
                    div()
                        .absolute()
                        .bottom(rpx(SPACE_LG))
                        .right(rpx(SPACE_LG))
                        .child(
                            button("highlights-video", "Play video", window, cx)
                                .on_click(cx.listener(|this, _, _, cx| this.play_video(cx))),
                        ),
                );
            }
            _ => {}
        }
        let mut dots = div()
            .id("highlights-progress")
            .flex()
            .gap(rpx(SPACE_SM))
            .aria_label(format!(
                "Highlight {} of {}",
                self.index + 1,
                self.release.slides.len()
            ));
        for index in 0..self.release.slides.len() {
            dots = dots.child(
                div()
                    .id(format!("highlights-dot-{index}"))
                    .tab_index(0)
                    .aria_label(format!("Go to highlight {}", index + 1))
                    .w(rpx(24.0))
                    .h(rpx(24.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.index = index;
                        this.playing = false;
                        this.media_error = None;
                        cx.notify();
                    }))
                    .on_key_down(cx.listener(
                        move |this, event: &gpui::KeyDownEvent, window, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                this.index = index;
                                this.playing = false;
                                this.media_error = None;
                                window.prevent_default();
                                cx.stop_propagation();
                                cx.notify();
                            }
                        },
                    ))
                    .child(div().w(rpx(6.0)).h(rpx(6.0)).rounded_full().bg(
                        if index == self.index {
                            c::FG()
                        } else {
                            c::BORDER_STRONG()
                        },
                    )),
            );
        }
        div()
            .id("release-highlights-overlay")
            .debug_selector(|| "release-highlights-overlay".into())
            .absolute()
            .inset_0()
            .occlude()
            .bg(c::SCRIM())
            .flex()
            .items_center()
            .justify_center()
            .capture_key_down(cx.listener(Self::key))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .id("release-highlights")
                    .debug_selector(|| "release-highlights".into())
                    .track_focus(&self.focus)
                    .w(rpx(width))
                    .max_h(rpx(height))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .rounded(rpx(RADIUS_PANEL))
                    .border_1()
                    .border_color(c::BORDER())
                    .bg(c::SURFACE_RAISED())
                    .text_color(c::FG())
                    .child(
                        div()
                            .flex_shrink_0()
                            .px(rpx(SPACE_2XL))
                            .py(rpx(SPACE_LG))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(div().text_size(rpx(TEXT_BODY)).child(format!(
                                "{} · {}",
                                self.release.title, self.release.version
                            )))
                            .child(
                                button("highlights-close", "Close", window, cx)
                                    .small()
                                    .compact()
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.close(window, cx)),
                                    ),
                            ),
                    )
                    .child(media)
                    .child(
                        div()
                            .debug_selector(|| "highlights-copy".into())
                            .flex_shrink_0()
                            .p(rpx(SPACE_2XL))
                            .flex()
                            .flex_col()
                            .gap(rpx(SPACE_SM))
                            .child(
                                div()
                                    .text_size(rpx(TEXT_DISPLAY))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(slide.title.clone()),
                            )
                            .child(
                                div()
                                    .text_size(rpx(TEXT_BODY))
                                    .text_color(c::FG_DIM())
                                    .child(slide.description.clone()),
                            )
                            .when_some(self.media_error.clone(), |element, error| {
                                element.child(div().text_color(c::FORM_ERROR()).child(error))
                            }),
                    )
                    .child(
                        div()
                            .debug_selector(|| "highlights-footer".into())
                            .flex_shrink_0()
                            .px(rpx(SPACE_2XL))
                            .pb(rpx(SPACE_2XL))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(rpx(SPACE_LG))
                                    .child(dots)
                                    .child(format!(
                                        "{} / {}",
                                        self.index + 1,
                                        self.release.slides.len()
                                    )),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap(rpx(SPACE_SM))
                                    .when(self.index > 0, |row| {
                                        row.child(
                                            button("highlights-back", "Back", window, cx).on_click(
                                                cx.listener(|this, _, _, cx| this.back(cx)),
                                            ),
                                        )
                                    })
                                    .child(
                                        button(
                                            "highlights-next",
                                            if self.index + 1 == self.release.slides.len() {
                                                "Done"
                                            } else {
                                                "Next"
                                            },
                                            window,
                                            cx,
                                        )
                                        .on_click(
                                            cx.listener(|this, _, window, cx| {
                                                this.next(window, cx);
                                            }),
                                        ),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grove_core::release_highlights::{HighlightMedia, HighlightSlide};
    fn release() -> ReleaseHighlights {
        ReleaseHighlights {
            version: "1.0.2".into(),
            enabled: false,
            title: "What's new".into(),
            slides: (0..3)
                .map(|index| HighlightSlide {
                    id: format!("slide-{index}"),
                    title: format!("Feature {index}"),
                    description: "A user-visible improvement.".into(),
                    media: HighlightMedia {
                        kind: MediaKind::Image,
                        src: "highlights/missing.png".into(),
                        alt: "Feature preview".into(),
                        poster: None,
                        captions: None,
                        frame: None,
                    },
                })
                .collect(),
        }
    }
    #[gpui::test]
    fn manual_navigation_stops_at_end_and_restores_focus(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let (carousel, cx) =
            cx.add_window_view(|window, cx| ReleaseCarousel::new(release(), window, cx));
        cx.update(|window, cx| {
            carousel.update(cx, |this, cx| {
                this.back(cx);
                assert_eq!(this.index, 0);
                this.next(window, cx);
                assert_eq!(this.index, 1);
                this.next(window, cx);
                assert_eq!(this.index, 2);
                let original = cx.focus_handle();
                this.return_focus = Some(original.clone());
                this.next(window, cx);
                assert_eq!(this.index, 2);
                assert!(original.is_focused(window));
            });
        });
    }
    #[gpui::test]
    fn dismiss_and_reopen_resets_slide_and_animation(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let (carousel, cx) =
            cx.add_window_view(|window, cx| ReleaseCarousel::new(release(), window, cx));
        cx.update(|window, cx| {
            carousel.update(cx, |this, cx| {
                this.next(window, cx);
                this.playing = true;
                this.close(window, cx);
                *this = ReleaseCarousel::new(release(), window, cx);
                assert_eq!(this.index, 0);
                assert!(!this.playing);
                assert!(this.focus.is_focused(window));
            });
        });
    }
    #[gpui::test]
    fn arrow_navigation_and_tab_stay_inside_modal(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let (carousel, cx) =
            cx.add_window_view(|window, cx| ReleaseCarousel::new(release(), window, cx));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        cx.simulate_keystrokes("right");
        assert_eq!(cx.update(|_, cx| carousel.read(cx).index), 1);
        cx.simulate_keystrokes("left");
        assert_eq!(cx.update(|_, cx| carousel.read(cx).index), 0);
        for _ in 0..10 {
            cx.simulate_keystrokes("tab");
        }
        assert!(cx.update(|window, cx| carousel.read(cx).focus.contains_focused(window, cx)));
        cx.simulate_keystrokes("escape");
    }
    #[gpui::test]
    fn decoded_screenshot_stays_inside_media_and_before_copy(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let mut release = manifest().unwrap().release("1.0.2").unwrap().clone();
        release.slides[0].media.frame = None;
        let bytes = Assets::get(
            release.slides[0]
                .media
                .poster
                .as_ref()
                .unwrap_or(&release.slides[0].media.src),
        )
        .unwrap()
        .data
        .into_owned();
        let image = std::sync::Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Png, bytes));
        let image_for_view = image.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let mut carousel = ReleaseCarousel::new(release, window, cx);
            // TestAppContext has no embedded asset source. Supply the same bundled bytes
            // directly so this regression exercises decoded intrinsic image dimensions.
            carousel.image_source_override = Some(image_for_view.into());
            carousel
        });
        for (width, height) in [(707.0, 629.0), (1280.0, 800.0), (360.0, 480.0)] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(height)));
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
                assert!(
                    image.clone().get_render_image(window, cx).is_some(),
                    "screenshot must be decoded"
                );
            });
            let panel = cx.debug_bounds("release-highlights").unwrap();
            let media = cx.debug_bounds("highlights-media").unwrap();
            let screenshot = cx.debug_bounds("highlights-image").unwrap();
            let copy = cx.debug_bounds("highlights-copy").unwrap();
            let footer = cx.debug_bounds("highlights-footer").unwrap();
            assert!(panel.top() >= gpui::px(0.0) && panel.bottom() <= gpui::px(height));
            assert!(panel.left() >= gpui::px(0.0) && panel.right() <= gpui::px(width));
            assert!(screenshot.top() >= media.top() && screenshot.bottom() <= media.bottom(), "decoded screenshot overflows media at {width}x{height}: {screenshot:?} / {media:?}");
            assert!(screenshot.left() >= media.left() && screenshot.right() <= media.right());
            assert!(
                media.bottom() <= copy.top(),
                "media overlaps copy at {width}x{height}"
            );
            assert!(
                copy.bottom() <= footer.top(),
                "copy overlaps controls at {width}x{height}"
            );
        }
    }

    #[gpui::test]
    fn authored_zoom_keeps_expand_control_visible_without_overlapping_copy(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_component::init);
        let release = manifest().unwrap().release("1.0.2").unwrap().clone();
        let image = std::sync::Arc::new(gpui::Image::from_bytes(
            gpui::ImageFormat::Gif,
            Assets::get(&release.slides[0].media.src)
                .unwrap()
                .data
                .into_owned(),
        ));
        let image_for_view = image.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let mut carousel = ReleaseCarousel::new(release, window, cx);
            carousel.image_source_override = Some(image_for_view.into());
            carousel.playing = true;
            carousel
        });
        for (width, height) in [(707.0, 629.0), (1280.0, 800.0), (360.0, 480.0)] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(height)));
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
                let decoded = image
                    .clone()
                    .get_render_image(window, cx)
                    .expect("authored GIF must decode");
                assert_eq!(decoded.frame_count(), 2);
            });
            let media = cx.debug_bounds("highlights-media").unwrap();
            let screenshot = cx.debug_bounds("highlights-image").unwrap();

            let copy = cx.debug_bounds("highlights-copy").unwrap();
            assert!(
                screenshot.size.width > media.size.width * 2.0,
                "authored zoom must magnify screenshot"
            );
            // Real capture coordinates of the expand and collapse controls. Both must
            // stay visible in the authored crop as the two-state animation alternates.
            for (x, y) in [
                (50.0 / 2560.0, 50.0 / 1602.0),
                (486.0 / 2560.0, 116.0 / 1602.0),
            ] {
                let x = screenshot.left() + screenshot.size.width * x;
                let y = screenshot.top() + screenshot.size.height * y;
                assert!(x >= media.left() && x <= media.right());
                assert!(y >= media.top() && y <= media.bottom());
            }
            assert!(media.bottom() <= copy.top());
        }
    }

    #[test]
    fn authored_manifest_media_are_bundled() {
        let manifest = manifest().expect("bundled manifest must parse");
        for release in manifest.releases {
            for slide in release.slides {
                for path in std::iter::once(&slide.media.src)
                    .chain(slide.media.poster.iter())
                    .chain(slide.media.captions.iter())
                {
                    assert!(
                        Assets::get(path).is_some(),
                        "Missing embedded release media: {path}"
                    );
                }
            }
        }
    }
    #[test]
    fn media_materialization_rejects_external_and_traversal_paths() {
        for source in [
            "https://example.com/demo.mp4",
            "highlights/../demo.mp4",
            "/tmp/demo.mp4",
            "file:///tmp/demo.mp4",
        ] {
            assert!(materialize(source).is_err());
        }
    }
}
