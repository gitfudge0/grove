//! `Modal::ThemeManager`'s EDITOR sub-view: the paste-first + grouped
//! 11-swatch theme editor (`Grove::theme_manager_editor`). The LIST sub-view
//! (row selection, rename, duplicate, delete) stays in `update.rs`/`view.rs`,
//! backed by `App::Modal::ThemeManager`'s own fields — out of scope here.

use super::metrics::MONO_FONT;
use super::palette as c;
use super::session_launcher::theme_editor_scroll_offset;
use super::state::Msg as GMsg;
use super::update::global_mods;
use super::view::{footer_mod_hint, input_field_style, theme_manager_scrollable_id};
use super::widgets::{
    divider_h, footer_container, footer_hint, ghost_scrollable, launcher_row, modal_action,
    modal_action_sized, modal_footer_hints, modal_header, modal_header_row, modal_panel,
    section_header, seg_button, ModalBtn, SegSide,
};
use crate::app::{App, Modal};
use crate::gui::state::ThemeManagerMsg;
use iced::border::Radius;
use iced::keyboard::{key::Named, Key, Modifiers};
use iced::widget::{button, column, container, row, text, text_input, Column, Id, Space};
use iced::{Background, Border, Color, Element, Length, Padding, Task};

/// Live state for `Modal::ThemeManager`'s EDITOR sub-view — same idiom as
/// `ScriptsEditorState`: holds the paste box's `text_editor::Content` (which
/// must persist across frames, so it can't live in the cloneable `Modal`)
/// plus everything else the editor needs. `Some` exactly when the manager
/// modal is showing the editor rather than the list; `None` renders the list.
pub struct ThemeManagerEditorState {
    /// Name of the `CUSTOM` entry this editor is bound to — identifies what
    /// `theme::update_custom` persists into on save (a rename here is just a
    /// field on `draft`, resolved by this original name).
    pub original_name: String,
    /// Live-edited copy. Every valid hex/name/kind edit updates this
    /// immediately; applied to the whole app via `theme::set` whenever
    /// `preview_on`.
    pub draft: grove_core::theme::Theme,
    /// Last-saved snapshot: the dirty check (`draft` vs `saved`) and what a
    /// discard reverts the *draft* back to.
    pub saved: grove_core::theme::Theme,
    /// Whole-app active theme when the editor was entered — restored on
    /// discard and whenever `preview_on` is toggled off.
    pub original_active: grove_core::theme::Theme,
    /// Which of the 11 rows (`theme::FIELD_NAMES` order) has the cursor.
    pub selected: usize,
    /// Per-row "currently showing invalid hex" flag.
    pub invalid: [bool; 11],
    /// The selected row's live hex-text edit buffer (its own small
    /// `text_input`, seeded from `draft.field(selected)` on every row change).
    pub hex_buf: String,
    /// Preview toggle (⌘P): true applies `draft` live; false shows
    /// `original_active` again without losing the draft.
    pub preview_on: bool,
    /// "Discard changes…?" confirmation, shown by Esc while dirty.
    pub confirm_discard: bool,
    /// The paste-first box's multiline buffer.
    pub paste: iced::widget::text_editor::Content,
    /// Last Apply outcome, shown inline under the paste box: `Ok(summary)` or
    /// `Err(parser message)`. `None` before the first Apply.
    pub paste_status: Option<Result<String, String>>,
    /// `Modal::ThemeManager::selected` to land the list on when the editor
    /// closes (save or discard) — the row for whichever theme was being
    /// edited, resolved fresh by name rather than trusted as a raw index.
    pub return_selected: usize,
    /// Set when this editor session was opened via "New theme" — that path
    /// (unlike Edit/⌘E on an existing theme) already persisted a fresh
    /// `CUSTOM` entry (auto-named "untitled ...") before the editor even
    /// opened, so the user can preview it. Cleared on the first successful
    /// Save; if the editor is discarded/Esc'd while this is still `true`,
    /// that never-actually-saved-by-the-user entry is deleted rather than
    /// left behind as an orphan.
    pub created_this_session: bool,
}

impl ThemeManagerEditorState {
    /// Whether `draft` differs from `saved` in any way Save would persist:
    /// colors, name, or kind (`Theme::colors_eq` alone misses the latter two).
    pub fn is_dirty(&self) -> bool {
        !self.draft.colors_eq(&self.saved)
            || self.draft.name != self.saved.name
            || self.draft.kind != self.saved.kind
    }
}

/// Whether the caller must call `Grove::invalidate_pty_render_cache` after an
/// `update`/`handle_key` call — a named replacement for a bare `bool` return
/// so call sites don't have to remember (or a reader guess) what the flag
/// means, and so it isn't confused with `add_project`'s same-shaped
/// `WtCacheRebuild` (a different signal entirely).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum PtyCacheInvalidate {
    Skip,
    Invalidate,
}

