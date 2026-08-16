//! DESIGN.md conformance tripwires — a lint-by-grep over the view layer, not a real linter (see DESIGN.md).

#![cfg(test)]

use fs_err as fs;
use std::fmt::Write as _;
use std::path::PathBuf;

/// A source line, with everything a failure message needs to be actionable.
struct Line {
    file: String,
    name: String,
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
            // Skip self: this file quotes every banned pattern verbatim and would flag itself.
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

// R1 — no bare numeric literal in a styling call (§6.1, §13 "Code")

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

/// §14 case 3 — optical corrections.
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

// R2 — exactly one border weight (§7.2)

/// The number between `open` and the next `)`, if it is a plain literal — `None` for a variable.
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

// R3 — every text run pins a font family (§5.1, §5.2)

/// Tuned to the widest legitimate gap in the real tree (confirm.rs's input_modal, 6 lines).
const R3_WINDOW: usize = 6;

const R3_EXEMPT_FILES: &[(&str, &str)] = &[(
    "components.rs",
    "§5.2: this file *is* the two text primitives. `ui` and `mono` are the \
     only places a family is bound to a size; every other view consumes them.",
)];

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

/// Debt tracker, not an exemption — must stay empty.
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

// R4 — display tiers are never chrome (§5.3, §13 "Visual")

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

// R5 — tracking is mono-only (§5.4)

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

// R6 — no bare numeric literal as a size argument (§5.3.1, §5.3.2, §8.5)

/// Functions R1 can't see: sizes passed positionally instead of via `.setter(rpx(N))`.
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

/// Covers the longest real call (icon_btn's ten args plus a multi-line closure).
const R6_WINDOW: usize = 24;

/// `true` if `t` is nothing but a numeric literal — an identifier or expression is not.
fn is_numeric_literal(t: &str) -> bool {
    let t = t.trim();
    !t.is_empty()
        && t.chars().all(|c| c.is_ascii_digit() || c == '.')
        && t.chars().any(|c| c.is_ascii_digit())
}

/// The top-level, comma-separated arguments of a call whose `(` was already consumed.
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

// R7 — no pictographic character literal in a text run (§9.3)

/// Scoped to Dingbats/Misc Symbols/Geometric Shapes/Box Drawing/Braille, per DESIGN.md §9.3.
fn is_unmapped_pictographic_mark(c: char) -> bool {
    let cp = c as u32;
    matches!(cp,
        0x2600..=0x27BF
        | 0x25A0..=0x25FF
        | 0x2500..=0x257F
        | 0x2800..=0x28FF
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

// R8 — in-row controls declare CONTROL_H (§8.1)

/// Named explicitly because a text scan can't tell "declares CONTROL_H directly" from "hands it to another function" (flat_icon_btn).
const CONTROL_H_FUNCTIONS: &[&str] = &["flat_icon_btn", "flat_text_btn", "seg_button"];

fn function_body<'a>(lines: &'a [Line], name: &str) -> Vec<&'a Line> {
    function_body_in(lines, "components.rs", name)
}

fn function_body_in<'a>(lines: &'a [Line], file: &str, name: &str) -> Vec<&'a Line> {
    let start = lines.iter().position(|l| {
        l.name == file
            && (l.text.trim_start().starts_with(&format!("pub fn {name}("))
                || l.text.trim_start().starts_with(&format!("fn {name}(")))
    });
    let Some(start) = start else {
        return Vec::new();
    };
    let mut end = lines.len();
    for (i, l) in lines.iter().enumerate().skip(start + 1) {
        if l.name != file {
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

// R9 — every modal panel's width is a token (§8.5)

#[test]
fn r9_every_modal_panel_width_is_a_token() {
    let lines = view_lines();
    let mut hits = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if is_comment(l) {
            continue;
        }
        let pat = "modal_panel(";
        let mut from = 0;
        while let Some(off) = l.text[from..].find(pat) {
            let at = from + off;
            from = at + pat.len();
            // Skip the definition itself — a signature is not a call site.
            let trimmed = l.text.trim_start();
            if trimmed.starts_with("pub fn ") || trimmed.starts_with("fn ") {
                continue;
            }
            let args = call_args(&lines, i, from);
            let Some(first) = args.first() else { continue };
            let first = first.trim();
            if first.is_empty() {
                continue;
            }
            if !first.starts_with("MODAL_W_") {
                let why = if is_numeric_literal(first) {
                    format!("modal_panel(...) width is a bare numeric literal ({first}), not a MODAL_W_* token")
                } else {
                    format!("modal_panel(...) width ({first}) is not a MODAL_W_* token")
                };
                hits.push((loc(l), format!("{}   [{why}]", l.text)));
            }
        }
    }
    report(
        "R9 every modal panel's width is a token",
        "§8.5",
        "modal_panel's first argument must be one of MODAL_W_SM/MD/LG/XL in \
         src/views/tokens.rs. This is the regression class that let a private \
         `PALETTE_W` (760) sit outside the four-notch width scale undetected: \
         the command palette used to call `modal_panel(PALETTE_W, ..)` where \
         PALETTE_W itself was never derived from the token scale. \
         `modal_panel(420.0, ..)` is wrong; `modal_panel(MODAL_W_SM, ..)` is \
         right.",
        hits,
    );
}

// R10 — no rule above a footer (§9.1.1 "the one rule after the header")

#[test]
fn r10_no_divider_directly_above_a_footer() {
    let lines = view_lines();
    let mut hits = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if is_comment(l) {
            continue;
        }
        if !l.text.contains("divider_h()") {
            continue;
        }
        let footer_pats = ["modal_footer(", "modal_footer_hints("];
        let same_line = footer_pats.iter().any(|p| l.text.contains(p));
        let next_line = lines
            .get(i + 1)
            .is_some_and(|n| n.file == l.file && footer_pats.iter().any(|p| n.text.contains(p)));
        if same_line || next_line {
            hits.push((loc(l), l.text.clone()));
        }
    }
    report(
        "R10 no rule above a footer",
        "§9.1.1 \"the one rule after the header\"",
        "divider_h() sits directly under the header and nowhere else. A \
         divider_h() on the same line as, or the line immediately before, a \
         modal_footer/modal_footer_hints call double-marks the footer seam: \
         footer_container now draws its own top divider as that seam, so a \
         second divider_h() at a call site is two rules for one boundary.",
        hits,
    );
}

