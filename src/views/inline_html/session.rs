//! Session-scoped HTML ownership, Markdown links, and automatic opening.

use super::*;

impl HtmlSession {
    #[cfg(test)]
    pub(in crate::views) fn artifact_count(&self) -> usize {
        self.artifacts.len()
    }
    #[cfg(test)]
    pub(in crate::views) fn opened_count(&self) -> usize {
        self.opened.values().map(Vec::len).sum()
    }
    #[cfg(test)]
    pub(in crate::views) fn artifact_ids(&self) -> HashSet<EntityId> {
        self.artifacts.values().map(Entity::entity_id).collect()
    }
    #[cfg(test)]
    pub(in crate::views) fn artifact_owners_match(&self, owner: EntityId, cx: &App) -> bool {
        self.artifacts
            .values()
            .all(|artifact| artifact.read(cx).owner == owner)
    }
    pub(in crate::views) fn new(cwd: PathBuf, scroll: ScrollHandle) -> Self {
        Self {
            legacy: true,
            artifacts: HashMap::new(),
            extensions: HashMap::new(),
            file_focus: HashMap::new(),
            browser_errors: HashMap::new(),
            opened: HashMap::new(),
            cwd,
            roots: Vec::new(),
            scroll,
            paused: Rc::default(),
            auto: AutoOpen::default(),
            identity: None,
        }
    }
    pub(in crate::views) fn sync(
        &mut self,
        state: &grove_core::codex::Snapshot,
        cwd: PathBuf,
        paused: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let identity = (state.saved.thread_id.clone(), state.cleared_conversations);
        if self
            .identity
            .as_ref()
            .is_some_and(|previous| previous != &identity)
        {
            self.artifacts.clear();
            self.extensions.clear();
            self.file_focus.clear();
            self.browser_errors.clear();
            self.opened.clear();
            self.auto = AutoOpen::default();
            if cx
                .try_global::<HtmlUi>()
                .is_some_and(|ui| ui.owner == Some(cx.entity_id()))
            {
                let ui = cx.global_mut::<HtmlUi>();
                ui.expanded = None;
                ui.owner = None;
                NativeDocument::hide_all();
                cx.refresh_windows();
            }
        }
        let legacy = state.saved.html_policy == grove_core::visuals::HtmlPolicy::Legacy;
        if self.legacy != legacy {
            self.extensions.clear();
            self.legacy = legacy;
        }
        self.identity = Some(identity);
        self.cwd = cwd;
        self.roots = state
            .saved
            .roots
            .iter()
            .map(|root| PathBuf::from(&root.wt_path))
            .collect();
        if self.paused.replace(paused) != paused {
            for artifact in self.artifacts.values() {
                artifact.update(cx, |_, cx| cx.notify());
            }
        }
        if !self.legacy {
            return;
        }
        let final_message = state
            .saved
            .items
            .iter()
            .rev()
            .find(|item| item["type"] == "agentMessage");
        let final_text = final_message.map(|item| grove_core::codex::item_text(item));
        let final_item = final_message
            .and_then(|item| item["id"].as_str())
            .zip(final_text.as_deref());
        if let Some((id, text)) = self.auto.observe(
            state.status.busy(),
            state.status == grove_core::codex::Status::Done,
            state.turn_id.as_deref(),
            final_item,
        ) {
            if cx
                .try_global::<HtmlUi>()
                .is_none_or(|ui| ui.expanded.is_none())
            {
                if let Some(target) = output_html_link(&text, &self.cwd) {
                    self.open_link(&id, &target, window, cx);
                }
            }
        }
    }
    pub(in crate::views) fn extensions(
        &mut self,
        message: &str,
        cx: &mut Context<Self>,
    ) -> MarkdownExtensions {
        if let Some(extensions) = self.extensions.get(message) {
            return extensions.clone();
        }
        let session = cx.entity().downgrade();
        let owner = message.to_owned();
        let links = session.clone();
        let link_owner = owner.clone();
        let legacy = self.legacy;
        let extensions = super::super::mermaid::extensions()
            .block_parser(move |node, cx| if legacy { parse(node, cx) } else { None })
            .block_renderer("grove-html", move |node, window, cx| {
                let Some(block) = node.data::<DocumentBlock>() else {
                    return div().into_any_element();
                };
                session
                    .update(cx, |session, cx| {
                        let key = format!("fence:{owner}:{}", block.offset);
                        let session_owner = cx.entity().downgrade();
                        let artifact = session
                            .artifacts
                            .entry(key)
                            .or_insert_with(|| {
                                cx.new(|cx| {
                                    HtmlArtifact::new(
                                        block.source.clone(),
                                        block.markdown.clone(),
                                        None,
                                        session_owner,
                                        session.paused.clone(),
                                        session.scroll.clone(),
                                        cx,
                                    )
                                })
                            })
                            .clone();
                        artifact.update(cx, |artifact, cx| {
                            if artifact
                                .version
                                .offer(block.source.clone(), block.markdown.clone())
                            {
                                cx.notify();
                            }
                        });
                        inline_card(artifact, window, cx)
                    })
                    .unwrap_or_else(|_| source_view(&block.markdown, block.offset, window))
            })
            .link_renderer(move |target, label, _window, cx| {
                links
                    .update(cx, |session, cx| {
                        let file = super::super::file_browser::local_target(target, &session.cwd)?;
                        let browser = browser_link_label(label)
                            && crate::platform::inline_html::browser_supported_path(&file.path);
                        let additional_browser = !browser
                            && file
                                .path
                                .extension()
                                .and_then(|ext| ext.to_str())
                                .is_some_and(|ext| {
                                    ext.eq_ignore_ascii_case("html")
                                        || ext.eq_ignore_ascii_case("htm")
                                });
                        let error = session
                            .browser_errors
                            .get(&(link_owner.clone(), target.to_owned()))
                            .cloned();
                        let primary = session.file_link_control(
                            &link_owner,
                            target,
                            label,
                            file.clone(),
                            browser,
                            cx,
                        );
                        if !additional_browser && error.is_none() {
                            return Some(primary);
                        }
                        Some(
                            div()
                                .flex()
                                .flex_col()
                                .min_w_0()
                                .max_w_full()
                                .child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .items_center()
                                        .gap(rpx(SPACE_SM))
                                        .child(primary)
                                        .when(additional_browser, |row| {
                                            row.child(session.file_link_control(
                                                &link_owner,
                                                target,
                                                "Open in browser",
                                                file,
                                                true,
                                                cx,
                                            ))
                                        }),
                                )
                                .when_some(error, |row, error| {
                                    row.child(
                                        div()
                                            .debug_selector(|| "transcript-browser-error".into())
                                            .text_size(rpx(TEXT_SMALL))
                                            .text_color(c::RED())
                                            .child(error),
                                    )
                                })
                                .into_any_element(),
                        )
                    })
                    .ok()
                    .flatten()
            })
            .link_renderer_filter(|target| {
                !(target.starts_with("http://") || target.starts_with("https://"))
            });

