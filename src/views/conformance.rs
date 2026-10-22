//! Executable DESIGN.md tripwires for the current native component architecture.
//! These source checks complement native interaction QA; they do not establish it.
#![cfg(test)]
use fs_err as fs;
use std::path::{Path, PathBuf};

fn sources() -> Vec<(String, String)> {
    fn collect(path: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                collect(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs")
                && path
                    .file_name()
                    .is_some_and(|name| name != "conformance.rs")
            {
                out.push(path);
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = vec![];
    collect(&root.join("src/views"), &mut files);
    assert!(files.len() > 20, "all nested native views must be scanned");
    files
        .into_iter()
        .map(|path| {
            (
                path.strip_prefix(root).unwrap().display().to_string(),
                fs::read_to_string(path).unwrap(),
            )
        })
        .collect()
}
fn reject(rule: &str, predicate: impl Fn(&str) -> bool) {
    let mut hits = vec![];
    for (file, source) in sources() {
        for (line, text) in source.lines().enumerate() {
            if !text.trim_start().starts_with("//") && predicate(text) {
                hits.push(format!("{file}:{}: {}", line + 1, text.trim()));
            }
        }
    }
    assert!(hits.is_empty(), "{rule} (DESIGN.md):\n{}", hits.join("\n"));
}
fn source(path: &str) -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(path)).unwrap()
}
fn literal_after(text: &str, marker: &str) -> bool {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    compact.match_indices(marker).any(|(at, _)| {
        let argument = compact[at + marker.len()..].trim_start_matches(['-', '+']);
        argument.starts_with(|c: char| c.is_ascii_digit() || c == '.')
    })
}
/// Capture one fluent constructor, ending at the caller's closing parenthesis.
fn constructor_chain(text: &str) -> &str {
    let mut depth = 0_i32;
    let mut quoted = false;
    let mut escaped = false;
    for (index, ch) in text.char_indices() {
        if quoted {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                quoted = false;
            }
            continue;
        }
        match ch {
            '"' => quoted = true,
            '(' | '{' | '[' => depth += 1,
            ')' | '}' | ']' => {
                depth -= 1;
                if depth < 0 {
                    return &text[..index];
                }
            }
            ',' | ';' if depth == 0 => return &text[..index],
            _ => {}
        }
    }
    text
}
fn missing_input_contract(chain: &str) -> Vec<&'static str> {
    [
        ".aria_label(",
        ".appearance(false)",
        ".bordered(false)",
        ".focus_bordered(false)",
        ".p_0()",
    ]
    .into_iter()
    .filter(|required| !chain.contains(required))
    .collect()
}
#[test]
fn r1_numeric_geometry_uses_tokens_or_module_constants() {
    reject("R1 numeric geometry", |line| literal_after(line, "rpx("));
}
#[test]
fn r2_border_weights_remain_unscaled() {
    reject("R2 unscaled borders", |line| {
        [".border(rpx(", ".border_width(rpx("]
            .iter()
            .any(|p| line.contains(p))
    });
}
#[test]
fn r3_native_ui_uses_bundled_family() {
    let shell = source("src/views/shell.rs");
    assert!(shell.contains(".font_family(crate::fonts::UI_FAMILY)"));
    reject("R3 bundled UI family", |line| {
        line.contains(".font_family(\"") || line.contains(".font(gpui::font(\"")
    });
}
#[test]
fn r4_display_icons_are_decorative() {
    reject("R4 display glyph in control", |line| {
        line.contains("header_control") && line.contains("ICON_DISPLAY")
    });
}
#[test]
fn r5_tracking_does_not_enter_ui_text() {
    reject("R5 UI tracking", |line| line.contains(".letter_spacing("));
}
#[test]
fn r6_catalog_icons_have_named_size_roles() {
    reject("R6 catalog icon size", numeric_icon_size);
}
fn numeric_icon_size(line: &str) -> bool {
    line.match_indices("icon(").any(|(at, _)| {
        let args = &line[at + "icon(".len()..];
        let first = constructor_chain(args);
        let rest = args[first.len()..]
            .strip_prefix(',')
            .unwrap_or("")
            .trim_start();
        let argument = constructor_chain(rest).trim();
        argument.parse::<f32>().is_ok()
    })
}

