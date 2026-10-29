//! Refresh an offscreen selectable owner using its already allocated bounds.
//! No new layout node is needed: the transcript owns the exact cached extent.
use super::TextViewState;
use gpui::{App, Bounds, Entity, Hitbox, HitboxBehavior, Pixels, Window};

pub struct TextViewProjection {
    owner: Entity<TextViewState>,
    hitbox: Option<Hitbox>,
}
impl TextViewProjection {
    pub fn new(owner: Entity<TextViewState>) -> Self {
        Self {
            owner,
            hitbox: None,
        }
    }

    pub fn prepaint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        self.owner
            .update(cx, |state, _| state.update_bounds(bounds));
        self.hitbox = Some(window.insert_hitbox(bounds, HitboxBehavior::Normal));
    }

    pub fn paint(&mut self, window: &mut Window, cx: &mut App) {
        if let Some(hitbox) = &self.hitbox {
            crate::Root::register_selectable_text_view(&self.owner, hitbox, window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::TextView;
    use gpui::{
        AppContext as _, Context, IntoElement as _, ParentElement as _, Render, Styled as _,
        TestAppContext, VisualTestContext, div, px,
    };

    struct Probe {
        owner: Entity<TextViewState>,
        projected: bool,
        size: gpui::Size<Pixels>,
    }
    impl Render for Probe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
            let owner = self.owner.clone();
            let size = self.size;
            div().w(px(360.)).child(if self.projected {
                gpui::canvas(
                    move |bounds, window, cx| {
                        let mut projection = TextViewProjection::new(owner);
                        projection.prepaint(
                            Bounds {
                                origin: bounds.origin + gpui::point(px(0.), px(1000.)),
                                size,
                            },
                            window,
                            cx,
                        );
                        projection
                    },
                    |_, mut projection, window, cx| projection.paint(window, cx),
                )
                .w_full()
                .h(size.height)
                .into_any_element()
            } else {
                TextView::new(&owner).selectable(true).into_any_element()
            })
        }
    }
    fn draw(view: &Entity<Probe>, cx: &mut VisualTestContext) {
        cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    #[gpui::test]
    fn projected_owner_refreshes_hitbox_clears_inline_geometry_and_reifies(
        cx: &mut TestAppContext,
    ) {
        cx.update(crate::init);
        let mut retained = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| Probe {
                owner: cx.new(|cx| {
                    TextViewState::markdown(
                        "## Retained source\n\nComplete Unicode text: café 👩‍💻.",
                        cx,
                    )
                }),
                projected: false,
                size: gpui::Size::default(),
            });
            retained = Some(view.clone());
            crate::Root::new(view, window, cx).bordered(false)
        });
        let view = retained.unwrap();
        draw(&view, cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let root = window.root::<crate::Root>().flatten().unwrap();
                assert!(!root.read(cx).selectable_text_inlines[&view.owner.entity_id()].is_empty());
                view.size = view.owner.read(cx).bounds().size;
                assert!(view.size.height > px(0.));
                view.projected = true;
                cx.notify();
            })
        });
        draw(&view, cx);
        cx.update(|window, cx| {
            view.read_with(cx, |view, cx| {
                let root = window.root::<crate::Root>().flatten().unwrap();
                let root = root.read(cx);
                let bounds = view.owner.read(cx).bounds();
                assert!(bounds.top() >= px(1000.));
                assert_eq!(
                    root.selectable_text_views[&view.owner.entity_id()].1.bounds,
                    bounds
                );
                assert!(
                    !root
                        .selectable_text_inlines
                        .contains_key(&view.owner.entity_id()),
                    "projected owners must not retain ghost glyph hitboxes"
                );
                assert!(view.owner.read(cx).source().contains("café 👩‍💻"));
            })
        });
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.projected = false;
                cx.notify();
            })
        });
        draw(&view, cx);
        cx.update(|window, cx| {
            view.read_with(cx, |view, cx| {
                let root = window.root::<crate::Root>().flatten().unwrap();
                assert!(!root.read(cx).selectable_text_inlines[&view.owner.entity_id()].is_empty());
                assert!(view.owner.read(cx).bounds().top() < px(1000.));
            })
        });
    }
}
