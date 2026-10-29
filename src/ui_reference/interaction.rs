//! Real component event observations using Metal + TestPlatform, not OS/provider QA.
use super::{
    contract::{hash, Snapshot},
    document::{Document, Kind, Node, State},
    native::{Measurements, Rect},
};
use gpui::{
    AnyWindowHandle, HeadlessAppContext, KeyDownEvent, KeyUpEvent, Keystroke, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PlatformInput,
};
use std::{cell::RefCell, collections::BTreeMap, path::Path, rc::Rc};

fn draw(cx: &mut HeadlessAppContext, window: AnyWindowHandle) {
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.simulate_next_frame(cx);
        let _ = window.draw(cx);
    })
    .unwrap();
}
fn key(cx: &mut HeadlessAppContext, window: AnyWindowHandle, key: &str) {
    let k = Keystroke::parse(key).unwrap();
    cx.update_window(window, |_, window, cx| {
        window.dispatch_event(
            PlatformInput::KeyDown(KeyDownEvent {
                keystroke: k.clone(),
                is_held: false,
                prefer_character_input: false,
            }),
            cx,
        );
        window.dispatch_event(PlatformInput::KeyUp(KeyUpEvent { keystroke: k }), cx);
    })
    .unwrap();
    draw(cx, window);
}
fn focus(cx: &mut HeadlessAppContext, window: AnyWindowHandle) -> Option<String> {
    cx.update_window(window, |_, window, cx| {
        window.focused(cx).map(|f| format!("{f:?}"))
    })
    .unwrap()
}
fn bounds(observed: &Measurements, id: &str) -> Rect {
    observed.borrow()[id].bounds.clone()
}
fn move_to(cx: &mut HeadlessAppContext, window: AnyWindowHandle, x: f32, y: f32) {
    cx.update_window(window, |_, window, cx| {
        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: gpui::point(gpui::px(x), gpui::px(y)),
                pressed_button: None,
                modifiers: Default::default(),
            }),
            cx,
        );
    })
    .unwrap();
    draw(cx, window);
}
fn click(cx: &mut HeadlessAppContext, window: AnyWindowHandle, b: &Rect) {
    let p = gpui::point(gpui::px(b.x + b.width / 2.), gpui::px(b.y + b.height / 2.));
    move_to(cx, window, b.x + b.width / 2., b.y + b.height / 2.);
    cx.update_window(window, |_, window, cx| {
        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                position: p,
                button: MouseButton::Left,
                click_count: 1,
                first_mouse: false,
                modifiers: Default::default(),
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseUp(MouseUpEvent {
                position: p,
                button: MouseButton::Left,
                click_count: 1,
                modifiers: Default::default(),
            }),
            cx,
        );
    })
    .unwrap();
    draw(cx, window);
}
fn slots(observed: &Measurements, id: &str) -> serde_json::Value {
    let map = observed.borrow();
    let parent = &map[id].bounds;
    let label = &map[&format!("{id}-label")].bounds;
    let icon=map.get(&format!("{id}-icon")).map(|n|serde_json::json!({"x":n.bounds.x-parent.x,"y":n.bounds.y-parent.y,"width":n.bounds.width,"height":n.bounds.height}));
    serde_json::json!({"icon":icon,"label":{"x":label.x-parent.x,"y":label.y-parent.y}})
}
fn image(
    cx: &mut HeadlessAppContext,
    window: AnyWindowHandle,
    name: &str,
    out: &Path,
    evidence: &mut BTreeMap<String, String>,
    sample: &Rect,
    scale: f32,
) -> [u8; 4] {
    let im = cx.capture_screenshot(window).unwrap();
    let x = ((sample.x + 3.) * scale).round() as u32;
    let y = ((sample.y + sample.height / 2.) * scale).round() as u32;
    let value = im.get_pixel(x, y).0;
    im.save(out.join(name)).unwrap();
    evidence.insert(name.into(), hash(fs_err::read(out.join(name)).unwrap()));
    value
}
fn after_activation(log: &Rc<RefCell<Vec<String>>>, before: usize, expected: &str) {
    let entries = log.borrow();
    assert_eq!(entries.len(), before + 1, "native event must activate once");
    assert_eq!(
        entries.last().unwrap(),
        expected,
        "generated callback identity must match authored node.action"
    );
}

