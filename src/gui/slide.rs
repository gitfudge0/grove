//! Draw-only translation wrapper used for the grid tile-slide animation.
//!
//! Translating a widget's *layout* (e.g. via `move_to`) would perturb the
//! grid's actual geometry mid-animation, and iced would just clobber it on
//! the next layout pass anyway. Instead this wrapper leaves layout alone and
//! nudges only the rendered pixels (and, to keep hit-testing honest, the
//! cursor position handed to the child) — so PTY sizes/viewports settle the
//! instant a swap happens and only the drawing eases into place.

use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{layout, mouse, renderer, Clipboard, Layout, Shell, Widget};
use iced::{Element, Event, Length, Rectangle, Renderer, Size, Theme, Vector};

/// Wraps `content` so it draws `offset` away from its laid-out position.
pub fn slide<'a, Msg>(content: Element<'a, Msg>, offset: Vector) -> Element<'a, Msg>
where
    Msg: 'a,
{
    Slide { content, offset }.into()
}

struct Slide<'a, Msg> {
    content: Element<'a, Msg>,
    offset: Vector,
}

impl<Msg> Widget<Msg, Theme, Renderer> for Slide<'_, Msg> {
    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content.as_widget_mut().layout(tree, renderer, limits)
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        renderer::Renderer::with_translation(renderer, self.offset, |renderer| {
            self.content
                .as_widget()
                .draw(tree, renderer, theme, style, layout, cursor, viewport);
        });
    }

    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        self.content.as_widget().tag()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        self.content.as_widget().state()
    }

    fn children(&self) -> Vec<Tree> {
        self.content.as_widget().children()
    }

    fn diff(&self, tree: &mut Tree) {
        self.content.as_widget().diff(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(tree, layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Msg>,
        viewport: &Rectangle,
    ) {
        self.content.as_widget_mut().update(
            tree,
            event,
            layout,
            cursor - self.offset,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            tree,
            layout,
            cursor - self.offset,
            viewport,
            renderer,
        )
    }
}

impl<'a, Msg> From<Slide<'a, Msg>> for Element<'a, Msg>
where
    Msg: 'a,
{
    fn from(widget: Slide<'a, Msg>) -> Self {
        Element::new(widget)
    }
}
