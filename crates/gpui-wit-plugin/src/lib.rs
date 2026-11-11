//! Draws, with GPUI, views that WASI preview 2 components describe.
//!
//! A plugin is a component exporting the `plugin` world of `wit/plugin.wit`.
//! It numbers the nodes of a tree itself and returns, from each call, the
//! operations that change it; [`PluginView`] keeps that tree and turns it
//! into GPUI elements when it renders. What the plugin sends is GPUI's own
//! vocabulary wherever GPUI has one: a node's style is a
//! [`StyleRefinement`] as GPUI serializes it, and anything beyond `div` and text is a
//! component the embedding application registers by name
//! ([`Runtime::register`]).
//!
//! The application decides what else a plugin reaches: the WASI context it
//! hands [`Runtime::instantiate`] (preopened directories, environment) and the
//! interfaces it adds to [`Runtime::linker`].
//!
//! Plugins run on the main thread, inside the call that drew or delivered
//! an event to them. A function the application added to the linker sees
//! only [`State::data`]; what it has to do with the application itself it
//! leaves there, and the application picks it up when the view emits
//! [`Returned`].

use std::{any::Any, collections::HashMap, path::Path, rc::Rc};

use anyhow::Result;
use gpui::{
    AnyElement, App, ClickEvent, Context, ElementId, Empty, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, Overflow,
    ParentElement, Render, ScrollHandle, SharedString, StatefulInteractiveElement, StyleRefinement,
    Styled, WeakEntity, Window, div, point, px,
};
use serde_json::{Value, json};
use wasmtime::{
    Engine, Store,
    component::{Component, Linker, ResourceTable},
};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

pub use wasmtime;
pub use wasmtime_wasi;

mod bindings {
    wasmtime::component::bindgen!({ path: "../../wit", world: "plugin" });
}

use bindings::{
    Plugin,
    gpui_wit::plugin::tree::{Kind, Mutation},
};

/// What a plugin's store holds besides its memory.
pub struct State {
    wasi: WasiCtx,
    table: ResourceTable,
    /// The embedding application's own, for the functions it added to
    /// [`Runtime::linker`].
    pub data: Box<dyn Any + Send>,
}

impl WasiView for State {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

/// What a registered component draws from.
pub struct Props<'a> {
    /// Stable for as long as the plugin keeps the node: the key for element
    /// state (`Window::use_keyed_state`).
    pub id: ElementId,
    pub props: &'a Value,
    pub style: &'a StyleRefinement,
    pub children: Vec<AnyElement>,
    pub emit: Emit,
}

/// Reports an event on a component's node to the plugin.
#[derive(Clone)]
pub struct Emit {
    view: WeakEntity<PluginView>,
    node: u64,
}

impl Emit {
    pub fn emit(&self, name: &str, payload: Value, cx: &mut App) {
        let _ = (self.view).update(cx, |view, cx| view.dispatch(self.node, name, &payload, cx));
    }
}

type ComponentFn = Box<dyn Fn(Props, &mut Window, &mut App) -> AnyElement>;

/// What every plugin of an application shares: the engine, the interfaces
/// plugins may import, and the components they may place.
pub struct Runtime {
    engine: Engine,
    linker: Linker<State>,
    components: HashMap<String, ComponentFn>,
}