/// R10's inverse: footer_container itself must still draw the seam, or the call-site check above passes vacuously.
#[test]
fn r10_footer_container_draws_its_own_top_divider() {
    let lines = view_lines();
    let body = function_body(&lines, "footer_container");
    assert!(
        !body.is_empty(),
        "R10 (inverse) — footer_container not found in components.rs — did it move or get renamed?"
    );
    let has_divider = body.iter().any(|l| l.text.contains("divider_h()"));
    assert!(
        has_divider,
        "R10 (inverse) — DESIGN.md §9.1.1: footer_container's body no longer \
         calls divider_h(). C2g's statusbar footer has no BG_STRIP fill to \
         mark the seam, so the top divider drawn inside footer_container is \
         the *only* boundary a footer has left; removing it silently drops \
         every modal's footer seam at once."
    );
}

// R11 — modal headers use only the two sanctioned accents (§9.1.1's accent rule)

const R11_ACCENT_ALLOWLIST: &[&str] = &["arch-close"];

#[test]
fn r11_modal_headers_use_only_the_two_sanctioned_accents() {
    let lines = view_lines();
    let mut hits = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if is_comment(l) {
            continue;
        }
        // Every accent that reaches a header arrives through one of these three call shapes.
        for pat in [
            "modal_header_with_close(",
            "modal_header_slotted(",
            "modal_header_slotted_custom(",
        ] {
            let mut from = 0;
            while let Some(off) = l.text[from..].find(pat) {
                let at = from + off;
                from = at + pat.len();
                let trimmed = l.text.trim_start();
                if trimmed.starts_with("pub fn ") || trimmed.starts_with("fn ") {
                    continue;
                }
                let args = call_args(&lines, i, from);
                let allowed = args
                    .first()
                    .is_some_and(|id| R11_ACCENT_ALLOWLIST.iter().any(|a| id.contains(a)));
                if !allowed
                    && args
                        .iter()
                        .any(|a| a.contains("c::CYAN()") || a.contains("c::AMBER()"))
                {
                    hits.push((loc(l), l.text.clone()));
                }
            }
        }
    }
    report(
        "R11 modal headers use only the two sanctioned accents",
        "§9.1.1's accent rule",
        "MAGENTA is the default accent for every modal header, RED is for a \
         destructive one. CYAN and AMBER are not modal-header colours — both \
         were, before the sweep, and both are gone now: one accent per \
         emphasis level (default, destructive), not a header-specific palette \
         of its own, save the R11_ACCENT_ALLOWLIST exceptions (currently: \
         the archive gate's caution AMBER). They may still legitimately \
         appear elsewhere in the view layer (status dots, icons); this rule \
         is scoped to modal header calls only.",
        hits,
    );
}

