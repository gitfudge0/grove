# Integrated HTML reports

An agent can return a report that reads as part of the conversation rather than
a framed document. Ordinary HTML previews are unchanged.

## Contract

A complete HTML document (`<!doctype html>` or `<html>` through `</html>`, at
most 2 MiB) opts in with the exact comment `<!-- grove:integrated-report -->`.
It reaches the transcript the same way as any HTML preview: a closed fenced
`html` block, or a Markdown link to a local `.html` file.

Grove injects one `<style id="grove-report-theme">` element and replaces its
contents whenever appearance or UI zoom changes, without reloading the document.
Colours come from `src/theme.rs`; sizes come from `src/views/tokens.rs` at the
current zoom.

| Variable | Source |
|---|---|
| `--grove-bg`, `--grove-fg`, `--grove-muted` | `BG`, `FG`, `FG_DIM` |
| `--grove-surface`, `--grove-border` | `BG_HOVER`, `BORDER` |
| `--grove-info`, `--grove-success`, `--grove-warning` | `BLUE`, `GREEN`, `AMBER` |
| `--grove-body`, `--grove-title`, `--grove-display` | `TEXT_BODY`, `TEXT_DISPLAY`, `TEXT_DISPLAY_LG` |
| `--grove-gap`, `--grove-section`, `--grove-radius` | `SPACE_LG`, `SPACE_20`, `RADIUS_GROUP` |
| `--grove-font` | Bundled DM Sans UI family |

The injected rules also set the root background, colour, font and size, zero the
body margin, and hide root overflow while the report is inline.

Claude and Codex learn this contract from Grove's injected session guidance
(`crates/grove-core/src/reports.rs`). `examples/integrated-report.html` is a
sample.

## Behaviour

- Inline, the report has no frame or title; a compact action row keeps Preview,
  Source, Reload (fenced reports), Expand and browser actions.
- The native view is sized to the measured body height, capped at eight times
  the standard preview height. Expand shows longer reports in full.
- Wheel input over an inline report scrolls the conversation. Expanded, the
  document scrolls itself.
- Document state (tabs, filters, inputs) survives Expand and Source toggles.
  Reload and restart reset it. Opening in a browser uses the original file,
  which does not receive Grove's theme.

## Platform boundaries

Height is pulled by the host: every 100 ms the view requests a read-only
evaluation of the body height and receives a single finite number.

| Platform | Wheel forwarding |
|---|---|
| macOS (WKWebView) | The web view passes wheel events to the GPUI host view. No document messages. |
| Linux (WebKitGTK, X11) | GTK scroll events are captured in-process and dispatched into the GPUI window during the GTK pump. No document messages. |
| Windows (WebView2) | WebView2 is out of process and does not return wheel input, so a host script posts wheel deltas. Web messages are enabled for this alone: the host accepts only finite, clamped wheel deltas from a visible inline report and ignores every other message. |

The existing sandbox otherwise applies unchanged: CSP, blocked network and file
requests, no host objects, no workers, no popups.

## Authoring limits

Avoid viewport-height layouts, inner scroll areas, fixed or sticky positioning,
body margins, and `!important` overrides of the injected rules. Embedded DOM text uses bundled DM Sans, with JetBrains Mono for semantic or authored monospace text. Font choices are normalized, including dynamically inserted text. Platform glyph fallback remains available for unsupported characters. Text drawn inside authored canvas/bitmap assets is intrinsic content.

## Verification

```sh
cargo test --bin grove views::inline_html::tests
clang -fobjc-arc -fblocks scripts/verify-integrated-report-webkit.m \
  -framework AppKit -framework WebKit -framework QuartzCore -o /tmp/grove-report-webkit-test
/tmp/grove-report-webkit-test examples/integrated-report.html
```

The macOS harness checks measured growth and shrinkage independent of viewport
height and the absence of script-message handlers. CI runs the equivalent
WebKitGTK and WebView2 harnesses.
