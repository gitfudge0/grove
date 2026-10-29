//! The exact generated source is compiled with cfg(test)=false by each public
//! author/capture invocation before the same source enters the instrumented test.
mod generated {
    include!(env!("GROVE_REFERENCE_GENERATED"));
}
const _: fn(&generated::Bindings, &mut gpui::Window, &mut gpui::App) -> gpui::AnyElement =
    generated::render;

const _: &str = generated::LAYOUT_HASH;
