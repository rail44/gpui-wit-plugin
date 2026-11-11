//! Elements, built the way GPUI's are. An element is styled through GPUI's
//! own [`Styled`], on GPUI's own [`StyleRefinement`], which crosses to the
//! host as GPUI serializes it.

use std::rc::Rc;

use gpui::{Overflow, StyleRefinement, Styled};
use serde_json::Value;

use crate::{AnyView, FocusHandle, Listener, Name, ScrollHandle};

pub(crate) enum Kind {
    Div,
    Text,
    Component(Name),
    View(Rc<dyn AnyView>),
}

pub struct Element {
    pub(crate) kind: Kind,
    pub(crate) text: String,
    pub(crate) style: StyleRefinement,
    // Boxed: a style is large, most elements have neither, and an element
    // is moved by every method that builds it.
    pub(crate) hover: Option<Box<StyleRefinement>>,
    pub(crate) group: Option<Name>,
    pub(crate) group_hover: Option<(Name, Box<StyleRefinement>)>,
    pub(crate) props: Value,
    pub(crate) listeners: Vec<(&'static str, u32)>,
    pub(crate) focus: Option<FocusHandle>,
    pub(crate) scroll: Option<ScrollHandle>,
    pub(crate) children: Vec<Element>,
}

fn element(kind: Kind) -> Element {
    Element {
        kind,
        text: String::new(),
        style: StyleRefinement::default(),
        hover: None,
        group: None,
        group_hover: None,
        props: Value::Null,
        listeners: Vec::new(),
        focus: None,
        scroll: None,
        children: Vec::new(),
    }
}

pub fn div() -> Element {
    element(Kind::Div)
}

pub fn text(text: impl Into<String>) -> Element {
    Element {
        text: text.into(),
        ..element(Kind::Text)
    }
}

/// Something the embedding application registered as `name`, drawn from
/// `props`.
pub fn component(name: impl Into<Name>, props: Value) -> Element {
    Element {
        props,
        ..element(Kind::Component(name.into()))
    }
}

impl From<&str> for Element {
    fn from(value: &str) -> Self {
        text(value)
    }
}

impl From<String> for Element {
    fn from(value: String) -> Self {
        text(value)
    }
}

impl Styled for Element {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl Element {
    pub(crate) fn view(view: Rc<dyn AnyView>) -> Self {
        element(Kind::View(view))
    }

    pub fn child(mut self, child: impl Into<Element>) -> Self {
        self.children.push(child.into());
        self
    }

    pub fn children<E: Into<Element>>(mut self, children: impl IntoIterator<Item = E>) -> Self {
        self.children.extend(children.into_iter().map(Into::into));
        self
    }

    /// Reports `event` to `listener`. The host decides which events an
    /// element or a component has.
    pub fn on(mut self, event: &'static str, listener: Listener) -> Self {
        self.listeners.push((event, listener.0));
        self
    }

    pub fn on_click(self, listener: Listener) -> Self {
        self.on("click", listener)
    }

    pub fn on_key_down(self, listener: Listener) -> Self {
        self.on("key_down", listener)
    }

    /// The left button pressed over the element. The payload has the
    /// pointer's `x` and `y` in the window.
    pub fn on_mouse_down(self, listener: Listener) -> Self {
        self.on("mouse_down", listener)
    }

    /// The pointer arriving over the element (`true`) or leaving it.
    pub fn on_hover(self, listener: Listener) -> Self {
        self.on("hover", listener)
    }

    /// What the pointer over the element adds to its style, as GPUI's
    /// `hover`.
    pub fn hover(mut self, style: impl FnOnce(StyleRefinement) -> StyleRefinement) -> Self {
        self.hover = Some(Box::new(style(StyleRefinement::default())));
        self
    }

    /// Names the element as a group for `group_hover`.
    pub fn group(mut self, name: impl Into<Name>) -> Self {
        self.group = Some(name.into());
        self
    }

    /// What the pointer over the group `name` adds to the element's style.
    pub fn group_hover(
        mut self,
        name: impl Into<Name>,
        style: impl FnOnce(StyleRefinement) -> StyleRefinement,
    ) -> Self {
        self.group_hover = Some((name.into(), Box::new(style(StyleRefinement::default()))));
        self
    }

    /// Scrolls its content, as the methods of the same names on a GPUI
    /// element that has an id.
    pub fn overflow_scroll(self) -> Self {
        self.overflow_x_scroll().overflow_y_scroll()
    }

    pub fn overflow_x_scroll(mut self) -> Self {
        self.style.overflow.x = Some(Overflow::Scroll);
        self
    }

    pub fn overflow_y_scroll(mut self) -> Self {
        self.style.overflow.y = Some(Overflow::Scroll);
        self
    }

    pub fn track_focus(mut self, focus: &FocusHandle) -> Self {
        self.focus = Some(focus.clone());
        self
    }

    pub fn track_scroll(mut self, scroll: &ScrollHandle) -> Self {
        self.scroll = Some(scroll.clone());
        self
    }
}
