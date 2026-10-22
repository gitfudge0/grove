# Change categories

Grove groups changed files under Code, Tests, Docs, and Config. Unrecognized file types appear under Uncategorized. Classification uses paths and filenames, never an AI guess about the intent of a change. A file belongs to exactly one parent and optionally one subcategory. Parents include files without a subcategory; there is no Other row.

Populated parents appear in the checkout card, even when only one parent is present. The arrow expands populated subcategories. Clicking a parent or child opens its filtered diff. Expansion is local to the selected checkout. New file and mapping changes are picked up by the existing status refresh.

Built-in subcategories recognize Tests → Unit, Integration, End-to-end, Fixtures & snapshots; Config → Dependencies & lockfiles, Build & tooling, CI/CD, Deployment & infrastructure, Environment & app settings; Code → Scripts & automation, Assets. Generic test filenames remain under Tests without guessing unit versus integration. Unit and integration directory names require test context. Bare JSON/YAML/TOML and arbitrary dotfiles do not imply Config. Test and docs context takes priority over extensions.

## Optional repository mappings

Create `.grove/change-categories.json` in each checkout. No mapping is required. For example:

```json
{
  "rules": [
    { "path": "src/views/special.rs", "category": "code", "subcategory": "Special UI" },
    { "path": "src/views/", "category": "code", "subcategory": "UI" },
    { "path": "resources/settings.json", "category": "config" }
  ]
}
```

Rules run in array order, before built-in conventions; the first match wins. Paths are case-sensitive, relative to the checkout, and use `/`. A trailing slash matches that directory's descendants; otherwise the match is exact. `src/views/` does not match `src/views-old/`. Globs and absolute or parent-relative paths are unsupported. Put specific rules before broader directory rules. Valid categories are `code`, `tests`, `docs`, `config`, and `uncategorized`. Omit `subcategory` to keep files directly in the parent. Custom subcategory labels must be nonempty, contain no control characters, and be at most 64 bytes.

Files are limited to 64 KiB and 256 rules. A missing mapping uses built-ins. An unreadable or invalid mapping logs a warning and falls back to built-ins as a whole; it never removes changed files from the summary or diff. Overrides are read as data and execute no commands. Commit the mapping to share it with teammates and future worktrees.