pub fn observe(
    cx: &mut HeadlessAppContext,
    window: AnyWindowHandle,
    doc: &Document,
    s: &Snapshot,
    observed: &Measurements,
    log: &Rc<RefCell<Vec<String>>>,
    out: &Path,
) -> serde_json::Value {
    let mut controls = Vec::<Node>::new();
    doc.root.walk(&mut |n| {
        if n.kind == Kind::DenseAction {
            controls.push(n.clone());
        }
    });
    let context = super::native::context(doc, s);
    if controls.is_empty() {
        return serde_json::json!({"schema_version":1,"status":"passed","scope":"inert","reference_hash":hash(serde_json::to_vec(doc).unwrap()),"contract_hash":s.contract_hash,"native_build_hash":context["native_build_hash"],"cases":[],"action_routes":[]});
    }
    let active = controls
        .iter()
        .filter(|n| n.state == State::Rest)
        .collect::<Vec<_>>();
    let primary = active
        .first()
        .expect("control reference requires a rest state");
    let disabled = controls
        .iter()
        .find(|n| n.state == State::Disabled)
        .expect("control reference requires a disabled state");
    let loading = controls
        .iter()
        .find(|n| n.state == State::Loading)
        .expect("control reference requires a loading state");
    let target = bounds(observed, &primary.id);
    // Compare only like treatments. An iconless rest action and an icon-bearing
    // loading action are separate authored controls, not one changing icon slot.
    let mut slot_groups = BTreeMap::<bool, Vec<serde_json::Value>>::new();
    for node in &controls {
        slot_groups.entry(node.icon.is_some()).or_default().push(
            serde_json::json!({"node_id":node.id,"state":node.state,"slots":slots(observed, &node.id)}),
        );
    }
    let original = "interaction-original-recipe.png";
    let mut evidence = BTreeMap::from([(
        original.into(),
        hash(fs_err::read(out.join(original)).unwrap()),
    )]);
    let mut cases = Vec::new();
    move_to(cx, window, 0., 0.);
    let rest = image(
        cx,
        window,
        "interaction-rest.png",
        out,
        &mut evidence,
        &target,
        doc.fixture.scale,
    );
    move_to(
        cx,
        window,
        target.x + target.width / 2.,
        target.y + target.height / 2.,
    );
    let hover = image(
        cx,
        window,
        "interaction-hover.png",
        out,
        &mut evidence,
        &target,
        doc.fixture.scale,
    );
    let hover_slots = slots(observed, &primary.id);
    move_to(cx, window, 0., 0.);
    let released = image(
        cx,
        window,
        "interaction-release.png",
        out,
        &mut evidence,
        &target,
        doc.fixture.scale,
    );
    assert_eq!(rest, released, "release must return native fill to rest");
    assert_ne!(
        rest, hover,
        "hover must exercise a different actual native fill"
    );
    cases.push(serde_json::json!({"name":"hover_release","status":"passed","observed":{"rest":rest,"hover":hover,"released":released}}));
    let before = log.borrow().len();
    click(cx, window, &target);
    after_activation(log, before, primary.action.as_ref().unwrap());
    let primary_route = serde_json::json!({"node_id":primary.id,"expected":primary.action,"observed":log.borrow().last(),"before":before,"after":log.borrow().len()});
    cases.push(serde_json::json!({"name":"pointer_activation","status":"passed","observed":{"before":before,"after":log.borrow().len(),"expected_action":primary.action,"actual_action":log.borrow().last()}}));
    move_to(cx, window, 0., 0.);
    key(cx, window, "tab");
    let first = focus(cx, window).expect("Tab must enter native button focus");
    key(cx, window, "tab");
    let second = focus(cx, window).expect("Tab must move native focus");
    assert_ne!(first, second);
    key(cx, window, "shift-tab");
    let returned = focus(cx, window).unwrap();
    assert_eq!(first, returned);
    cases.push(serde_json::json!({"name":"tab_traversal","status":"passed","observed":{"before":first,"after":second,"returned":returned}}));
    let focused_slots = slots(observed, &primary.id);
    image(
        cx,
        window,
        "interaction-focus.png",
        out,
        &mut evidence,
        &target,
        doc.fixture.scale,
    );
    for (name, stroke) in [("keyboard_enter", "enter"), ("keyboard_space", "space")] {
        let before = log.borrow().len();
        key(cx, window, stroke);
        after_activation(log, before, primary.action.as_ref().unwrap());
        cases.push(serde_json::json!({"name":name,"status":"passed","observed":{"before":before,"after":log.borrow().len(),"expected_action":primary.action,"actual_action":log.borrow().last()}}));
    }
    let focused_before = focus(cx, window).unwrap();
    for (name, node) in [
        ("disabled_rejection", disabled),
        ("loading_rejection", loading),
    ] {
        let before = log.borrow().len();
        click(cx, window, &bounds(observed, &node.id));
        assert_eq!(
            log.borrow().len(),
            before,
            "disabled/loading native button must reject pointer activation"
        );
        cases.push(serde_json::json!({"name":name,"status":"passed","observed":{"before":before,"after":log.borrow().len()}}));
    }
    let focused_after = focus(cx, window).unwrap();
    assert_eq!(
        focused_before, focused_after,
        "inactive pointer actions must preserve source focus"
    );
    cases.push(serde_json::json!({"name":"focus_restoration","status":"passed","observed":{"before":focused_before,"after":focused_after}}));
    let primary_group = slot_groups.get_mut(&primary.icon.is_some()).unwrap();
    primary_group
        .push(serde_json::json!({"node_id":primary.id,"state":"hover","slots":hover_slots}));
    primary_group
        .push(serde_json::json!({"node_id":primary.id,"state":"focused","slots":focused_slots}));
    let mut groups = Vec::new();
    for (has_icon, samples) in slot_groups {
        let baseline = &samples[0]["slots"];
        for sample in &samples {
            assert_eq!(
                baseline, &sample["slots"],
                "native icon/label slot offsets must not drift within an icon treatment"
            );
        }
        groups.push(serde_json::json!({"has_icon":has_icon,"samples":samples}));
    }
    cases.push(
        serde_json::json!({"name":"stable_slots","status":"passed","observed":{"groups":groups}}),
    );
    let mut routes = vec![primary_route];
    for node in active.iter().skip(1) {
        let before = log.borrow().len();
        click(cx, window, &bounds(observed, &node.id));
        after_activation(log, before, node.action.as_ref().unwrap());
        routes.push(serde_json::json!({"node_id":node.id,"expected":node.action,"observed":log.borrow().last(),"before":before,"after":log.borrow().len()}));
    }
    for case in &mut cases {
        case["evidence"] = serde_json::to_value(&evidence).unwrap();
    }
    serde_json::json!({"schema_version":1,"status":"passed","scope":"controls","reference_hash":hash(serde_json::to_vec(doc).unwrap()),"contract_hash":s.contract_hash,"native_build_hash":context["native_build_hash"],"cases":cases,"action_routes":routes})
}