#[test]
fn r7_action_glyphs_come_from_catalog() {
    for (file, source) in sources() {
        assert!(
            !has_text_icon_child(&source),
            "R7 action glyph (DESIGN.md): {file} must use catalog icons"
        );
        for name in literal_icon_references(&source) {
            let path = if name.starts_with("icons/") {
                name.to_owned()
            } else {
                format!("icons/{name}.svg")
            };
            let svg = crate::icons::svg_for_path(&path)
                .unwrap_or_else(|| panic!("R7 missing icon asset: {file}: {path}"));
            let (_, body) = svg.split_once('>').expect("SVG root");
            assert!(
                !body.strip_suffix("</svg>").unwrap().trim().is_empty(),
                "R7 empty icon asset: {file}: {path}"
            );
        }
    }
}

/// Inspect complete child arguments so conditional and multiline glyphs cannot
/// evade the guard. Signed counts and keyboard hints remain ordinary text.
fn has_text_icon_child(source: &str) -> bool {
    source.match_indices(".child(").any(|(at, marker)| {
        let argument = constructor_chain(&source[at + marker.len()..]);
        quoted_literals(argument).any(|text| {
            matches!(
                text,
                "+" | "-"
                    | "−"
                    | "✓"
                    | "✔"
                    | "✕"
                    | "✖"
                    | "×"
                    | "▶"
                    | "▷"
                    | "◀"
                    | "◁"
                    | "▸"
                    | "▾"
                    | "▼"
                    | "▲"
                    | "↑"
                    | "↓"
                    | "←"
                    | "→"
                    | "↗"
                    | "⚙"
                    | "⋮"
                    | "⋯"
                    | "☰"
                    | "⚠"
                    | "●"
                    | "○"
                    | "◆"
                    | "◇"
                    | "■"
                    | "□"
                    | "★"
                    | "☆"
            )
        })
    })
}

fn quoted_literals(mut source: &str) -> impl Iterator<Item = &str> {
    std::iter::from_fn(move || {
        let start = source.find('"')? + 1;
        source = &source[start..];
        let mut escaped = false;
        for (end, ch) in source.char_indices() {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                let literal = &source[..end];
                source = &source[end + 1..];
                return Some(literal);
            }
        }
        None
    })
}

fn literal_icon_references(source: &str) -> Vec<&str> {
    let mut names: Vec<_> = quoted_literals(source)
        .filter(|text| text.starts_with("icons/") && text.ends_with(".svg") && !text.contains('{'))
        .collect();
    for (at, marker) in source.match_indices("icon(") {
        if at > 0
            && source[..at]
                .chars()
                .next_back()
                .is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
        {
            continue;
        }
        let argument = source[at + marker.len()..].trim_start();
        if argument.starts_with('"') {
            if let Some(name) = quoted_literals(argument).next() {
                names.push(name);
            }
        }
    }
    names
}

