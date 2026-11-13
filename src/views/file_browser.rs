//! Session-owned, read-only checkout browsing. Disk work is bounded and never blocks paint.
use super::{
    components::{
        header_control_frame, header_control_with_label_tooltip, project_field_well,
        transcript_action,
    },
    rpx,
    tokens::*,
};
use crate::{icons::icon, theme as c};
use gpui::{
    div, prelude::*, AnyElement, App, Context, FocusHandle, Focusable, ScrollHandle, SharedString,
    Window,
};
use gpui_component::text::TextView;
use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};

pub(super) const EXPLORER_WIDTH: f32 = 280.;
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_DIRECTORY_ENTRIES: usize = 2000;
const SOURCE_LINE_HEIGHT: f32 = 20.;

#[derive(Clone, Debug)]
pub(super) struct FileTarget {
    pub path: PathBuf,
    pub line: Option<usize>,
    /// Only an explicitly activated transcript link may read outside the checkout.
    pub transcript_link: bool,
}

/// Recognize local references without ever interpreting a URL fragment as a disk path.
pub(super) fn local_target(target: &str, cwd: &Path) -> Option<FileTarget> {
    use gpui::http_client::Url;
    if target.starts_with("//") || target.starts_with('#') || target.is_empty() {
        return None;
    }
    let (target, fragment) = target
        .split_once('#')
        .map_or((target, None), |(path, fragment)| (path, Some(fragment)));
    let (target, colon_line) = target
        .rsplit_once(':')
        .filter(|(_, n)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        .map_or((target, None), |(p, n)| (p, n.parse::<usize>().ok()));
    // A final numeric pair is line:column; the viewer focuses the whole source line.
    let (target, colon_line) = if colon_line.is_some() {
        target
            .rsplit_once(':')
            .filter(|(_, n)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            .map_or((target, colon_line), |(path, line)| {
                (path, line.parse::<usize>().ok())
            })
    } else {
        (target, colon_line)
    };
    let mut url = if target.starts_with('/') {
        Url::parse(&format!("file://{target}")).ok()?
    } else if Path::new(target).is_absolute() {
        let (path, query) = target
            .split_once('?')
            .map_or((target, None), |(path, query)| (path, Some(query)));
        let mut url = Url::from_file_path(path).ok()?;
        url.set_query(query);
        url
    } else if target.starts_with("file:") {
        Url::parse(target).ok()?
    } else if Url::parse(target).is_ok() {
        return None;
    } else {
        Url::from_directory_path(cwd).ok()?.join(target).ok()?
    };
    url.set_fragment(fragment);
    if url.scheme() != "file" {
        return None;
    }
    let line = url
        .fragment()
        .and_then(|f| {
            f.trim_start_matches('L')
                .split(['-', ':', 'C'])
                .next()?
                .parse()
                .ok()
        })
        .or(colon_line)
        .filter(|n| *n > 0);
    Some(FileTarget {
        path: url.to_file_path().ok()?,
        line,
        transcript_link: true,
    })
}

fn checked_path(root: &Path, path: &Path) -> Result<PathBuf, String> {
    let root = root
        .canonicalize()
        .map_err(|e| format!("Checkout unavailable: {e}"))?;
    let path = path
        .canonicalize()
        .map_err(|e| format!("File unavailable: {e}"))?;
    if !path.starts_with(root) {
        return Err("This file is outside the selected checkout.".into());
    }
    Ok(path)
}

#[derive(Clone, PartialEq, Eq)]
struct Entry {
    path: PathBuf,
    directory: bool,
}
fn read_directory(root: &Path, path: &Path) -> Result<Vec<Entry>, String> {
    let path = checked_path(root, path)?;
    let mut entries = Vec::new();
    for item in fs_err::read_dir(path)
        .map_err(|e| e.to_string())?
        .take(MAX_DIRECTORY_ENTRIES + 1)
    {
        if entries.len() == MAX_DIRECTORY_ENTRIES {
            return Err("This folder has more than 2,000 entries. Open a smaller folder.".into());
        }
        let item = item.map_err(|e| e.to_string())?;
        if item.file_name() == ".git" {
            continue;
        }
        // Do not follow symlink directories, avoiding cycles and checkout escapes.
        let metadata = item.file_type().map_err(|e| e.to_string())?;
        entries.push(Entry {
            path: item.path(),
            directory: metadata.is_dir(),
        });
    }
    entries.sort_by(|a, b| {
        b.directory
            .cmp(&a.directory)
            .then_with(|| a.path.file_name().cmp(&b.path.file_name()))
    });
    Ok(entries)
}

enum Content {
    Loading,
    Text {
        text: SharedString,
        language: &'static str,
    },
    Image(Arc<gpui::Image>),
    Error(String),
}
fn language(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "rs" => "rust",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "ts" | "tsx" => "typescript",
        "json" => "json",
        "toml" => "toml",
        "md" | "markdown" => "markdown",
        "py" => "python",
        "go" => "go",
        "sh" | "bash" | "zsh" => "bash",
        "html" | "htm" => "html",
        "css" => "css",
        "yaml" | "yml" => "yaml",
        "xml" | "svg" => "xml",
        "c" | "h" => "c",
        "cpp" | "hpp" | "cc" => "cpp",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "swift" => "swift",
        "sql" => "sql",
        "rb" => "ruby",
        _ => "plain",
    }
}
fn read_content(root: &Path, path: &Path, transcript_link: bool) -> Result<Content, String> {
    let path = if transcript_link {
        path.canonicalize()
            .map_err(|e| format!("File unavailable: {e}"))?
    } else {
        checked_path(root, path)?
    };
    if !fs_err::metadata(&path)
        .map_err(|e| e.to_string())?
        .is_file()
    {
        return Err("Choose a regular file to preview.".into());
    }
    let mut file = fs_err::File::open(&path).map_err(|e| e.to_string())?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err("Choose a regular file to preview.".into());
    }
    let image_format = match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => Some(gpui::ImageFormat::Png),
        "jpg" | "jpeg" => Some(gpui::ImageFormat::Jpeg),
        "gif" => Some(gpui::ImageFormat::Gif),
        "webp" => Some(gpui::ImageFormat::Webp),
        _ => None,
    };
    let limit = if image_format.is_some() {
        MAX_IMAGE_BYTES
    } else {
        MAX_FILE_BYTES
    };
    if metadata.len() > limit {
        return Err(format!(
            "Preview unavailable: this file exceeds the {} MiB limit.",
            limit / (1024 * 1024)
        ));
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err(
            "The file grew beyond the preview limit. Try again after it stops changing.".into(),
        );
    }
    if let Some(format) = image_format {
        // Reject oversized decoded images before handing data to the renderer.
        let reader = image::ImageReader::new(std::io::Cursor::new(&bytes))
            .with_guessed_format()
            .map_err(|e| e.to_string())?;
        let (w, h) = reader.into_dimensions().map_err(|e| e.to_string())?;
        if u64::from(w) * u64::from(h) > 40_000_000 {
            return Err("Image preview exceeds the 40 megapixel limit.".into());
        }
        return Ok(Content::Image(Arc::new(gpui::Image::from_bytes(
            format, bytes,
        ))));
    }
    if bytes.contains(&0) {
        return Err("Preview unavailable for binary files.".into());
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| "Preview unavailable: this file is not UTF-8 text.".to_owned())?;
    if text.lines().count() > 20_000 {
        return Err("Preview unavailable: this file has more than 20,000 lines.".into());
    }
    Ok(Content::Text {
        text: text.into(),
        language: language(&path),
    })
}

