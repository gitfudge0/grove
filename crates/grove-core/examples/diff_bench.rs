//! Throwaway baseline-measurement harness for the diff viewer's
//! row-derivation + span-projection path. No GUI, no git, no filesystem: a
//! synthetic `Patch::Text` is built in memory and timed directly. This file
//! exists purely to print numbers for a future optimization pass — it is not
//! itself part of the product and may be deleted once that work is done.
#![allow(clippy::cast_precision_loss, clippy::print_stdout)]

use std::fmt::Write as _;
use std::time::Instant;

use grove_core::diff::{word_runs, Hunk, Line, LineKind, PairedRow, Patch, UnifiedRow};
use grove_core::highlight::{highlight_file, line_spans};
use grove_core::render_rows::{
    split_render_rows, unified_render_rows, widest_unified_row, SplitRenderRow, UnifiedRenderRow,
};

const ITERATIONS: u32 = 50;

/// Build one synthetic line of realistic-looking Rust source text, varying
/// content by index so lines aren't all identical (which would make
/// highlighting and word-diff trivially cheap in a way real code never is).
fn make_line_text(seed: usize, kind: LineKind) -> String {
    match seed % 12 {
        0 => format!(
            "    let value_{seed}: u32 = compute_offset(base, {seed}) + 1; // step {seed}"
        ),
        1 => format!(
            "    pub fn helper_{seed}(input: &str, count: usize) -> Result<Vec<String>, Error> {{"
        ),
        2 => format!(
            "        return Err(Error::InvalidState(format!(\"bad state at {{}}: {{}}\", {seed}, input)));"
        ),
        3 => format!(
            "    // TODO: revisit this branch once issue-{seed} lands; it currently allocates twice"
        ),
        4 => format!(
            "        let s = \"a fairly long literal string used to pad line width near {seed}\";"
        ),
        5 => {
            // A very long line (300+ chars) to exercise wide-span handling.
            let mut long = format!(
                "    let long_expr_{seed} = base_value.checked_add(offset_{seed}).unwrap_or(0)"
            );
            while long.len() < 320 {
                let _ = write!(long, " + extra_term_{}_padding", long.len());
            }
            long.push(';');
            long
        }
        6 => "        // 多字节注释：这一行包含中文字符，用于测试多字节文本处理路径".to_string(),
        7 => format!("    struct Config{seed} {{ enabled: bool, retries: u32, name: String }}"),
        8 => format!(
            "        match kind_{seed} {{ Kind::A => 1, Kind::B => 2, Kind::C(x) => x + {seed}, }}"
        ),
        9 => format!("        assert_eq!(result_{seed}.len(), expected_{seed});"),
        10 => "        let emoji_line = \"日本語のテキスト行 for CJK coverage 🦀\";".to_string(),
        _ => format!("        counter += {seed}; // kind={kind:?} running total tracked here"),
    }
}

/// Build `hunk_count` hunks with a mix of context / del-run / add-run
/// segments, and simultaneously build the whole "old file" and "new file"
/// text those hunks are drawn from, so line numbers index correctly into
/// per-side syntax highlighting.
fn build_synthetic_patch(hunk_count: usize) -> (Patch, String, String) {
    let mut hunks = Vec::with_capacity(hunk_count);
    let mut old_file = String::new();
    let mut new_file = String::new();
    let mut old_no: u32 = 1;
    let mut new_no: u32 = 1;
    let mut seed: usize = 0;

    for h in 0..hunk_count {
        let old_start = old_no;
        let new_start = new_no;
        let mut lines = Vec::new();

        // Leading context (2-4 lines).
        let ctx_lead = 3 + (h % 4);
        for _ in 0..ctx_lead {
            let text = make_line_text(seed, LineKind::Context);
            seed += 1;
            old_file.push_str(&text);
            old_file.push('\n');
            new_file.push_str(&text);
            new_file.push('\n');
            lines.push(Line {
                kind: LineKind::Context,
                text,
                old_no: Some(old_no),
                new_no: Some(new_no),
            });
            old_no += 1;
            new_no += 1;
        }

        // A del-run immediately followed by an add-run, so split mode's
        // pairing and word-diff paths both get exercised.
        let del_count = 5 + (h % 6);
        let add_count = 4 + (h % 7);
        for _ in 0..del_count {
            let text = make_line_text(seed, LineKind::Del);
            seed += 1;
            old_file.push_str(&text);
            old_file.push('\n');
            lines.push(Line {
                kind: LineKind::Del,
                text,
                old_no: Some(old_no),
                new_no: None,
            });
            old_no += 1;
        }
        for _ in 0..add_count {
            let text = make_line_text(seed, LineKind::Add);
            seed += 1;
            new_file.push_str(&text);
            new_file.push('\n');
            lines.push(Line {
                kind: LineKind::Add,
                text,
                old_no: None,
                new_no: Some(new_no),
            });
            new_no += 1;
        }

        // Trailing context.
        let ctx_tail = 3 + ((h + 1) % 4);
        for _ in 0..ctx_tail {
            let text = make_line_text(seed, LineKind::Context);
            seed += 1;
            old_file.push_str(&text);
            old_file.push('\n');
            new_file.push_str(&text);
            new_file.push('\n');
            lines.push(Line {
                kind: LineKind::Context,
                text,
                old_no: Some(old_no),
                new_no: Some(new_no),
            });
            old_no += 1;
            new_no += 1;
        }

        hunks.push(Hunk {
            old_start,
            new_start,
            header: format!("fn region_{h}()"),
            lines,
        });
    }

    (
        Patch::Text {
            hunks,
            no_newline_at_eof: false,
        },
        old_file,
        new_file,
    )
}