// R12 — every text field keeps the zeroed-inset Input contract (§14, field_box's doc comment)

/// Scoped to Input::new( sites, not just field_box( — the launcher's search zone builds a bare Input directly.
const INPUT_CONTRACT_CALLS: &[&[&str]] = &[
    &[".appearance(false)"],
    &[".pl(px(0", ".pl(gpui::px(0"],
    &[".pr(px(0", ".pr(gpui::px(0"],
    &[".py(px(0", ".py(gpui::px(0"],
    &[".w_full("],
];

const INPUT_CONTRACT_WINDOW: usize = 16;

#[test]
fn r12_every_text_field_keeps_the_zeroed_inset_input_contract() {
    let lines = view_lines();
    let mut hits = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if is_comment(l) {
            continue;
        }
        let pat = "Input::new(";
        if l.text.find(pat).is_none() {
            continue;
        }
        let hi = (i + INPUT_CONTRACT_WINDOW).min(lines.len());
        let window: String = lines[i..hi]
            .iter()
            .take_while(|n| n.file == l.file)
            .filter(|n| !is_comment(n))
            .map(|n| n.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let missing: Vec<&str> = INPUT_CONTRACT_CALLS
            .iter()
            .filter(|alts| !alts.iter().any(|c| window.contains(*c)))
            .map(|alts| alts[0])
            .collect();
        if !missing.is_empty() {
            hits.push((
                loc(l),
                format!("{}   [missing: {}]", l.text, missing.join(", ")),
            ));
        }
    }
    report(
        "R12 every text field keeps the zeroed-inset Input contract",
        "field_box's doc comment (§14)",
        "Every Input::new(..) in the view layer must chain \
         .appearance(false).pl(px(0.0)).pr(px(0.0)).py(px(0.0)).w_full() — \
         Input applies its own input_px/input_py padding regardless of \
         .appearance(false), and a missing .appearance(false) draws the \
         third-party widget's own border and fill inside field_box, which \
         deliberately paints neither, reintroducing exactly what §14 removed.",
        hits,
    );
}

// R13 — field_underline is retired and cannot come back (plan.md §1)

#[test]
fn r13_field_underline_is_gone_and_cannot_come_back() {
    let mut hits = Vec::new();
    for l in view_lines() {
        if is_comment(&l) {
            continue;
        }
        if l.text.contains("field_underline") {
            hits.push((loc(&l), l.text.clone()));
        }
    }
    report(
        "R13 field_underline is gone and cannot come back",
        "plan.md §1",
        "field_underline retired in favour of field_box, which now paints \
         nothing at all per §14 (it was the boxed-plus-focus-ring variant C1c \
         when the rule was written; the retirement stands either way). The \
         identifier must not appear anywhere in the view layer outside a \
         comment — not even as a dangling doc-comment reference to a function \
         that no longer exists.",
        hits,
    );
}

