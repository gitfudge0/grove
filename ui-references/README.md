# Design-system-bound component references

Use this workflow when an HTML mock will become Grove GPUI components. The
authoring source is a strict typed document, not arbitrary HTML/CSS. Existing
Rust owners supply the values and behavior: [tokens](../src/views/tokens.rs),
[theme roles](../src/theme.rs), [fonts](../src/fonts.rs),
[icons](../src/icons.rs) and [complete component recipes](../src/views/components.rs).
[DESIGN.md](../DESIGN.md) remains the semantic contract.

The document generates reviewable Rust composition that calls those recipes.
Canonical HTML uses measured native layout and actual shaped text clusters,
including the visible ellipsis string. It does not approximate native geometry
with a second browser flex layout or text-measurement rule. Fonts and catalog
assets are embedded from the native export. Application data, callbacks,
session identity and focus routing remain in typed bindings owned by the view.

## Supported scope

The initial vocabulary is row, column, surface, text, icon, dense action and a
checkout-style row with fixed icon/action slots. Validation enforces allowed
property combinations and explicit state/layout semantics. Single-line text
and supported truncation are bounded cases; wrapping, unsupported glyphs,
unknown properties/variants and unmeasured contexts are rejected. Application
Unicode fallback remains intact, but a reference cannot certify an unrecorded
fallback font.

Iconless loading is rejected before generation until its introduced native loading slot is separately certified. Iconless rest actions remain supported in a reference that also declares the required state family.

The initial native capture backend is macOS Metal with the bundled fonts,
supported rem sizes, scale 2, `en-US` and reduced motion. The validator is the
authority for accepted context values. A renderer or platform not exercised by
the capture backend is unsupported, not a successful zero-test run. Schema
validity alone never certifies a component, state or reference.

Extend a missing treatment in the existing native owner first. Update its
export, typed mapping, measured HTML adapter and failure/valid acceptance cases
together. Update DESIGN.md when an approved invariant changes. Do not add
arbitrary CSS, copied token values or a local control implementation to bypass
validation. Free-form HTML remains useful for exploration but cannot acquire
implementation-ready status without this conversion.

## Author and freeze a reference

Run the commands from the task's checkout. Use new, task-owned output
directories and keep the selected revision in task state.

```sh
node scripts/ui-reference.mjs init .fudge/ui-reference/bootstrap grove-dark
node scripts/ui-reference.mjs validate .fudge/ui-reference/bootstrap/document.json .fudge/ui-reference/validated
node scripts/ui-reference.mjs generate .fudge/ui-reference/bootstrap/document.json .fudge/ui-reference/generated
```

`init` emits `document.json`, `snapshot.json`, `generated.rs` and `manifest.json`.
It produces no final HTML. Edit the typed document within the validated
vocabulary and run validation again. Unknown or irrelevant properties fail
instead of being silently ignored. Action/binding names and node IDs are
validated, including reserved generated IDs. Interactive documents must include
explicit `rest`, `disabled` and `loading` dense-action source variants before
`author`; inert documents have no action nodes or action bindings. Static
`hover` and `focus` poses are rejected. Actual native event tests must exercise
those transient states instead.

The `author` and `capture` wrapper compiles the exact document's generated Rust
from its task-owned revision directory. It supplies that document's declared
typed bindings and action identities to the fixture; there is no fixed tracked
pilot slot to replace. The capture guard compares the compiled source with
deterministic generation. Do not substitute a handwritten lookalike or copy
generated code over another reference's source.

```sh
node scripts/ui-reference.mjs author .fudge/ui-reference/bootstrap/document.json .fudge/ui-reference/revision-1
node scripts/ui-reference.mjs render .fudge/ui-reference/bootstrap/document.json .fudge/ui-reference/revision-1
```

`author` requires a new revision directory. It measures the real native
composition and freezes `reference.html`, `measured-layout.json` and the
reference inputs. Before using native measurements, the semantic audit checks
the authored IDs, kinds, full source text, recipe roles and icon identities.
A missing or incorrect native node cannot silently become the reference.
`render` checks the frozen reference and returns its HTML location; it refuses
an unmeasured or nominal CSS preview.

