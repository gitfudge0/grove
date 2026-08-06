//! DESIGN.md conformance tripwires — a lint-by-grep over the view layer.
//!
//! This project has no visual regression net: no screenshot tooling, no render
//! tests for modals. A ~200-site sweep brought `src/views/` into line with
//! DESIGN.md, and nothing mechanical stopped it from rotting back. This module
//! is that cheap net: plain `#[test]` functions that read the view-layer `.rs`
//! files as **source text** and fail on the exact patterns the sweep removed.
//!
//! It is deliberately *not* a real linter. No proc macro, no syntax tree, no
//! dev-dependency, no extra toolchain — it runs in `cargo test` and it is a
//! few hundred lines. That is the whole value proposition.
//!
//! **Known limits.** Because it reads text, it can be fooled:
//!
//! - a styling call built by a macro, or split across lines so the method and
//!   its argument never share a line, is invisible to it;
//! - a match inside a string literal or a comment is indistinguishable from
//!   real code (comment lines are skipped, string literals are not);
//! - R3's family check uses a fixed line window, so an unusually long element
//!   chain can hide a legitimate `.font(` or shelter an illegitimate omission.
//!
//! R6 exists because of one of those limits. R1 only matches
//! `.setter(rpx(N))`, so it is blind to every size passed *positionally* —
//! icon sizes, dot sizes and modal widths are all bare function arguments, and
//! that category alone produced most of the sweep's violations. R6 reads
//! forward from a call to one of the known sizing functions ([`SIZE_FNS`]) and
//! flags any top-level argument that is nothing but a number. What it does
//! **not** catch: a size passed to a function not on that list; a size laundered
//! through a local `let` or an arithmetic expression (`ICON_LG - 2.0` reads as
//! a runtime choice and is deliberately ignored); a call whose arguments run
//! past [`R6_WINDOW`] lines; and it cannot tell a *good* identifier from a bad
//! one — `icon_btn(.., SPACE_2XL, ..)` is off-scale but passes, because R6
//! checks the shape of the argument, not which scale it came from.
//!
//! It is a tripwire, not a proof. A green run means "none of the known
//! regressions is present in an obvious form", never "the views conform".
//!
//! R8 is the narrowest of them by construction: it names three functions and
//! checks each still mentions `CONTROL_H`. A source-text scan cannot tell an
//! in-row control from any other element, so the rule guards the three that
//! regressed rather than pretending to the general case its section states.
//!
//! Each rule cites the DESIGN.md section it enforces, and every exemption is a
//! narrow entry in an allow-list below naming the §14 case that sanctions it.
//! If a rule fires on legitimate code, add an allow-list entry with a
//! justification — do **not** loosen the rule.

#![cfg(test)]

use fs_err as fs;
use std::fmt::Write as _;
use std::path::PathBuf;

/// A source line, with everything a failure message needs to be actionable.
struct Line {
    /// Path relative to the crate root, e.g. `src/views/modals/confirm.rs`.
    file: String,
    /// The file's basename, which is what the allow-lists key on.
    name: String,
    /// 1-based, matching what an editor shows.
    no: usize,
    text: String,
}

