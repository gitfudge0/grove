//! These tests use Grove's dependency features, including the release grammar bundle.
use gpui_component::highlighter::{HighlightTheme, LanguageRegistry, SyntaxHighlighter};

#[test]
fn fenced_code_languages_produce_themed_keyword_and_string_colors() {
    for (language, code, keyword, string) in [
        ("rust", "fn main() { println!(\"Hello\"); }", "fn", "Hello"),
        (
            "ruby",
            "def greet(name)\n  puts \"Hello, #{name}!\"\nend",
            "def",
            "Hello",
        ),
        (
            "python",
            "def greet():\n    return \"Hello\"",
            "def",
            "Hello",
        ),
        (
            "js",
            "function greet() { return \"Hello\"; }",
            "function",
            "Hello",
        ),
    ] {
        let config = LanguageRegistry::singleton().language(language).unwrap();
        assert!(config.has_grammar(), "{language} must ship its grammar");
        let mut highlighter = SyntaxHighlighter::new(language);
        assert!(highlighter.update(None, &code.into(), None));
        assert_eq!(highlighter.language(), &config.name);
        let mut keyword_colors = Vec::new();
        for theme in [
            HighlightTheme::default_dark(),
            HighlightTheme::default_light(),
        ] {
            let styles = highlighter.styles(&(0..code.len()), &theme);
            let color_at = |token: &str| {
                let offset = code.find(token).unwrap();
                styles
                    .iter()
                    .find_map(|(range, style)| {
                        range.contains(&offset).then_some(style.color).flatten()
                    })
                    .unwrap_or_else(|| panic!("{language}: {token} must have syntax color"))
            };
            let keyword_color = color_at(keyword);
            assert_ne!(
                keyword_color,
                color_at(string),
                "{language} token roles must differ"
            );
            keyword_colors.push(keyword_color);
        }
        assert_ne!(
            keyword_colors[0], keyword_colors[1],
            "{language} must follow appearance"
        );
    }
}

#[test]
fn language_aliases_and_unknown_fences_keep_their_expected_behavior() {
    for (alias, name) in [
        ("rs", "rust"),
        ("rb", "ruby"),
        ("js", "javascript"),
        ("py", "python"),
        ("sh", "bash"),
    ] {
        assert_eq!(
            LanguageRegistry::singleton()
                .language(alias)
                .unwrap()
                .name
                .as_ref(),
            name
        );
    }
    for language in ["text", "not-a-language"] {
        let code = "ordinary text";
        let mut highlighter = SyntaxHighlighter::new(language);
        assert!(highlighter.update(None, &code.into(), None));
        assert!(highlighter
            .styles(&(0..code.len()), &HighlightTheme::default_dark())
            .iter()
            .all(|(_, style)| style.color.is_none()));
    }
}
