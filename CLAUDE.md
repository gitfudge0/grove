# CLAUDE.md

- Run `./install.sh` after finishing any code changes.
- `DESIGN.md` is normative for all UI work. Every numeric value comes from
  `src/views/tokens.rs` and every colour from `src/theme.rs` — never a bare
  literal. Use the shared components in `src/views/components.rs` rather than
  forking a local shape. `cargo test` runs a conformance suite
  (`src/views/conformance.rs`, rules R1–R20) that enforces this; the fix for a
  firing rule is to change the code, never to add an allow-list entry — and an
  allow-list entry that is genuinely sanctioned must name the DESIGN.md clause
  that sanctions it in more than 40 characters of real justification, which
  `every_allow_list_entry_carries_a_justification` enforces. A token with
  exactly one consumer is a module constant, not a scale entry. If a
  design seems to need a token or tier that does not exist, that is a signal
  the design is wrong, per DESIGN.md §13.