impl Runtime {
    pub fn new() -> Result<Self> {
        let engine = Engine::default();
        let mut linker = Linker::new(&engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
        Ok(Self {
            engine,
            linker,
            components: HashMap::new(),
        })
    }

    /// Where the application adds the interfaces of its own that plugins
    /// import.
    pub fn linker(&mut self) -> &mut Linker<State> {
        &mut self.linker
    }

    /// Lets plugins place `component` as `name`.
    pub fn register(
        &mut self,
        name: impl Into<String>,
        component: impl Fn(Props, &mut Window, &mut App) -> AnyElement + 'static,
    ) {
        self.components.insert(name.into(), Box::new(component));
    }

    pub fn load(&self, path: impl AsRef<Path>) -> Result<Component> {
        Component::from_file(&self.engine, path)
    }

    /// Instantiates `component` without running it: `wasi` is what it sees
    /// of the machine, `data` what the application's own functions find in
    /// [`State::data`].
    pub fn instantiate(
        &self,
        component: &Component,
        wasi: WasiCtx,
        data: Box<dyn Any + Send>,
    ) -> Result<Instance> {
        let state = State {
            wasi,
            table: ResourceTable::new(),
            data,
        };
        let mut store = Store::new(&self.engine, state);
        let plugin = Plugin::instantiate(&mut store, component, &self.linker)?;
        Ok(Instance { store, plugin })
    }
}

/// A plugin that is loaded and has not run yet.
pub struct Instance {
    store: Store<State>,
    plugin: Plugin,
}

struct Node {
    kind: Kind,
    style: StyleRefinement,
    hover: Option<StyleRefinement>,
    group: Option<SharedString>,
    group_hover: Option<(SharedString, StyleRefinement)>,
    text: SharedString,
    props: Value,
    children: Vec<u64>,
    /// The node this one was last made a child of.
    parent: Option<u64>,
    events: Vec<String>,
    focus: Option<FocusHandle>,
    scroll: Option<ScrollHandle>,
}

/// Emitted each time a call into the plugin has returned: whatever the
/// plugin's calls to the application's functions left in [`State::data`] is
/// there to pick up. The first call, made by [`PluginView::new`], returns
/// before anything can subscribe: look once after making the view.
pub struct Returned;

impl EventEmitter<Returned> for PluginView {}

/// One plugin instance and the tree it has described so far.
pub struct PluginView {
    runtime: Rc<Runtime>,
    store: Store<State>,
    plugin: Plugin,
    nodes: HashMap<u64, Node>,
    root: Option<u64>,
    /// The view's own focus, which the root node tracks: focusing the view
    /// sends keys to the plugin's root element.
    focus: FocusHandle,
    /// Where the plugin asked focus to go, moved there by the next draw.
    pending_focus: Option<FocusHandle>,
    /// Whether the plugin asked that the event it just handled stop here.
    stop: bool,
    /// Why the plugin stopped, drawn in its place.
    error: Option<SharedString>,
}

impl Focusable for PluginView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl PluginView {
    /// Runs the plugin's `init` and keeps what it described. A plugin that
    /// fails, here or later, is drawn as the error it failed with.
    pub fn new(runtime: &Rc<Runtime>, instance: Instance, cx: &mut Context<Self>) -> Self {
        let Instance { mut store, plugin } = instance;
        let init = plugin.call_init(&mut store);
        let mut view = Self::unstarted(runtime, Instance { store, plugin }, cx);
        match init {
            Ok(batch) => view.apply(batch, cx),
            Err(error) => view.error = Some(format!("{error:#}").into()),
        }
        view
    }

    fn unstarted(runtime: &Rc<Runtime>, instance: Instance, cx: &mut Context<Self>) -> Self {
        Self {
            runtime: runtime.clone(),
            store: instance.store,
            plugin: instance.plugin,
            nodes: HashMap::new(),
            root: None,
            focus: cx.focus_handle(),
            pending_focus: None,
            stop: false,
            error: None,
        }
    }

    /// What the plugin has described, for a test or a log to read: each
    /// text and each component (as `<name props>`) with its node's number,
    /// in the order they are drawn. An error the plugin stopped with is the
    /// only line, numbered 0.
    pub fn outline(&self) -> Vec<(u64, String)> {
        fn walk(view: &PluginView, id: u64, lines: &mut Vec<(u64, String)>) {
            let Some(node) = view.nodes.get(&id) else {
                return;
            };
            match &node.kind {
                Kind::Text => lines.push((id, node.text.to_string())),
                Kind::Component(name) => lines.push((id, format!("<{name} {}>", node.props))),
                Kind::Element => {}
            }
            for child in &node.children {
                walk(view, *child, lines);
            }
        }
        if let Some(error) = &self.error {
            return vec![(0, error.to_string())];
        }
        let mut lines = Vec::new();
        if let Some(root) = self.root {
            walk(self, root, &mut lines);
        }
        lines
    }

    /// The plugin's store state, where [`State::data`] is.
    pub fn state(&mut self) -> &mut State {
        self.store.data_mut()
    }

    /// Hands the plugin an event and applies what it answers. `target` 0 is
    /// the view as a whole: how an application tells a plugin something.
    pub fn dispatch(&mut self, target: u64, name: &str, payload: &Value, cx: &mut Context<Self>) {
        if self.error.is_some() {
            return;
        }
        // An event the plugin does not listen for on that node is not worth
        // a call into it.
        let listened = |node: &Node| node.events.iter().any(|event| event == name);
        if target != 0 && !self.nodes.get(&target).is_some_and(listened) {
            return;
        }
        // What an earlier call asked about its own event is not this one's.
        self.stop = false;
        let payload = payload.to_string();
        match self
            .plugin
            .call_event(&mut self.store, target, name, &payload)
        {
            Ok(batch) => self.apply(batch, cx),
            Err(error) => self.error = Some(format!("{error:#}").into()),
        }
        cx.emit(Returned);
        cx.notify();
    }