// R14 — esc hint vocabulary is exactly cancel / close / back (plan.md §3)

const R14_ALLOW: &[(&str, &str, &str)] = &[(
    "project.rs",
    "skip & remove",
    "Teardown's esc hint (project.rs, ArchiveProject in-progress state): \
     Escape there doesn't just dismiss the modal, it skips the running \
     script and proceeds straight to removal — a real, semantic action \
     distinct from cancel/close/back, and the modal's own body text says so \
     next to the hint.",
)];

fn char_before(text: &str, at: usize) -> Option<char> {
    text[..at].chars().next_back()
}

#[test]
fn r14_esc_hint_vocabulary_is_exactly_cancel_close_or_back() {
    let lines = view_lines();
    let allowed = ["cancel", "close", "back"];
    let mut hits = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if is_comment(l) {
            continue;
        }
        // Case A: the common single-line shape, `("esc", "label")`.
        let pat = "(\"esc\"";
        if let Some(off) = l.text.find(pat) {
            let preceding = char_before(&l.text, off);
            let is_call = preceding.is_some_and(|c| c.is_alphanumeric() || c == '_');
            if !is_call {
                let rest = &l.text[off + pat.len()..];
                if let Some(q1) = rest.find('"') {
                    if let Some(q2) = rest[q1 + 1..].find('"') {
                        let label = &rest[q1 + 1..q1 + 1 + q2];
                        if !allowed.contains(&label)
                            && !R14_ALLOW
                                .iter()
                                .any(|(f, snip, _)| *f == l.name && label == *snip)
                        {
                            hits.push((loc(l), format!("{}   [label: {label:?}]", l.text)));
                        }
                        continue;
                    }
                }
                // Case B: key and label on separate lines (rustfmt one-per-line tuple).
                for n in lines.iter().skip(i + 1).take(4) {
                    if n.file != l.file || is_comment(n) {
                        continue;
                    }
                    let t = n.text.trim().trim_end_matches(',');
                    if let Some(label) = t.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
                        if !allowed.contains(&label)
                            && !R14_ALLOW
                                .iter()
                                .any(|(f, snip, _)| *f == n.name && label == *snip)
                        {
                            hits.push((loc(n), format!("{}   [label: {label:?}]", n.text.trim())));
                        }
                        break;
                    }
                }
            }
        } else if l.text.trim() == "\"esc\"," {
            // Standalone-line half of case B — the `(` sits on the previous line.
            for n in lines.iter().skip(i + 1).take(4) {
                if n.file != l.file || is_comment(n) {
                    continue;
                }
                let t = n.text.trim().trim_end_matches(',');
                if let Some(label) = t.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
                    if !allowed.contains(&label)
                        && !R14_ALLOW
                            .iter()
                            .any(|(f, snip, _)| *f == n.name && label == *snip)
                    {
                        hits.push((loc(n), format!("{}   [label: {label:?}]", n.text.trim())));
                    }
                    break;
                }
            }
        }
    }
    report(
        "R14 esc hint vocabulary is exactly cancel / close / back",
        "plan.md §3 \"esc hint vocabulary\"",
        "esc means exactly one of three things: cancel (abandons typed \
         input), close (nothing to lose) or back (returns to the parent \
         view). A fourth word — \"later\", \"back to settings\" — is a \
         vocabulary the reader has to learn per-modal instead of once for \
         the whole app.",
        hits,
    );
}

// R15 — footer button order is always Plain before Primary/Danger (plan.md §3)

