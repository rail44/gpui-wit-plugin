//! Writes a gpui-wit-plugin plugin the way a GPUI view is written: a struct
//! whose [`Render::render`] returns its whole element tree and runs again
//! after `cx.notify()`.
//!
//! Each render is compared with the one before, by position, and only the
//! difference crosses to the host ([`diff`]). A view splits itself into
//! child views ([`Entity`], made with `cx.new`) that hold their own state:
//! a child renders again only when it notified, and a parent rendering
//! again leaves the children it places untouched. A child tells its parent
//! something with `cx.emit`, heard through `cx.subscribe`.
//!
//! ```ignore
//! struct Counter(u32);
//!
//! impl Render for Counter {
//!     fn render(&mut self, cx: &mut Cx<Self>) -> Element {
//!         div()
//!             .p(8.)
//!             .on_click(cx.listener(|this, _, cx| {
//!                 this.0 += 1;
//!                 cx.notify();
//!             }))
//!             .child(format!("{} clicks", self.0))
//!     }
//! }
//!
//! export_view!(|_cx| Counter(0));
//! ```

use std::{
    any::Any,
    borrow::Cow,
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::{Rc, Weak},
};

pub use serde_json::{self, Value, json};

mod diff;
mod element;

pub use element::{Element, Length, component, div, px, relative, rems, text};

#[doc(hidden)]
pub mod bindings {
    wit_bindgen::generate!({ path: "../../wit", world: "plugin", pub_export_macro: true });
}

#[doc(hidden)]
pub use bindings::gpui_wit::plugin::tree::Mutation;

use diff::{Differ, Retained};

/// A view: what GPUI's `Render` is.
pub trait Render: Sized + 'static {
    fn render(&mut self, cx: &mut Cx<Self>) -> Element;

    /// Something the embedding application told the plugin's own view.
    fn host_event(&mut self, _name: &str, _payload: &Value, _cx: &mut Cx<Self>) {}
}

type Handler<V> = Rc<dyn Fn(&mut V, &Value, &mut Cx<V>)>;
type Subscriber<V> = Rc<dyn Fn(&mut V, &dyn Any, &mut Cx<V>)>;

/// A handler registered for the render in progress.
#[derive(Clone, Copy, PartialEq)]
pub struct Listener(pub(crate) u32);

/// What a view's code is handed alongside the view.
pub struct Cx<V> {
    key: u32,
    notified: bool,
    handlers: Vec<Handler<V>>,
    subscriptions: Vec<(u32, Subscriber<V>)>,
}

impl<V: 'static> Cx<V> {
    /// Renders the view again once the code running now returns.
    pub fn notify(&mut self) {
        self.notified = true;
    }

    /// A handler for an element of the render in progress. It receives the
    /// event's payload.
    pub fn listener(&mut self, handler: impl Fn(&mut V, &Value, &mut Cx<V>) + 'static) -> Listener {
        self.handlers.push(Rc::new(handler));
        Listener(self.handlers.len() as u32 - 1)
    }

    /// Makes a child view, as GPUI's `cx.new` makes an entity. Keep the
    /// handle in the view's state and place it from `render`.
    // Named for GPUI's `cx.new`, which returns the entity, not the context.
    #[allow(clippy::new_ret_no_self)]
    pub fn new<C: Render>(&mut self, build: impl FnOnce(&mut Cx<C>) -> C) -> Entity<C> {
        Entity::new(build)
    }

    /// Tells whoever subscribed to this view that something happened.
    pub fn emit<E: 'static>(&mut self, event: E) {
        let key = self.key;
        EMITTED.with(|emitted| emitted.borrow_mut().push((key, Box::new(event))));
    }

    /// Hears what `entity` emits.
    pub fn subscribe<C: Render, E: 'static>(
        &mut self,
        entity: &Entity<C>,
        on_event: impl Fn(&mut V, &E, &mut Cx<V>) + 'static,
    ) {
        self.subscriptions.push((
            entity.0.key,
            Rc::new(move |view, event, cx| {
                if let Some(event) = event.downcast_ref::<E>() {
                    on_event(view, event, cx);
                }
            }),
        ));
    }
}

/// Where keyboard focus can be, as GPUI's `FocusHandle`: an element takes it
/// with `track_focus`, and `focus` moves focus there.
#[derive(Clone, Default)]
pub struct FocusHandle(Rc<Cell<u64>>);

impl FocusHandle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn focus(&self) {
        FOCUS.with(|focus| *focus.borrow_mut() = Some(self.clone()));
    }
}

