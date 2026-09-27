# Vendored `sum_tree`

This directory is a local copy of Zed's `crates/sum_tree` at commit
`1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba` from
<https://github.com/zed-industries/zed>. The crate's manifest declares the
Apache-2.0 license; the upstream license text and copyright notice are in
[`LICENSE-APACHE`](LICENSE-APACHE). Grove redirects the pinned Zed git source
to this path with Cargo's `[patch]` mechanism.

The source files were copied from that commit. Changes made here:

- Removed seven `#[instrument(skip_all)]` attributes and two `ztracing` imports.
  At this Zed revision, `ztracing::instrument` is a no-op unless the custom
  `ztracing` cfg is enabled. Grove does not enable that cfg.
- Removed the test-only `zlog::init_test` initializer and its `ctor` attribute.
  It only configured logging for upstream tests.
- Replaced workspace-inherited manifest fields and dependencies with their
  values from Zed's root `Cargo.toml`; removed `ztracing`, `zlog`, `ctor`, and
  the unused `tracing` dependency. The `proptest` dependency keeps Zed's exact
  git revision for the optional `test-support` API.

The crate's data structure algorithms and public API are unchanged. Update
this provenance and change list if the vendored source is modified again.