#[test]
fn r15_footer_button_order_is_plain_before_primary_or_danger() {
    let lines = view_lines();
    let mut hits = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if is_comment(l) {
            continue;
        }
        let pat = "modal_footer(";
        let mut from = 0;
        while let Some(off) = l.text[from..].find(pat) {
            let at = from + off;
            from = at + pat.len();
            let trimmed = l.text.trim_start();
            if trimmed.starts_with("pub fn ") || trimmed.starts_with("fn ") {
                continue;
            }
            let args = call_args(&lines, i, from);
            let Some(buttons) = args.last() else { continue };
            let plain_at = buttons.find("ModalBtn::Plain");
            let Some(plain_at) = plain_at else { continue };
            for kind in ["ModalBtn::Primary", "ModalBtn::Danger"] {
                if let Some(k_at) = buttons.find(kind) {
                    if k_at < plain_at {
                        hits.push((loc(l), format!("{}   [{kind} before Plain]", l.text)));
                    }
                }
            }
        }
    }
    report(
        "R15 footer button order is always Plain before Primary/Danger",
        "plan.md §3 \"Fix Updating(Updated) button order\"",
        "Every modal_footer's buttons run secondary (Plain) then affirmative \
         (Primary/Danger), left to right — Cancel before Save, Later before \
         Restart. A primary-first footer reads as \"the default action is \
         the one on the left\", which is backwards everywhere else in the \
         app.",
        hits,
    );
}

// R16 — FOOTER_RADIUS is retired; the panel radius stays full RADIUS_PANEL (plan.md §2)

#[test]
fn r16_footer_radius_is_retired() {
    let lines = view_lines();
    let mut hits = Vec::new();
    for l in &lines {
        if is_comment(l) {
            continue;
        }
        if l.text.contains("FOOTER_RADIUS") {
            hits.push((loc(l), l.text.clone()));
        }
    }
    let body = function_body(&lines, "footer_container");
    for l in &body {
        if is_comment(l) {
            continue;
        }
        if l.text.contains("rounded_bl") || l.text.contains("rounded_br") {
            hits.push((
                loc(l),
                format!("{}   [footer_container re-rounds a corner]", l.text),
            ));
        }
        if l.text.contains("bg(c::BG_STRIP())") {
            hits.push((
                loc(l),
                format!("{}   [footer_container re-fills the strip]", l.text),
            ));
        }
    }
    report(
        "R16 FOOTER_RADIUS is retired",
        "plan.md §2",
        "C2g's statusbar footer paints no fill, so the panel's own \
         RADIUS_PANEL (12) is the only corner radius left — the old \
         FOOTER_RADIUS inner-corner notch has no fill to stay flush with. A \
         re-introduced bg(BG_STRIP()) with the radius gone produces a \
         square-cornered strip poking outside the panel's rounded corners, \
         which no other rule can see.",
        hits,
    );
}

// R17 — radius arguments are RADIUS_* tokens (§7.1's four-notch scale, extends R1/R6)

const RADIUS_TOKENS: &[&str] = &[
    "RADIUS_CONTROL",
    "RADIUS_GROUP",
    "RADIUS_PANEL",
    "RADIUS_FULL",
    "SWATCH_RADIUS",
];

const ROUNDED_SETTERS: &[&str] = &[
    "rounded_bl",
    "rounded_br",
    "rounded_tl",
    "rounded_tr",
    "rounded",
];