thread_local! {
    static VIEWS: RefCell<HashMap<u32, Weak<dyn AnyView>>> = RefCell::new(HashMap::new());
    static NEXT_VIEW: Cell<u32> = const { Cell::new(1) };
    /// Views that notified and have not rendered since.
    static DIRTY: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
    /// What views emitted, not yet handed to subscribers.
    static EMITTED: RefCell<Vec<(u32, Box<dyn Any>)>> = const { RefCell::new(Vec::new()) };
    static FOCUS: RefCell<Option<FocusHandle>> = const { RefCell::new(None) };
    static DIFFER: RefCell<Differ> = RefCell::new(Differ::default());
    static ROOT: RefCell<Option<Rc<dyn AnyView>>> = const { RefCell::new(None) };
}

fn view(key: u32) -> Option<Rc<dyn AnyView>> {
    VIEWS.with(|views| views.borrow().get(&key).and_then(Weak::upgrade))
}

/// A view's handle, as a GPUI `Entity` is: cloning it shares the view.
pub struct Entity<C: Render>(Rc<ViewCell<C>>);

impl<C: Render> Clone for Entity<C> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<C: Render> Entity<C> {
    fn new(build: impl FnOnce(&mut Cx<C>) -> C) -> Self {
        let key = NEXT_VIEW.with(|next| next.replace(next.get() + 1));
        let mut cx = Cx {
            key,
            notified: false,
            handlers: Vec::new(),
            subscriptions: Vec::new(),
        };
        let view = build(&mut cx);
        let cell = Rc::new(ViewCell {
            key,
            state: RefCell::new((view, cx)),
            dirty: Cell::new(false),
            root: RefCell::new(None),
            claimed: Cell::new(0),
        });
        let weak: Weak<dyn AnyView> = Rc::downgrade(&(cell.clone() as Rc<dyn AnyView>));
        VIEWS.with(|views| views.borrow_mut().insert(key, weak));
        Entity(cell)
    }

    pub fn read<R>(&self, read: impl FnOnce(&C) -> R) -> R {
        read(&self.0.state.borrow().0)
    }

    /// Changes the view's state. It renders again if `update` notifies.
    pub fn update<V, R>(&self, _cx: &mut Cx<V>, update: impl FnOnce(&mut C, &mut Cx<C>) -> R) -> R {
        self.0.run(update)
    }
}

impl<C: Render> From<&Entity<C>> for Element {
    fn from(entity: &Entity<C>) -> Self {
        Element::view(entity.0.clone())
    }
}

struct ViewCell<C> {
    key: u32,
    state: RefCell<(C, Cx<C>)>,
    dirty: Cell<bool>,
    /// The view's last render, while it is placed somewhere.
    root: RefCell<Option<Retained>>,
    /// The comparison that last placed the view, so that one finding the
    /// view's old place afterwards knows it moved rather than went.
    claimed: Cell<u64>,
}

impl<C> ViewCell<C> {
    /// Runs the view's code, and queues a render if it notified.
    fn run<R>(&self, code: impl FnOnce(&mut C, &mut Cx<C>) -> R) -> R {
        let mut state = self.state.borrow_mut();
        let (view, cx) = &mut *state;
        let result = code(view, cx);
        if std::mem::take(&mut cx.notified) && !self.dirty.replace(true) {
            DIRTY.with(|dirty| dirty.borrow_mut().push(self.key));
        }
        result
    }
}

impl<C> Drop for ViewCell<C> {
    fn drop(&mut self) {
        let _ = VIEWS.try_with(|views| views.borrow_mut().remove(&self.key));
    }
}

/// A view of any type, as the comparison and the event routing handle it.
pub(crate) trait AnyView {
    fn key(&self) -> u32;
    fn root_id(&self) -> Option<u64>;
    fn claimed(&self) -> u64;
    fn claim(&self, pass: u64);
    /// Takes the view's last render away: it is no longer placed.
    fn take_root(&self) -> Option<Retained>;
    /// Renders the view if it is new here or notified. `standalone` is a
    /// render outside its parent's: the view keeps its own place.
    fn sync(&self, differ: &mut Differ, standalone: bool);
    fn dispatch(&self, callback: u32, payload: &Value);
    fn deliver(&self, from: u32, event: &dyn Any);
    fn host_event(&self, name: &str, payload: &Value);
}

impl<C: Render> AnyView for ViewCell<C> {
    fn key(&self) -> u32 {
        self.key
    }

    fn root_id(&self) -> Option<u64> {
        self.root.borrow().as_ref().map(|root| root.id)
    }

