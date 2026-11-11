//! Elements, built the way GPUI's are. A style method writes the field of
//! GPUI's `StyleRefinement` its GPUI namesake writes, in the JSON form GPUI
//! reads; [`Element::style`] reaches every other field.

use std::rc::Rc;

use serde_json::{Map, Value};

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
    pub(crate) style: Map<String, Value>,
    pub(crate) hover: Map<String, Value>,
    pub(crate) group: Option<Name>,
    pub(crate) group_hover: Option<(Name, Map<String, Value>)>,
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
        style: Map::new(),
        hover: Map::new(),
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

    /// The left button pressed over the element. The payload has the
    /// pointer's `x` and `y` in the window.
    pub fn on_mouse_down(self, listener: Listener) -> Self {
        self.on("mouse_down", listener)
    }

    /// The pointer arriving over the element (`true`) or leaving it.
    pub fn on_hover(self, listener: Listener) -> Self {
        self.on("hover", listener)
    }

    /// What the pointer over the element adds to its style:
    /// `.hover(|style| style.bg("#eeeeee"))`.
    pub fn hover(mut self, style: impl FnOnce(Element) -> Element) -> Self {
        self.hover = style(div()).style;
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
        style: impl FnOnce(Element) -> Element,
    ) -> Self {
        self.group_hover = Some((name.into(), style(div()).style));
        self
    }

    /// The element's style as it crosses to the host.
    #[doc(hidden)]
    pub fn style_json(&self) -> String {
        serde_json::to_string(&self.style).unwrap_or_default()
    }

    pub fn track_focus(mut self, focus: &FocusHandle) -> Self {
        self.focus = Some(focus.clone());
        self
    }

    pub fn track_scroll(mut self, scroll: &ScrollHandle) -> Self {
        self.scroll = Some(scroll.clone());
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

    pub fn flex_none(self) -> Self {
        self.style(&["flex_grow"], 0.).style(&["flex_shrink"], 0.)
    }

    pub fn flex_wrap(self) -> Self {
        self.style(&["flex_wrap"], "Wrap")
    }

    pub fn items_start(self) -> Self {
        self.style(&["align_items"], "FlexStart")
    }

    pub fn items_center(self) -> Self {
        self.style(&["align_items"], "Center")
    }

    pub fn items_end(self) -> Self {
        self.style(&["align_items"], "FlexEnd")
    }

    pub fn justify_start(self) -> Self {
        self.style(&["justify_content"], "FlexStart")
    }

    pub fn justify_center(self) -> Self {
        self.style(&["justify_content"], "Center")
    }

    pub fn justify_end(self) -> Self {
        self.style(&["justify_content"], "FlexEnd")
    }

    pub fn justify_between(self) -> Self {
        self.style(&["justify_content"], "SpaceBetween")
    }

    /// Placed by `top`/`right`/`bottom`/`left` from where it would be.
    pub fn relative(self) -> Self {
        self.style(&["position"], "Relative")
    }

    /// Placed by `top`/`right`/`bottom`/`left` in its positioned parent,
    /// out of the flow.
    pub fn absolute(self) -> Self {
        self.style(&["position"], "Absolute")
    }

    pub fn top(self, length: impl Into<Length>) -> Self {
        self.each("inset", &["top"], length.into())
    }

    pub fn right(self, length: impl Into<Length>) -> Self {
        self.each("inset", &["right"], length.into())
    }

    pub fn bottom(self, length: impl Into<Length>) -> Self {
        self.each("inset", &["bottom"], length.into())
    }

    pub fn left(self, length: impl Into<Length>) -> Self {
        self.each("inset", &["left"], length.into())
    }

    pub fn m(self, length: impl Into<Length>) -> Self {
        self.each("margin", &EDGES, length.into())
    }

    pub fn mx(self, length: impl Into<Length>) -> Self {
        self.each("margin", &["left", "right"], length.into())
    }

    pub fn my(self, length: impl Into<Length>) -> Self {
        self.each("margin", &["top", "bottom"], length.into())
    }

    pub fn mt(self, length: impl Into<Length>) -> Self {
        self.each("margin", &["top"], length.into())
    }

    pub fn mb(self, length: impl Into<Length>) -> Self {
        self.each("margin", &["bottom"], length.into())
    }

    pub fn ml(self, length: impl Into<Length>) -> Self {
        self.each("margin", &["left"], length.into())
    }

    pub fn mr(self, length: impl Into<Length>) -> Self {
        self.each("margin", &["right"], length.into())
    }

    pub fn max_w(self, length: impl Into<Length>) -> Self {
        self.style(&["max_size", "width"], length.into().0)
    }

    pub fn max_h(self, length: impl Into<Length>) -> Self {
        self.style(&["max_size", "height"], length.into().0)
    }

    pub fn pl(self, length: impl Into<Length>) -> Self {
        self.each("padding", &["left"], length.into())
    }

    pub fn pr(self, length: impl Into<Length>) -> Self {
        self.each("padding", &["right"], length.into())
    }

    pub fn border_t(self, length: impl Into<Length>) -> Self {
        self.each("border_widths", &["top"], length.into())
    }

    pub fn border_b(self, length: impl Into<Length>) -> Self {
        self.each("border_widths", &["bottom"], length.into())
    }

    pub fn border_l(self, length: impl Into<Length>) -> Self {
        self.each("border_widths", &["left"], length.into())
    }

    pub fn border_r(self, length: impl Into<Length>) -> Self {
        self.each("border_widths", &["right"], length.into())
    }

    /// From 0 (invisible) to 1.
    pub fn opacity(self, opacity: f32) -> Self {
        self.style(&["opacity"], opacity)
    }

    pub fn whitespace_nowrap(self) -> Self {
        self.style(&["text", "white_space"], "Nowrap")
    }

    /// Ends text that does not fit with an ellipsis.
    pub fn text_ellipsis(self) -> Self {
        self.style(
            &["text", "text_overflow"],
            serde_json::json!({ "Truncate": "…" }),
        )
    }

    /// One line, ended with an ellipsis where it does not fit.
    pub fn truncate(self) -> Self {
        self.overflow_hidden().whitespace_nowrap().text_ellipsis()
    }

    /// At most `lines` lines of text.
    pub fn line_clamp(self, lines: u32) -> Self {
        self.style(&["text", "line_clamp"], lines).overflow_hidden()
    }

    pub fn w(self, length: impl Into<Length>) -> Self {
        self.style(&["size", "width"], length.into().0)
    }

    pub fn h(self, length: impl Into<Length>) -> Self {
        self.style(&["size", "height"], length.into().0)
    }

    pub fn w_full(self) -> Self {
        self.w(relative(1.))
    }

    pub fn min_w(self, length: impl Into<Length>) -> Self {
        self.style(&["min_size", "width"], length.into().0)
    }

    pub fn min_h(self, length: impl Into<Length>) -> Self {
        self.style(&["min_size", "height"], length.into().0)
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

    pub fn pt(self, length: impl Into<Length>) -> Self {
        self.each("padding", &["top"], length.into())
    }

    pub fn pb(self, length: impl Into<Length>) -> Self {
        self.each("padding", &["bottom"], length.into())
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

    pub fn line_height(self, length: impl Into<Length>) -> Self {
        self.style(&["text", "line_height"], length.into().0)
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
