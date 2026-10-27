//! Source edits for the composer's opt-in Markdown newline behavior.
use std::ops::Range;

/// Return one atomic edit, keeping suffix text and all offsets in source bytes.
/// Selections use ordinary replacement rather than guessing a new block context.
pub(super) fn newline_edit(source: &str, selection: Range<usize>) -> (Range<usize>, String) {
    let plain = || (selection.clone(), "\n".to_owned());
    if !selection.is_empty() {
        return plain();
    }
    let caret = selection.start;
    let start = source[..caret].rfind('\n').map_or(0, |i| i + 1);
    let end = source[caret..]
        .find('\n')
        .map_or(source.len(), |i| caret + i);
    let line = &source[start..end];
    let before = &source[start..caret];
    let mut fence = None;
    for previous in source[..start].lines() {
        update_fence(previous, &mut fence);
    }
    // Fence delimiters are never list content. Inside a fence, preserve only
    // quote context and literal indentation, even when code starts with '- '.
    reset_fence_context(line, &mut fence);
    let was_fenced = fence.is_some();
    let delimiter = update_fence(line, &mut fence);
    let (quote_end, content) = quote_context(before);
    let indent_len = content.len() - content.trim_start_matches([' ', '\t']).len();
    let content_start = quote_end + indent_len;
    if was_fenced || delimiter {
        let extra = fence.map_or(0, |(_, _, _, minimum_indent)| {
            minimum_indent.saturating_sub(indent_len)
        });
        return (
            selection,
            format!("\n{}{}", &before[..content_start], " ".repeat(extra)),
        );
    }

    // A standalone four-space/tab-indented block is literal Markdown code.
    if (content[..indent_len].contains('\t') || indent_len >= 4)
        && !has_list_parent(&source[..start], indent_len)
    {
        return (selection, format!("\n{}", &before[..content_start]));
    }
    let rest = &before[content_start..];
    if let Some((mut marker_end, mut next_marker)) = list_prefix(rest) {
        let task = &rest[marker_end..];
        if task.len() >= 4
            && matches!(
                &task.as_bytes()[..4],
                [b'[', b' ' | b'x' | b'X', b']', b' ']
            )
        {
            marker_end += 4;
            next_marker.push_str("[ ] ");
        }
        // Empty items exit one structure without consuming the parent quote.
        if line[content_start + marker_end..].trim().is_empty() {
            return (start + content_start..end, String::new());
        }
        return (
            selection,
            format!("\n{}{next_marker}", &before[..content_start]),
        );
    }
    if quote_end > 0 {
        if line[quote_end..].trim().is_empty() {
            // Remove the innermost quote marker, retaining enclosing quotes.
            let prefix = &line[..quote_end];
            let last = prefix.rfind('>').unwrap_or(0);
            return (start + last..end, String::new());
        }
        return (selection, format!("\n{}", &before[..content_start]));
    }
    plain()
}

// A deeper list remains a list even when its indentation would start code at root.
fn has_list_parent(previous: &str, indentation: usize) -> bool {
    for line in previous
        .lines()
        .rev()
        .filter(|line| !line.trim().is_empty())
    {
        let (_, content) = quote_context(line);
        let trimmed = content.trim_start_matches([' ', '\t']);
        let indent = content.len() - trimmed.len();
        if indent >= indentation {
            continue;
        }
        return list_prefix(trimmed).is_some();
    }
    false
}

fn list_prefix(rest: &str) -> Option<(usize, String)> {
    let bytes = rest.as_bytes();
    let marker_end = if bytes.len() >= 2 && matches!(bytes[0], b'-' | b'*' | b'+') {
        1
    } else {
        let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
        if !(1..=9).contains(&digits) || !matches!(bytes.get(digits), Some(b'.' | b')')) {
            return None;
        }
        digits + 1
    };
    if !matches!(bytes.get(marker_end), Some(b' ' | b'\t')) {
        return None;
    }
    let end = marker_end + rest[marker_end..].len()
        - rest[marker_end..].trim_start_matches([' ', '\t']).len();
    if marker_end == 1 {
        return Some((end, rest[..end].to_owned()));
    }
    let number = rest[..marker_end - 1].parse::<u32>().ok()?;
    let next = number.checked_add(1).filter(|n| *n <= 999_999_999)?;
    Some((end, format!("{next}{}", &rest[marker_end - 1..end])))
}

/// Preserve exact whitespace and quote depth; leave list indentation in content.
fn quote_context(line: &str) -> (usize, &str) {
    let mut end = 0;
    loop {
        let tail = &line[end..];
        let spaces = tail.len() - tail.trim_start_matches(' ').len();
        if spaces > 3 || tail.as_bytes().get(spaces) != Some(&b'>') {
            break;
        }
        end += spaces + 1;
        if line.as_bytes().get(end) == Some(&b' ') {
            end += 1;
        }
    }
    (end, &line[end..])
}