        self.extensions
            .insert(message.to_owned(), extensions.clone());
        extensions
    }

    /// Open an explicitly selected immutable plan asset in the existing sandbox.
    pub(in crate::views) fn open_snapshot(
        &mut self,
        key: String,
        source: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let owner = cx.entity().downgrade();
        let artifact = self
            .artifacts
            .entry(key)
            .or_insert_with(|| {
                cx.new(|cx| {
                    HtmlArtifact::new(
                        source,
                        String::new(),
                        None,
                        owner,
                        self.paused.clone(),
                        self.scroll.clone(),
                        cx,
                    )
                })
            })
            .clone();
        let return_focus = window.focused(cx);
        // Expansion notifies sibling artifacts through their owning session.
        // Release this session update before that callback can borrow it again.
        window.defer(cx, move |window, cx| {
            artifact.update(cx, |artifact, cx| {
                artifact.external_return_focus = return_focus;
                if !expanded(&cx.entity(), cx) {
                    artifact.toggle_expand(window, cx);
                }
            });
        });
        cx.notify();
    }

    fn file_link_control(
        &mut self,
        message: &str,
        target: &str,
        label: &str,
        file: super::super::file_browser::FileTarget,
        browser: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let owner = cx.entity().downgrade();
        let keyboard = owner.clone();
        let keyboard_file = file.clone();
        let error_key = (message.to_owned(), target.to_owned());
        let keyboard_error_key = error_key.clone();
        let action = format!("local-file-{browser}-{target}");
        let focus = self
            .file_focus
            .entry((message.to_owned(), action.clone()))
            .or_insert_with(|| cx.focus_handle())
            .clone();
        let mouse_focus = focus.clone();
        super::super::components::inline_disclosure(SharedString::from(action), label.to_owned())
            .debug_selector(move || {
                if browser {
                    "transcript-browser-link"
                } else {
                    "transcript-file-link"
                }
                .into()
            })
            .track_focus(&focus)
            .tab_index(0)
            .tab_stop(true)
            .role(gpui::Role::Link)
            .focus_visible(|s| s.bg(c::BG_HOVER()))
            .w_auto()
            .h_auto()
            .max_w_full()
            .min_w_0()
            .p_0()
            .text_color(c::STATUS_INFO())
            .items_center()
            .justify_start()
            .gap(rpx(SPACE_SM))
            .when(browser, |link| {
                link.child(crate::icons::icon(
                    "external-link",
                    ICON_SM,
                    c::STATUS_INFO(),
                ))
            })
            .child(div().min_w_0().whitespace_normal().child(label.to_owned()))
            .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                mouse_focus.focus(window, cx);
            })
            .on_click(move |_, _, cx| {
                let _ = owner.update(cx, |session, cx| {
                    session.activate_file_link(file.clone(), browser, error_key.clone(), cx);
                });
                cx.stop_propagation();
            })
            .on_key_down(move |event: &gpui::KeyDownEvent, window, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") && !event.is_held {
                    let _ = keyboard.update(cx, |session, cx| {
                        session.activate_file_link(
                            keyboard_file.clone(),
                            browser,
                            keyboard_error_key.clone(),
                            cx,
                        );
                    });
                    window.prevent_default();
                    cx.stop_propagation();
                }
            })
            .into_any_element()
    }

    fn activate_file_link(
        &mut self,
        file: super::super::file_browser::FileTarget,
        browser: bool,
        error_key: (String, String),
        cx: &mut Context<Self>,
    ) {
        if browser {
            let path = file.path;
            self.browser_errors.remove(&error_key);
            let identity = self.identity.clone();
            let check_path = path.clone();
            // Missing targets must report a failure even if the OS launcher would
            // accept the URL. Disk work stays off the foreground/paint thread.
            let check = cx.background_executor().spawn(async move {
                match fs_err::metadata(&check_path) {
                    Ok(metadata) if metadata.is_file() => Ok(()),
                    Ok(_) => Err("The browser target must be a regular file".to_owned()),
                    Err(error) => Err(format!("File unavailable: {error}")),
                }
            });
            cx.spawn(async move |this, cx| {
                let result = match check.await {
                    Ok(()) => match gpui::http_client::Url::from_file_path(&path) {
                        Ok(url) => {
                            crate::platform::inline_html::open_in_browser(url.to_string()).await
                        }
                        Err(()) => Err("Invalid document path".to_owned()),
                    },
                    Err(error) => Err(error),
                };
                let _ = this.update(cx, |session, cx| {
                    if session.identity != identity {
                        return;
                    }
                    if let Err(error) = result {
                        session
                            .browser_errors
                            .insert(error_key, format!("Couldn’t open in browser: {error}"));
                    }
                    cx.notify();
                });
            })
            .detach();
            cx.notify();
        } else {
            cx.emit(OpenLocalFile(file));
        }
    }

    pub(super) fn open_link(
        &mut self,
        message: &str,
        target: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = local_html_path(target, &self.cwd) else {
            return;
        };
        // A reference in a later reply owns a new source snapshot and native view.
        // Reopening this reply must never reread a successfully captured file.
        let key = format!("link:{message}:{}", path.display());
        let owner = cx.entity().downgrade();
        self.artifacts.entry(key.clone()).or_insert_with(|| {
            cx.new(|cx| {
                HtmlArtifact::new(
                    String::new(),
                    String::new(),
                    Some(path),
                    owner,
                    self.paused.clone(),
                    self.scroll.clone(),
                    cx,
                )
            })
        });
        if let Some(artifact) = self.artifacts.get(&key) {
            artifact.update(cx, |artifact, cx| {
                artifact.reveal = true;
                cx.notify();
            });
        }
        let opened = self.opened.entry(message.to_owned()).or_default();
        if !opened.contains(&key) {
            opened.push(key);
        }
        cx.notify();
    }
    pub(in crate::views) fn published(
        &mut self,
        reference: &grove_core::visuals::Reference,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = format!("published:{}:{}", reference.scope, reference.publication_id);
        let owner = cx.entity().downgrade();
        let artifact = self
            .artifacts
            .entry(key)
            .or_insert_with(|| {
                cx.new(|cx| {
                    let mut artifact = HtmlArtifact::new(
                        String::new(),
                        String::new(),
                        None,
                        owner,
                        self.paused.clone(),
                        self.scroll.clone(),
                        cx,
                    );
                    artifact.publication = Some(reference.clone());
                    artifact.read_publication(cx);
                    artifact
                })
            })
            .clone();
        inline_card(artifact, window, cx)
    }
    pub(in crate::views) fn has_linked(&self, message: &str) -> bool {
        self.opened
            .get(message)
            .is_some_and(|cards| !cards.is_empty())
    }

    pub(in crate::views) fn linked(
        &self,
        message: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<AnyElement> {
        self.opened
            .get(message)
            .into_iter()
            .flatten()
            .filter_map(|key| self.artifacts.get(key))
            .map(|artifact| inline_card(artifact.clone(), window, cx))
            .collect()
    }
}

pub(super) fn browser_link_label(label: &str) -> bool {
    label.trim().to_ascii_lowercase().contains("in browser")
}

pub(super) fn output_html_link(text: &str, cwd: &Path) -> Option<String> {
    fn find(node: &markdown_ast::Node, cwd: &Path, links: &mut Vec<String>) {
        if let markdown_ast::Node::Link(link) = node {
            let label = link
                .children
                .iter()
                .map(markdown_ast::Node::to_string)
                .collect::<String>();
            if !browser_link_label(&label) && local_html_path(&link.url, cwd).is_some() {
                links.push(link.url.clone());
            }
        }
        if let Some(children) = node.children() {
            for child in children {
                find(child, cwd, links);
            }
        }
    }
    let root = markdown::to_mdast(text, &markdown::ParseOptions::gfm()).ok()?;
    let mut links = Vec::new();
    find(&root, cwd, &mut links);
    links.pop()
}