/// Messages the theme-manager editor can emit. `Edit` is intercepted by the
/// parent (`Grove::update`'s `Msg::ThemeManager(ThemeManagerMsg::Editor)` arm) before it ever
/// reaches `update` here — it needs to resolve a LIST row index via
/// `grove_core::theme::all_custom_themes()`, a list concern this module doesn't
/// own — so it's handled by the parent's `theme_manager_edit_start`, which
/// then calls this module's `open`.
#[derive(Debug, Clone)]
pub enum Msg {
    /// List row's "Edit" button: opens the editor on the custom theme at
    /// row `idx`.
    Edit(usize),
    /// Editor: click row `i` (`theme::FIELD_NAMES` order) — selects it as the
    /// row ↑/↓ would.
    RowSelect(usize),
    /// Editor: live edit of the selected row's hex-text buffer.
    HexChanged(String),
    /// Editor: live edit of the name field.
    NameChanged(String),
    /// Editor: Dark/Light kind toggle.
    KindDark,
    KindLight,
    /// Editor Preview button / ⌘P: toggles applying the draft to the whole
    /// app vs. showing the last-applied saved theme.
    TogglePreview,
    /// Editor ⏎ (only reaches here when no text field has focus) / ⌘S /
    /// Save button: persists the draft via `theme::update_custom`.
    Save,
    /// Editor Esc: dirty edits open the discard confirmation; otherwise
    /// discards immediately and returns to the list.
    Esc,
    /// Editor discard-confirmation "Discard" button (mirrors ⏎ there).
    DiscardConfirm,
    /// Editor discard-confirmation "Keep editing" button (mirrors Esc there).
    DiscardCancel,
    /// Paste box edit (any `text_editor::Action` — move, select, or edit).
    PasteAction(iced::widget::text_editor::Action),
    /// Paste box "Apply" button / ⌘⏎: runs `theme_file::parse_paste` on the
    /// paste box's current text and applies the result to the draft.
    PasteApply,
}

/// Shared landing for every path into `Modal::ThemeManager`'s EDITOR
/// sub-view (list row's Edit button, "New theme", the palette's ⌘E):
/// opens/keeps the modal on the list's state (so Esc-back-to-list always
/// has somewhere sane to land), captures the pre-edit whole-app theme for
/// discard/preview-off, seeds the row-0 hex buffer, applies `draft` as a
/// live preview immediately, and scrolls to row 0.
///
/// Does NOT invalidate the PTY render cache itself — every caller does that
/// unconditionally right after (this always changes the live theme), mirroring
/// how the pre-extraction `open_theme_manager_editor` always ended with
/// `self.invalidate_pty_render_cache()` regardless of call site.
pub fn open(
    app: &mut App,
    editor: &mut Option<ThemeManagerEditorState>,
    draft: grove_core::theme::Theme,
) -> Task<Msg> {
    let list_selected = grove_core::theme::all_custom_themes()
        .iter()
        .position(|t| t.name == draft.name)
        .unwrap_or(0);
    // Always land on a clean list state (no stray rename/delete
    // confirmation left open underneath) with the cursor on this theme's
    // row, whether the modal was already open or not.
    app.modal = Modal::ThemeManager {
        selected: list_selected,
        rename: None,
        rename_error: None,
        pending_delete: None,
    };
    let original_active = grove_core::theme::current();
    let original_name = draft.name.to_string();
    let saved = draft.clone();
    let hex_buf = grove_core::theme_file::to_hex(draft.field(0));
    // Prefill the paste box with the draft's own current values (named-
    // lines format) — a copyable reference and an editable starting
    // point, rather than an empty box.
    let paste = iced::widget::text_editor::Content::with_text(
        &grove_core::theme_file::to_named_lines(&draft),
    );
    *editor = Some(ThemeManagerEditorState {
        original_name,
        draft: draft.clone(),
        saved,
        original_active,
        selected: 0,
        invalid: [false; 11],
        hex_buf,
        preview_on: true,
        confirm_discard: false,
        paste,
        paste_status: None,
        return_selected: list_selected,
        created_this_session: false,
    });
    grove_core::theme::set(draft);
    scroll_to_selection(editor)
}

/// Editor ↑/↓: moves the row cursor and reseeds `hex_buf` with the new
/// row's live hex text.
fn move_row(editor: &mut Option<ThemeManagerEditorState>, delta: i32) -> Task<Msg> {
    let Some(ed) = &*editor else {
        return Task::none();
    };
    let new_idx = crate::gui::launcher::clamp(ed.selected, delta, 11);
    row_select(editor, new_idx)
}

/// Editor row click / `move_row`'s landing: jumps the row cursor straight to
/// `idx` and reseeds `hex_buf` with that row's hex text.
fn row_select(editor: &mut Option<ThemeManagerEditorState>, idx: usize) -> Task<Msg> {
    let Some(ed) = editor.as_mut() else {
        return Task::none();
    };
    if idx >= 11 {
        return Task::none();
    }
    ed.selected = idx;
    ed.hex_buf = grove_core::theme_file::to_hex(ed.draft.field(idx));
    scroll_to_selection(editor)
}