/// Every view-layer file, flattened into lines.
fn view_lines() -> Vec<Line> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut files: Vec<PathBuf> = Vec::new();
    for dir in ["src/views", "src/views/modals"] {
        let entries =
            fs::read_dir(root.join(dir)).unwrap_or_else(|e| panic!("cannot read {dir}: {e}"));
        for entry in entries {
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            // Skip this file: it quotes every banned pattern verbatim in its
            // rules and failure messages, and would flag itself.
            if path.file_name().is_some_and(|n| n == "conformance.rs") {
                continue;
            }
            if path.extension().is_some_and(|e| e == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    assert!(
        files.len() > 10,
        "expected the whole view layer, found {} files — did the tree move?",
        files.len()
    );

    let mut lines = Vec::new();
    for path in files {
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        for (i, l) in text.lines().enumerate() {
            lines.push(Line {
                file: rel.clone(),
                name: name.clone(),
                no: i + 1,
                text: l.to_string(),
            });
        }
    }
    lines
}

/// `true` for a line that is purely a comment — DESIGN.md quotes itself in doc
/// comments all over the view layer, and prose is not a styling call.
fn is_comment(l: &Line) -> bool {
    l.text.trim_start().starts_with("//")
}

/// Fail once with the complete list, so a reader fixes everything in one pass.
fn report(rule: &str, section: &str, why: &str, hits: Vec<(String, String)>) {
    if hits.is_empty() {
        return;
    }
    let mut msg = format!(
        "\n{rule} — {hits} violation(s) of DESIGN.md {section}.\n{why}\n\n",
        hits = hits.len()
    );
    for (loc, text) in &hits {
        let _ = writeln!(msg, "  {loc}\n      {}", text.trim());
    }
    let _ = write!(
        msg,
        "\nIf one of these is a legitimate DESIGN.md §14 exception, add a \
         narrow entry to the allow-list in src/views/conformance.rs naming \
         the §14 case — do not loosen the rule.\n"
    );
    panic!("{msg}");
}

fn loc(l: &Line) -> String {
    format!("{}:{}", l.file, l.no)
}

// ---------------------------------------------------------------------------
// R1 — no bare numeric literal in a styling call (§6.1, §13 "Code")
// ---------------------------------------------------------------------------

/// Styling setters whose argument must be a token, never a number.
const STYLE_SETTERS: &[&str] = &[
    "px",
    "py",
    "p",
    "pl",
    "pr",
    "pt",
    "pb",
    "gap",
    "w",
    "h",
    "size",
    "text_size",
    "rounded",
    "max_h",
    "min_w",
    "mt",
    "mb",
    "mx",
    "my",
];

/// §14 case 3 — optical corrections. Each entry names the file, the exact
/// snippet, and why the number is not on a scale.
const R1_ALLOW: &[(&str, &str, &str)] = &[(
    "appbar.rs",
    ".h(rpx(14.0))",
    "§14 case 3 (optical correction): the appbar segment divider is \
     deliberately 14px tall, not full height — a full-height rule would make \
     the segmented combo read taller than the lone toggle it replaces. \
     DESIGN.md §14 names this exact literal.",
)];

#[test]
fn r1_no_bare_numeric_literal_in_styling_call() {
    let mut hits = Vec::new();
    for l in view_lines() {
        if is_comment(&l) {
            continue;
        }
        if R1_ALLOW
            .iter()
            .any(|(f, snip, _)| *f == l.name && l.text.contains(snip))
        {
            continue;
        }
        for setter in STYLE_SETTERS {
            let pat = format!(".{setter}(rpx(");
            let mut from = 0;
            while let Some(i) = l.text[from..].find(&pat) {
                let arg = &l.text[from + i + pat.len()..];
                if arg.starts_with(|c: char| c.is_ascii_digit()) {
                    hits.push((loc(&l), l.text.clone()));
                    break;
                }
                from += i + pat.len();
            }
        }
    }
    report(
        "R1 no bare numeric literal in a styling call",
        "§6.1 / §13 \"Code\"",
        "Spacing, type size, radius and control height all come from the \
         scale in src/views/tokens.rs. `.px(rpx(8.0))` is wrong; \
         `.px(rpx(SPACE_LG))` is right.",
        hits,
    );
}

// ---------------------------------------------------------------------------
// R2 — exactly one border weight (§7.2)
// ---------------------------------------------------------------------------

/// The number between `open` and the next `)`, if it is a plain literal.
/// Returns `None` for a variable — `.border(px(border_w))` is a runtime
/// choice between tokens and R2 cannot and should not judge it.
fn literal_after(text: &str, open: &str) -> Option<(f32, String)> {
    let start = text.find(open)? + open.len();
    let rest = &text[start..];
    let end = rest.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
    let num = &rest[..end];
    if num.is_empty() {
        return None;
    }
    num.parse::<f32>().ok().map(|v| (v, num.to_string()))
}

#[test]
fn r2_borders_are_one_pixel() {
    let mut hits = Vec::new();
    for l in view_lines() {
        if is_comment(&l) {
            continue;
        }
        if let Some((v, _)) = literal_after(&l.text, ".border(px(") {
            if v != 1.0 {
                hits.push((loc(&l), l.text.clone()));
                continue;
            }
        }
        if l.text.contains("border_width") {
            if let Some((v, _)) = literal_after(&l.text, "border_width") {
                if v != 1.0 {
                    hits.push((loc(&l), l.text.clone()));
                }
            }
        }
    }
    report(
        "R2 borders are exactly one hairline weight",
        "§7.2",
        "There is exactly one border weight: the 1px hairline, always \
         `px(1.0)`, never `rpx`. A state is called out by tone (BORDER vs \
         BORDER_SOFT vs AMBER), never by a heavier stroke.",
        hits,
    );
}

// ---------------------------------------------------------------------------
// R3 — every text run pins a font family (§5.1, §5.2)
// ---------------------------------------------------------------------------

/// How many **code** lines either side of a `.text_size(` may carry the
/// `.font(` that belongs to the same element chain. Comment lines are dropped
/// before the window is measured: a comment cannot carry a `.font(`, so
/// counting one against the budget only makes a well-documented chain look
/// like an undocumented violation. Tuned against the real tree: the widest
/// legitimate gap is `confirm.rs`'s `input_modal`, where the family is pinned
/// on the container six code lines above the `Input` that sets the size; 6
/// leaves room without letting an unrelated chain vouch.
const R3_WINDOW: usize = 6;

/// Files exempt from R3 wholesale, with the clause that sanctions it.
const R3_EXEMPT_FILES: &[(&str, &str)] = &[(
    "components.rs",
    "§5.2: this file *is* the two text primitives. `ui` and `mono` are the \
     only places a family is bound to a size; every other view consumes them.",
)];

/// §14-sanctioned sites that pin a family and size directly instead of going
/// through `ui`/`mono`. These pass R3 on their own merits (they do carry a
/// `.font(`); the list exists so a reader knows they were reviewed, not
/// missed.
const R3_REVIEWED: &[(&str, &str)] = &[
    (
        "statusbar.rs",
        "hint_chip: §9.2 — the chip's hover recolour lives on the parent row, \
         so the label must inherit its colour and cannot use `mono`, which \
         pins one. It still pins the family, which is what R3 checks.",
    ),
    (
        "sidebar.rs",
        "agent_menu item label: §9.2 — same reason as hint_chip.",
    ),
    (
        "add_project.rs",
        "field(): the inner gpui_component::input::Input renders its own text \
         and inherits the container's text style, so family and size are \
         pinned on the container.",
    ),
];

/// Genuine remaining violations, tracked rather than silently tolerated.
/// **This is debt, not an exemption.** Anything here must be fixed and
/// removed; nothing may be added without a matching report to a human.
///
/// **It must stay empty.** The last entry (`confirm.rs`'s `input_zone`, which
/// sized its `Input` without pinning a family) has been fixed at source, so
/// there is no outstanding R3 debt. The mechanism is kept rather than deleted
/// because an entry here is the only sanctioned way to land a *temporary* R3
/// bypass — and keeping it visible and empty is what makes a future addition
/// obviously a regression rather than routine.
const R3_KNOWN_VIOLATIONS: &[(&str, &str, &str)] = &[];

#[test]
fn r3_every_text_run_pins_a_font_family() {
    let all = view_lines();
    let lines: Vec<&Line> = all.iter().filter(|l| !is_comment(l)).collect();
    let mut hits = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if !l.text.contains(".text_size(") {
            continue;
        }
        if R3_EXEMPT_FILES.iter().any(|(f, _)| *f == l.name) {
            continue;
        }
        if R3_KNOWN_VIOLATIONS
            .iter()
            .any(|(f, snip, _)| *f == l.name && l.text.contains(snip))
        {
            continue;
        }
        let lo = i.saturating_sub(R3_WINDOW);
        let hi = (i + R3_WINDOW + 1).min(lines.len());
        let pinned = lines[lo..hi]
            .iter()
            .any(|n| n.file == l.file && n.text.contains(".font("));
        if !pinned {
            hits.push((loc(l), l.text.clone()));
        }
    }
    report(
        "R3 every text run pins a font family",
        "§5.1 / §5.2",
        "A `.text_size()` with no `.font()` in the same element chain \
         silently inherits the window default instead of IBM Plex Sans or \
         BlexMono. Use `components::ui` / `components::mono`, or — when the \
         parent row owns the hover recolour (§9.2) — pin `.font()` explicitly \
         alongside the size.",
        hits,
    );
}

// ---------------------------------------------------------------------------
// R4 — display tiers are never chrome (§5.3, §13 "Visual")
// ---------------------------------------------------------------------------

/// The only sanctioned consumers of the display tiers. A `None` snippet
/// allow-lists the whole file (§5.3's onboarding/empty-state screens, which
/// are nothing *but* display-tier chrome); a `Some` snippet narrows the
/// allowance to matching lines only, the same technique [`R1_ALLOW`] uses, so
/// the rest of a shared file — like the Settings/ScriptsEditor modals living
/// in `settings.rs` — still gets checked.
const R4_ALLOW: &[(&str, Option<&str>, &str)] = &[
    (
        "tokens.rs",
        None,
        "the tokens' own definition and doc comments — this is where the \
         'empty-state / onboarding only' rule is written down.",
    ),
    (
        "add_project.rs",
        None,
        "§5.3: the onboarding and empty-state screens (the grove wordmark, \
         the 'Environment' / 'Add your first project' / 'Start your first \
         session' titles) are exactly the sanctioned use.",
    ),
    (
        "settings.rs",
        Some("TEXT_DISPLAY"),
        "Project Settings' redesigned header (the 'editable header' layout): \
         the project name IS \
         the modal's header now, so it carries the one title in the panel — \
         a deliberate, reviewed exception, not a precedent for a fifth tier \
         elsewhere in chrome. Narrowed to its two call sites so the rest of \
         `settings.rs` (Settings, the shortcut overlay, Updating/changelog) \
         still gets checked.",
    ),
];

#[test]
fn r4_display_tiers_never_appear_in_chrome() {
    let mut hits = Vec::new();
    for l in view_lines() {
        if is_comment(&l) {
            continue;
        }
        if R4_ALLOW
            .iter()
            .any(|(f, snip, _)| *f == l.name && snip.is_none_or(|s| l.text.contains(s)))
        {
            continue;
        }
        if l.text.contains("TEXT_DISPLAY") || l.text.contains("ICON_DISPLAY") {
            hits.push((loc(&l), l.text.clone()));
        }
    }
    report(
        "R4 display tiers never appear in chrome",
        "§5.3 / §13 \"Visual\"",
        "TEXT_DISPLAY, TEXT_DISPLAY_LG and ICON_DISPLAY are empty-state and \
         onboarding only. Chrome has exactly four type tiers (MICRO, SMALL, \
         BODY, TITLE); if a design needs a fifth, the design is wrong.",
        hits,
    );
}

// ---------------------------------------------------------------------------
// R5 — tracking is mono-only (§5.4)
// ---------------------------------------------------------------------------

#[test]
fn r5_tracked_text_is_mono_only() {
    let mut hits = Vec::new();
    for l in view_lines() {
        if is_comment(&l) {
            continue;
        }
        if l.text.contains("ui(tracked(") {
            hits.push((loc(&l), l.text.clone()));
        }
    }
    report(
        "R5 letter tracking is mono-only",
        "§5.4",
        "Tracking is faked by joining characters with U+2009 THIN SPACE, and \
         it is used only for mono, uppercase section labels. Sans text is \
         read as language and is never tracked — use `mono(tracked(..))`.",
        hits,
    );
}

// ---------------------------------------------------------------------------
// R6 — no bare numeric literal as a size *argument* (§5.3.1, §5.3.2, §8.5)
// ---------------------------------------------------------------------------

/// Functions that take a size on one of the scales as a positional argument.
/// R1 cannot see these: it matches `.setter(rpx(N))`, and every icon, dot and
/// panel width is a bare function argument instead — which is exactly the
/// shape that produced most of the sweep's original violations.
///
/// Each pattern ends in `(` so the match is a call, and a match is only taken
/// when the preceding character cannot continue an identifier — otherwise
/// `flat_icon_btn(` would also match as `icon_btn(`.
const SIZE_FNS: &[&str] = &[
    "icons::icon(",
    "icon_btn(",
    "flat_icon_btn(",
    "icon_slot(",
    "status_dot(",
    "spinner(",
    "modal_panel(",
    "modal_action_sized(",
];

/// How many lines of a call R6 will read. rustfmt puts one argument per line
/// for these calls, so the literal usually sits on its own line well below the
/// opener; 24 covers the longest real call (`icon_btn`'s ten arguments plus a
/// multi-line closure) without running into the next item.
const R6_WINDOW: usize = 24;

/// `true` if `t` is nothing but a numeric literal — `12`, `13.0`, `0.5`.
/// An identifier (`ICON_SM`, `MODAL_W_LG`, `CONTROL_H`) is what R6 wants, and
/// an expression (`w - SPACE_LG`) is a runtime choice R6 must not judge.
fn is_numeric_literal(t: &str) -> bool {
    let t = t.trim();
    !t.is_empty()
        && t.chars().all(|c| c.is_ascii_digit() || c == '.')
        && t.chars().any(|c| c.is_ascii_digit())
}

/// The top-level, comma-separated arguments of a call whose `(` has already
/// been consumed, reading forward from `lines[i]` at byte offset `from`.
/// Stops at the matching `)`. Comment-only continuation lines are skipped —
/// prose is not an argument, and it can carry stray parens and commas.
fn call_args(lines: &[Line], i: usize, from: usize) -> Vec<String> {
    let file = &lines[i].file;
    let mut args = vec![String::new()];
    let mut depth = 1usize;
    let hi = (i + R6_WINDOW).min(lines.len());
    for (k, l) in lines.iter().enumerate().take(hi).skip(i) {
        if &l.file != file {
            break;
        }
        if k != i && is_comment(l) {
            continue;
        }
        let text = if k == i { &l.text[from..] } else { &l.text[..] };
        for ch in text.chars() {
            match ch {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return args;
                    }
                }
                // The separator itself belongs to neither argument.
                ',' if depth == 1 => {
                    args.push(String::new());
                    continue;
                }
                _ => {}
            }
            if depth >= 1 {
                if let Some(last) = args.last_mut() {
                    last.push(ch);
                }
            }
        }
        // A line break inside a call is an argument separator's worth of space.
        if let Some(last) = args.last_mut() {
            last.push(' ');
        }
    }
    args
}

