//! Construction and subscriptions for editable and read-only conversations.

use super::{
    composer_placeholder, image_from_url, Action, Cell, CodexSession, CodexView, CodexViewEvent,
    ComposerMenu, HashMap, HashSet, InputEvent, InputState, Rc, TurnHistory,
};
use gpui::AppContext as _;
use gpui::{Context, Entity, Window};

impl CodexView {
    pub(in crate::views) fn child_preview(
        session: Entity<CodexSession>,
        child: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self::new(session, window, cx);
        view.preview_child = Some(child);
        view
    }

    /// Render the original retained transcript with disclosure and Copy only.
    pub fn history(
        session: Entity<CodexSession>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_history_mode(session, true, window, cx)
    }

    pub(super) fn transcript_only(&self) -> bool {
        self.history_readonly || self.preview_child.is_some()
    }

    pub fn new(session: Entity<CodexSession>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_with_history_mode(session, false, window, cx)
    }

    fn new_with_history_mode(
        session: Entity<CodexSession>,
        history_readonly: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let stream_thread = session.read(cx).state.saved.thread_id.clone();
        let draft = session.read(cx).state.saved.draft.clone();
        let images = session.read(cx).state.saved.draft_images.clone();
        let reference = session.read(cx).state.saved.draft_reference.clone();
        let previews = images
            .iter()
            .filter_map(|url| image_from_url(url).map(|image| (url.clone(), image)))
            .collect();
        let placeholder = composer_placeholder(session.read(cx).state.saved.provider);
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .auto_grow(3, 10)
                .accept_image_paste(true)
                .accept_file_drop(true)
                .markdown_auto_continue(true)
                .submit_on_enter(true)
                .placeholder(placeholder)
                .default_value(draft)
        });
        let markdown_decorations = input.update(cx, |input, cx| {
            input.create_decorations_collection(Vec::new(), cx)
        });
        let card_focus = cx.focus_handle();
        let conversation_focus = cx.focus_handle().tab_stop(false);
        let mut subscriptions = vec![
            cx.observe_in(&session, window, |this, _, window, cx| {
                this.validate_stop_confirmation(cx);
                this.flush_dashboard_submission(window, cx);
                this.sync_native_plans(window, cx);
                this.sync_plan_answer(window, cx);
                if this.menu.is_some() && !this.composer_menu_valid(cx) {
                    this.close_menu(cx);
                }
                cx.notify();
            }),
            cx.observe(&input, |this, _, cx| {
                this.validate_picked_tokens(cx);
                this.refresh_files(cx);
                cx.notify();
            }),
            cx.subscribe_in(&input, window, |this, _, event, window, cx| {
                if this.transcript_only() {
                    return;
                }
                if this.dashboard_pending {
                    // The launched first message belongs to the handoff until send or
                    // failure. An opened conversation must not edit that submission.
                    if matches!(event, InputEvent::Change) {
                        let original = this.session.read(cx).state.saved.draft.clone();
                        if this.input.read(cx).value().as_ref() != original {
                            this.input
                                .update(cx, |input, cx| input.set_value(original, window, cx));
                        }
                    }
                    return;
                }
                match event {
                    InputEvent::PressEnter {
                        secondary: true,
                        shift: false,
                    } => this.submit_modified_enter(true, window, cx),
                    InputEvent::PressEnter {
                        secondary: false,
                        shift: false,
                    } => this.submit(window, cx),
                    InputEvent::DropFiles(paths) => {
                        this.attach_paths(paths.clone(), window, cx);
                    }
                    InputEvent::PasteImages(images) => {
                        this.paste_images(images.clone(), window, cx);
                    }
                    InputEvent::Focus => {
                        crate::platform::inline_html::NativeDocument::focus_host(window);
                        this.set_composer_collapsed(false, cx);
                        cx.notify();
                    }
                    InputEvent::Blur => cx.notify(),
                    InputEvent::Change => {
                        this.clear_stop_confirmation(cx);
                        if this.plan_answer.is_some() {
                            this.persist_plan_answer_draft(cx);
                            this.sync_plan_composer_pending(cx);
                            cx.notify();
                            return;
                        }
                        this.set_composer_collapsed(false, cx);
                        let text = this.input.read(cx).value().to_string();
                        if !this.transcript_only()
                            && this.queue_editor.is_none()
                            && !this.edit_request_pending
                            && this.session.read(cx).state.saved.pending_edit.is_none()
                        {
                            this.queue_selected = None;
                            this.session
                                .update(cx, |session, cx| session.send(Action::Draft(text), cx));
                        }
                        this.dismissed_suggestions = None;
                        this.suggestion_index = 0;
                        this.suggestion_scroll.scroll_to_item(0);
                        this.validate_picked_tokens(cx);
                        this.refresh_files(cx);
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => {}
                }
            }),
        ];
        let scroll = gpui::ScrollHandle::new();
        let html_scroll = scroll.clone();
        let cwd = std::path::PathBuf::from(&session.read(cx).state.saved.cwd);
        let file_browser =
            cx.new(|cx| super::super::file_browser::FileBrowser::new(cwd.clone(), window, cx));
        subscriptions.push(cx.observe(&file_browser, |_, _, cx| cx.notify()));
        subscriptions.push(cx.subscribe(&file_browser, |this, _, event, cx| {
            match event {
                super::super::file_browser::FileBrowserEvent::Opened => {
                    this.close_menu(cx);
                    cx.emit(CodexViewEvent::FileOpened);
                }
                super::super::file_browser::FileBrowserEvent::Closed => {
                    cx.emit(CodexViewEvent::FileClosed);
                }
            }
            cx.notify();
        }));
        let html = cx.new(|_| super::super::inline_html::HtmlSession::new(cwd, html_scroll));
        subscriptions.push(cx.subscribe_in(
            &html,
            window,
            |this, _, event: &super::super::inline_html::OpenLocalFile, window, cx| {
                this.file_browser
                    .update(cx, |browser, cx| browser.open(event.0.clone(), window, cx));
                cx.notify();
            },
        ));
        subscriptions.push(cx.observe(&html, |this, _, cx| {
            this.smooth_follow = false;
            this.follow_after_submit = false;
            this.composer_pin_bottom = false;
            cx.notify();
        }));
        subscriptions.push(cx.on_focus_out(&card_focus, window, |this, _, _, cx| {
            if this.menu == Some(ComposerMenu::Settings) {
                this.close_menu(cx);
            }
        }));
        subscriptions.push(
            cx.on_focus_out(&conversation_focus, window, |this, _, _, cx| {
                this.clear_stop_confirmation(cx);
            }),
        );
        let next_reply_seen =
            grove_core::next_reply::Request::capture(&session.read(cx).state).map(|r| r.key);
        let composer_rail_scroll = gpui::ScrollHandle::new();
        let mut view = Self {
            preview_child: None,
            history_readonly,
            session,
            html,
            file_browser,
            plan_answer: None,
            native_plans: HashMap::new(),
            description_scope: None,
            review_scope: None,
            markdown_decorations,
            markdown_source: None,
            next_reply_seen,
            next_reply_request: None,
            next_reply: grove_core::next_reply::Candidate::pending(),
            next_reply_control: None,
            next_reply_task: None,
            next_reply_focus: std::array::from_fn(|_| cx.focus_handle()),
            next_reply_generate: grove_core::next_reply::Request::generate,
            input,
            reference,
            submitted_reference: None,
            reference_expanded: false,
            reference_disclosure_focus: cx.focus_handle(),
            reference_remove_focus: cx.focus_handle(),
            selection_views: HashMap::new(),
            selection_subscriptions: HashMap::new(),
            focus_subscriptions: HashMap::new(),
            focus_revision: None,
            focus_thread: None,
            focus_sources: HashMap::new(),
            message_slots: rustc_hash::FxHashMap::default(),
            tool_slots: rustc_hash::FxHashMap::default(),
            work_header_slots: rustc_hash::FxHashMap::default(),
            selection_dismissed: None,
            selection_root_observed: false,
            selection_reply_focus: cx.focus_handle(),
            submitted_images: Vec::new(),
            transcript_bounds: Rc::default(),
            composer_rail_scroll: composer_rail_scroll.clone(),
            expanded: HashSet::new(),
            agents_in_checkout: false,
            work_tick_pending: Rc::new(Cell::new(false)),
            question_drafts: HashMap::new(),
            question_receipts: Vec::new(),
            question_generation: 0,
            approval_drafts: HashMap::new(),
            elicitation_drafts: HashMap::new(),
            activity_request_focus: None,
            agent_controls_scroll: gpui::ScrollHandle::new(),
            agent_controls_anchor: gpui::ScrollAnchor::for_handle(composer_rail_scroll.clone()),
            interaction_scope: String::new(),
            menu: None,
            popup_motion: super::super::motion::PopupMotion::default(),
            closing_popup: None,
            popup_generation: 0,
            popup: None,
            popup_subscription: None,
            composer_menu_scope: None,
            composer_menu_launch_config: None,
            model_from_card: false,
            model_picker: None,
            effort_drag: None,
            effort_visual: None,
            effort_pending: None,
            effort_bounds: Rc::default(),
            card_model_focus: cx.focus_handle(),
            card_focus,
            card_reset_focus: cx.focus_handle(),
            speed_focus: std::array::from_fn(|_| cx.focus_handle()),
            option_bounds: std::array::from_fn(|_| Rc::default()),
            option_focus: std::array::from_fn(|_| cx.focus_handle()),
            file_suggestions: Vec::new(),
            picked_tokens: Vec::new(),
            suggestion_index: 0,
            suggestion_scroll: gpui::ScrollHandle::new(),
            file_query: String::new(),
            file_lookup: None,
            dismissed_suggestions: None,
            submitted: None,
            submitted_ack: None,
            pending_clear: None,
            message_action_bounds: HashMap::new(),
            edit_confirmation: None,
            edit_request_pending: false,
            rewind_attempt: None,
            edited_messages: 0,
            edit_cancel_focus: cx.focus_handle(),
            edit_confirm_focus: cx.focus_handle(),
            edit_keep_focus: cx.focus_handle(),
            queue_resume_focus: cx.focus_handle().tab_stop(true),
            queue_selected: None,
            queue_collapsed: false,
            queue_editor: None,
            queue_motion_rows: Vec::new(),
            bubble_entrances: Vec::new(),
            animating_bubbles: HashMap::new(),
            interrupt_requested_for_turn: None,
            stop_confirmation: None,
            stop_confirmation_timer: None,
            conversation_focus,
            images,
            previews,
            transcript_previews: rustc_hash::FxHashMap::default(),
            pending_image_pastes: 0,
            pending_attachment_picker: false,
            scroll,
            preview_follow_latest: true,
            last_content: None,
            smooth_follow: false,
            last_follow_offset: None,
            last_follow_max: gpui::Pixels::ZERO,
            motion_frame: None,
            motion_frame_pending: Rc::default(),
            stream_text: HashMap::new(),
            message_hover: HashSet::new(),
            message_focus: rustc_hash::FxHashMap::default(),
            copied_message: None,
            copied_code_blocks: HashMap::new(),
            snippets: HashMap::new(),
            preview_snapshot: None,
            copy_generation: 0,
            timestamp_tick_pending: Rc::new(Cell::new(false)),
            stream_thread,
            follow_after_submit: true,
            composer_collapsed: false,
            pending_composer_scroll: None,
            composer_pin_bottom: false,
            composer_body_height: Rc::default(),
            composer_body_target: Rc::default(),
            composer_expanded_height: Rc::default(),
            composer_transition_from: None,
            composer_transition_generation: 0,
            composer_motion_finished: Rc::new(Cell::new(true)),
            tasks_expanded: true,
            status_card_keys: HashSet::new(),
            task_cache: None,
            transcript_cache: None,
            minimap_view: None,
            snippet_sources: std::cell::RefCell::default(),
            turn_history: TurnHistory::default(),
            history_earlier_focus: cx.focus_handle().tab_stop(true).tab_index(0),
            history_latest_focus: cx.focus_handle().tab_stop(true).tab_index(0),
            pending_history_jump: None,
            dashboard_tray: None,
            dashboard_pending: false,
            dashboard_handoffs: Vec::new(),
            dashboard_status_expanded: false,
            dashboard_status_signature: Vec::new(),
            dashboard_more_focus: cx.focus_handle(),
            dashboard_tray_observer: None,
            dashboard_retry_focus: std::array::from_fn(|_| cx.focus_handle()),
            script_runtime: None,
            checkout_context: None,
            subscriptions,
        };
        view.subscriptions
            .push(cx.observe(&view.session, |view, _, cx| view.reconcile_snippets(cx)));
        view.subscriptions.push(cx.on_app_quit(|view, cx| {
            for snippet in view.snippets.values() {
                for run in &snippet.runs {
                    run.terminal.update(
                        cx,
                        crate::entities::terminal_session::TerminalSession::stop_script,
                    );
                }
            }
            async {}
        }));
        view.sync_native_plans(window, cx);
        view
    }
    pub(crate) fn set_script_runtime(
        &mut self,
        runtime: Entity<crate::runtime::Runtime>,
        _cx: &mut Context<Self>,
    ) {
        self.script_runtime = Some(runtime);
    }

    pub(crate) fn set_checkout_context(
        &mut self,
        location: String,
        branch: String,
        cx: &mut Context<Self>,
    ) {
        let context = Some((location, branch));
        if self.checkout_context != context {
            self.checkout_context = context;
            cx.notify();
        }
    }
}