// Fence state: delimiter, length, quote depth, minimum list-content indentation.
type Fence = (u8, usize, usize, usize);

fn reset_fence_context(line: &str, fence: &mut Option<Fence>) {
    let (quote_end, content) = quote_context(line);
    let depth = line[..quote_end].bytes().filter(|b| *b == b'>').count();
    let indent = content.len() - content.trim_start_matches([' ', '\t']).len();
    if fence.is_some_and(|(_, _, open_depth, minimum_indent)| {
        open_depth != depth || (!content.trim().is_empty() && indent < minimum_indent)
    }) {
        *fence = None;
    }
}

/// Recognize opening/closing fences, including quoted and list-nested code.
fn update_fence(line: &str, fence: &mut Option<Fence>) -> bool {
    reset_fence_context(line, fence);
    let (quote_end, content) = quote_context(line);
    let quote_depth = line[..quote_end].bytes().filter(|b| *b == b'>').count();
    let mut minimum_indent = content.len() - content.trim_start_matches([' ', '\t']).len();
    let mut content = content.trim_start_matches([' ', '\t']);
    if fence.is_none() {
        if let Some((end, _)) = list_prefix(content) {
            minimum_indent += end;
            content = &content[end..];
        } else {
            // Root/quoted fence indentation doesn't require indenting its body.
            minimum_indent = 0;
        }
    }
    let Some(&kind) = content.as_bytes().first() else {
        return false;
    };
    if !matches!(kind, b'`' | b'~') {
        return false;
    }
    let count = content.bytes().take_while(|b| *b == kind).count();
    if count < 3 {
        return false;
    }
    if let Some((open_kind, open_count, _, _)) = *fence {
        if kind == open_kind && count >= open_count && content[count..].trim().is_empty() {
            *fence = None;
            return true;
        }
        return false;
    }
    if kind == b'`' && content[count..].contains('`') {
        return false;
    }
    *fence = Some((kind, count, quote_depth, minimum_indent));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn insert(source: &str) -> String {
        let (range, text) = newline_edit(source, source.len()..source.len());
        let mut result = source.to_owned();
        result.replace_range(range, &text);
        result
    }

    #[test]
    fn continues_blocks_and_exits_empty_innermost_structure() {
        for (source, expected) in [
            ("- é🦀", "- é🦀\n- "),
            ("  + item", "  + item\n  + "),
            ("* [X] done", "* [X] done\n* [ ] "),
            ("9) item", "9) item\n10) "),
            ("> 2. [x] done", "> 2. [x] done\n> 3. [ ] "),
            ("> > quote", "> > quote\n> > "),
            ("- ", ""),
            ("> - [ ] ", "> "),
            ("> > ", "> "),
            ("## Heading", "## Heading\n"),
            ("ordinary", "ordinary\n"),
            ("---", "---\n"),
            ("-\titem", "-\titem\n-\t"),
            ("1.  item", "1.  item\n2.  "),
            ("999999999. item", "999999999. item\n"),
            ("    - code", "    - code\n    "),
            ("- parent\n    - child", "- parent\n    - child\n    - "),
            ("- parent\n\t- child", "- parent\n\t- child\n\t- "),
            ("> ```\n> code\n\n- item", "> ```\n> code\n\n- item\n- "),
            ("1234567890. item", "1234567890. item\n"),
            ("```rust", "```rust\n"),
            ("- ```rust", "- ```rust\n  "),
            ("- ```\n  - code", "- ```\n  - code\n  "),
            ("- ```\n  code\n- next", "- ```\n  code\n- next\n- "),
            (
                "- ```\n  code\n  ```\n- next",
                "- ```\n  code\n  ```\n- next\n- ",
            ),
            ("```\n  - code", "```\n  - code\n  "),
            ("~~~\n* code", "~~~\n* code\n"),
            ("```\n- code\n```\n- item", "```\n- code\n```\n- item\n- "),
            ("> ```\n>   - code", "> ```\n>   - code\n>   "),
        ] {
            assert_eq!(insert(source), expected, "{source:?}");
        }
    }

    #[test]
    fn preserves_midline_suffix_and_replaces_selection_literally() {
        let source = "- é🦀 tail";
        let caret = "- é🦀".len();
        let (range, text) = newline_edit(source, caret..caret);
        let mut result = source.to_owned();
        result.replace_range(range, &text);
        assert_eq!(result, "- é🦀\n-  tail");
        assert_eq!(newline_edit(source, 2..caret), (2..caret, "\n".into()));
        assert_eq!(newline_edit(source, 0..0), (0..0, "\n".into()));
    }
}