/// Editor hex-field edit: a valid `#rrggbb` updates the focused row's
/// color in `draft` and, while previewing, re-applies the whole draft
/// live; invalid hex just flags that row's error state without touching
/// `draft`. Always signals a PTY-cache invalidation, same as before
/// extraction.
fn hex_changed(editor: &mut Option<ThemeManagerEditorState>, s: String) {
    let parsed = grove_core::theme_file::parse_hex(&s).ok();
    let Some(ed) = editor.as_mut() else {
        return;
    };
    ed.hex_buf = s;
    match parsed {
        Some(color) => {
            ed.draft.set_field(ed.selected, color);
            ed.invalid[ed.selected] = false;
            if ed.preview_on {
                grove_core::theme::set(ed.draft.clone());
            }
            // The paste box reflects the draft, not a scratchpad the
            // user's own edits there must survive — resync it every time
            // a swatch-row hex commits to a valid value.
            ed.paste = iced::widget::text_editor::Content::with_text(
                &grove_core::theme_file::to_named_lines(&ed.draft),
            );
        }
        None => ed.invalid[ed.selected] = true,
    }
}

/// Editor name field edit: live-updates `draft.name` (collision checking
/// stays deferred to Save's `theme::update_custom` call, same as the old
/// design's rename-on-save).
fn name_changed(editor: &mut Option<ThemeManagerEditorState>, s: String) {
    if let Some(ed) = editor.as_mut() {
        ed.draft.name = std::borrow::Cow::Owned(s);
    }
}

/// Editor Dark/Light toggle: updates `draft.kind` and re-previews.
fn set_kind(editor: &mut Option<ThemeManagerEditorState>, kind: grove_core::theme::ThemeKind) {
    if let Some(ed) = editor.as_mut() {
        ed.draft.kind = kind;
        if ed.preview_on {
            grove_core::theme::set(ed.draft.clone());
        }
    }
}

/// Editor ⌘P / Preview button: toggles applying the draft to the whole
/// app. ON re-applies `draft`; OFF shows `original_active` again without
/// losing the draft's edits.
fn toggle_preview(editor: &mut Option<ThemeManagerEditorState>) {
    let Some(ed) = editor.as_mut() else {
        return;
    };
    ed.preview_on = !ed.preview_on;
    let next = if ed.preview_on {
        ed.draft.clone()
    } else {
        ed.original_active.clone()
    };
    grove_core::theme::set(next);
}

/// Same fields `theme_manager_rename_submit` (list, untouched, in update.rs)
/// keeps in sync — duplicated here rather than sharing that Grove method so
/// this module never needs `&mut Grove`, only `&mut App`.
fn persist_theme_rename(app: &mut App, old: &str, new: &str) {
    let mut changed = false;
    if app.store.theme.as_deref() == Some(old) {
        app.store.theme = Some(new.to_string());
        changed = true;
    }
    if app.store.theme_dark.as_deref() == Some(old) {
        app.store.theme_dark = Some(new.to_string());
        changed = true;
    }
    if app.store.theme_light.as_deref() == Some(old) {
        app.store.theme_light = Some(new.to_string());
        changed = true;
    }
    for proj in &mut app.store.projects {
        if proj.theme.as_deref() == Some(old) {
            proj.theme = Some(new.to_string());
            changed = true;
        }
    }
    if changed {
        grove_core::storage::persist(&app.store);
    }
}

/// Editor ⏎ (only reaches here with no text field focused) / ⌘S / Save
/// button: persists `draft` into the `CUSTOM` registry via
/// `theme::update_custom` (which rejects an empty/whitespace-only name —
/// same as the list-view rename path — and trims a valid one, so `name`
/// below is recomputed from the trimmed value rather than trusted from
/// the untrimmed draft), updates any persisted `store`/project-pin
/// reference if the name changed, and returns to the list landed on the
/// saved theme's row. The save is always persisted regardless of
/// Preview, but only re-applies the draft app-wide when Preview is ON —
/// with it OFF, `original_active` is what's actually showing, and Save
/// must not silently switch the whole app to a theme the user opted out
/// of previewing.
///
/// Returns whether the PTY render cache needs invalidating — only the
/// success path changes the live theme.
fn save(
    app: &mut App,
    editor: &mut Option<ThemeManagerEditorState>,
) -> (Task<Msg>, PtyCacheInvalidate) {
    let Some(ed) = editor.take() else {
        return (Task::none(), PtyCacheInvalidate::Skip);
    };
    if let Err(e) = grove_core::theme::update_custom(&ed.original_name, ed.draft.clone()) {
        app.set_error_toast(e);
        *editor = Some(ed);
        return (Task::none(), PtyCacheInvalidate::Skip);
    }
    let name = ed.draft.name.trim().to_string();
    app.set_toast(format!("theme saved: {name}"));
    if name != ed.original_name {
        persist_theme_rename(app, &ed.original_name, &name);
    }
    let mut saved = ed.draft;
    saved.name = std::borrow::Cow::Owned(name.clone());
    // Save always persists the draft into the registry (above); whether
    // it also becomes the whole app's live theme depends on Preview:
    // ON means the draft is already what's showing (leave it applied),
    // OFF means `original_active` is what's showing and Save must not
    // silently switch the app to a theme the user opted out of
    // previewing.
    if ed.preview_on {
        grove_core::theme::set(saved);
    } else {
        grove_core::theme::set(ed.original_active);
    }
    let idx = grove_core::theme::all_custom_themes()
        .iter()
        .position(|t| t.name == name)
        .unwrap_or(ed.return_selected);
    if let Modal::ThemeManager { selected, .. } = &mut app.modal {
        *selected = idx;
    }
    (Task::none(), PtyCacheInvalidate::Invalidate)
}