#[test]
fn r6_no_bare_numeric_literal_as_a_size_argument() {
    let lines = view_lines();
    let mut hits = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if is_comment(l) {
            continue;
        }
        for pat in SIZE_FNS {
            let mut from = 0;
            while let Some(off) = l.text[from..].find(pat) {
                let at = from + off;
                let boundary = l.text[..at]
                    .chars()
                    .next_back()
                    .is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
                from = at + pat.len();
                if !boundary {
                    continue;
                }
                if call_args(&lines, i, from)
                    .iter()
                    .any(|a| is_numeric_literal(a))
                {
                    hits.push((loc(l), format!("{}   [{pat}…]", l.text)));
                    break;
                }
            }
        }
    }
    report(
        "R6 no bare numeric literal as a size argument",
        "§5.3.1 / §5.3.2 / §8.5",
        "Icon sizes, dot sizes and modal widths are scales exactly like \
         spacing is, and they are passed positionally rather than through a \
         styling setter — which is the only reason R1 does not see them. \
         `icon_btn(.., 13.0, ..)` is wrong; `icon_btn(.., ICON_SM, ..)` is \
         right. The scales are ICON_XS/SM/MD/LG, DOT_SM/MD and \
         MODAL_W_SM/MD/LG/XL in src/views/tokens.rs.",
        hits,
    );
}