fn mean_and_total(elapsed_ns_per_iter: &[u128]) -> (f64, f64) {
    let total: u128 = elapsed_ns_per_iter.iter().sum();
    let mean = total as f64 / elapsed_ns_per_iter.len() as f64;
    (mean / 1000.0, total as f64 / 1000.0) // microseconds
}

fn main() {
    const HUNK_COUNT: usize = 120;
    /// A typical on-screen window of diff rows — what `uniform_list` builds
    /// per frame, versus the whole list the old view built.
    const VISIBLE_ROWS: usize = 40;

    let (patch, old_file, new_file) = build_synthetic_patch(HUNK_COUNT);

    let (total_lines, dels, adds, ctxs) = if let Patch::Text { hunks, .. } = &patch {
        let mut d = 0usize;
        let mut a = 0usize;
        let mut c = 0usize;
        for hunk in hunks {
            for line in &hunk.lines {
                match line.kind {
                    LineKind::Del => d += 1,
                    LineKind::Add => a += 1,
                    LineKind::Context => c += 1,
                }
            }
        }
        (d + a + c, d, a, c)
    } else {
        (0, 0, 0, 0)
    };

    println!("BASELINE hunks={HUNK_COUNT} total_lines={total_lines} del={dels} add={adds} context={ctxs}");

    let old_spans = highlight_file(&old_file, "bench.rs");
    let new_spans = highlight_file(&new_file, "bench.rs");
    println!(
        "BASELINE old_side_highlighted_lines={} new_side_highlighted_lines={}",
        old_spans.len(),
        new_spans.len()
    );

    let mut a_ns = Vec::with_capacity(ITERATIONS as usize);
    let mut unified_rows = Vec::new();
    for _ in 0..ITERATIONS {
        let start = Instant::now();
        unified_rows = patch.unified_rows();
        a_ns.push(start.elapsed().as_nanos());
    }
    let (a_mean_us, a_total_us) = mean_and_total(&a_ns);
    println!(
        "BASELINE unified_rows derivation: mean={a_mean_us:.2}us total_over_{ITERATIONS}_iters={a_total_us:.2}us rows={}",
        unified_rows.len()
    );

    let mut b_ns = Vec::with_capacity(ITERATIONS as usize);
    let mut paired_rows = Vec::new();
    for _ in 0..ITERATIONS {
        let start = Instant::now();
        paired_rows = patch.paired_rows();
        b_ns.push(start.elapsed().as_nanos());
    }
    let (b_mean_us, b_total_us) = mean_and_total(&b_ns);
    println!(
        "BASELINE paired_rows derivation: mean={b_mean_us:.2}us total_over_{ITERATIONS}_iters={b_total_us:.2}us rows={}",
        paired_rows.len()
    );

    let mut c_ns = Vec::with_capacity(ITERATIONS as usize);
    let mut total_unified_spans: usize = 0;
    let mut unified_line_count: usize = 0;
    for _ in 0..ITERATIONS {
        let start = Instant::now();
        let mut span_count = 0usize;
        let mut line_count = 0usize;
        for row in &unified_rows {
            if let UnifiedRow::Line(line) = row {
                let spans = line_spans(line, &old_spans, &new_spans);
                span_count += spans.len();
                line_count += 1;
            }
        }
        c_ns.push(start.elapsed().as_nanos());
        total_unified_spans = span_count;
        unified_line_count = line_count;
    }
    let (c_mean_us, c_total_us) = mean_and_total(&c_ns);
    println!(
        "BASELINE unified span projection (line_spans over all UnifiedRow::Line): mean={c_mean_us:.2}us total_over_{ITERATIONS}_iters={c_total_us:.2}us"
    );

    let mut d_ns = Vec::with_capacity(ITERATIONS as usize);
    let mut total_word_run_pairs: usize = 0;
    let mut total_split_spans: usize = 0;
    for _ in 0..ITERATIONS {
        let start = Instant::now();
        let mut word_run_pairs = 0usize;
        let mut split_spans = 0usize;
        for row in &paired_rows {
            if let PairedRow::Lines { old, new } = row {
                if let (Some(old_line), Some(new_line)) = (old, new) {
                    if old_line.kind == LineKind::Del && new_line.kind == LineKind::Add {
                        let (old_runs, new_runs) = word_runs(&old_line.text, &new_line.text);
                        word_run_pairs += old_runs.len() + new_runs.len();
                    }
                }
                if let Some(old_line) = old {
                    split_spans += line_spans(old_line, &old_spans, &new_spans).len();
                }
                if let Some(new_line) = new {
                    split_spans += line_spans(new_line, &old_spans, &new_spans).len();
                }
            }
        }
        d_ns.push(start.elapsed().as_nanos());
        total_word_run_pairs = word_run_pairs;
        total_split_spans = split_spans;
    }
    let (d_mean_us, d_total_us) = mean_and_total(&d_ns);
    println!(
        "BASELINE split-mode per-frame work (word_runs on Del/Add pairs + line_spans both sides): mean={d_mean_us:.2}us total_over_{ITERATIONS}_iters={d_total_us:.2}us word_run_segments={total_word_run_pairs} split_spans={total_split_spans}"
    );

    // The current view re-derives paired_rows twice per split frame (once
    // for layout, once for painting/measurement) — see the split-view call
    // sites in src/views; that duplication is reproduced here rather than
    // assumed away.
    let unified_frame_us = a_mean_us + c_mean_us;
    let split_frame_us = b_mean_us + b_mean_us + d_mean_us;
    println!(
        "BASELINE one unified frame (unified_rows + span projection): {unified_frame_us:.2}us"
    );
    println!(
        "BASELINE one split frame (paired_rows x2, current view derives twice, + per-frame word-diff/span work): {split_frame_us:.2}us"
    );

    let mut hunk_header_count = 0usize;
    for row in &unified_rows {
        if matches!(row, UnifiedRow::HunkHeader(_)) {
            hunk_header_count += 1;
        }
    }
    // 3 fixed children per line row (2 gutter cells + sign) + max(1, spans).
    let mut unified_elements = hunk_header_count; // 1 per hunk header
    for row in &unified_rows {
        if let UnifiedRow::Line(line) = row {
            let spans = line_spans(line, &old_spans, &new_spans);
            unified_elements += 3 + spans.len().max(1);
        }
    }
    let mean_spans_per_line = if unified_line_count > 0 {
        total_unified_spans as f64 / unified_line_count as f64
    } else {
        0.0
    };
    println!(
        "BASELINE unified mode element count (current view's shape): {unified_elements} rows={} hunk_headers={hunk_header_count} mean_spans_per_line={mean_spans_per_line:.2}",
        unified_rows.len()
    );

    // Split mode: each row builds two cells; approximate each present side's
    // cell element count as (word-run segments for that line, if any) +
    // (spans for that line) — this is explicitly an approximation of the
    // real per-run-x-per-span chunking the view does, not an exact replay
    // of it.
    let mut split_elements = 0usize;
    let mut split_hunk_headers = 0usize;
    for row in &paired_rows {
        match row {
            PairedRow::HunkHeader(_) => split_hunk_headers += 1,
            PairedRow::Lines { old, new } => {
                let is_word_diff_pair = matches!(
                    (old, new),
                    (Some(o), Some(n)) if o.kind == LineKind::Del && n.kind == LineKind::Add
                );
                let (old_runs, new_runs) = if is_word_diff_pair {
                    if let (Some(o), Some(n)) = (old, new) {
                        let (o_runs, n_runs) = word_runs(&o.text, &n.text);
                        (o_runs.len(), n_runs.len())
                    } else {
                        (0, 0)
                    }
                } else {
                    (0, 0)
                };
                if let Some(o) = old {
                    let spans = line_spans(o, &old_spans, &new_spans).len();
                    split_elements += old_runs.max(1) + spans;
                }
                if let Some(n) = new {
                    let spans = line_spans(n, &old_spans, &new_spans).len();
                    split_elements += new_runs.max(1) + spans;
                }
            }
        }
    }
    split_elements += split_hunk_headers;
    println!(
        "BASELINE split mode element count APPROXIMATION (per-side: max(1,word_run_segments)+spans; not an exact replay of the view's run-x-span chunking): {split_elements} rows={} hunk_headers={split_hunk_headers}",
        paired_rows.len()
    );

    // Rows and spans are now baked once per patch load into an `Rc`'d
    // vector (`grove_core::render_rows`), and the unified body is a
    // `uniform_list` that builds only its visible range.

    let mut bake_u_ns = Vec::with_capacity(ITERATIONS as usize);
    let mut baked_unified = Vec::new();
    let mut baked_unified_widest = 0usize;
    for _ in 0..ITERATIONS {
        let start = Instant::now();
        let rows = unified_render_rows(&patch, &old_spans, &new_spans);
        let widest = widest_unified_row(&rows);
        bake_u_ns.push(start.elapsed().as_nanos());
        baked_unified = rows;
        baked_unified_widest = widest;
    }
    let (bake_u_mean_us, _) = mean_and_total(&bake_u_ns);
    println!(
        "AFTER one-time row bake (unified_render_rows + widest_unified_row): {bake_u_mean_us:.2}us rows={} widest_ix={baked_unified_widest}",
        baked_unified.len()
    );

    let mut bake_s_ns = Vec::with_capacity(ITERATIONS as usize);
    let mut baked_split_len = 0usize;
    for _ in 0..ITERATIONS {
        let start = Instant::now();
        let rows = split_render_rows(&patch, &old_spans, &new_spans);
        bake_s_ns.push(start.elapsed().as_nanos());
        baked_split_len = rows.len();
    }
    let (bake_s_mean_us, _) = mean_and_total(&bake_s_ns);
    println!(
        "AFTER one-time row bake (split_render_rows): {bake_s_mean_us:.2}us rows={baked_split_len} (no widest-row index — split columns size from measured text width, not a widest row)"
    );

    // The per-frame cost is now just indexing the cached vector — measured
    // here by touching every row's precomputed spans, which is strictly
    // more work than the visible-window-only render actually does.
    let mut lookup_ns = Vec::with_capacity(ITERATIONS as usize);
    let mut touched_spans = 0usize;
    for _ in 0..ITERATIONS {
        let start = Instant::now();
        let mut spans = 0usize;
        for ix in 0..baked_unified.len() {
            if let Some(UnifiedRenderRow::Line { spans: s, .. }) = baked_unified.get(ix) {
                spans += s.len();
            }
        }
        lookup_ns.push(start.elapsed().as_nanos());
        touched_spans = spans;
    }
    let (lookup_mean_us, _) = mean_and_total(&lookup_ns);
    println!(
        "AFTER per-frame unified derivation+span cost: 0us (pure Rc index lookup; rows and spans are precomputed and cached) measured_full_scan={lookup_mean_us:.2}us spans_touched={touched_spans}"
    );

    // Element count for the window `uniform_list` actually builds, using the
    // same per-row formula as the BASELINE element-count line above.
    let mut window_elements = 0usize;
    for row in baked_unified.iter().take(VISIBLE_ROWS) {
        match row {
            UnifiedRenderRow::HunkHeader(_) => window_elements += 1,
            UnifiedRenderRow::Line { spans, .. } => window_elements += 3 + spans.len().max(1),
        }
    }
    println!(
        "AFTER unified mode element count for a ~{VISIBLE_ROWS}-row visible window (uniform_list builds only the visible range): {window_elements} (full list for contrast: {unified_elements})"
    );

    // Split is now a `uniform_list` of paired rows, exactly like unified —
    // one `SplitRenderRow` per item, so the visible window is just the first
    // `VISIBLE_ROWS` rows, no item grouping to translate back through.
    let baked_split = split_render_rows(&patch, &old_spans, &new_spans);
    let mut split_window_elements = 0usize;
    for row in baked_split.iter().take(VISIBLE_ROWS) {
        match row {
            SplitRenderRow::HunkHeader(_) => split_window_elements += 1,
            SplitRenderRow::Lines { old, new } => {
                for cell in [old, new].into_iter().flatten() {
                    let run_segments = cell.runs.as_ref().map_or(1, |r| r.len().max(1));
                    split_window_elements += run_segments.max(cell.spans.len().max(1));
                }
            }
        }
    }
    println!(
        "AFTER split mode element count for a ~{VISIBLE_ROWS}-row visible window (uniform_list builds only the visible range): {split_window_elements} (full list for contrast: {split_elements}, {} rows total)",
        baked_split.len()
    );

    println!(
        "NOTE: gpui element construction and paint cost is NOT measured here (requires a running window); only row derivation, span projection, word-diff and element counts are measured."
    );
}