/// Editor Esc: dirty edits show the "Discard changes…?" confirmation
/// (mirroring `Modal::Confirm`'s Enter=yes/Esc=no convention); with
/// nothing to lose, Esc discards immediately.
fn esc(
    app: &mut App,
    editor: &mut Option<ThemeManagerEditorState>,
) -> (Task<Msg>, PtyCacheInvalidate) {
    let Some(ed) = editor.as_mut() else {
        return (Task::none(), PtyCacheInvalidate::Skip);
    };
    if !ed.is_dirty() {
        return discard(app, editor);
    }
    ed.confirm_discard = true;
    (Task::none(), PtyCacheInvalidate::Skip)
}

/// Editor discard-confirmation Esc ("Keep editing"): drops the
/// confirmation, keeps the draft and the editor open.
fn discard_cancel(editor: &mut Option<ThemeManagerEditorState>) {
    if let Some(ed) = editor.as_mut() {
        ed.confirm_discard = false;
    }
}

/// Editor discard-confirmation Enter ("Discard"), or Esc outright when
/// there was nothing dirty to confirm: reverts the whole app to
/// `original_active` and returns to the list. If this editor session was
/// opened via "New theme" and never reached a successful Save,
/// `theme_manager_new` already persisted a fresh "untitled ..." entry
/// into the registry so the user could preview it — discarding here
/// deletes that entry too, so it doesn't linger as an orphan the user
/// never actually chose to keep.
fn discard(
    app: &mut App,
    editor: &mut Option<ThemeManagerEditorState>,
) -> (Task<Msg>, PtyCacheInvalidate) {
    let Some(ed) = editor.take() else {
        return (Task::none(), PtyCacheInvalidate::Skip);
    };
    if ed.created_this_session {
        grove_core::theme::delete_custom(&ed.original_name);
    }
    grove_core::theme::set(ed.original_active);
    // Deleting the never-saved entry can shrink the list out from under
    // `return_selected` (it was resolved back when that row still
    // existed) — clamp the same way `theme_manager_delete_confirm` does.
    let total = grove_core::theme::all_custom_themes().len();
    if let Modal::ThemeManager { selected, .. } = &mut app.modal {
        *selected = ed.return_selected.min(total.saturating_sub(1));
    }
    (Task::none(), PtyCacheInvalidate::Invalidate)
}

/// Paste box edit: routes through the same `global_mods`/`live_mods`
/// spurious-edit guard `session_launcher::Msg::InputChanged` uses — a chord like ⌘S or
/// ⌘⏎ is a command, never text, but a focused `text_editor` (like
/// `text_input`) doesn't special-case an arbitrary Cmd/Ctrl+Shift chord
/// and would otherwise insert/delete on it unconditionally.
///
/// `Edit::Paste` is carved out of that guard: iced's `text_editor`
/// resolves ⌘V to `Action::Edit(Edit::Paste(_))` itself (see its
/// `Binding::Paste` handling) before this callback ever runs, so by the
/// time we're asked to perform it the chord's ⌘ is still held in
/// `live_mods` — without the carve-out, a real paste would be dropped
/// exactly like the spurious edit this guard exists to catch. ⌘C (copy)
/// needs no carve-out: the widget writes the clipboard directly and
/// never publishes an `Action` for it, so it can't reach this guard at
/// all. ⌘X (cut) resolves to plain `Edit::Delete` — indistinguishable
/// from a bare Delete keypress, so it's left under the existing guard
/// rather than carved out too; this finding is scoped to restoring ⌘V.
fn paste_action(
    editor: &mut Option<ThemeManagerEditorState>,
    live_mods: Modifiers,
    action: iced::widget::text_editor::Action,
) {
    use iced::widget::text_editor::{Action as EditorAction, Edit};
    let is_paste = matches!(action, EditorAction::Edit(Edit::Paste(_)));
    if action.is_edit() && !is_paste && global_mods(live_mods) {
        return;
    }
    if let Some(ed) = editor.as_mut() {
        ed.paste.perform(action);
    }
}

/// Paste box "Apply" / ⌘⏎: parses the paste box's current text via
/// `theme_file::parse_paste` and applies the result to the draft (a
/// subset of fields is valid — only those fields change; `name`/`kind`
/// prefill when the paste included them). Success shows "Applied N
/// colors"; failure shows the parser's message, both inline under the
/// box.
fn paste_apply(editor: &mut Option<ThemeManagerEditorState>) {
    let Some(ed) = editor.as_mut() else {
        return;
    };
    let text = ed.paste.text();
    match grove_core::theme_file::parse_paste(&text) {
        Ok(applied) => {
            let n = applied.colors.len();
            grove_core::theme_file::apply_pasted_colors(&mut ed.draft, &applied);
            ed.invalid = [false; 11];
            ed.hex_buf = grove_core::theme_file::to_hex(ed.draft.field(ed.selected));
            if ed.preview_on {
                grove_core::theme::set(ed.draft.clone());
            }
            // Resync the box to the (now-updated) draft — it's a
            // reflection of the draft, not a scratchpad to preserve.
            ed.paste = iced::widget::text_editor::Content::with_text(
                &grove_core::theme_file::to_named_lines(&ed.draft),
            );
            ed.paste_status = Some(Ok(format!("Applied {n} colors")));
        }
        Err(e) => ed.paste_status = Some(Err(e)),
    }
}