Inspect the rendered reference, record the chosen revision and preserve its
lock. Frozen means immutable, not user-approved or implementation-authorized.
Existing task authorization and settled design direction carry forward; these
commands do not create another approval gate.

## Integrate the generated composition

Register the revision's generated Rust as a module in the owning Grove view
(for example, with a module `include!` pointing to its checked generated file).
Construct that module's `Bindings` with application-owned values and callbacks,
then call `render(&bindings, window, cx)` from the view's render path. Each
name in `actions` becomes an `Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>`
field; each declared value becomes a `SharedString` field. Use the exact field
names emitted for that document. The application retains data freshness,
session identity, persistence, focus ownership and action effects. Fixture
values are exposed only through the test-only `Bindings::fixture` constructor.

The generated full tree calls the production
[composition adapter](../src/ui_reference/compose.rs); capture decorates those
same elements under test configuration. Production `render` takes no
measurement recorder or test-platform argument. Compile the owning integration
and run its affected interaction checks; successful fixture capture alone does
not verify application wiring or live Codex/Claude behavior.

The supported mapping is deliberately bounded. The validator rejects property
combinations outside this table; token names resolve through native owners,
with `rpx` applied once.

| Document property | Native composition |
| --- | --- |
| `row` / `column` | `div().flex()` / `.flex_col()`, full width; explicit token `gap` and `padding` become `.gap(rpx(token))` and `.p(rpx(token))`. |
| `surface` | Column layout plus the current theme's `BG_RAIL` fill. |
| `width: fill` | Wrapper `.w_full().min_w_0()`; required on containers and checkout rows. |
| `width: auto` | Wrapper `.flex_shrink_0()`; required on icons and dense actions. Text supports either width policy. |
| Text role and color | `text_small`, `text_body` or `text_title` select native tokens; UI font, equal token line height and `.whitespace_nowrap()`. Allowlisted color roles select current theme APIs. |
| Icon | Catalog asset through `icons::icon`, native `ICON_SM` size and foreground role; checkout leading icons use the supporting role. |
| Dense action | Shared complete dense-action recipe; `rest`, `disabled`, `loading` map to native flags. Optional icon occupies the native recipe slot; loading uses the recipe's Working ring. |
| Checkout row | Shared frame, fixed leading slot, `.flex_1().min_w_0().truncate()` label and one trailing dense action; recipe owns padding, gap, text and height. |
| Text policy | `single_line` for ordinary text/containers/icons/actions; `truncate` only for the checkout recipe. Its native truncation owns clipping and visible ellipsis. |

Leaf `gap`/`padding` fields must carry the validator's prescribed recipe
identities; they do not add outer padding to controls. Arbitrary fixed widths,
heights, grow/shrink factors, alignment, min/max overrides, clipping rules and
wrapping have no authoring mapping. Unknown fields and unsupported combinations
fail rather than relying on browser defaults or an approximate GPUI substitute.

## Check candidates without changing the baseline

```sh
node scripts/ui-reference.mjs capture .fudge/ui-reference/bootstrap/document.json .fudge/ui-reference/revision-1
node scripts/ui-reference.mjs check .fudge/ui-reference/bootstrap/document.json .fudge/ui-reference/revision-1
node scripts/ui-reference.mjs ready .fudge/ui-reference/bootstrap/document.json .fudge/ui-reference/revision-1
```

`capture` writes candidate native evidence only. It never overwrites the
reference HTML, measured payload, manifest or lock. `check` is read-only: it
checks regeneration and hashes against the actual frozen measured mode.
`ready` obtains a fresh candidate, runs the pinned browser comparison and
validates the complete current evidence. A changed reference or design-system
dependency requires an explicit new revision; do not regenerate a baseline to
hide candidate drift.

