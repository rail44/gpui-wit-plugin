//! Draws, with GPUI, views that WASI preview 2 components describe.
//!
//! A plugin is a component exporting the `plugin` world of `wit/plugin.wit`.
//! It numbers the nodes of a tree itself and returns, from each call, the
//! operations that change it; [`PluginView`] keeps that tree and turns it
//! into GPUI elements when it renders. What the plugin sends is GPUI's own
//! vocabulary wherever GPUI has one: a node's style is a
//! [`StyleRefinement`] as JSON, and anything beyond `div` and text is a
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
    AnyElement, App, ClickEvent, Context, ElementId, Empty, EventEmitter, Fill, FocusHandle,
    Focusable, Hsla, InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Render,
    ScrollHandle, SharedString, StatefulInteractiveElement, StyleRefinement, Styled, WeakEntity,
    Window, div, point, px,
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
    pub fn emit(&self, name: &str, payload: Value, window: &mut Window, cx: &mut App) {
        let _ = self.view.update(cx, |view, cx| {
            view.dispatch(self.node, name, &payload, window, cx)
        });
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
    text: SharedString,
    props: Value,
    children: Vec<u64>,
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
    pub fn new(
        runtime: &Rc<Runtime>,
        instance: Instance,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let Instance { mut store, plugin } = instance;
        let init = plugin.call_init(&mut store);
        let mut view = Self {
            runtime: runtime.clone(),
            store,
            plugin,
            nodes: HashMap::new(),
            root: None,
            focus: cx.focus_handle(),
            stop: false,
            error: None,
        };
        match init {
            Ok(batch) => view.apply(batch, window, cx),
            Err(error) => view.error = Some(format!("{error:#}").into()),
        }
        view
    }

    /// The plugin's store state, where [`State::data`] is.
    pub fn state(&mut self) -> &mut State {
        self.store.data_mut()
    }

    /// Hands the plugin an event and applies what it answers. `target` 0 is
    /// the view as a whole: how an application tells a plugin something.
    pub fn dispatch(
        &mut self,
        target: u64,
        name: &str,
        payload: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.error.is_some() {
            return;
        }
        let payload = payload.to_string();
        match self
            .plugin
            .call_event(&mut self.store, target, name, &payload)
        {
            Ok(batch) => self.apply(batch, window, cx),
            Err(error) => self.error = Some(format!("{error:#}").into()),
        }
        cx.emit(Returned);
        cx.notify();
    }

    fn apply(&mut self, batch: Vec<Mutation>, window: &mut Window, cx: &mut Context<Self>) {
        for mutation in batch {
            if let Err(error) = self.apply_one(mutation, window, cx) {
                self.error = Some(format!("{error:#}").into());
                return;
            }
        }
    }

    fn apply_one(
        &mut self,
        mutation: Mutation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let (id, change) = match mutation {
            Mutation::Create((id, kind)) => {
                let node = Node {
                    kind,
                    style: StyleRefinement::default(),
                    text: SharedString::default(),
                    props: Value::Null,
                    children: Vec::new(),
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
                for node in self.nodes.values_mut() {
                    for child in &mut node.children {
                        if *child == old {
                            *child = new;
                        }
                    }
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
            | Mutation::SetText((id, _))
            | Mutation::SetProps((id, _))
            | Mutation::SetChildren((id, _))
            | Mutation::Listen((id, _))
            | Mutation::ScrollBy((id, ..))
            | Mutation::ScrollTo((id, _))
            | Mutation::Focus(id) => (id, mutation),
        };
        // The root node's focus is the view's own.
        let view_focus = (self.root == Some(id)).then(|| self.focus.clone());
        let node = self.nodes.get_mut(&id).ok_or_else(|| {
            anyhow::anyhow!("the plugin changed node {id}, which it has not made")
        })?;
        match change {
            Mutation::SetStyle((_, style)) => node.style = parse_style(&style)?,
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
                window.focus(&handle, cx);
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
                for event in &node.events {
                    element = match event.as_str() {
                        "click" => element.on_click(cx.listener(
                            move |view, _: &ClickEvent, window, cx| {
                                view.dispatch(id, "click", &Value::Null, window, cx);
                                if std::mem::take(&mut view.stop) {
                                    cx.stop_propagation();
                                }
                            },
                        )),
                        "key_down" => element.on_key_down(cx.listener(
                            move |view, event: &KeyDownEvent, window, cx| {
                                let modifiers = event.keystroke.modifiers;
                                let payload = json!({
                                    "key": event.keystroke.key,
                                    "control": modifiers.control,
                                    "alt": modifiers.alt,
                                    "shift": modifiers.shift,
                                    "platform": modifiers.platform,
                                });
                                view.dispatch(id, "key_down", &payload, window, cx);
                                if std::mem::take(&mut view.stop) {
                                    cx.stop_propagation();
                                }
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
        match (&self.error, self.root) {
            (Some(error), _) => error.clone().into_any_element(),
            (None, Some(root)) => self.element(root, window, cx),
            (None, None) => Empty.into_any_element(),
        }
    }
}

/// A node's style: [`StyleRefinement`] as GPUI itself reads it, except that
/// `background` is a plain color.
fn parse_style(json: &str) -> Result<StyleRefinement> {
    let mut value: Value = serde_json::from_str(json)?;
    let background = value
        .as_object_mut()
        .and_then(|style| style.remove("background"));
    let mut style: StyleRefinement = serde_json::from_value(value)?;
    if let Some(background) = background {
        style.background = Some(Fill::Color(
            serde_json::from_value::<Hsla>(background)?.into(),
        ));
    }
    Ok(style)
}

#[cfg(test)]
mod tests;