/// Scrolls the scrollable with the given [`Id`] to an absolute offset. Local
/// copy of `update.rs`'s `scroll_to` helper, scoped to this module's own
/// `Msg` rather than the parent's — see `scripts_editor`'s equivalent note
/// (batch 1) for why: iced's `Task::discard` is generic over the output
/// message type, but the parent's helper hardcodes its return type to
/// `super::state::Msg`.
fn scroll_to(id: Id, offset: iced::widget::scrollable::AbsoluteOffset) -> Task<Msg> {
    iced::advanced::widget::operate(
        iced::advanced::widget::operation::scrollable::scroll_to::<()>(id, offset.into()),
    )
    .discard()
}

/// Scroll the editor's row list so the selected color row is centered —
/// same idiom as `scroll_launcher_theme_to_selection`, via
/// `theme_editor_scroll_offset`'s section-aware walk (Surfaces/Text/
/// Accents headers, then the derived strip, aren't uniform-height rows).
fn scroll_to_selection(editor: &Option<ThemeManagerEditorState>) -> Task<Msg> {
    use iced::widget::scrollable::AbsoluteOffset;
    let Some(ed) = editor else {
        return Task::none();
    };
    let y = theme_editor_scroll_offset(ed.selected);
    scroll_to(theme_manager_scrollable_id(), AbsoluteOffset { x: 0.0, y })
}

/// Handles every `Msg` variant except `Edit`, which the parent intercepts
/// before calling this function (see `Msg`'s doc comment). `live_mods` is
/// `Grove::live_mods`, needed only by `PasteAction`'s spurious-edit guard —
/// threaded in explicitly since this module never holds a `Grove` reference.
///
/// Returns whether the parent must call `Grove::invalidate_pty_render_cache`
/// — mirrors exactly which pre-extraction methods called it and on which
/// paths (several only invalidate on a success path, not on early returns).
pub fn update(
    app: &mut App,
    editor: &mut Option<ThemeManagerEditorState>,
    live_mods: Modifiers,
    msg: Msg,
) -> (Task<Msg>, PtyCacheInvalidate) {
    match msg {
        // Unreachable in practice — the parent intercepts `Edit` before
        // calling `update`. Kept for match exhaustiveness.
        Msg::Edit(_) => (Task::none(), PtyCacheInvalidate::Skip),
        Msg::RowSelect(i) => (row_select(editor, i), PtyCacheInvalidate::Skip),
        Msg::HexChanged(s) => {
            hex_changed(editor, s);
            (Task::none(), PtyCacheInvalidate::Invalidate)
        }
        Msg::NameChanged(s) => {
            name_changed(editor, s);
            (Task::none(), PtyCacheInvalidate::Skip)
        }
        Msg::KindDark => {
            set_kind(editor, grove_core::theme::ThemeKind::Dark);
            (Task::none(), PtyCacheInvalidate::Invalidate)
        }
        Msg::KindLight => {
            set_kind(editor, grove_core::theme::ThemeKind::Light);
            (Task::none(), PtyCacheInvalidate::Invalidate)
        }
        Msg::TogglePreview => {
            toggle_preview(editor);
            (Task::none(), PtyCacheInvalidate::Invalidate)
        }
        Msg::Save => save(app, editor),
        Msg::Esc => esc(app, editor),
        Msg::DiscardConfirm => discard(app, editor),
        Msg::DiscardCancel => {
            discard_cancel(editor);
            (Task::none(), PtyCacheInvalidate::Skip)
        }
        Msg::PasteAction(action) => {
            paste_action(editor, live_mods, action);
            (Task::none(), PtyCacheInvalidate::Skip)
        }
        Msg::PasteApply => {
            paste_apply(editor);
            (Task::none(), PtyCacheInvalidate::Invalidate)
        }
    }
}