Use an installed supported Playwright module and a pinned browser executable.
`GROVE_PLAYWRIGHT_MODULE` can select an installed module;
`GROVE_CHROME_EXECUTABLE` selects the browser. Record the actual engine/version
in evidence and recertify changed environments. The tool must report a missing
or failing browser, not silently choose another renderer or download one.
`CARGO_TARGET_DIR` may point to a task-coordinated build cache; avoid concurrent
Cargo invocations against it. No command in this workflow installs Grove.

## Evidence and acceptance

`reference-lock.json` binds the immutable document, native contract, generated
Rust, snapshot, manifest, HTML and measured payload with their capture context.
The lock pins six artifacts: `document.json`, `snapshot.json`, `generated.rs`,
`manifest.json`, `reference.html` and `measured-layout.json`. Candidate evidence
includes `native.json`/`native.png` and actual native interaction observations.
Browser verification retains `browser.json`/`browser.png`,
`geometry-diff.json`, `diff.png`, `overlay.png` and `certificate.json`.

Readiness checks source and artifact fingerprints, current native/browser
builds, OS/backend identity, actual fonts, theme, rem, scale, viewport, motion
and locale. It requires the authored semantic audit, exact measured geometry,
the supported raster criterion and actual native interaction cases. A
certificate with untested or unsupported required evidence is not certified;
status booleans alone are insufficient.

Raster checks do not use a global similarity threshold. Any supported
antialiasing allowance must be characterized at matching coverage edges with
equal font, paint, radius and path identities. Flat interior pixels must match;
the criterion must reject changed content, offsets, weights, glyphs, colors,
paths and radii. Preserve failing and restored-valid mutation evidence. When
the criterion cannot establish that result, retain the failure or untested
status rather than weakening it.

Native focus, pointer, keyboard, disabled/loading and release checks are
separate from screenshot similarity. The nine event cases run through real
Metal rendering and GPUI TestPlatform input routes; they do not establish OS
desktop interaction or a live provider boundary. Name the method actually exercised;
explicit focus traversal is not evidence that an app's Tab event routed
correctly. For shared provider UI, report **Codex** and **Claude** separately
as verified, unsupported or untested, with the fixture/live method or reason.
Fixtures do not establish live provider behavior. Report macOS, Linux and
Windows coverage independently when affected.

## Native matrix and mutation gates

With the pinned browser configured, run these from the task checkout with new
evidence directories:

```sh
node scripts/test-ui-reference-native.mjs .fudge/ui-reference/native-evidence
node scripts/test-ui-reference-native-mutations.mjs .fudge/ui-reference/native-evidence/registered-row .fudge/ui-reference/mutation-evidence
```

The matrix authors and checks three independent documents: Grove Dark;
Grove Light with a 20 px rem base in a narrow viewport with two distinct actions; and Nord
with nested surfaces; the Grove Light case also exercises an iconless rest action. Each goes through `check` and
`ready`. The mutation gate renders six actual native defects (inset, font,
radius, icon, incorrect text and flat fill), requires their rejection, restores
and certifies the valid candidate, and proves frozen artifact bytes unchanged.
It retains per-case capture/verification logs and available native/comparison
artifacts, plus `restored.log` and `result.json`. A failed capture without a
changed native screenshot is not mutation evidence.

Portable CI runs the Node verifier unit checks and typed fixture validation.
[Native reference acceptance](../.github/workflows/ui-reference-native.yml)
is opt-in via workflow dispatch on a self-hosted macOS runner labeled
`grove-metal-reference`. Its pinned browser comes from repository variable
`GROVE_REFERENCE_CHROME_EXECUTABLE`; failure artifacts are uploaded even when
a gate fails. An unavailable runner leaves native verification unfinished.
These commands and jobs define the required procedure; they do not assert the
current matrix has passed. Live Codex and Claude checks remain untested until
separately exercised and recorded.

This guide defines the acceptance workflow, not a claim that every supported
state, engine, theme or provider has passed. The current certificate and its
retained evidence determine each reference's measured coverage. Delivery,
commit, installation and release follow the task's explicit endpoint and
AGENTS.md; a no-install instruction remains effective throughout this workflow.