#[test]
fn icon_guards_detect_conditional_glyphs_and_missing_assets() {
    for bad in [
        ".child(\"+\")",
        ".child(\n  \"−\"\n)",
        ".child(if selected { \"✓\" } else { \"\" })",
        ".child(div().child(if open { \"▾\" } else { \"▸\" }))",
    ] {
        assert!(has_text_icon_child(bad), "missed {bad}");
    }
    for valid in [
        ".child(icon(\"check\", ICON_MD, c::FG()))",
        ".child(format!(\"+{added}\"))",
        ".child(\"↑ ↓ navigate · Enter open\")",
        ".child(\"A → B\")",
        "matches!(branch, \"-\" | \"—\")",
    ] {
        assert!(!has_text_icon_child(valid), "rejected {valid}");
    }
    assert_eq!(
        literal_icon_references("icon(\n \"check\", ICON_MD, color).path(\"icons/expand.svg\")"),
        ["icons/expand.svg", "check"]
    );
    let missing = crate::icons::svg_for_path("icons/expand.svg").unwrap();
    assert_eq!(
        missing.split_once('>').unwrap().1,
        "</svg>",
        "regression fixture must be empty"
    );
    let present = crate::icons::svg_for_path("icons/zen.svg").unwrap();
    assert_ne!(present.split_once('>').unwrap().1, "</svg>");
}
#[test]
fn r8_dense_controls_share_geometry() {
    let components = source("src/views/components.rs");
    for name in [
        "header_control_frame",
        "settings_control",
        "dense_action_with_colors",
    ] {
        let body = components
            .split(&format!("fn {name}("))
            .nth(1)
            .unwrap()
            .split("\npub ")
            .next()
            .unwrap();
        assert!(
            body.contains("CONTROL_H"),
            "{name} must use shared control height"
        );
    }
    let dense = components
        .split("pub fn dense_action_with_colors(")
        .nth(1)
        .unwrap()
        .split("\npub ")
        .next()
        .unwrap();
    for required in [
        ".with_size(Size::XSmall)",
        ".px(rpx(SPACE_LG))",
        ".text_size(rpx(TEXT_SMALL))",
        ".rounded(rpx(RADIUS_CONTROL)",
    ] {
        assert!(dense.contains(required), "dense recipe missing {required}");
    }
    let transcript = components
        .split("pub fn transcript_action(")
        .nth(1)
        .unwrap()
        .split("\npub ")
        .next()
        .unwrap();
    assert!(transcript.contains("dense_action_with_colors("));
    assert!(components
        .split("pub fn settings_control(")
        .nth(1)
        .unwrap()
        .split("\npub ")
        .next()
        .unwrap()
        .contains(".gap(rpx(SPACE_SM))"));
}
#[test]
fn r9_retired_private_modal_width_does_not_return() {
    reject("R9 modal widths", |line| {
        line.contains("const PALETTE_W") || line.contains("modal_panel(760")
    });
}
#[test]
fn r10_confirmations_use_quiet_veil() {
    assert!(source("src/theme.rs").contains("pub fn QUIET_VEIL()"));
    let shell = source("src/views/shell.rs");
    assert!(shell.contains("QUIET_VEIL()"));
    assert!(
        !shell
            .split("fn backend_choice(")
            .nth(1)
            .unwrap()
            .split("\n    fn ")
            .next()
            .unwrap()
            .contains(".bg(c::SCRIM())"),
        "backend source tray must not restore full scrim"
    );
    let vendor = source("vendor/gpui-component/ui/src/styled.rs");
    assert!(vendor.contains("RING_BORDER_WIDTH: Pixels = px(1.0)"));
    assert!(vendor.contains("!window.last_input_was_keyboard()"));
}
#[test]
fn r11_neutral_selection_avoids_accent_wash() {
    reject("R11 neutral selection", |line| {
        line.contains(".bg(c::SEL_TINT_SOFT())")
    });
}
#[test]
fn r12_inputs_preserve_accessible_names_and_reset_vendor_chrome() {
    for (file, text) in sources() {
        for (at, _) in text.match_indices("Input::new(") {
            let chain = constructor_chain(&text[at..]);
            let missing = missing_input_contract(chain);
            assert!(
                missing.is_empty(),
                "{file}:{} Input missing {missing:?}",
                text[..at].lines().count()
            );
        }
    }
}
#[test]
fn r13_retired_field_underlines_stay_removed() {
    reject("R13 field underline", |line| {
        line.contains("fn field_underline") || line.contains("field_underline(")
    });
}
#[test]
fn r14_placeholder_copy_uses_examples() {
    reject("R14 meaningful examples", |line| {
        [
            ".placeholder(\"Workspace\")",
            ".placeholder(\"Project name\")",
            ".placeholder(\"Worktree name\")",
        ]
        .iter()
        .any(|p| line.contains(p))
    });
}
#[test]
fn r15_form_buttons_have_shared_height() {
    let components = source("src/views/components.rs");
    let body = components
        .split("pub fn form_action(")
        .nth(1)
        .unwrap()
        .split("pub fn transcript_action(")
        .next()
        .unwrap();
    for required in [
        ".h(rpx(FORM_BUTTON_H))",
        ".with_size(Size::Medium)",
        ".px(rpx(FIELD_INSET_X))",
        ".text_size(rpx(FORM_VALUE_SIZE))",
    ] {
        assert!(body.contains(required), "form recipe missing {required}");
    }
}
#[test]
fn r16_retired_footer_radius_stays_removed() {
    reject("R16 retired radius", |line| line.contains("RADIUS_FOOTER"));
}
#[test]
fn r17_radii_do_not_borrow_spacing_tokens() {
    reject("R17 radius roles", invalid_radius);
}
fn invalid_radius(line: &str) -> bool {
    line.match_indices(".rounded").any(|(at, _)| {
        let call = &line[at..];
        let Some(open) = call.find('(') else {
            return false;
        };
        let argument = call[open + 1..].split(')').next().unwrap_or("");
        argument.contains("rpx(SPACE_") || argument.contains("rpx(CONTROL_H")
    })
}