/// Editor-only key handling for `Modal::ThemeManager` — the
/// `self.theme_manager_editor.is_some()` branch of `handle_modal_key`'s
/// `Modal::ThemeManager` arm, extracted verbatim. Checked first in that
/// arm's `if/else if` chain (list rename/delete/plain-list branches are
/// untouched, in `update.rs`), so this only ever runs while the editor is
/// open. Same invalidate-flag convention as `update`.
pub fn handle_key(
    editor: &mut Option<ThemeManagerEditorState>,
    app: &mut App,
    key: Key,
    mods: Modifiers,
) -> (Task<Msg>, PtyCacheInvalidate) {
    let confirm_discard = editor.as_ref().is_some_and(|ed| ed.confirm_discard);
    if confirm_discard {
        // "Discard changes…?" up: only its own Enter (discard)/Esc (keep
        // editing) apply — rows are frozen underneath it.
        match key {
            Key::Named(Named::Enter) => return discard(app, editor),
            Key::Named(Named::Escape) => discard_cancel(editor),
            _ => {}
        }
        return (Task::none(), PtyCacheInvalidate::Skip);
    }
    let dir_delta: Option<i32> = match &key {
        Key::Named(Named::ArrowDown) => Some(1),
        Key::Named(Named::ArrowUp) => Some(-1),
        _ => None,
    };
    if let Some(delta) = dir_delta {
        return (move_row(editor, delta), PtyCacheInvalidate::Skip);
    }
    match key {
        // ⌘⏎ (Apply the paste box) must be checked ahead of the plain-Enter
        // arm below, which would otherwise shadow it.
        Key::Named(Named::Enter) if global_mods(mods) => {
            paste_apply(editor);
            (Task::none(), PtyCacheInvalidate::Invalidate)
        }
        Key::Named(Named::Escape) => esc(app, editor),
        Key::Named(Named::Enter) => save(app, editor),
        Key::Character(s) if global_mods(mods) => match s.as_str() {
            "s" | "S" => save(app, editor),
            "p" | "P" => {
                toggle_preview(editor);
                (Task::none(), PtyCacheInvalidate::Invalidate)
            }
            _ => (Task::none(), PtyCacheInvalidate::Skip),
        },
        _ => (Task::none(), PtyCacheInvalidate::Skip),
    }
}

/// `Modal::ThemeManager`'s EDITOR sub-view (Stage B): paste-first box at
/// the top, then name/kind, then the grouped 11-swatch manual editor
/// migrated from the old `SettingsPane::ThemeEditor` pane (rows, hex
/// editing, invalid-row state, contrast badges, derived strip).
///
/// Returns the *parent's* `Msg` (wrapped in `GMsg::ThemeManager(ThemeManagerMsg::Editor)`)
/// rather than this module's own `Msg` — same reason as `scripts_editor`'s
/// `view` (batch 1): the shared modal widgets are hardcoded to
/// `super::state::Msg`, not generic over a message type.
/// The discard-confirmation dialog that replaces the whole editor panel when
/// Esc is pressed with unsaved changes — same convention as the LIST view's
/// delete confirm.
fn discard_confirm_panel(ed: &ThemeManagerEditorState) -> Element<'_, GMsg> {
    let body_zone = column![
        text(format!("Discard changes to \"{}\"?", ed.original_name))
            .size(13)
            .color(c::FG_DIM())
            .wrapping(iced::widget::text::Wrapping::Word),
        Space::new().height(4),
        row![
            Space::new().width(Length::Fill),
            modal_action(
                "Keep editing",
                ModalBtn::Plain,
                GMsg::ThemeManager(ThemeManagerMsg::Editor(Msg::DiscardCancel))
            ),
            modal_action(
                "Discard",
                ModalBtn::Danger,
                GMsg::ThemeManager(ThemeManagerMsg::Editor(Msg::DiscardConfirm))
            ),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center),
    ]
    .spacing(8);
    let body = column![
        modal_header("Discard changes", c::RED()),
        divider_h(c::BORDER_SOFT()),
        container(body_zone).padding(Padding::from([14, 16])),
        divider_h(c::BORDER_SOFT()),
        modal_footer_hints(&[("⏎", "discard"), ("esc", "keep editing")]),
    ];
    modal_panel(body.into(), 420.0)
}

/// The paste-first box at the top of the editor, plus its status/format
/// caption line.
fn paste_zone(ed: &ThemeManagerEditorState) -> Element<'_, GMsg> {
    let paste_editor = iced::widget::text_editor(&ed.paste)
        .height(Length::Fixed(190.0))
        .font(iced::Font::MONOSPACE)
        .size(12)
        .padding(8)
        .placeholder("field #hex per line, 11 hex values, or a themes.json entry")
        .style(|_, status| {
            use iced::widget::text_editor::Status;
            let border_color = match status {
                Status::Focused { .. } => c::CYAN(),
                Status::Hovered => c::BORDER(),
                _ => c::BORDER_SOFT(),
            };
            iced::widget::text_editor::Style {
                background: Background::Color(c::BG_STRIP()),
                border: Border {
                    color: border_color,
                    width: 1.0,
                    radius: Radius::from(4.0),
                },
                placeholder: c::FG_MUTE(),
                value: c::FG(),
                selection: c::BG_HL(),
            }
        })
        .on_action(|a| GMsg::ThemeManager(ThemeManagerMsg::Editor(Msg::PasteAction(a))));
    let mut paste_col = column![
        row![
            text("Paste colors").size(12).color(c::FG()),
            Space::new().width(Length::Fill),
            modal_action_sized(
                "Apply (⌘⏎)",
                ModalBtn::Plain,
                11,
                GMsg::ThemeManager(ThemeManagerMsg::Editor(Msg::PasteApply))
            ),
        ]
        .align_y(iced::Alignment::Center),
        paste_editor,
    ]
    .spacing(4);
    // A successful/failed Apply's own message takes priority; otherwise
    // a short, permanent one-line caption about the accepted alternate
    // formats (the box itself is now the example, prefilled from the
    // draft — no separate sample block needed).
    match &ed.paste_status {
        Some(Ok(s)) => paste_col = paste_col.push(text(s.clone()).size(11).color(c::GREEN())),
        Some(Err(e)) => paste_col = paste_col.push(text(e.clone()).size(11).color(c::RED())),
        None => {
            paste_col = paste_col.push(
                text("also accepts 11 hex values or a themes.json entry")
                    .size(10)
                    .color(c::FG_MUTE()),
            );
        }
    }

    paste_col.into()
}