pub(super) enum FileBrowserEvent {
    Opened,
    Closed,
}
pub(super) struct FileBrowser {
    pub(super) root: PathBuf,
    directories: HashMap<PathBuf, Result<Vec<Entry>, String>>,
    expanded: HashSet<PathBuf>,
    pending: HashSet<PathBuf>,
    pub target: Option<FileTarget>,
    content: Content,
    pub visible: bool,
    source: bool,
    generation: u64,
    root_generation: u64,
    focus: FocusHandle,
    back_focus: FocusHandle,
    source_focus: FocusHandle,
    browser_focus: FocusHandle,
    refresh_task: Option<gpui::Task<()>>,
    explorer_seen: std::time::Instant,
    return_focus: Option<FocusHandle>,
    row_focus: HashMap<PathBuf, FocusHandle>,
    scroll: ScrollHandle,
    explorer_scroll: ScrollHandle,
    pending_line: Option<usize>,
}
impl gpui::EventEmitter<FileBrowserEvent> for FileBrowser {}
impl FileBrowser {
    pub fn new(root: PathBuf, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            root: root.clone(),
            directories: HashMap::new(),
            expanded: HashSet::from([root.clone()]),
            pending: HashSet::new(),
            target: None,
            content: Content::Loading,
            visible: false,
            source: false,
            generation: 0,
            root_generation: 0,
            focus: cx.focus_handle(),
            back_focus: cx.focus_handle(),
            source_focus: cx.focus_handle(),
            browser_focus: cx.focus_handle(),
            refresh_task: None,
            explorer_seen: std::time::Instant::now(),
            return_focus: None,
            row_focus: HashMap::new(),
            scroll: ScrollHandle::new(),
            explorer_scroll: ScrollHandle::new(),
            pending_line: None,
        };
        this.load_directory(root, cx);
        this
    }
    fn open_in_browser(&self, cx: &mut Context<Self>) {
        let Some(target) = &self.target else {
            return;
        };
        let Ok(url) = gpui::http_client::Url::from_file_path(&target.path) else {
            return;
        };
        cx.spawn(async move |_, _| {
            if let Err(error) = crate::platform::inline_html::open_in_browser(url.to_string()).await
            {
                eprintln!("Couldn’t open in browser: {error}");
            }
        })
        .detach();
    }

    #[cfg(test)]
    pub fn loaded_text(&self) -> Option<&str> {
        match &self.content {
            Content::Text { text, .. } => Some(text.as_ref()),
            _ => None,
        }
    }
    pub fn set_root(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        if self.root == root {
            return;
        }
        self.root.clone_from(&root);
        self.root_generation = self.root_generation.wrapping_add(1);
        self.generation = self.generation.wrapping_add(1);
        self.visible = false;
        self.target = None;
        self.return_focus = None;
        self.directories.clear();
        self.pending.clear();
        self.expanded = HashSet::from([root.clone()]);
        self.row_focus.clear();
        self.load_directory(root, cx);
        cx.notify();
    }
    fn load_directory(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !self.pending.insert(path.clone()) {
            return;
        }
        let root = self.root.clone();
        let generation = self.root_generation;
        let requested = path.clone();
        let work = cx
            .background_executor()
            .spawn(async move { read_directory(&root, &path) });
        // The generation belongs to this entity/root; a removed browser cannot receive results.
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.root_generation == generation {
                    this.pending.remove(&requested);
                    if this.directories.get(&requested) != Some(&result) {
                        this.directories.insert(requested, result);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }
    pub fn open(&mut self, mut target: FileTarget, window: &mut Window, cx: &mut Context<Self>) {
        if !self.visible {
            self.return_focus = window.focused(cx);
        }
        // Reveal checkout files using the same canonical paths as directory rows.
        // Explicit outside-checkout links never broaden the explorer root.
        if let Ok(path) = checked_path(&self.root, &target.path) {
            target.path.clone_from(&path);
            if let Ok(root) = self.root.canonicalize() {
                for parent in path
                    .ancestors()
                    .skip(1)
                    .take_while(|p| p.starts_with(&root))
                {
                    self.expanded.insert(parent.to_path_buf());
                    self.load_directory(parent.to_path_buf(), cx);
                }
            }
        }
        self.visible = true;
        self.source = target.line.is_some();
        self.pending_line = target.line;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.target = Some(target.clone());
        self.content = Content::Loading;
        self.scroll
            .set_offset(gpui::point(gpui::px(0.), gpui::px(0.)));
        let root = self.root.clone();
        let path = target.path.clone();
        let work = cx
            .background_executor()
            .spawn(async move { read_content(&root, &path, target.transcript_link) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.generation == generation {
                    this.content = result.unwrap_or_else(Content::Error);
                    cx.notify();
                }
            });
        })
        .detach();
        self.focus.focus(window, cx);
        cx.emit(FileBrowserEvent::Opened);
        cx.notify();
    }
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.visible = false;
        if let Some(focus) = self.return_focus.take() {
            focus.focus(window, cx);
        }
        cx.emit(FileBrowserEvent::Closed);
        cx.notify();
    }
    fn toggle_directory(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !self.expanded.remove(&path) {
            self.expanded.insert(path.clone());
            self.load_directory(path, cx);
        }
        cx.notify();
    }
    fn rows(&mut self, path: &Path, depth: usize, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(entries) = self.directories.get(path).cloned() else {
            return vec![div()
                .px(rpx(SPACE_LG))
                .text_color(c::FG_DIM())
                .child("Loading files…")
                .into_any_element()];
        };
        let entries = match entries {
            Ok(entries) => entries,
            Err(error) => {
                return vec![div()
                    .p(rpx(SPACE_LG))
                    .text_color(c::FORM_ERROR())
                    .child(error)
                    .into_any_element()]
            }
        };
        let mut rows = Vec::new();
        for entry in entries {
            let name = entry
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let expanded = self.expanded.contains(&entry.path);
            let focus = self
                .row_focus
                .entry(entry.path.clone())
                .or_insert_with(|| cx.focus_handle())
                .clone();
            let path = entry.path.clone();
            let keyboard_path = path.clone();
            let directory = entry.directory;
            let mouse_focus = focus.clone();
            let selected = self
                .target
                .as_ref()
                .is_some_and(|target| target.path == path);
            let (glyph, color) = file_icon(&path, directory, expanded);
            let row = header_control_frame(
                SharedString::from(format!("file-row-{}", path.display())),
                SharedString::from(name.clone()),
                c::BG_HOVER(),
            )
            .debug_selector(move || format!("file-row-{name}"))
            .track_focus(&focus)
            .tab_index(0)
            .focus_visible(|s| s.bg(c::BG_HOVER()))
            .w_full()
            .h_auto()
            .min_h(rpx(CONTROL_H))
            .pl(rpx(SPACE_LG + depth as f32 * SPACE_2XL))
            .pr(rpx(SPACE_LG))
            .gap(rpx(SPACE_SM))
            .justify_start()
            .when(selected, |row| row.bg(c::BG_HOVER()))
            .when(directory, |row| row.aria_expanded(expanded))
            .child(icon(glyph, ICON_MD, color))
            .child(
                div().flex_1().min_w_0().truncate().child(
                    entry
                        .path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                ),
            )
            .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                mouse_focus.focus(window, cx);
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                if directory {
                    this.toggle_directory(path.clone(), cx);
                } else {
                    this.open(
                        FileTarget {
                            path: path.clone(),
                            line: None,
                            transcript_link: false,
                        },
                        window,
                        cx,
                    );
                }
            }))
            .on_key_down(cx.listener(
                move |this, event: &gpui::KeyDownEvent, window, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") && !event.is_held {
                        if directory {
                            this.toggle_directory(keyboard_path.clone(), cx);
                        } else {
                            this.open(
                                FileTarget {
                                    path: keyboard_path.clone(),
                                    line: None,
                                    transcript_link: false,
                                },
                                window,
                                cx,
                            );
                        }
                        window.prevent_default();
                        cx.stop_propagation();
                    }
                },
            ));
            rows.push(row.into_any_element());
            if directory && expanded {
                rows.extend(self.rows(&entry.path, depth + 1, cx));
            }
        }
        rows
    }
    pub fn explorer(
        &mut self,
        project: String,
        worktree: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.explorer_seen = std::time::Instant::now();
        if self.refresh_task.is_none() {
            // Poll only expanded directories while the explorer is being displayed.
            // Retaining the task cancels it with the browser; root generations reject stale reads.
            self.refresh_task = Some(cx.spawn(async move |this, cx| loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(1))
                    .await;
                let active = this
                    .update(cx, |this, cx| {
                        if this.explorer_seen.elapsed() > std::time::Duration::from_secs(2) {
                            this.refresh_task = None;
                            return false;
                        }
                        cx.notify();
                        for path in this.expanded.clone() {
                            this.load_directory(path, cx);
                        }
                        true
                    })
                    .unwrap_or(false);
                if !active {
                    break;
                }
            }));
            for path in self.expanded.clone() {
                self.load_directory(path, cx);
            }
        }
        let root = self.root.clone();
        let rows = self.rows(&root, 0, cx);
        div()
            .debug_selector(|| "file-explorer".into())
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(c::BG())
            // Reserve the toolbar row; persistent side-container controls own its actions.
            .child(div().h(rpx(APPBAR_H)).flex_shrink_0())
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .p(rpx(SPACE_3XL))
                    .flex()
                    .flex_col()
                    .child(
                        project_field_well(false, false)
                            .debug_selector(|| "file-explorer-card".into())
                            .h_auto()
                            .flex_1()
                            .min_h_0()
                            .justify_start()
                            .px(rpx(SPACE_LG))
                            .py(rpx(SPACE_2XL))
                            .gap(rpx(SPACE_XS))
                            .overflow_hidden()
                            .child(
                                div()
                                    .debug_selector(|| "file-explorer-checkout".into())
                                    .px(rpx(SPACE_LG))
                                    .pb(rpx(SPACE_LG))
                                    .flex()
                                    .items_center()
                                    .gap(rpx(SPACE_SM))
                                    .text_size(rpx(TEXT_SMALL))
                                    .child(icon("git-branch", ICON_MD, c::FG_DIM()))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_color(c::FG_DIM())
                                            .child(project),
                                    )
                                    .child(div().text_color(c::FG_DIM()).child("/"))
                                    .child(div().min_w_0().truncate().child(worktree)),
                            )
                            .child(
                                div()
                                    .id("file-explorer-scroll")
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    .track_scroll(&self.explorer_scroll)
                                    .children(rows),
                            ),
                    ),
            )
            .into_any_element()
    }
}
pub(super) fn file_icon(
    path: &Path,
    directory: bool,
    expanded: bool,
) -> (&'static str, gpui::Hsla) {
    if directory {
        return (if expanded { "folder-open" } else { "folder" }, c::AMBER());
    }
    let color = match language(path) {
        "rust" => c::AMBER(),
        "markdown" | "typescript" => c::BLUE(),
        "toml" | "json" | "yaml" => c::GREEN(),
        "python" | "javascript" => c::AMBER(),
        _ => c::FG_DIM(),
    };
    ("file", color)
}
impl Focusable for FileBrowser {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Render for FileBrowser {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let path = self
            .target
            .as_ref()
            .map(|t| {
                t.path
                    .strip_prefix(&self.root)
                    .unwrap_or(&t.path)
                    .display()
                    .to_string()
            })
            .unwrap_or_default();
        let markdown = matches!(
            &self.content,
            Content::Text {
                language: "markdown",
                ..
            }
        );
        let content = match &self.content {
            Content::Loading => div()
                .p(rpx(SPACE_3XL))
                .text_color(c::FG_DIM())
                .child("Loading file…")
                .into_any_element(),
            Content::Error(error) => div()
                .p(rpx(SPACE_3XL))
                .text_color(c::FG_DIM())
                .child(error.clone())
                .into_any_element(),
            Content::Image(image) => div()
                .p(rpx(SPACE_3XL))
                .child(
                    gpui::img(image.clone())
                        .max_w_full()
                        .object_fit(gpui::ObjectFit::Contain),
                )
                .into_any_element(),
            Content::Text { text, language } => {
                let body = if *language == "markdown" && !self.source {
                    text.to_string()
                } else {
                    let fence = "`".repeat(
                        text.split(|c| c != '`')
                            .map(str::len)
                            .max()
                            .unwrap_or(0)
                            .max(2)
                            + 1,
                    );
                    format!("{fence}{language}\n{text}\n{fence}")
                };
                if let Some(line) = self.pending_line.take() {
                    if *language != "markdown" || self.source {
                        let y = (line.saturating_sub(1) as f32 * SOURCE_LINE_HEIGHT)
                            .min(text.lines().count() as f32 * SOURCE_LINE_HEIGHT);
                        self.scroll.set_offset(gpui::point(
                            gpui::px(0.),
                            -rpx(y).to_pixels(window.rem_size()),
                        ));
                    }
                }
                {
                    let code = *language != "markdown" || self.source;
                    let mut style = super::codex_view::markdown_style(window);
                    if code {
                        style.code_block = div()
                            .p_0()
                            .bg(c::BG())
                            .text_color(c::FG())
                            .text_size(rpx(TEXT_CODE))
                            .line_height(rpx(SOURCE_LINE_HEIGHT))
                            .whitespace_nowrap()
                            .style()
                            .clone();
                    }
                    let view = TextView::markdown("file-content-text", body)
                        .style(style)
                        .selectable(true);
                    let gutter = div()
                        .flex_shrink_0()
                        .text_size(rpx(TEXT_CODE))
                        .font_family(crate::fonts::MONO_FAMILY)
                        .text_color(c::FG_DIM())
                        .pr(rpx(SPACE_2XL))
                        .children((1..=text.split('\n').count()).map(|n| {
                            div()
                                .h(rpx(SOURCE_LINE_HEIGHT))
                                .text_right()
                                .when(
                                    self.target.as_ref().and_then(|t| t.line) == Some(n),
                                    |row| row.bg(c::BG_HOVER()).text_color(c::BLUE()),
                                )
                                .child(n.to_string())
                        }));
                    div()
                        .p(rpx(SPACE_3XL))
                        .flex()
                        .items_start()
                        .when(code, |row| row.child(gutter))
                        .child(div().flex_1().min_w_0().child(view))
                        .into_any_element()
                }
            }
        };
        div()
            .id("file-viewer")
            .debug_selector(|| "file-viewer".into())
            .track_focus(&self.focus)
            .key_context("FileViewer")
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(c::BG())
            .occlude()
            .capture_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.close(window, cx);
                    window.prevent_default();
                    cx.stop_propagation();
                } else if matches!(event.keystroke.key.as_str(), "enter" | "space")
                    && !event.is_held
                {
                    if this.back_focus.is_focused(window) {
                        this.close(window, cx);
                        window.prevent_default();
                        cx.stop_propagation();
                    } else if this.browser_focus.is_focused(window) {
                        this.open_in_browser(cx);
                        window.prevent_default();
                        cx.stop_propagation();
                    } else if this.source_focus.is_focused(window) {
                        this.source = !this.source;
                        cx.notify();
                        window.prevent_default();
                        cx.stop_propagation();
                    }
                } else if event.keystroke.key == "tab" {
                    if matches!(
                        &this.content,
                        Content::Text {
                            language: "markdown",
                            ..
                        }
                    ) && !this.source_focus.is_focused(window)
                    {
                        this.source_focus.focus(window, cx);
                    } else if this.target.as_ref().is_some_and(|t| {
                        crate::platform::inline_html::browser_supported_path(&t.path)
                    }) && !this.browser_focus.is_focused(window)
                    {
                        this.browser_focus.focus(window, cx);
                    } else {
                        this.back_focus.focus(window, cx);
                    }
                    window.prevent_default();
                    cx.stop_propagation();
                }
            }))
            .child(
                super::components::content_header()
                    .debug_selector(|| "file-viewer-header".into())
                    .px(rpx(SPACE_LG))
                    .py(rpx(SPACE_LG))
                    .child(
                        header_control_with_label_tooltip("file-back", "Back to conversation")
                            .debug_selector(|| "file-viewer-back".into())
                            .track_focus(&self.back_focus)
                            .tab_index(0)
                            .focus_visible(|s| s.bg(c::BG_HOVER()))
                            .child(icon("arrow-left", ICON_SM, c::FG_DIM()))
                            .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))),
                    )
                    .child(
                        div()
                            .id("file-viewer-path")
                            .debug_selector(|| "file-viewer-path".into())
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(path.clone())
                            .tooltip(move |window, cx| {
                                super::components::tooltip(path.clone(), window).build(window, cx)
                            }),
                    )
                    .when(
                        self.target.as_ref().is_some_and(|target| {
                            crate::platform::inline_html::browser_supported_path(&target.path)
                        }),
                        |row| {
                            row.child(
                                header_control_with_label_tooltip(
                                    "file-browser-open",
                                    "Open in browser",
                                )
                                .child(icon("external-link", ICON_SM, c::FG_DIM()))
                                .debug_selector(|| "file-viewer-browser".into())
                                .track_focus(&self.browser_focus)
                                .tab_index(0)
                                .focus_visible(|s| s.bg(c::BG_HOVER()))
                                .on_click(cx.listener(|this, _, _, cx| this.open_in_browser(cx))),
                            )
                        },
                    )
                    .when(markdown, |row| {
                        row.child(
                            transcript_action(
                                "file-source",
                                if self.source { "Preview" } else { "Source" },
                                false,
                                window,
                                cx,
                            )
                            .icon(
                                gpui_component::Icon::default()
                                    .path(if self.source {
                                        "icons/eye.svg"
                                    } else {
                                        "icons/file.svg"
                                    })
                                    .size(rpx(ICON_SM)),
                            )
                            .debug_selector(|| "file-viewer-source".into())
                            .with_focus_handle(self.source_focus.clone())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.source = !this.source;
                                cx.notify();
                            })),
                        )
                    }),
            )
            .child(
                div()
                    .id("file-content-scroll")
                    .debug_selector(|| "file-content-scroll".into())
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .overflow_x_scroll()
                    .track_scroll(&self.scroll)
                    .child(content),
            )
            .child(
                div()
                    .px(rpx(SPACE_2XL))
                    .py(rpx(SPACE_SM))
                    .text_size(rpx(TEXT_MICRO))
                    .text_color(c::FG_DIM())
                    .child(format!(
                        "{} · Read-only{}",
                        self.target.as_ref().map_or("File", |t| language(&t.path)),
                        self.target
                            .as_ref()
                            .and_then(|t| t.line)
                            .map_or(String::new(), |n| format!(" · Line {n}"))
                    )),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[gpui::test]
    fn opening_reveals_nested_checkout_files_without_expanding_outside_root(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_component::init);
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let nested = root.join("src/views");
        fs_err::create_dir_all(&nested).unwrap();
        let file = nested.join("example.rs");
        fs_err::write(&file, "example").unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        let (browser, cx) =
            cx.add_window_view(|window, cx| FileBrowser::new(root.clone(), window, cx));
        cx.update(|window, cx| {
            browser.update(cx, |browser, cx| {
                browser.open(
                    local_target(file.to_str().unwrap(), &root).unwrap(),
                    window,
                    cx,
                );
                assert!(browser.expanded.contains(&root.join("src")));
                assert!(browser.expanded.contains(&nested));
                assert_eq!(browser.target.as_ref().unwrap().path, file);
                let expanded = browser.expanded.clone();
                browser.open(
                    local_target(outside.path().to_str().unwrap(), &root).unwrap(),
                    window,
                    cx,
                );
                assert_eq!(browser.expanded, expanded);
                assert_eq!(browser.root, root);
            });
        });
        cx.run_until_parked();
        assert!(
            browser.read_with(cx, |browser, _| browser.directories[&nested]
                .as_ref()
                .unwrap()
                .iter()
                .any(|entry| entry.path == file))
        );
    }

    #[gpui::test]
    fn explorer_automatically_refreshes_without_resetting_expansion(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let root = tempfile::tempdir().unwrap();
        // Directory entries use checked_path's canonical checkout path. macOS
        // temporary paths may begin at the /var alias for /private/var.
        let root_path = fs_err::canonicalize(root.path()).unwrap();
        let nested = root_path.join("nested");
        fs_err::create_dir(&nested).unwrap();
        let (browser, cx) =
            cx.add_window_view(|window, cx| FileBrowser::new(root_path, window, cx));
        cx.run_until_parked();
        cx.update(|window, cx| {
            browser.update(cx, |browser, cx| {
                browser.toggle_directory(nested.clone(), cx);
                let _ = browser.explorer("Project".into(), "Checkout".into(), window, cx);
            });
        });
        cx.run_until_parked();
        let first = nested.join("created.rs");
        let renamed = nested.join("renamed.rs");
        for (before, after) in [
            (None, Some(&first)),
            (Some(&first), Some(&renamed)),
            (Some(&renamed), None),
        ] {
            match (before, after) {
                (None, Some(path)) => fs_err::write(path, "fn main() {}").unwrap(),
                (Some(old), Some(new)) => fs_err::rename(old, new).unwrap(),
                (Some(path), None) => fs_err::remove_file(path).unwrap(),
                _ => unreachable!(),
            }
            cx.executor()
                .advance_clock(std::time::Duration::from_secs(1));
            cx.run_until_parked();
            browser.read_with(cx, |browser, _| {
                assert!(browser.expanded.contains(&nested));
                let entries = browser.directories.get(&nested).unwrap().as_ref().unwrap();
                assert_eq!(
                    entries.iter().map(|entry| &entry.path).collect::<Vec<_>>(),
                    after.into_iter().collect::<Vec<_>>()
                );
            });
        }
    }
    #[gpui::test]
    fn narrow_file_header_keeps_back_path_and_source_inside_viewer(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("long-file-name-for-narrow-review.md");
        let (browser, cx) = cx.add_window_view(|window, cx| {
            let mut browser = FileBrowser::new(root.path().to_owned(), window, cx);
            browser.target = Some(FileTarget {
                path,
                line: None,
                transcript_link: false,
            });
            browser.content = Content::Text {
                text: "# Narrow header".into(),
                language: "markdown",
            };
            browser.visible = true;
            browser
        });
        for (width, rem) in [(240., 16.), (240., 20.), (500., 16.)] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(600.)));
            cx.update(|window, cx| {
                window.set_rem_size(gpui::px(rem));
                browser.update(cx, |_, cx| cx.notify());
                let _ = window.draw(cx);
            });
            let viewer = cx.debug_bounds("file-viewer").unwrap();
            let back = cx.debug_bounds("file-viewer-back").unwrap();
            let path = cx.debug_bounds("file-viewer-path").unwrap();
            let source = cx.debug_bounds("file-viewer-source").unwrap();
            for bounds in [back, path, source] {
                assert!(bounds.left()>=viewer.left() && bounds.right()<=viewer.right(),"header control clipped at width={width} rem={rem}: {bounds:?} outside {viewer:?}");
            }
            assert!(path.size.width > gpui::px(0.));
            assert!((source.center().y - back.center().y).abs() <= gpui::px(0.5));
            assert!((path.center().y - back.center().y).abs() <= gpui::px(0.5));
            assert!(back.right() <= path.left());
            assert!(path.right() <= source.left());
        }
    }
    #[gpui::test]
    fn browser_icon_aligns_with_back_at_narrow_widths_and_zoom(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("long-file-name-for-narrow-review.html");
        let (browser, cx) = cx.add_window_view(|window, cx| {
            let mut browser = FileBrowser::new(root.path().to_owned(), window, cx);
            browser.target = Some(FileTarget {
                path,
                line: None,
                transcript_link: false,
            });
            browser.content = Content::Text {
                text: "# Narrow header".into(),
                language: "html",
            };
            browser.visible = true;
            browser
        });
        for (width, rem) in [(240., 16.), (240., 20.), (500., 16.)] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(600.)));
            cx.update(|window, cx| {
                window.set_rem_size(gpui::px(rem));
                browser.update(cx, |_, cx| cx.notify());
                let _ = window.draw(cx);
            });
            let viewer = cx.debug_bounds("file-viewer").unwrap();
            let back = cx.debug_bounds("file-viewer-back").unwrap();
            let path = cx.debug_bounds("file-viewer-path").unwrap();
            let source = cx.debug_bounds("file-viewer-browser").unwrap();
            for bounds in [back, path, source] {
                assert!(bounds.left()>=viewer.left() && bounds.right()<=viewer.right(),"header control clipped at width={width} rem={rem}: {bounds:?} outside {viewer:?}");
            }
            assert!(path.size.width > gpui::px(0.));
            assert!((source.center().y - back.center().y).abs() <= gpui::px(0.5));
            assert!((path.center().y - back.center().y).abs() <= gpui::px(0.5));
            assert!(back.right() <= path.left());
            assert!(path.right() <= source.left());
        }
    }
    #[gpui::test]
    fn checkout_changes_discard_inflight_file_and_folder_results(cx: &mut gpui::TestAppContext) {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let file = first.path().join("first.rs");
        fs_err::write(&file, "fn first() {}\n").unwrap();
        cx.update(gpui_component::init);
        let (browser, cx) =
            cx.add_window_view(|window, cx| FileBrowser::new(first.path().to_owned(), window, cx));
        cx.update(|window, cx| {
            browser.update(cx, |browser, cx| {
                browser.open(
                    FileTarget {
                        path: file,
                        line: Some(1),
                        transcript_link: false,
                    },
                    window,
                    cx,
                );
                browser.set_root(second.path().to_owned(), cx);
            });
        });
        cx.run_until_parked();
        browser.read_with(cx, |browser, _| {
            assert!(!browser.visible);
            assert!(browser.target.is_none());
            assert!(browser
                .directories
                .keys()
                .all(|path| path.starts_with(second.path())));
            assert!(browser.pending.is_empty());
        });
    }
    #[cfg(unix)]
    #[test]
    fn nonregular_files_are_rejected_before_opening() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("pipe");
        assert!(std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap()
            .success());
        assert!(read_content(root.path(), &path, false).is_err());
    }
    #[test]
    fn local_links_decode_and_keep_lines_out_of_paths() {
        let checkout = tempfile::tempdir().unwrap();
        let root = checkout.path();
        let spaced = root.join("a b.rs");
        let file_url = gpui::http_client::Url::from_file_path(&spaced).unwrap();
        for (target, path, line) in [
            (
                "src/main.rs:42".to_owned(),
                root.join("src/main.rs"),
                Some(42),
            ),
            (
                "src/main.rs:42:7".to_owned(),
                root.join("src/main.rs"),
                Some(42),
            ),
            (
                "src/main.rs#L12".to_owned(),
                root.join("src/main.rs"),
                Some(12),
            ),
            (format!("{file_url}#L9"), spaced.clone(), Some(9)),
            (
                format!("{}#L11", spaced.display()),
                spaced.clone(),
                Some(11),
            ),
        ] {
            let result = local_target(&target, root).unwrap();
            assert_eq!(result.path, path);
            assert_eq!(result.line, line);
        }
        for target in [
            "https://example.com/file.rs",
            "mailto:a@b.com",
            "//server/file",
        ] {
            assert!(local_target(target, root).is_none());
        }
    }

    #[test]
    fn explicit_transcript_file_reads_do_not_enable_external_directory_browsing() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let path = outside.path().join("output.md");
        fs_err::write(&path, "# Selected file").unwrap();
        assert!(read_content(root.path(), &path, false).is_err());
        assert!(matches!(
            read_content(root.path(), &path, true).unwrap(),
            Content::Text { .. }
        ));
        assert!(read_directory(root.path(), outside.path()).is_err());
        assert!(read_content(root.path(), outside.path(), true).is_err());
    }
    #[test]
    fn reads_reject_escape_binary_and_large_files() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        assert!(checked_path(root.path(), outside.path()).is_err());
        let binary = root.path().join("binary");
        fs_err::write(&binary, b"a\0b").unwrap();
        assert!(read_content(root.path(), &binary, false).is_err());
        let large = root.path().join("large.rs");
        let f = fs_err::File::create(&large).unwrap();
        f.set_len(MAX_FILE_BYTES + 1).unwrap();
        assert!(read_content(root.path(), &large, false).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        let link = root.path().join("link.rs");
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();
        assert!(read_content(root.path(), &link, false).is_err());
    }
}