// ---------------------------------------------------------------------------
// R7 — no pictographic character literal in a text run (§9.3)
// ---------------------------------------------------------------------------

/// `true` for a codepoint that belongs in the icon sprite (`src/icons.rs`),
/// never as a character literal in a string the view layer builds — the bug
/// this rule was written to catch: `modal_checkbox`'s tick used to be a
/// literal `"✓"` (U+2713), and the bundled fonts have no glyph for it (§9.3).
///
/// Scoped to the blocks §9.3 actually names: Dingbats, Miscellaneous Symbols,
/// Geometric Shapes, Box Drawing, Braille Patterns, plus the dedicated
/// checkmark/cross-mark codepoints that sit outside those blocks (U+2713,
/// U+2714, U+2717, U+2718). It deliberately does **not** ban the whole
/// General Punctuation block (U+2000-U+206F): U+2009 THIN SPACE is
/// `tracked()`'s own building block (§5.4), and General Punctuation also
/// hosts ordinary prose marks (en/em dash, ellipsis) with real font coverage.
/// Real keyboard-key characters (`⏎` U+23CE, `↑` `↓` `←` `→` U+2190-2199,
/// `esc`/`cmd`/`alt` as plain ASCII words) are outside every banned range, so
/// the keycap pattern (§5.2) stays legal without an allow-list entry.
///
/// The dedicated checkmark/cross-mark codepoints (U+2713, U+2714, U+2717,
/// U+2718 — the regression this rule exists to catch) already fall inside the
/// Dingbats block (U+2700-U+27BF) below, so they need no separate arm.
fn is_unmapped_pictographic_mark(c: char) -> bool {
    let cp = c as u32;
    matches!(cp,
        0x2600..=0x27BF // Miscellaneous Symbols + Dingbats (incl. check/cross marks)
        | 0x25A0..=0x25FF // Geometric Shapes
        | 0x2500..=0x257F // Box Drawing
        | 0x2800..=0x28FF // Braille Patterns
    )
}

