# Live provider size recovery check

Production actor harness: `oversized_artifact_smoke.rs` (copy to `crates/grove-core/examples/oversized_artifact_smoke.rs` to run).

Build: `cargo build -p grove-core --example oversized_artifact_smoke`.

For each provider separately, create fresh empty temporary cwd and config directories, then run:

`GROVE_CONFIG_DIR=<disposable-config> target/debug/examples/oversized_artifact_smoke codex <empty-disposable-cwd>`

Repeat with `claude`. The harness uses installed authenticated CLIs via Grove production `codex::connect`; CLI permission defaults remain unchanged. It never submits approval responses. No existing Grove sessions/config are used. Timeout is 90 seconds per actor wait.

Before connecting, the harness writes two harmless fixtures into its temporary cwd: valid schema-v1 plan JSON padded with whitespace to 1,048,726 bytes, and HTML padded with an inert comment above 2 MiB. The provider prompt requests only literal Markdown links without tools/file access. The completed actual provider response is processed by the production actor. Assertions require structured plan size metadata (exact bytes, 1,048,576-byte limit, canonical path) and HTML link in actual assistant items. The harness then shrinks the plan itself and sends production RetryPlanImport for the same message/reference. Assertions require error removal and successful import.

Results: Codex verified; Claude verified. Both processes exited 0. See adjacent raw provider logs.

Scope limit: this checks live provider-to-production-plan-import behavior plus actual HTML response link presence. It does not exercise the HTML renderer, automatic file watcher/UI refresh, native recovery-card appearance, browser opening, or Reveal file interaction. Those require separate native checks.