/// The theme-name input paired with the Dark/Light kind segmented control.
fn name_and_kind_row(ed: &ThemeManagerEditorState) -> Element<'_, GMsg> {
    let name_field = text_input("theme name", &ed.draft.name)
        .on_input(|s| GMsg::ThemeManager(ThemeManagerMsg::Editor(Msg::NameChanged(s))))
        .style(input_field_style)
        .size(13)
        .width(Length::Fill);
    let kind_seg = container(
        row![
            seg_button(
                "Dark",
                matches!(ed.draft.kind, grove_core::theme::ThemeKind::Dark),
                SegSide::Left,
                GMsg::ThemeManager(ThemeManagerMsg::Editor(Msg::KindDark)),
            ),
            seg_button(
                "Light",
                matches!(ed.draft.kind, grove_core::theme::ThemeKind::Light),
                SegSide::Right,
                GMsg::ThemeManager(ThemeManagerMsg::Editor(Msg::KindLight)),
            ),
        ]
        .spacing(0),
    )
    .style(|_| container::Style {
        border: Border {
            color: c::BORDER(),
            width: 1.0,
            radius: Radius::from(6.0),
        },
        ..Default::default()
    });
    let name_row = row![name_field, kind_seg]
        .spacing(8)
        .align_y(iced::Alignment::Center);

    name_row.into()
}

/// The editor's header zone content: title + dirty dot on the left, the
/// Preview toggle on the right.
fn editor_header(ed: &ThemeManagerEditorState, dirty: bool) -> Element<'_, GMsg> {
    let mut header_row = row![text(format!("Edit theme — {}", ed.original_name))
        .size(13)
        .color(c::MAGENTA())]
    .spacing(6)
    .align_y(iced::Alignment::Center);
    if dirty {
        header_row = header_row.push(text("●").size(8).color(c::YELLOW()));
    }
    header_row = header_row.push(Space::new().width(Length::Fill));
    header_row = header_row.push(
        button(
            container(
                text(if ed.preview_on {
                    "Preview: on"
                } else {
                    "Preview: off"
                })
                .size(10)
                .color(if ed.preview_on {
                    c::CYAN()
                } else {
                    c::FG_MUTE()
                }),
            )
            .padding(Padding::from([2, 8])),
        )
        .style(|_, _| button::Style {
            background: Some(Background::Color(c::BG_HL())),
            border: Border {
                color: c::BORDER(),
                width: 1.0,
                radius: Radius::from(4.0),
            },
            text_color: c::FG(),
            ..Default::default()
        })
        .on_press(GMsg::ThemeManager(ThemeManagerMsg::Editor(
            Msg::TogglePreview,
        ))),
    );

    header_row.into()
}