    fn claimed(&self) -> u64 {
        self.claimed.get()
    }

    fn claim(&self, pass: u64) {
        self.claimed.set(pass);
    }

    fn take_root(&self) -> Option<Retained> {
        self.root.borrow_mut().take()
    }

    fn sync(&self, differ: &mut Differ, standalone: bool) {
        let old = self.take_root();
        if old.is_some() && !self.dirty.get() {
            *self.root.borrow_mut() = old;
            return;
        }
        self.dirty.set(false);
        let element = {
            let mut state = self.state.borrow_mut();
            let (view, cx) = &mut *state;
            cx.handlers.clear();
            view.render(cx)
        };
        let root = differ.root(self.key, old, element, standalone);
        *self.root.borrow_mut() = Some(root);
    }

    fn dispatch(&self, callback: u32, payload: &Value) {
        let handler = self
            .state
            .borrow()
            .1
            .handlers
            .get(callback as usize)
            .cloned();
        if let Some(handler) = handler {
            self.run(|view, cx| handler(view, payload, cx));
        }
    }

    fn deliver(&self, from: u32, event: &dyn Any) {
        let subscribers: Vec<_> = (self.state.borrow().1.subscriptions.iter())
            .filter(|(key, _)| *key == from)
            .map(|(_, subscriber)| subscriber.clone())
            .collect();
        for subscriber in subscribers {
            self.run(|view, cx| subscriber(view, event, cx));
        }
    }

    fn host_event(&self, name: &str, payload: &Value) {
        self.run(|view, cx| view.host_event(name, payload, cx));
    }
}

/// Settles what the code that just ran left behind, and returns what the
/// host has to change.
fn flush(differ: &mut Differ) -> Vec<Mutation> {
    loop {
        let emitted = EMITTED.with(|emitted| std::mem::take(&mut *emitted.borrow_mut()));
        let dirty = DIRTY.with(|dirty| std::mem::take(&mut *dirty.borrow_mut()));
        if emitted.is_empty() && dirty.is_empty() {
            break;
        }
        let views: Vec<_> = VIEWS.with(|views| views.borrow().values().cloned().collect());
        for (from, event) in emitted {
            for view in views.iter().filter_map(Weak::upgrade) {
                view.deliver(from, &*event);
            }
        }
        differ.pass += 1;
        for key in dirty {
            // A view that is not placed renders when it is.
            if let Some(view) = view(key)
                && view.root_id().is_some()
            {
                view.sync(differ, true);
            }
        }
    }
    let mut batch = differ.take();
    if let Some(focus) = FOCUS.with(|focus| focus.borrow_mut().take())
        && focus.0.get() != 0
    {
        batch.push(Mutation::Focus(focus.0.get()));
    }
    batch
}

#[doc(hidden)]
pub fn __init<V: Render>(build: impl FnOnce(&mut Cx<V>) -> V) -> Vec<Mutation> {
    let root = Entity::new(build).0;
    ROOT.with(|slot| *slot.borrow_mut() = Some(root.clone()));
    DIFFER.with(|differ| {
        let differ = &mut *differ.borrow_mut();
        differ.pass += 1;
        root.sync(differ, false);
        differ.set_root(root.root_id().unwrap_or(0));
        flush(differ)
    })
}

#[doc(hidden)]
pub fn __event(target: u64, name: &str, payload: &str) -> Vec<Mutation> {
    let payload: Value = serde_json::from_str(payload).unwrap_or(Value::Null);
    let handler = DIFFER.with(|differ| differ.borrow().handler(target, name));
    if target == 0 {
        if let Some(root) = ROOT.with(|root| root.borrow().clone()) {
            root.host_event(name, &payload);
        }
    } else if let Some((key, callback)) = handler
        && let Some(view) = view(key)
    {
        view.dispatch(callback, &payload);
    }
    DIFFER.with(|differ| flush(&mut differ.borrow_mut()))
}

/// Exports the plugin: `build` makes its view, as the closure handed to
/// GPUI's `cx.new` does.
#[macro_export]
macro_rules! export_view {
    ($build:expr) => {
        struct __GpuiWitPlugin;

        impl $crate::bindings::Guest for __GpuiWitPlugin {
            fn init() -> Vec<$crate::Mutation> {
                $crate::__init($build)
            }

            fn event(target: u64, name: String, payload: String) -> Vec<$crate::Mutation> {
                $crate::__event(target, &name, &payload)
            }
        }

        $crate::bindings::export!(__GpuiWitPlugin with_types_in $crate::bindings);
    };
}

pub(crate) type Name = Cow<'static, str>;