#[test]
fn r18_scrolling_surfaces_keep_overflow() {
    for path in [
        "src/views/worktree_launcher.rs",
        "src/views/settings_panel.rs",
        "src/views/workspace_manager.rs",
    ] {
        assert!(
            source(path).contains("overflow_y_scroll"),
            "{path} needs scrolling for long content"
        );
    }
}
#[test]
fn r19_header_tooltip_shape_is_shared() {
    assert!(source("src/views/components.rs").contains("Tooltip::element("));
    for (file, text) in sources() {
        if !file.ends_with("components.rs") {
            assert!(
                !text.contains("Tooltip::element("),
                "{file} must use shared tooltip"
            );
        }
    }
}
#[test]
fn r20_views_do_not_construct_palette_colors() {
    reject("R20 palette centralization", |line| {
        line.contains("gpui::rgb(") || line.contains("gpui::rgba(") || line.contains("gpui::hsla(")
    });
}
#[test]
fn every_allow_list_entry_carries_a_justification() {
    // No exceptions: current architecture permits inherited Geist, named module
    // geometry and shared focus rings directly, without per-file exclusions.
    let source = source("src/views/conformance.rs");
    for line in source
        .lines()
        .filter(|line| line.trim_start().starts_with("const "))
    {
        let name = line.split_whitespace().nth(1).unwrap();
        assert!(
            !name.to_ascii_lowercase().contains("allow"),
            "no unchecked allow-list: {name}"
        );
    }
}

#[test]
fn source_guards_detect_regressions_without_borrowing_neighbor_contracts() {
    for bad in ["rpx(14.)", "rpx ( -14. )", "rpx( .5)"] {
        assert!(literal_after(bad, "rpx("));
    }
    assert!(!literal_after("rpx(FIELD_H)", "rpx("));
    assert!(numeric_icon_size("icon(choose(1, 2), 14., c::FG())"));
    assert!(!numeric_icon_size("icon(choose(1, 2), ICON_MD, c::FG())"));
    assert!(invalid_radius(".rounded_t(rpx(SPACE_20))"));
    assert!(!invalid_radius(
        ".p(rpx(SPACE_LG)).rounded(rpx(RADIUS_PANEL))"
    ));
    let valid = "Input::new(&input).aria_label(\"Name\").appearance(false).bordered(false).focus_bordered(false).p_0()";
    assert!(missing_input_contract(valid).is_empty());
    let adjacent = "Input::new(&first)), child(Input::new(&second).aria_label(\"Name\").appearance(false).bordered(false).focus_bordered(false).p_0())";
    assert!(!missing_input_contract(constructor_chain(adjacent)).is_empty());
}