/// The scrollable, grouped 11-swatch grid plus the read-only derived strip.
fn swatch_grid<'a>(ed: &'a ThemeManagerEditorState) -> Element<'a, GMsg> {
    let mut list = Column::new().spacing(2);
    let mut printed_group: Option<&'static str> = None;
    for i in 0..grove_core::theme::FIELD_NAMES.len() {
        let group = grove_core::theme::FIELD_GROUPS[i];
        if printed_group != Some(group) {
            let top = if printed_group.is_none() { 0.0 } else { 10.0 };
            list = list.push(section_header(group, top, 4.0));
            printed_group = Some(group);
        }
        let active = i == ed.selected;
        let color = ed.draft.field(i);
        let swatch =
            container(Space::new().width(14.0).height(14.0)).style(move |_| container::Style {
                background: Some(Background::Color(c::ic(color))),
                border: Border {
                    color: c::BORDER(),
                    width: 1.0,
                    radius: Radius::from(3.0),
                },
                ..Default::default()
            });
        let is_invalid = ed.invalid[i];
        let row_el: Element<'a, GMsg> = if active {
            let field = text_input("#rrggbb", &ed.hex_buf)
                .on_input(|s| GMsg::ThemeManager(ThemeManagerMsg::Editor(Msg::HexChanged(s))))
                .style(input_field_style)
                .font(MONO_FONT)
                .size(12)
                .width(Length::Fixed(90.0));
            let mut row_content = row![
                swatch,
                text(grove_core::theme::FIELD_NAMES[i])
                    .size(12)
                    .color(c::FG_DIM()),
                Space::new().width(Length::Fill),
                field,
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center);
            if let Some(partner) = grove_core::theme::CONTRAST_PARTNER[i] {
                let ratio = grove_core::theme::contrast_ratio(color, ed.draft.field(partner));
                if ratio < 3.0 {
                    row_content =
                        row_content.push(text(format!("{ratio:.1}:1")).size(10).color(c::RED()));
                } else if ratio < 4.5 {
                    row_content =
                        row_content.push(text(format!("{ratio:.1}:1")).size(10).color(c::YELLOW()));
                }
            }
            container(row_content)
                .width(Length::Fill)
                .height(36.0)
                .padding(Padding::from([0.0, 12.0]))
                .align_y(iced::Alignment::Center)
                .style(move |_| container::Style {
                    background: Some(Background::Color(if is_invalid {
                        c::RED_WASH()
                    } else {
                        c::SEL_TINT_STRONG()
                    })),
                    border: Border {
                        color: if is_invalid { c::RED() } else { c::SEL_RING() },
                        width: 1.0,
                        radius: Radius::from(6.0),
                    },
                    ..Default::default()
                })
                .into()
        } else {
            let hex_text = grove_core::theme_file::to_hex(color);
            let mut row_content = row![
                swatch,
                text(grove_core::theme::FIELD_NAMES[i])
                    .size(12)
                    .color(c::FG_DIM()),
                Space::new().width(Length::Fill),
                text(hex_text).font(MONO_FONT).size(12).color(c::FG()),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center);
            if let Some(partner) = grove_core::theme::CONTRAST_PARTNER[i] {
                let ratio = grove_core::theme::contrast_ratio(color, ed.draft.field(partner));
                if ratio < 3.0 {
                    row_content =
                        row_content.push(text(format!("{ratio:.1}:1")).size(10).color(c::RED()));
                } else if ratio < 4.5 {
                    row_content =
                        row_content.push(text(format!("{ratio:.1}:1")).size(10).color(c::YELLOW()));
                }
            }
            launcher_row(
                row_content,
                false,
                true,
                GMsg::ThemeManager(ThemeManagerMsg::Editor(Msg::RowSelect(i))),
                36.0,
            )
        };
        list = list.push(row_el);
    }
    // Derived strip: read-only synthesized chips, reusing
    // `palette.rs`'s `_of` blend helpers directly on the draft.
    list = list.push(section_header("DERIVED — NOT EDITABLE", 10.0, 4.0));
    let derived_chip = |label: &'static str, color: Color| -> Element<'a, GMsg> {
        row![
            container(Space::new().width(12.0).height(12.0)).style(move |_| {
                container::Style {
                    background: Some(Background::Color(color)),
                    border: Border {
                        color: c::BORDER(),
                        width: 1.0,
                        radius: Radius::from(3.0),
                    },
                    ..Default::default()
                }
            }),
            text(label).size(10).color(c::FG_MUTE()),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center)
        .into()
    };
    list = list.push(
        container(
            row![
                derived_chip("hover", c::bg_hover_of(&ed.draft)),
                derived_chip("border", c::border_of(&ed.draft)),
                derived_chip("highlight", c::bg_hl_of(&ed.draft)),
                derived_chip("selection", c::sel_ring_of(&ed.draft)),
            ]
            .spacing(14)
            .align_y(iced::Alignment::Center),
        )
        .padding(Padding::from([4, 12])),
    );
    container(
        ghost_scrollable(list)
            .id(theme_manager_scrollable_id())
            .height(Length::Shrink),
    )
    .max_height(280.0)
    .width(Length::Fill)
    .into()
}

/// The editor's hints-only footer strip.
fn editor_footer<'a>() -> Element<'a, GMsg> {
    footer_container(
        row![
            footer_hint("↑↓", "row"),
            footer_mod_hint("p", "preview"),
            footer_hint("esc", "back"),
        ]
        .spacing(14)
        .align_y(iced::Alignment::Center)
        .into(),
    )
}

pub fn view(ed: &ThemeManagerEditorState) -> Element<'_, GMsg> {
    let dirty = ed.is_dirty();

    // Discard confirmation swaps the whole panel for a `confirm_modal`-
    // shaped dialog, same convention as the LIST view's delete confirm.
    if ed.confirm_discard {
        return discard_confirm_panel(ed);
    }

    // Trailing Back/Save action row lives in the body zone, not the
    // footer — same convention `project_settings_modal`'s Cancel/Save row
    // and `confirm_modal`'s button row use; the footer strip stays
    // hints-only.
    let body_zone = column![
        paste_zone(ed),
        name_and_kind_row(ed),
        swatch_grid(ed),
        row![
            Space::new().width(Length::Fill),
            modal_action(
                "Back",
                ModalBtn::Plain,
                GMsg::ThemeManager(ThemeManagerMsg::Editor(Msg::Esc))
            ),
            modal_action(
                "Save",
                ModalBtn::Primary,
                GMsg::ThemeManager(ThemeManagerMsg::Editor(Msg::Save))
            ),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center),
    ]
    .spacing(12);

    let body = column![
        modal_header_row(editor_header(ed, dirty)),
        divider_h(c::BORDER_SOFT()),
        container(body_zone).padding(Padding::from([14, 16])),
        divider_h(c::BORDER_SOFT()),
        editor_footer(),
    ];

    modal_panel(body.into(), 560.0)
}