#[test]
fn r17_radius_arguments_are_radius_tokens() {
    let lines = view_lines();
    let mut hits = Vec::new();
    for l in &lines {
        if is_comment(l) {
            continue;
        }
        for setter in ROUNDED_SETTERS {
            let pat = format!(".{setter}(rpx(");
            let mut from = 0;
            while let Some(i) = l.text[from..].find(&pat) {
                let at = from + i;
                from = at + pat.len();
                let arg = &l.text[from..];
                let is_token = RADIUS_TOKENS.iter().any(|t| arg.starts_with(t));
                if is_token {
                    continue;
                }
                // An off-scale ALL-CAPS identifier is the blind spot R1 admits it has.
                let ident: String = arg
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                let is_bare_number = ident.chars().next().is_some_and(|c| c.is_ascii_digit());
                let looks_like_a_token = !ident.is_empty()
                    && ident
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit());
                if is_bare_number || looks_like_a_token {
                    hits.push((loc(l), format!("{}   [.{setter}(rpx(…))]", l.text)));
                }
            }
        }
    }
    report(
        "R17 radius arguments are RADIUS_* tokens",
        "§7.1's four-notch radius scale",
        "Every rounded/rounded_bl/rounded_br/rounded_tl/rounded_tr call \
         takes RADIUS_CONTROL (4), RADIUS_GROUP (6), RADIUS_PANEL (12), \
         RADIUS_FULL, or the tokenized SWATCH_RADIUS (2) — never a bare \
         number or an off-scale identifier. R1 catches a bare number here; \
         this catches the identifier case R1's own doc comment admits it \
         cannot (\"it cannot tell a good identifier from a bad one\").",
        hits,
    );
}

// R18 — the retired overflow windows do not come back (plan.md §3 "one overflow strategy")

const R18_CAPPED_FNS: &[(&str, &str)] = &[
    ("add_project.rs", "dir_list"),
    ("theme_picker.rs", "picker"),
    ("theme_picker.rs", "manager"),
    ("settings.rs", "shortcut_overlay"),
];

#[test]
fn r18_retired_overflow_windows_do_not_come_back() {
    let lines = view_lines();
    let mut hits = Vec::new();
    for l in &lines {
        if is_comment(l) {
            continue;
        }
        if l.text.contains("DIR_ROWS") {
            hits.push((loc(l), format!("{}   [DIR_ROWS window retired]", l.text)));
        }
        if l.text.contains(".take(8)") {
            hits.push((loc(l), format!("{}   [8-row window retired]", l.text)));
        }
    }
    for (file, func) in R18_CAPPED_FNS {
        let body = function_body_in(&lines, file, func);
        if body.is_empty() {
            hits.push((
                format!("src/views/modals/{file}"),
                format!("{func}: function not found — did it move or get renamed?"),
            ));
            continue;
        }
        let present = body.iter().any(|l| l.text.contains("MODAL_SCROLL_MAX_H"));
        if !present {
            hits.push((
                loc(body[0]),
                format!(
                    "{func}: MODAL_SCROLL_MAX_H not found in its body — this surface's list/body must scroll under the shared cap"
                ),
            ));
        }
    }
    report(
        "R18 the retired overflow windows do not come back",
        "plan.md §3 \"one overflow strategy\"",
        "AddProject's DIR_ROWS window and ThemePicker's 8-row take(8) both \
         retired in favour of one shared MODAL_SCROLL_MAX_H (456) body cap \
         with scroll, which ShortcutOverlay's previously-uncapped body now \
         shares too.",
        hits,
    );
}

// R19 — one slotted header; the header-row fork is components-only (plan.md §3)

