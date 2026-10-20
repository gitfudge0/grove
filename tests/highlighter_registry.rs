//! Registry coverage and override behavior must survive lazy grammar loading.
use std::collections::HashSet;

use gpui_component::highlighter::{Language, LanguageConfig, LanguageRegistry};

#[test]
fn registry_preserves_all_languages_aliases_and_custom_overrides() {
    let registry = LanguageRegistry::singleton();
    let expected: HashSet<_> = Language::all().map(|language| language.name()).collect();
    let names: HashSet<_> = registry
        .languages()
        .into_iter()
        .map(|name| name.to_string())
        .collect();
    for name in expected {
        assert!(names.contains(name), "missing bundled language {name}");
        let first = registry.language(name).unwrap();
        let second = registry.language(name).unwrap();
        assert_eq!(first, second);
        if !first.highlights.is_empty() {
            assert_eq!(first.highlights.as_ptr(), second.highlights.as_ptr());
        }
    }
    for (alias, canonical) in [("jsonc", "json"), ("rs", "rust"), ("js", "javascript")] {
        assert_eq!(registry.language(alias), registry.language(canonical));
    }
    assert!(registry.language("grove-unknown-registry-test").is_none());

    let custom_name = "grove-custom-registry-test";
    let custom = LanguageConfig::plain("custom");
    registry.register(custom_name, &custom);
    assert_eq!(registry.language(custom_name), Some(custom.clone()));
    assert!(registry.languages().iter().any(|name| name == custom_name));

    // The singleton belongs to this integration-test process. Register aliases
    // after coverage checks so the test also proves exact-name precedence.
    registry.register("json", &custom);
    assert_eq!(registry.language("jsonc"), Some(custom));
    let alias = LanguageConfig::plain("alias");
    registry.register("jsonc", &alias);
    assert_eq!(registry.language("jsonc"), Some(alias));
}
