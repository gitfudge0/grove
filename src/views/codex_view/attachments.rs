//! Composer attachment loading and image token synchronization.

use super::*;

impl CodexView {
    pub(super) fn attach_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dashboard_pending
            || self.plan_answer.is_some()
            || self.submitted.is_some()
            || self.pending_attachment_picker
            || self.edit_request_pending
            || self.session.read(cx).state.saved.pending_edit.is_some()
        {
            return;
        }
        self.pending_attachment_picker = true;
        cx.notify();
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some("Attach files or folders".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                let _ = this.update_in(cx, |this, _, cx| {
                    this.pending_attachment_picker = false;
                    cx.notify();
                });
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.pending_attachment_picker = false;
                this.attach_paths(paths, window, cx);
            });
        })
        .detach();
    }

    /// Picker and native drops share image preparation and draft ownership locks.
    pub(super) fn attach_paths(
        &mut self,
        paths: Vec<std::path::PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dashboard_pending
            || self.plan_answer.is_some()
            || paths.is_empty()
            || self.submitted.is_some()
            || self.pending_attachment_picker
            || self.edit_request_pending
            || self.edit_confirmation.is_some()
            || self.session.read(cx).state.saved.pending_edit.is_some()
            || self
                .queue_editor
                .as_ref()
                .is_some_and(|editor| !editor.loaded || editor.saving)
        {
            return;
        }
        self.set_composer_collapsed(false, cx);
        self.pending_attachment_picker = true;
        let editor = self.queue_editor.as_ref().map(|editor| editor.id);
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let attachments = cx
                .background_executor()
                .spawn(async move {
                    let mut references = Vec::new();
                    let mut images = Vec::new();
                    for path in paths {
                        let format = path
                            .extension()
                            .and_then(|extension| extension.to_str())
                            .and_then(|extension| match extension.to_lowercase().as_str() {
                                "png" => Some(gpui::ImageFormat::Png),
                                "jpg" | "jpeg" => Some(gpui::ImageFormat::Jpeg),
                                "webp" => Some(gpui::ImageFormat::Webp),
                                "gif" => Some(gpui::ImageFormat::Gif),
                                "tiff" | "tif" => Some(gpui::ImageFormat::Tiff),
                                "bmp" => Some(gpui::ImageFormat::Bmp),
                                _ => None,
                            });
                        if let Some(format) = format {
                            if let Ok(bytes) = fs_err::read(&path) {
                                images.push(gpui::Image::from_bytes(format, bytes));
                                continue;
                            }
                        }
                        references.push(format!("@{}", path.display()));
                    }
                    (references.join("\n"), images)
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.pending_attachment_picker = false;
                if this.submitted.is_some()
                    || this.edit_request_pending
                    || this.session.read(cx).state.saved.pending_edit.is_some()
                    || this.queue_editor.as_ref().map(|editor| editor.id) != editor
                {
                    return;
                }
                let (references, images) = attachments;
                if !images.is_empty() {
                    this.paste_images(images, window, cx);
                }
                if !references.is_empty() {
                    this.input.update(cx, |input, cx| {
                        input.insert(format!(" {references} "), window, cx);
                    });
                }
                let text = this.input.read(cx).value().to_string();
                if this.queue_editor.is_none() {
                    this.session
                        .update(cx, |session, cx| session.send(Action::Draft(text), cx));
                }
                this.input.focus_handle(cx).focus(window, cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn restore_image_tokens(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.validate_picked_tokens(cx);
        let text = self.input.read(cx).value();
        for number in 1..=self.images.len() {
            let value = format!("Image {number}");
            for (start, _) in text.match_indices(&value) {
                let range = start..start + value.len();
                if text[..range.start]
                    .chars()
                    .next_back()
                    .is_some_and(|c| !c.is_whitespace())
                    || text[range.end..]
                        .chars()
                        .next()
                        .is_some_and(|c| !c.is_whitespace())
                    || self.picked_tokens.iter().any(|token| {
                        token.value == value && token.decoration.get_ranges(cx).contains(&range)
                    })
                {
                    continue;
                }
                let decoration = self.input.update(cx, |input, cx| {
                    input.create_decorations_collection(
                        vec![TextDecoration::new(
                            range,
                            gpui::HighlightStyle {
                                background_color: Some(c::BG_HL()),
                                underline: Some(gpui::UnderlineStyle {
                                    thickness: rpx(COMPOSER_TOKEN_UNDERLINE)
                                        .to_pixels(window.rem_size()),
                                    color: Some(c::BORDER_STRONG()),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            },
                        )],
                        cx,
                    )
                });
                self.picked_tokens.push(ComposerToken {
                    value: value.clone(),
                    decoration,
                });
            }
        }
    }

    pub(super) fn remove_image(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dashboard_pending {
            return;
        }
        let mut edits = Vec::new();
        for token in &self.picked_tokens {
            if let Some(number) = token
                .value
                .strip_prefix("Image ")
                .and_then(|n| n.parse::<usize>().ok())
            {
                if number > index {
                    if let Some(range) = token.decoration.get_ranges(cx).first().cloned() {
                        edits.push((
                            range,
                            if number == index + 1 {
                                String::new()
                            } else {
                                format!("Image {}", number - 1)
                            },
                        ));
                    }
                }
            }
        }
        edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
        self.input.update(cx, |input, cx| {
            for (range, replacement) in edits {
                input.set_selected_range(range, cx);
                input.replace(replacement, window, cx);
            }
        });
        self.images.remove(index);
        self.restore_image_tokens(window, cx);
        let text = self.input.read(cx).value().to_string();
        if self.queue_editor.is_none() {
            self.session
                .update(cx, |session, cx| session.send(Action::Draft(text), cx));
        }
        self.persist_images(cx);
    }

    fn persist_images(&mut self, cx: &mut Context<Self>) {
        if self.queue_editor.is_some() {
            cx.notify();
            return;
        }
        self.session.update(cx, |session, cx| {
            session.send(Action::DraftImages(self.images.clone()), cx);
        });
        cx.notify();
    }

    pub(super) fn paste_images(
        &mut self,
        images: Vec<gpui::Image>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dashboard_pending || self.plan_answer.is_some() || self.submitted.is_some() {
            return;
        }
        let editor = self.queue_editor.as_ref().map(|editor| editor.id);
        let cursor = self.input.read(cx).cursor();
        let original = self.input.read(cx).value();
        let previous = original[..cursor]
            .char_indices()
            .next_back()
            .map(|(offset, _)| offset);
        let anchor = previous.map(|start| {
            self.input.update(cx, |input, cx| {
                input.create_decorations_collection(
                    vec![TextDecoration::new(
                        start..cursor,
                        gpui::HighlightStyle::default(),
                    )],
                    cx,
                )
            })
        });
        self.pending_image_pastes += 1;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { prepare_images(images) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.pending_image_pastes -= 1;
                if this.queue_editor.as_ref().map(|editor| editor.id) != editor {
                    if let Some(anchor) = &anchor {
                        anchor.clear(cx);
                    }
                    cx.notify();
                    return;
                }
                match result {
                    Ok(images) => {
                        let size = this.images.iter().map(String::len).sum::<usize>()
                            + images.iter().map(|(url, _)| url.len()).sum::<usize>();
                        if this.images.len() + images.len() > 10 || size > 20 * 1024 * 1024 {
                            this.session.update(cx, |session, cx| {
                                session.state.error =
                                    Some("Attach up to 10 images, totaling at most 20 MiB.".into());
                                cx.notify();
                            });
                        } else {
                            let selection = this.input.read(cx).selected_range();
                            let insertion = anchor
                                .as_ref()
                                .and_then(|anchor| {
                                    anchor.get_ranges(cx).first().map(|range| range.end)
                                })
                                .unwrap_or(0)
                                .min(this.input.read(cx).value().len());
                            this.input.update(cx, |input, cx| {
                                input.set_selected_range(insertion..insertion, cx);
                            });
                            let mut inserted = 0;
                            for (url, image) in images {
                                let value = format!("Image {}", this.images.len() + 1);
                                let start = this.input.read(cx).cursor();
                                let prefix = if start > 0
                                    && !this.input.read(cx).value()[..start]
                                        .ends_with(char::is_whitespace)
                                {
                                    " "
                                } else {
                                    ""
                                };
                                this.input.update(cx, |input, cx| {
                                    input.insert(format!("{prefix}{value} "), window, cx);
                                });
                                let start = start + prefix.len();
                                let decoration = this.input.update(cx, |input, cx| {
                                    input.create_decorations_collection(
                                        vec![TextDecoration::new(
                                            start..start + value.len(),
                                            gpui::HighlightStyle {
                                                background_color: Some(c::BG_HL()),
                                                underline: Some(gpui::UnderlineStyle {
                                                    thickness: rpx(COMPOSER_TOKEN_UNDERLINE)
                                                        .to_pixels(window.rem_size()),
                                                    color: Some(c::BORDER_STRONG()),
                                                    ..Default::default()
                                                }),
                                                ..Default::default()
                                            },
                                        )],
                                        cx,
                                    )
                                });
                                inserted += prefix.len() + value.len() + 1;
                                this.picked_tokens.push(ComposerToken { value, decoration });
                                this.previews.insert(url.clone(), image);
                                this.images.push(url);
                            }
                            if selection != (cursor..cursor) {
                                let shift = |offset| {
                                    if offset >= insertion {
                                        offset + inserted
                                    } else {
                                        offset
                                    }
                                };
                                this.input.update(cx, |input, cx| {
                                    input.set_selected_range(
                                        shift(selection.start)..shift(selection.end),
                                        cx,
                                    );
                                });
                            }
                            let text = this.input.read(cx).value().to_string();
                            if this.queue_editor.is_none() {
                                this.session.update(cx, |session, cx| {
                                    session.send(Action::Draft(text), cx);
                                });
                            }
                            this.persist_images(cx);
                        }
                    }
                    Err(error) => this.session.update(cx, |session, cx| {
                        session.state.error = Some(error);
                        cx.notify();
                    }),
                }
                if let Some(anchor) = &anchor {
                    anchor.clear(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }
}

fn prepare_images(images: Vec<gpui::Image>) -> Result<Vec<(String, Arc<gpui::Image>)>, String> {
    use std::io::Cursor;
    if images.len() > 10
        || images
            .iter()
            .map(|image| image.bytes().len())
            .sum::<usize>()
            > 20 * 1024 * 1024
    {
        return Err("Attach up to 10 images, totaling at most 20 MiB.".into());
    }
    images
        .into_iter()
        .map(|image| {
            let format = match image.format {
                gpui::ImageFormat::Png => image::ImageFormat::Png,
                gpui::ImageFormat::Jpeg => image::ImageFormat::Jpeg,
                gpui::ImageFormat::Webp => image::ImageFormat::WebP,
                gpui::ImageFormat::Gif => image::ImageFormat::Gif,
                gpui::ImageFormat::Tiff => image::ImageFormat::Tiff,
                gpui::ImageFormat::Bmp => image::ImageFormat::Bmp,
                _ => return Err("Paste a PNG, JPEG, WebP, GIF, TIFF, or BMP image.".into()),
            };
            let mut reader = image::ImageReader::with_format(Cursor::new(image.bytes()), format);
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(8192);
            limits.max_image_height = Some(8192);
            limits.max_alloc = Some(128 * 1024 * 1024);
            reader.limits(limits);
            let decoded = reader
                .decode()
                .map_err(|error| format!("Couldn't read pasted image: {error}"))?;
            let image = if matches!(
                image.format,
                gpui::ImageFormat::Tiff | gpui::ImageFormat::Bmp
            ) {
                let mut bytes = Cursor::new(Vec::new());
                decoded
                    .write_to(&mut bytes, image::ImageFormat::Png)
                    .map_err(|error| format!("Couldn't prepare pasted image: {error}"))?;
                gpui::Image::from_bytes(gpui::ImageFormat::Png, bytes.into_inner())
            } else {
                image
            };
            let url = format!(
                "data:{};base64,{}",
                image.format.mime_type(),
                STANDARD.encode(image.bytes())
            );
            Ok((url, Arc::new(image)))
        })
        .collect()
}

pub(super) fn image_from_url(url: &str) -> Option<Arc<gpui::Image>> {
    let (mime, encoded) = url.strip_prefix("data:")?.split_once(";base64,")?;
    let format = gpui::ImageFormat::from_mime_type(mime)?;
    Some(Arc::new(gpui::Image::from_bytes(
        format,
        STANDARD.decode(encoded).ok()?,
    )))
}
