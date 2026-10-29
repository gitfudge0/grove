//! Design-system-bound reference authoring and deterministic native composition.
//!
//! Tooling is test-only: it cannot introduce a product CLI or execute document code.
pub mod compose;
#[cfg(test)]
mod contract;
#[cfg(test)]
mod document;
#[cfg(test)]
mod generate;
#[cfg(test)]
mod html;
#[cfg(all(grove_reference_capture, not(test)))]
mod production_guard;
pub(crate) mod runtime;
// Native capture helpers are selected by the tooling build cfg; ordinary unit
// tests compile the schema/measurement types without invoking the renderer.
#[cfg(all(test, grove_reference_capture))]
mod interaction;
#[cfg(test)]
#[allow(dead_code)]
mod native;
#[cfg(test)]
mod tests;
