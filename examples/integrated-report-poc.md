# Integrated HTML report POC

Exploratory native experiment: can an interactive HTML report share Grove's theme and conversation scroll while its tabs change document height?

All displayed records are synthetic. The native renderer and macOS WebKit boundary are real; no provider service is simulated or invoked by the demo.

## Run

From this worktree:

```sh
CARGO_TARGET_DIR=/Users/digvijaymahapatra/Sandbox/grove-html-report-poc/target cargo build
GROVE_CONFIG_DIR=/tmp/grove-html-report-poc-state GROVE_HTML_REPORT_POC=1 /Users/digvijaymahapatra/Sandbox/grove-html-report-poc/target/debug/grove
```

The debug-only flag opens the synthetic report in an isolated, scrollable conversation overlay. Use a separate process with the supplied config directory; it does not alter real sessions. Switch The story / Code & activity / Full ledger. Filter the ledger, including spaces and a query with no matches. Scroll over the HTML to reach the native text below it. Try Source, Preview, Reload, Expand, and Return to conversation. Resize the window and change appearance/zoom through a normal session when inspecting the linked sample.

In the installed app, have either Codex or Claude return a Markdown link to this worktree's `examples/integrated-report-poc.html`, then open the preview. This exercises the actual shared transcript path. No live provider has been exercised by the standalone harnesses.

## Contract

Opt in with the exact comment `<!-- grove:integrated-report -->` in a complete HTML document. Ordinary HTML keeps its framed viewer. On macOS the report has a matching canvas, a compact borderless action row, theme CSS variables, and content-driven height. Semantic colors resolve from `src/theme.rs`; CSS type and spacing resolve from `src/views/tokens.rs` at current UI zoom. The document can consume `--grove-bg`, `--grove-fg`, `--grove-muted`, `--grove-surface`, `--grove-border`, `--grove-info`, `--grove-success`, `--grove-warning`, `--grove-body`, `--grove-title`, `--grove-display`, `--grove-gap`, `--grove-section`, `--grove-radius`, and `--grove-font`.

Theme changes update CSS in place. Tab/filter/input state stays in the retained WebKit document across expansion and source toggles. Reload clears it. Restarting the demo resets the report. File previews retain the existing explicit new-version adoption flow. Browser-open actions remain explicit and use the original standalone file, which does not receive Grove's injected theme.

## Checks

```sh
CARGO_TARGET_DIR=/Users/digvijaymahapatra/Sandbox/grove-html-report-poc/target cargo test --bin grove views::inline_html::tests
python3 scripts/verify-inline-html-webkit.py --output /tmp/grove-html-report-poc-native
clang -fobjc-arc -fblocks scripts/verify-integrated-report-webkit.m -framework AppKit -framework WebKit -framework QuartzCore -o /tmp/grove-report-webkit-test
/tmp/grove-report-webkit-test examples/integrated-report-poc.html
```

The new native harness checks measured growth and shrinkage below viewport height, retained JavaScript state, and absence of script-message handlers. The existing native harness checks navigation/network/file restrictions, editing, focus, accessibility, and surface lifetime. These are macOS native checks, not live Codex or Claude provider checks.

## Limits and cleanup

macOS only for integrated behavior. Windows and Linux deliberately retain the framed viewer, including marked HTML. Those platforms were not exercised. Height is polled through a read-only native JavaScript evaluation every 100 ms, with only a finite scalar returned to Rust. The content height is capped at 2,880 logical pixels; use Expand for larger documents. Avoid viewport-height-dependent report layouts, inner scroll wells, fixed/sticky content, author `!important` overrides of host theme rules, and margins on the body. Geist may fall back to the system font when unavailable to WebKit. This is a POC, not a generalized authoring system or performance benchmark.

Quit only the POC process to stop it. Relaunch without `GROVE_HTML_REPORT_POC` for normal behavior. No new dependencies or persistent report state were added. The isolated config directory is disposable; remove it only after quitting the POC if a fresh startup is wanted. Removing the worktree requires preserving or intentionally discarding these uncommitted POC changes first.

## Observed validation and remaining limits

On this macOS host, native inspection verified tab growth/shrinkage, wheel scrolling to the surrounding native text, empty-filter resizing, Expand/Return, and retained tab/filter state through Source/Preview. Both standalone native harnesses passed, including sandbox/network restrictions and measured height changes of 420 → 180 → 1,200 pixels without adding document message handlers.

An isolated Rust rebuild was stopped because concurrent builds exhausted available resources; isolated Rust tests remain incomplete. Installation was attempted and stopped during resource contention. Review uses the isolated debug POC app; the installed application is not claimed to contain this POC. Live Codex and Claude provider paths remain untested.
