//! Elements, built the way GPUI's are. A style method writes the field of
//! GPUI's `StyleRefinement` its GPUI namesake writes, in the JSON form GPUI
//! reads; [`Element::style`] reaches every other field.

use std::rc::Rc;

use serde_json::{Map, Value};

use crate::{AnyView, FocusHandle, Listener, Name};

pub(crate) enum Kind {
    Div,
    Text,
    Component(Name),
    View(Rc<dyn AnyView>),
}

pub struct Element {
    pub(crate) kind: Kind,
    pub(crate) text: String,
    pub(crate) style: Map<String, Value>,
    pub(crate) props: Value,
    pub(crate) listeners: Vec<(&'static str, u32)>,
    pub(crate) focus: Option<FocusHandle>,
    pub(crate) children: Vec<Element>,
}

fn element(kind: Kind) -> Element {
    Element {
        kind,
        text: String::new(),
        style: Map::new(),
        props: Value::Null,
        listeners: Vec::new(),
        focus: None,
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

/// A length, as GPUI writes one: `"8px"`, `"1.5rem"`, `"50%"`, `"auto"`.
pub struct Length(String);

impl From<f32> for Length {
    fn from(pixels: f32) -> Self {
        px(pixels)
    }
}

impl From<&str> for Length {
    fn from(length: &str) -> Self {
        Length(length.to_owned())
    }
}

pub fn px(pixels: f32) -> Length {
    Length(format!("{pixels}px"))
}

pub fn rems(rems: f32) -> Length {
    Length(format!("{rems}rem"))
}

/// A fraction of the parent's size.
pub fn relative(fraction: f32) -> Length {
    Length(format!("{}%", fraction * 100.))
}

const EDGES: [&str; 4] = ["top", "right", "bottom", "left"];
const CORNERS: [&str; 4] = ["top_left", "top_right", "bottom_right", "bottom_left"];

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

    pub fn track_focus(mut self, focus: &FocusHandle) -> Self {
        self.focus = Some(focus.clone());
        self
    }

    /// Sets the field of `StyleRefinement` at `path` (`["padding", "top"]`).
    pub fn style(mut self, path: &[&str], value: impl Into<Value>) -> Self {
        let (field, groups) = path.split_last().expect("a field");
        let mut fields = &mut self.style;
        for group in groups {
            fields = fields
                .entry(*group)
                .or_insert_with(|| Value::Object(Map::new()))
                .as_object_mut()
                .expect("a group of fields");
        }
        fields.insert((*field).to_owned(), value.into());
        self
    }

    fn each(mut self, group: &str, fields: &[&str], length: Length) -> Self {
        for field in fields {
            self = self.style(&[group, field], length.0.clone());
        }
        self
    }

    pub fn flex(self) -> Self {
        self.style(&["display"], "Flex")
    }

    pub fn flex_col(self) -> Self {
        self.flex().style(&["flex_direction"], "Column")
    }

    pub fn flex_row(self) -> Self {
        self.flex().style(&["flex_direction"], "Row")
    }

    pub fn flex_1(self) -> Self {
        self.style(&["flex_grow"], 1.)
            .style(&["flex_shrink"], 1.)
            .style(&["flex_basis"], "0%")
    }

    pub fn items_center(self) -> Self {
        self.style(&["align_items"], "Center")
    }

    pub fn justify_between(self) -> Self {
        self.style(&["justify_content"], "SpaceBetween")
    }

    pub fn w(self, length: impl Into<Length>) -> Self {
        self.style(&["size", "width"], length.into().0)
    }

    pub fn h(self, length: impl Into<Length>) -> Self {
        self.style(&["size", "height"], length.into().0)
    }

    pub fn size_full(self) -> Self {
        self.w(relative(1.)).h(relative(1.))
    }

    pub fn p(self, length: impl Into<Length>) -> Self {
        self.each("padding", &EDGES, length.into())
    }

    pub fn px(self, length: impl Into<Length>) -> Self {
        self.each("padding", &["left", "right"], length.into())
    }

    pub fn py(self, length: impl Into<Length>) -> Self {
        self.each("padding", &["top", "bottom"], length.into())
    }

    pub fn gap(self, length: impl Into<Length>) -> Self {
        self.each("gap", &["width", "height"], length.into())
    }

    pub fn border(self, length: impl Into<Length>) -> Self {
        self.each("border_widths", &EDGES, length.into())
    }

    pub fn rounded(self, length: impl Into<Length>) -> Self {
        self.each("corner_radii", &CORNERS, length.into())
    }

    /// Colors are `"#rrggbb"` or `"#rrggbbaa"`.
    pub fn bg(self, color: &str) -> Self {
        self.style(&["background"], color)
    }

    pub fn border_color(self, color: &str) -> Self {
        self.style(&["border_color"], color)
    }

    pub fn text_color(self, color: &str) -> Self {
        self.style(&["text", "color"], color)
    }

    pub fn text_size(self, length: impl Into<Length>) -> Self {
        self.style(&["text", "font_size"], length.into().0)
    }

    pub fn font_weight(self, weight: f32) -> Self {
        self.style(&["text", "font_weight"], weight)
    }

    pub fn overflow_hidden(self) -> Self {
        self.style(&["overflow", "x"], "Hidden")
            .style(&["overflow", "y"], "Hidden")
    }

    pub fn overflow_y_scroll(self) -> Self {
        self.style(&["overflow", "y"], "Scroll")
    }

    pub fn cursor_pointer(self) -> Self {
        self.style(&["mouse_cursor"], "PointingHand")
    }
}