    /// Hands the plugin an event of one of its elements, and keeps the
    /// event from the elements further out if the plugin asked.
    fn report(&mut self, id: u64, name: &str, payload: Value, cx: &mut Context<Self>) {
        self.dispatch(id, name, &payload, cx);
        if std::mem::take(&mut self.stop) {
            cx.stop_propagation();
        }
    }

    fn apply(&mut self, batch: Vec<Mutation>, cx: &mut Context<Self>) {
        for mutation in batch {
            if let Err(error) = self.apply_one(mutation, cx) {
                self.error = Some(format!("{error:#}").into());
                return;
            }
        }
    }

    fn apply_one(&mut self, mutation: Mutation, cx: &mut Context<Self>) -> Result<()> {
        let (id, change) = match mutation {
            Mutation::Create((id, kind)) => {
                let node = Node {
                    kind,
                    style: StyleRefinement::default(),
                    hover: None,
                    group: None,
                    group_hover: None,
                    text: SharedString::default(),
                    props: Value::Null,
                    children: Vec::new(),
                    parent: None,
                    events: Vec::new(),
                    focus: None,
                    scroll: None,
                };
                self.nodes.insert(id, node);
                return Ok(());
            }
            Mutation::Replace((old, new)) => {
                if self.root == Some(old) {
                    self.root = Some(new);
                }
                let parent = self.nodes.get(&old).and_then(|node| node.parent);
                if let Some(parent) = parent.and_then(|parent| self.nodes.get_mut(&parent)) {
                    for child in &mut parent.children {
                        if *child == old {
                            *child = new;
                        }
                    }
                }
                if let Some(node) = self.nodes.get_mut(&new) {
                    node.parent = parent;
                }
                return Ok(());
            }
            Mutation::Dispose(id) => {
                self.nodes.remove(&id);
                return Ok(());
            }
            Mutation::SetRoot(id) => {
                self.root = Some(id);
                return Ok(());
            }
            Mutation::StopPropagation => {
                self.stop = true;
                return Ok(());
            }
            Mutation::SetStyle((id, _))
            | Mutation::SetHoverStyle((id, _))
            | Mutation::SetGroup((id, _))
            | Mutation::SetGroupHoverStyle((id, ..))
            | Mutation::SetText((id, _))
            | Mutation::SetProps((id, _))
            | Mutation::SetChildren((id, _))
            | Mutation::Listen((id, _))
            | Mutation::ScrollBy((id, ..))
            | Mutation::ScrollTo((id, _))
            | Mutation::Focus(id) => (id, mutation),
        };
        if let Mutation::SetChildren((_, children)) = &change {
            for child in children {
                if let Some(child) = self.nodes.get_mut(child) {
                    child.parent = Some(id);
                }
            }
        }
        // The root node's focus is the view's own.
        let view_focus = (self.root == Some(id)).then(|| self.focus.clone());
        let node = self.nodes.get_mut(&id).ok_or_else(|| {
            anyhow::anyhow!("the plugin changed node {id}, which it has not made")
        })?;
        match change {
            Mutation::SetStyle((_, style)) => {
                node.style = parse_style(&style)?;
                // An element that scrolls is measured from its first draw,
                // so that a scroll the plugin asks for has bounds to meet.
                let overflow = &node.style.overflow;
                if [overflow.x, overflow.y].contains(&Some(Overflow::Scroll)) {
                    node.scroll.get_or_insert_default();
                }
            }
            Mutation::SetHoverStyle((_, style)) => node.hover = Some(parse_style(&style)?),
            Mutation::SetGroup((_, group)) => {
                node.group = (!group.is_empty()).then(|| group.into());
            }
            Mutation::SetGroupHoverStyle((_, group, style)) => {
                node.group_hover = match group.is_empty() {
                    true => None,
                    false => Some((group.into(), parse_style(&style)?)),
                };
            }
            Mutation::SetText((_, text)) => node.text = text.into(),
            Mutation::SetProps((_, props)) => node.props = serde_json::from_str(&props)?,
            Mutation::SetChildren((_, children)) => node.children = children,
            Mutation::Listen((_, events)) => {
                // Keys reach the element that has focus.
                if view_focus.is_none() && events.iter().any(|event| event == "key_down") {
                    node.focus.get_or_insert_with(|| cx.focus_handle());
                }
                node.events = events;
            }
            Mutation::Focus(_) => {
                let handle = view_focus
                    .unwrap_or_else(|| node.focus.get_or_insert_with(|| cx.focus_handle()).clone());
                self.pending_focus = Some(handle);
            }
            Mutation::ScrollBy((_, pixels, pages)) => {
                let handle = node.scroll.get_or_insert_default();
                let down = px(pixels) + handle.bounds().size.height * pages;
                // Offsets grow negative as the content moves up.
                let top = (handle.offset().y - down).clamp(-handle.max_offset().y, px(0.));
                handle.set_offset(point(handle.offset().x, top));
            }
            Mutation::ScrollTo((_, pixels)) => {
                let handle = node.scroll.get_or_insert_default();
                handle.set_offset(point(handle.offset().x, px(-pixels)));
            }
            _ => unreachable!(),
        }
        Ok(())
    }

