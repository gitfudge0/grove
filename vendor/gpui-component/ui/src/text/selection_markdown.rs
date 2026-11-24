//! Serialize only the text runs covered by the native selection.
use std::{ops::Range, sync::Mutex};

use super::{
    inline::InlineState,
    node::{BlockNode, ColumnumnAlign, LinkMark, NodeContext, Paragraph, Table, TextMark},
};

pub(super) fn selected_markdown(nodes: &[BlockNode], all: bool, cx: &NodeContext) -> String {
    nodes
        .iter()
        .filter_map(|node| block(node, all, cx))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn block(node: &BlockNode, all: bool, cx: &NodeContext) -> Option<String> {
    let selection = match node {
        BlockNode::Root { children, .. } => selected_markdown(children, all, cx),
        BlockNode::Paragraph(text) => paragraph(text, all, cx),
        BlockNode::Heading {
            level, children, ..
        } => {
            let text = paragraph(children, all, cx);
            if text.is_empty() {
                return None;
            }
            format!("{} {}", "#".repeat(usize::from(*level)), text)
        }
        BlockNode::Blockquote { children, .. } => {
            let text = selected_markdown(children, all, cx);
            if text.is_empty() {
                return None;
            }
            text.split('\n')
                .map(|line| {
                    if line.is_empty() {
                        ">".into()
                    } else {
                        format!("> {line}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        BlockNode::List {
            children, ordered, ..
        } => children
            .iter()
            .enumerate()
            .filter_map(|(index, node)| {
                let item = block(node, all, cx)?;
                let prefix = if *ordered {
                    format!("{}. ", index + 1)
                } else {
                    "- ".into()
                };
                let indent = " ".repeat(prefix.len());
                let mut lines = item.split('\n');
                let mut output = format!("{prefix}{}", lines.next()?);
                for line in lines {
                    output.push('\n');
                    if !line.is_empty() {
                        output.push_str(&indent);
                        output.push_str(line);
                    }
                }
                Some(output)
            })
            .collect::<Vec<_>>()
            .join("\n"),
        BlockNode::ListItem {
            children,
            checked,
            spread,
            ..
        } => {
            let parts: Vec<_> = children
                .iter()
                .filter_map(|node| block(node, all, cx))
                .collect();
            if parts.is_empty() {
                return None;
            }
            let prefix = match checked {
                Some(true) => "[x] ",
                Some(false) => "[ ] ",
                None => "",
            };
            format!(
                "{prefix}{}",
                parts.join(if *spread { "\n\n" } else { "\n" })
            )
        }
        BlockNode::CodeBlock(code) => {
            let text = if all {
                code.code().to_string()
            } else {
                code.selected_text()
            };
            if text.is_empty() {
                return None;
            }
            let fence = "`".repeat(3.max(backticks(&text) + 1));
            let lang = code
                .lang()
                .filter(|lang| !lang.contains(['`', '\r', '\n']))
                .unwrap_or_default();
            format!("{fence}{lang}\n{text}\n{fence}")
        }
        BlockNode::Table(table) => table_markdown(table, all, cx)?,
        BlockNode::HorizontalRule { .. } if all => "---".into(),
        _ => return None,
    };
    (!selection.is_empty()).then_some(selection)
}

fn table_markdown(table: &Table, all: bool, cx: &NodeContext) -> Option<String> {
    let mut rows: Vec<_> = table
        .children
        .iter()
        .enumerate()
        .filter_map(|(index, row)| {
            let cells: Vec<_> = row
                .children
                .iter()
                .map(|cell| {
                    paragraph(&cell.children, all, cx)
                        .replace('|', "\\|")
                        .replace('\n', " ")
                })
                .collect();
            cells
                .iter()
                .any(|cell| !cell.is_empty())
                .then_some((index, cells))
        })
        .collect();
    if rows.is_empty() {
        return None;
    }
    let columns = rows[0].1.len();
    if rows[0].0 != 0 {
        // Keep body cells in the body without including the unselected heading.
        rows.insert(0, (0, vec![String::new(); columns]));
    }
    rows.insert(
        1,
        (
            0,
            (0..columns)
                .map(|column| {
                    match table.column_align(column) {
                        ColumnumnAlign::Left => "---",
                        ColumnumnAlign::Center => ":---:",
                        ColumnumnAlign::Right => "---:",
                    }
                    .to_owned()
                })
                .collect(),
        ),
    );
    Some(
        rows.into_iter()
            .map(|(_, cells)| format!("| {} |", cells.join(" | ")))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn paragraph(paragraph: &Paragraph, all: bool, cx: &NodeContext) -> String {
    let mut runs = Vec::new();
    let mut group = Vec::new();
    for child in &paragraph.children {
        if child.image.is_some() {
            // Text on either side of an image has a separate native selection range.
            collect_group(&group, &child.state, all, cx, &mut runs);
            group.clear();
        } else {
            group.push(child);
        }
    }
    collect_group(&group, &paragraph.state, all, cx, &mut runs);
    escape_block_start(&write_runs(&runs, 0))
}

fn collect_group<'a>(
    group: &[&'a super::node::InlineNode],
    state: &Mutex<InlineState>,
    all: bool,
    cx: &'a NodeContext,
    runs: &mut Vec<Run<'a>>,
) {
    let range = if all {
        0..group.iter().map(|child| child.text.len()).sum()
    } else {
        let Some(range) = state.lock().ok().and_then(|state| {
            state
                .selection
                .as_ref()
                .map(|selection| selection.start..selection.end)
        }) else {
            return;
        };
        range
    };
    let mut offset = 0;
    for child in group {
        let len = child.text.len();
        let part =
            range.start.saturating_sub(offset).min(len)..range.end.saturating_sub(offset).min(len);
        if !part.is_empty() {
            collect_runs(&child.text, &child.marks, part, cx, runs);
        }
        offset += len;
    }
}

struct Run<'a> {
    text: &'a str,
    mark: TextMark,
    link: Option<&'a LinkMark>,
}

fn collect_runs<'a>(
    text: &'a str,
    marks: &'a [(Range<usize>, TextMark)],
    selection: Range<usize>,
    cx: &'a NodeContext,
    runs: &mut Vec<Run<'a>>,
) {
    let mut boundaries = vec![selection.start, selection.end];
    for (range, _) in marks {
        boundaries.extend(
            [range.start, range.end]
                .into_iter()
                .filter(|point| selection.start < *point && *point < selection.end),
        );
    }
    boundaries.sort_unstable();
    boundaries.dedup();
    for pair in boundaries.windows(2) {
        let Some(text) = text.get(pair[0]..pair[1]) else {
            continue;
        };
        let mut mark = TextMark::default();
        let mut link = None;
        for (_, style) in marks.iter().filter(|(range, _)| range.contains(&pair[0])) {
            mark.bold |= style.bold;
            mark.italic |= style.italic;
            mark.strikethrough |= style.strikethrough;
            mark.code |= style.code;
            if let Some(target) = style.link.as_ref() {
                link = Some(
                    target
                        .identifier
                        .as_ref()
                        .and_then(|id| cx.link_refs.get(id))
                        .unwrap_or(target),
                );
            }
        }
        runs.push(Run { text, mark, link });
    }
}

#[derive(PartialEq)]
enum InlineFormat<'a> {
    Link(&'a LinkMark),
    Bold,
    Italic,
    Strike,
    Code,
}

impl<'a> Run<'a> {
    fn format(&self, level: usize) -> Option<InlineFormat<'a>> {
        match level {
            0 => self.link.map(InlineFormat::Link),
            1 => self.mark.bold.then_some(InlineFormat::Bold),
            2 => self.mark.italic.then_some(InlineFormat::Italic),
            3 => self.mark.strikethrough.then_some(InlineFormat::Strike),
            4 => self.mark.code.then_some(InlineFormat::Code),
            _ => unreachable!(),
        }
    }
}

fn write_runs(runs: &[Run<'_>], level: usize) -> String {
    if level == 5 {
        return escape_inline(runs.iter().flat_map(|run| run.text.chars()));
    }
    let mut text = String::new();
    for group in runs.chunk_by(|a, b| a.format(level) == b.format(level)) {
        let Some(format) = group[0].format(level) else {
            text.push_str(&write_runs(group, level + 1));
            continue;
        };
        match format {
            InlineFormat::Link(link) => {
                let label = write_runs(group, level + 1);
                let destination = link
                    .url
                    .replace('<', "%3C")
                    .replace('>', "%3E")
                    .replace('\\', "%5C")
                    .replace('\n', "%0A")
                    .replace('\r', "%0D");
                if destination.is_empty() {
                    text.push_str(&label);
                } else if destination
                    .chars()
                    .any(|ch| ch.is_whitespace() || matches!(ch, '(' | ')'))
                {
                    text.push_str(&format!("[{label}](<{destination}>)"));
                } else {
                    text.push_str(&format!("[{label}]({destination})"));
                }
            }
            InlineFormat::Code => {
                let value: String = group.iter().map(|run| run.text).collect();
                let ticks = "`".repeat(backticks(&value) + 1);
                let pad = value.starts_with('`')
                    || value.ends_with('`')
                    || (value.starts_with(' ') && value.ends_with(' ') && !value.trim().is_empty());
                let space = if pad { " " } else { "" };
                text.push_str(&format!("{ticks}{space}{value}{space}{ticks}"));
            }
            InlineFormat::Bold | InlineFormat::Italic | InlineFormat::Strike => {
                let delimiter = match format {
                    InlineFormat::Bold => "**",
                    InlineFormat::Italic => "*",
                    _ => "~~",
                };
                let value = write_runs(group, level + 1);
                let clipped = value.trim();
                if clipped.is_empty() {
                    text.push_str(&value);
                } else {
                    let start = value.len() - value.trim_start().len();
                    text.push_str(&format!(
                        "{}{delimiter}{clipped}{delimiter}{}",
                        &value[..start],
                        &value[start + clipped.len()..],
                    ));
                }
            }
        }
    }
    text
}

pub(super) fn literal_markdown(text: &str) -> String {
    escape_block_start(&escape_inline(text.chars()))
}

fn escape_inline(chars: impl Iterator<Item = char>) -> String {
    let mut escaped = String::new();
    for ch in chars {
        if matches!(
            ch,
            '\\' | '`' | '*' | '_' | '~' | '[' | ']' | '<' | '>' | '&' | '!'
        ) {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

fn backticks(text: &str) -> usize {
    text.split(|ch| ch != '`').map(str::len).max().unwrap_or(0)
}

// A literal marker that was escaped in the source must remain prose when copied
// on its own. Escaping a leading delimiter also prevents a clipped line from
// becoming a list or heading in the destination.
fn escape_block_start(text: &str) -> String {
    text.split('\n')
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with(['#', '+', '-', '=']) {
                let offset = line.len() - trimmed.len();
                return format!("{}\\{trimmed}", &line[..offset]);
            }
            let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
            if digits > 0
                && (trimmed[digits..].starts_with(". ") || trimmed[digits..].starts_with(") "))
            {
                let offset = line.len() - trimmed.len() + digits;
                return format!("{}\\{}", &line[..offset], &line[offset..]);
            }
            line.to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}
