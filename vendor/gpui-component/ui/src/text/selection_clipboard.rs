use gpui::{App, ClipboardItem, Global, actions};

use crate::Root;

actions!(text_view, [CopyAsMarkdown]);

type WriteHtml = fn(html: &str, plain: &str) -> bool;
struct RichClipboardWriter(WriteHtml);

impl Global for RichClipboardWriter {}

/// Install the host's OS writer for a single clipboard item with both formats.
pub fn set_rich_clipboard_writer(cx: &mut App, writer: WriteHtml) {
    cx.set_global(RichClipboardWriter(writer));
}

impl Root {
    pub(crate) fn copy_text_selection(&self, as_markdown: bool, cx: &mut App) -> bool {
        let Some(selection) = self.window_text_selection_snapshot(cx) else {
            return false;
        };
        let writer = cx
            .try_global::<RichClipboardWriter>()
            .map(|writer| writer.0);
        if (as_markdown || writer.is_some())
            && let Some(markdown) = self.window_selected_markdown(&selection, cx)
        {
            if as_markdown {
                cx.write_to_clipboard(ClipboardItem::new_string(markdown));
                return true;
            }
            // Default GFM compilation escapes HTML and filters unsafe link schemes.
            if let Some(write) = writer
                && let Ok(html) =
                    markdown::to_html_with_options(&markdown, &markdown::Options::gfm())
                && write(&html, &selection.text)
            {
                return true;
            }
        }
        cx.write_to_clipboard(ClipboardItem::new_string(selection.text));
        true
    }

    fn window_selected_markdown(
        &self,
        selection: &super::TextSelectionSnapshot,
        cx: &App,
    ) -> Option<String> {
        let mut formatted = false;
        let parts: Vec<_> = selection
            .spans
            .iter()
            .map(|span| {
                let markdown = self
                    .selectable_text_views
                    .get(&span.owner)
                    .and_then(|(view, _, _)| view.upgrade())
                    .and_then(|view| view.read(cx).selected_markdown());
                formatted |= markdown.is_some();
                markdown.unwrap_or_else(|| {
                    super::selection_markdown::literal_markdown(span.text.trim_end_matches('\n'))
                })
            })
            .collect();
        formatted.then(|| parts.join("\n\n"))
    }
}