#[test]
fn r19_one_slotted_header_the_row_fork_is_components_only() {
    let lines = view_lines();
    let mut hits = Vec::new();

    // modal_header_row must stay private, or the cross-file ban below is unfalsifiable.
    let sig = lines.iter().find(|l| {
        l.name == "components.rs" && l.text.trim_start().starts_with("fn modal_header_row(")
    });
    match sig {
        None => hits.push((
            "src/views/components.rs".to_string(),
            "modal_header_row: function not found — did it move or get renamed?".to_string(),
        )),
        Some(l) => {
            if l.text.trim_start().starts_with("pub fn ") {
                hits.push((
                    loc(l),
                    format!(
                        "{}   [modal_header_row is pub — the cross-file ban below is only \
                         meaningful while it stays private]",
                        l.text
                    ),
                ));
            }
        }
    }

    for l in &lines {
        if is_comment(l) {
            continue;
        }
        if l.name == "components.rs" {
            continue;
        }
        if l.text.contains("modal_header_row(") {
            hits.push((loc(l), l.text.clone()));
        }
    }
    report(
        "R19 one slotted header; the header-row fork is components-only",
        "plan.md §3 \"one header component with optional slots\"",
        "modal_header_row is modal_header_slotted's internal row shell, not \
         a second header primitive — every modal reaches its header through \
         modal_header_slotted / modal_header_slotted_custom / \
         modal_header_with_close. A modal file calling modal_header_row \
         directly is a fourth fork rebuilding the header by hand, and \
         staying private is what makes that impossible rather than merely \
         unwritten; R11 now also scans the slotted-header call sites \
         directly, so a hand-built header row is no longer the accent-check \
         hole it used to be.",
        hits,
    );
}

// R20 — no hard-coded panel shadow, no bordered click_action inside a body (plan.md §3)

const R20_BANNED_BORDERED_LABELS: &[&str] = &["Change", "Kill all sessions", "Browse"];

#[test]
fn r20_no_hardcoded_shadow_or_bordered_in_body_action() {
    let lines = view_lines();
    let mut hits = Vec::new();

    // Distinguish "genuinely shadow-free" from "renamed out from under this rule".
    let body = function_body(&lines, "modal_panel");
    assert!(
        !body.is_empty(),
        "R20 — modal_panel not found in components.rs — did it move or get renamed?"
    );
    let has_shadow_token = body.iter().any(|l| l.text.contains("c::PANEL_SHADOW()"));
    assert!(
        has_shadow_token,
        "R20 — modal_panel's body no longer mentions c::PANEL_SHADOW() at \
         all; the panel shadow was likely deleted rather than themed."
    );
    for l in &body {
        if is_comment(l) {
            continue;
        }
        if l.text.contains("color:") && !l.text.contains("c::PANEL_SHADOW()") {
            hits.push((
                loc(l),
                format!("{}   [shadow color not PANEL_SHADOW()]", l.text),
            ));
        }
    }

    for (i, l) in lines.iter().enumerate() {
        if is_comment(l) {
            continue;
        }
        let pat = "click_action(";
        let mut from = 0;
        while let Some(off) = l.text[from..].find(pat) {
            let at = from + off;
            from = at + pat.len();
            let trimmed = l.text.trim_start();
            if trimmed.starts_with("pub fn ") || trimmed.starts_with("fn ") {
                continue;
            }
            let args = call_args(&lines, i, from);
            let Some(label) = args.get(1) else { continue };
            if let Some(name) = R20_BANNED_BORDERED_LABELS
                .iter()
                .find(|banned| label.contains(**banned))
            {
                hits.push((
                    loc(l),
                    format!("{}   [bordered click_action for {name:?}]", l.text),
                ));
            }
        }
    }

    report(
        "R20 no hard-coded panel shadow or bordered in-body action",
        "plan.md §3",
        "modal_panel's drop shadow is crate::theme::PANEL_SHADOW() in every \
         theme, not a hard-coded rgba(0,0,0,.35) — the one piece of panel \
         chrome that used to never track a theme swap. And a bordered \
         ModalBtn shell (\"Change\", \"Kill all sessions\", \"Browse…\") \
         inside a modal body reads as a second call to action competing \
         with the footer's own; those three route through body_action's \
         flat tinted text instead.",
        hits,
    );
}

/// The allow-lists are the load-bearing part of this module — guards against one losing its justification.
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
        .chain(R14_ALLOW.iter().map(|(f, _, why)| (*f, *why)))
        .collect();
    for (file, why) in entries {
        assert!(
            why.len() > 40,
            "allow-list entry for {file} needs a real justification naming \
             the DESIGN.md clause that sanctions it, got: {why:?}"
        );
    }
}