    fn element(&self, id: u64, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(node) = self.nodes.get(&id) else {
            return Empty.into_any_element();
        };
        let children: Vec<_> = node
            .children
            .iter()
            .map(|child| self.element(*child, window, cx))
            .collect();
        match &node.kind {
            Kind::Text => node.text.clone().into_any_element(),
            Kind::Element => {
                let mut element = div().id(ElementId::Integer(id));
                *element.style() = node.style.clone();
                let focus = match self.root == Some(id) {
                    true => Some(&self.focus),
                    false => node.focus.as_ref(),
                };
                if let Some(focus) = focus {
                    element = element.track_focus(focus);
                }
                if let Some(scroll) = &node.scroll {
                    element = element.track_scroll(scroll);
                }
                if let Some(hover) = &node.hover {
                    element = element.hover(|_| hover.clone());
                }
                if let Some(group) = &node.group {
                    element = element.group(group.clone());
                }
                if let Some((group, style)) = &node.group_hover {
                    element = element.group_hover(group.clone(), |_| style.clone());
                }
                for event in &node.events {
                    element = match event.as_str() {
                        "click" => {
                            element.on_click(cx.listener(move |view, _: &ClickEvent, _, cx| {
                                view.report(id, "click", Value::Null, cx)
                            }))
                        }
                        "mouse_down" => element.on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |view, event: &MouseDownEvent, _, cx| {
                                let at = event.position;
                                let payload = json!({ "x": f32::from(at.x), "y": f32::from(at.y) });
                                view.report(id, "mouse_down", payload, cx)
                            }),
                        ),
                        "hover" => {
                            element.on_hover(cx.listener(move |view, over: &bool, _, cx| {
                                view.report(id, "hover", json!(over), cx)
                            }))
                        }
                        "key_down" => element.on_key_down(cx.listener(
                            move |view, event: &KeyDownEvent, _, cx| {
                                let modifiers = event.keystroke.modifiers;
                                let payload = json!({
                                    "key": event.keystroke.key,
                                    "control": modifiers.control,
                                    "alt": modifiers.alt,
                                    "shift": modifiers.shift,
                                    "platform": modifiers.platform,
                                });
                                view.report(id, "key_down", payload, cx)
                            },
                        )),
                        _ => element,
                    };
                }
                element.children(children).into_any_element()
            }
            Kind::Component(name) => match self.runtime.components.get(name) {
                Some(component) => {
                    let props = Props {
                        id: ElementId::Integer(id),
                        props: &node.props,
                        style: &node.style,
                        children,
                        emit: Emit {
                            view: cx.weak_entity(),
                            node: id,
                        },
                    };
                    component(props, window, cx)
                }
                None => SharedString::from(format!("no component named {name}")).into_any_element(),
            },
        }
    }
}

impl Render for PluginView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(focus) = self.pending_focus.take() {
            window.focus(&focus, cx);
        }
        match (&self.error, self.root) {
            (Some(error), _) => error.clone().into_any_element(),
            (None, Some(root)) => self.element(root, window, cx),
            (None, None) => Empty.into_any_element(),
        }
    }
}

/// A node's style: [`StyleRefinement`] as GPUI itself writes and reads it.
fn parse_style(json: &str) -> Result<StyleRefinement> {
    Ok(serde_json::from_str(json)?)
}

#[cfg(test)]
mod tests;