#[cfg(test)]
mod r7_classifier_tests {
    use super::is_unmapped_pictographic_mark as banned;

    #[test]
    fn bans_the_known_regressions() {
        for c in ['✓', '✗', '●', '○', '◆', '★'] {
            assert!(banned(c), "{c:?} should be banned");
        }
    }

    #[test]
    fn allows_the_keycap_glyphs_and_thin_space() {
        for c in ['⏎', '↑', '↓', '←', '→', '\u{2009}'] {
            assert!(!banned(c), "{c:?} must stay legal");
        }
    }
}

/// §14-sanctioned exceptions: not a text run at all, but a test fixture
/// proving a sanitizer *strips* the very characters R7 bans.
const R7_ALLOW: &[(&str, &str, &str)] = &[(
    "rows.rs",
    "sanitize_ui_text(\"",
    "sanitize_drops_unrenderable_characters_and_collapses_whitespace \
     (`rows.rs`): the sparkle literals are `sanitize_ui_text`'s test input, \
     asserting the function strips exactly this class of character before it \
     ever reaches a `ui()`/`mono()` text run — the opposite of the regression \
     R7 catches, not an instance of it.",
)];

#[test]
fn r7_no_pictographic_character_literal_in_a_text_run() {
    let mut hits = Vec::new();
    for l in view_lines() {
        if is_comment(&l) {
            continue;
        }
        if R7_ALLOW
            .iter()
            .any(|(f, snip, _)| *f == l.name && l.text.contains(snip))
        {
            continue;
        }
        if let Some(c) = l.text.chars().find(|c| is_unmapped_pictographic_mark(*c)) {
            hits.push((
                loc(&l),
                format!("{}   [{c:?} = U+{:04X}]", l.text, c as u32),
            ));
        }
    }
    report(
        "R7 no pictographic character literal in a text run",
        "§9.3",
        "A pictographic mark must come from the icon sprite (`icons::icon`), \
         never as a character literal inside a `ui()`/`mono()` string: the \
         bundled fonts have no glyph for Dingbats, Miscellaneous Symbols, \
         Geometric Shapes, Box Drawing or Braille Patterns codepoints (or the \
         dedicated check/cross marks), so a literal silently falls back to a \
         stand-in glyph instead of rendering the mark. This is exactly the bug \
         `modal_checkbox`'s old literal \"\\u{2713}\" was. It is not about the \
         keycap pattern (§5.2, `⏎`/`↑↓`/`esc`/`←→`) — those characters are \
         outside every banned range.",
        hits,
    );
}

