# Preparing and publishing releases

Release preparation and publication are separate. Keep native builds parallel:
Apple Silicon macOS, Intel macOS (cross-compiled on Apple Silicon), Linux amd64,
and Windows x86_64. Compilation settings and installer formats are unchanged.

## Prepare installers

**Release bundles** (`release.yml`) prepares candidates on source changes pushed
to `main`. A newer preparation run cancels an older one on the same ref. It can
also be dispatched manually from `main` with `source_tag` empty. No release or tag
is created, and no candidate is automatically published.

Set the final workspace version and release notes before preparation. Each job
retains one installer and `candidate.json`, recording its source commit, version,
platform, filename and SHA-256 checksum. The four `candidate-*` workflow artifacts
expire after 14 days. Record the successful workflow run ID. Documentation-only
pushes do not automatically prepare candidates; dispatch preparation manually if
the intended tag includes such a commit.

Compiled caches prefer the exact dependency inputs, then fall back across version
and dependency changes within the same platform, architecture, toolchain and Cargo
configuration. Cargo checks fingerprints and rebuilds invalid outputs; these are
build caches, never substitutes for candidate validation. Previous exact v2 keys
remain readable during migration. Normal CI gets the same fallback behavior but
keeps its own cache namespace and build profile.

**Prepare release tools** warms the pinned host packaging tools on `main` twice a
week, when tool setup changes, or on manual dispatch. The same composite action
restores tools in candidate builds. Cold installations are saved immediately,
before Grove compilation, so cancelling a build does not discard completed tool
installation. Both macOS jobs use the same host-tool cache. Cold concurrent jobs
can still compile the same tool if no cache exists yet; warming avoids that cost.

## Publish prepared installers

Publication remains an explicit action:

1. Choose a successful full **Release bundles** run on `main`. All four platforms
   must have completed. The cancelled 1.2.1 run is not eligible.
2. Create the version tag at that run's exact commit, push the tag, and create a
   draft GitHub release with reviewed release notes. The tag must match the
   workspace version, for example `v1.2.1` for `1.2.1`.
3. Dispatch **Publish prepared release** (`publish-release.yml`) from `main`,
   supplying `source_tag` and `candidate_run`.

The publication job validates that the preparation run succeeded in this
repository on `main` and matches the tagged commit. It downloads all four
artifacts and checks their identities, filenames, completeness and checksums
before any upload. It then attaches the verified installers to the existing
draft and publishes it as latest. It never runs Cargo or rebuilds installers.
The draft's release title and notes are preserved.

A failed upload leaves the release draft for retry. A missing, expired, cancelled,
mixed-version or modified candidate is rejected; prepare a complete candidate
again. Publication and tagged recovery share a per-tag lock; publication rejects an already published
release. Do not edit the draft concurrently during promotion. Publishing a release
through GitHub's UI no longer starts a fallback compilation; use this workflow
for the completeness checks.

## Recover a draft bundle

For a draft whose tag already includes this tooling, **Release bundles** accepts
`source_tag` and optionally `platform=macos-x86_64` to rebuild a missing Intel Mac
installer. It checks the tag and workspace version and requires an existing draft.
Recovery uploads only installer files, keeps the release draft, and does not
cancel independent candidate preparation. Prefer a complete candidate and the
publication workflow for the normal release path.

## Evidence and remaining checks

The cancelled baseline run spent 5.7–6.9 minutes compiling `cargo-bundle`; all four
compiled-artifact caches missed. These changes target that setup and repeated
compilation, and move compilation before publication. A warm hosted run is still
needed to measure actual savings, exercise each OS's tool-cache restoration, and
verify artifact download and promotion against GitHub. Local checks cannot prove
those boundaries. Paid runners, self-hosted machines, `sccache`, and compiler
optimization changes are not part of this workflow.
