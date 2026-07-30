# gpui verified patterns (mid-2026)

Each pattern notes its source. EXACT = quoted from the source at research time; RECONSTRUCTED = assembled from documented signatures — compile-check against your pinned rev. All revs drift: recheck signatures after any rev bump.

## Bootstrap + window — EXACT
Source: `crates/gpui/examples/hello_world.rs` (zed-industries/zed)
```rust
use gpui_platform::application;
fn main() {
    application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(500.), px(500.0)), cx);
        cx.open_window(
            WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() },
            |_, cx| cx.new(|_| HelloWorld { text: "World".into() }),
        ).unwrap();
        cx.activate(true);
    });
}
```

## Entity + notify — EXACT
Source: `crates/gpui/src/_ownership_and_data_flow.rs`
```rust
struct Counter { count: usize }
let counter: Entity<Counter> = cx.new(|_cx| Counter { count: 0 });
counter.update(cx, |counter, cx| {
    counter.count += 1;
    cx.notify(); // triggers re-render of observers
});
```

## Typed events (subscribe/emit) — EXACT
Source: `crates/gpui/src/_ownership_and_data_flow.rs`
```rust
struct CounterChangeEvent { increment: usize }
impl EventEmitter<CounterChangeEvent> for Counter {}

let second = cx.new(|cx: &mut Context<Counter>| {
    cx.subscribe(&first_counter, |second, _first, event, _cx| {
        second.count += event.increment * 2;
    }).detach(); // Subscription cancels on drop — detach or store it
    Counter { count: 0 }
});
first_counter.update(cx, |first, cx| {
    first.count += 2;
    cx.emit(CounterChangeEvent { increment: 2 });
    cx.notify();
});
```

## Actions + keymap + context — EXACT
Source: `crates/gpui/docs/key_dispatch.md`
```rust
mod menu { actions!(gpui, [MoveUp, MoveDown]); }
div()
    .key_context("menu")
    .on_action(|this: &mut Menu, _: &MoveUp, cx| { /* handler */ })
```
```json
{ "context": "menu", "bindings": { "up": "menu::MoveUp", "down": "menu::MoveDown" } }
```
Focus-path dispatch; context predicates support `"Workspace > MyModal"` style. Same specificity → later-registered wins (how user keymaps override built-ins).

## Async bridge from std::thread — RECONSTRUCTED
Sources: docs.rs/gpui; zed.dev/blog/zed-decoded-async-rust ("they don't use tokio … GPUI implements its own execution model")
```rust
cx.spawn(async move |cx| {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = do_blocking_work();
        let _ = tx.send(result);
    });
    let result = cx.background_spawn(async move { rx.recv().unwrap() }).await;
    // back on foreground executor: safe to update entities via cx here
}).detach();
```

## Custom Element (terminal-grid style) — EXACT signatures
Source: `crates/terminal_view/src/terminal_element.rs`
```rust
impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = LayoutState; // ShapedLines, hitboxes, colors
    fn request_layout(&mut self, id: Option<&GlobalElementId>, window: &mut Window, cx: &mut App)
        -> (LayoutId, Self::RequestLayoutState) { /* Taffy layout */ }
    fn prepaint(&mut self, id: Option<&GlobalElementId>, inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>, _: &mut Self::RequestLayoutState, window: &mut Window, cx: &mut App)
        -> Self::PrepaintState { /* shape lines, build hitboxes */ }
    fn paint(&mut self, id: Option<&GlobalElementId>, inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>, _: &mut Self::RequestLayoutState, layout: &mut Self::PrepaintState,
        window: &mut Window, cx: &mut App) {
        window.paint_quad(fill(bounds, layout.background_color));
        // per row: window.text_system().shape_line(text, size, &runs, None).paint(origin, line_height, window, cx)
    }
}
```
Batch a whole row into one shaped line with per-cell `TextRun`s; clip with `with_content_mask`; cull to visible rows; hit-testing is your own bounds math.

## uniform_list — RECONSTRUCTED
Source: docs.rs/gpui; duanebester/gpui-list
```rust
uniform_list("sidebar-list", items.len(), move |range, _window, _cx| {
    items[range].iter().map(|item| item.render()).collect::<Vec<_>>()
})
.track_scroll(self.scroll_handle.clone()) // handle stored in the view
```

## with_animation — EXACT
Source: `crates/gpui/examples/animation.rs`
```rust
svg.with_animation(
    "image_circle",
    Animation::new(Duration::from_secs(2)).repeat().with_easing(bounce(ease_in_out)),
    |svg, delta| svg.with_transformation(Transformation::rotate(percentage(delta))),
)
```

## AssetSource + svg — RECONSTRUCTED
Source: gpui-component docs (Icons & Assets). Tint method (`.text_color()` vs `.color()`) unverified — check the `Svg` struct on your rev.
```rust
#[derive(rust_embed::RustEmbed)]
#[folder = "assets"]
struct Assets;
impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> { Ok(Self::get(path).map(|f| f.data)) }
    fn list(&self, path: &str) -> Result<Vec<SharedString>> { Ok(Self::iter().map(Into::into).collect()) }
}
svg().path("icons/check.svg").text_color(rgb(0x00ff00))
```

## Dependency pinning — convention
```toml
gpui = { git = "https://github.com/zed-industries/zed", rev = "<exact-sha>" }
gpui-component = { git = "https://github.com/longbridge/gpui-component", rev = "<exact-sha>" }
# gpui-component pins its own zed rev — read its Cargo.toml and match it.
# gpui_platform backend features on Linux: ["font-kit", "wayland", "x11"]
```

## Learning path (ranked)
1. `crates/gpui/README.md` — philosophy, pointers
2. `crates/gpui/src/_ownership_and_data_flow.rs` — canonical Entity/observe/subscribe doc-code
3. `crates/gpui/examples/` — copy-paste starting points (compile-check first; they drift)
4. `crates/gpui/docs/key_dispatch.md` — the authoritative keymap/focus doc
5. Zed production crates (`terminal_view`, `editor`, `ui`) — advanced real-world patterns
6. zed.dev/blog/gpui-ownership — prose companion to #2
7. longbridge/gpui-component + its docs site — widgets; its `gpui-element` best-practices doc is a good Element guide
8. DeepWiki zed-industries/zed — index only; always cross-check against source