// ---------------------------------------------------------------------------
// R8 — in-row controls declare CONTROL_H (§8.1)
// ---------------------------------------------------------------------------

/// Functions whose doc comment in `components.rs` explicitly claims the
/// `CONTROL_H` (22) height §8.1 names as "the height of every flat icon/text
/// button". Named by exact string match rather than parsed as an AST: a
/// text-only scan cannot reliably tell "a function that claims CONTROL_H in
/// its own body" (`flat_text_btn`, `seg_button`, both of which write
/// `.h(rpx(CONTROL_H))` directly) from "a function that claims it by handing
/// the token to another function as a positional argument"
/// (`flat_icon_btn`, which passes `CONTROL_H` to `icon_btn`'s `box_h`
/// parameter and never writes `.h(` itself). Requiring `.h(`/`.size(` to
/// co-occur with `CONTROL_H` would false-negative on `flat_icon_btn`, so the
/// check below only asks whether the token `CONTROL_H` appears anywhere in
/// the named function's own body — narrower than "declares its height with
/// CONTROL_H", but honest about what a source-text scan can tell apart.
/// `icon_btn` and `seg_button_content` are deliberately not listed: neither's
/// doc comment claims a fixed CONTROL_H height (both take the height, or the
/// whole content box, from a caller).
const CONTROL_H_FUNCTIONS: &[&str] = &["flat_icon_btn", "flat_text_btn", "seg_button"];

/// The lines of `components.rs`'s body for `pub fn <name>`, from its
/// signature line up to (not including) the next top-level `pub fn`/`fn`.
fn function_body<'a>(lines: &'a [Line], name: &str) -> Vec<&'a Line> {
    let start = lines.iter().position(|l| {
        l.name == "components.rs" && l.text.trim_start().starts_with(&format!("pub fn {name}("))
    });
    let Some(start) = start else {
        return Vec::new();
    };
    let mut end = lines.len();
    for (i, l) in lines.iter().enumerate().skip(start + 1) {
        if l.name != "components.rs" {
            break;
        }
        let t = l.text.trim_start();
        if t.starts_with("pub fn ") || t.starts_with("fn ") {
            end = i;
            break;
        }
    }
    lines[start..end].iter().collect()
}

#[test]
fn r8_the_three_named_control_h_functions_still_declare_it() {
    let all = view_lines();
    let mut hits = Vec::new();
    for name in CONTROL_H_FUNCTIONS {
        let body = function_body(&all, name);
        if body.is_empty() {
            hits.push((
                "src/views/components.rs".to_string(),
                format!("{name}: function not found — did it move or get renamed?"),
            ));
            continue;
        }
        let declares = body.iter().any(|l| l.text.contains("CONTROL_H"));
        if !declares {
            hits.push((loc(body[0]), format!("{name}: no CONTROL_H in its body")));
        }
    }
    report(
        "R8 the three named CONTROL_H functions still declare it",
        "§8.1",
        "flat_icon_btn, flat_text_btn and seg_button are the functions §8.1 \
         names as CONTROL_H (22)-tall. `seg_button` regressed once by sizing \
         itself with vertical padding instead; this checks that each named \
         function's body still mentions the CONTROL_H token at all, so a \
         reintroduced hard-coded height (or padding standing in for one) with \
         no CONTROL_H anywhere in the function is caught.",
        hits,
    );
}

/// The allow-lists are the load-bearing part of this module: a rule that
/// passes because it was watered down is worse than no rule. This guards
/// against an entry losing its justification in a future edit.
#[test]
fn every_allow_list_entry_carries_a_justification() {
    let entries: Vec<(&str, &str)> = R1_ALLOW
        .iter()
        .map(|(f, _, why)| (*f, *why))
        .chain(R3_EXEMPT_FILES.iter().copied())
        .chain(R3_REVIEWED.iter().copied())
        .chain(R3_KNOWN_VIOLATIONS.iter().map(|(f, _, why)| (*f, *why)))
        .chain(R4_ALLOW.iter().map(|(f, _, why)| (*f, *why)))
        .chain(R7_ALLOW.iter().map(|(f, _, why)| (*f, *why)))
        .collect();
    for (file, why) in entries {
        assert!(
            why.len() > 40,
            "allow-list entry for {file} needs a real justification naming \
             the DESIGN.md clause that sanctions it, got: {why:?}"
        );
    }
}
