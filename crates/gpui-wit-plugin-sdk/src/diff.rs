//! Turns renders into what the host has to change.
//!
//! A render is compared with the one before by position: a node keeps its
//! number while the element in its position keeps its kind, and only what
//! differs about it crosses. A view placed in a render is not rendered for
//! it: its own last render stays as it is unless the view notified.

use std::{collections::HashMap, rc::Rc};

use serde_json::{Map, Value};

use crate::{
    AnyView, Mutation, Name,
    bindings::gpui_wit::plugin::tree::Kind as WireKind,
    element::{Element, Kind},
};

/// An element's events, each with the callback it reaches.
type Listeners = Vec<(&'static str, u32)>;

/// One node of a view's last render, with the number the host knows it by.
pub(crate) struct Retained {
    pub(crate) id: u64,
    /// The component's name; `None` for a `div` or text.
    component: Option<Name>,
    is_text: bool,
    text: String,
    style: Map<String, Value>,
    props: Value,
    listeners: Listeners,
    children: Vec<Kept>,
}

/// What sits in a node's child position: a node, or a view's root.
// Nearly every child is a node, so boxing one would cost an allocation each.
#[allow(clippy::large_enum_variant)]
enum Kept {
    Node(Retained),
    View(Rc<dyn AnyView>),
}

impl Kept {
    fn id(&self) -> u64 {
        match self {
            Kept::Node(node) => node.id,
            Kept::View(view) => view.root_id().unwrap_or(0),
        }
    }
}

#[derive(Default)]
pub(crate) struct Differ {
    next_id: u64,
    /// For each node with handlers: the view that registered them, and its
    /// callback for each event, as of that view's latest render.
    handlers: HashMap<u64, (u32, Listeners)>,
    batch: Vec<Mutation>,
    /// The view whose render is being compared.
    owner: u32,
    /// Counts comparisons, so a view placed in this one is told from one
    /// placed before.
    pub(crate) pass: u64,
}

impl Differ {
    /// The view and callback a node's event reaches.
    pub(crate) fn handler(&self, node: u64, event: &str) -> Option<(u32, u32)> {
        let (owner, listeners) = self.handlers.get(&node)?;
        (listeners.iter())
            .find_map(|(name, callback)| (*name == event).then_some((*owner, *callback)))
    }

    pub(crate) fn take(&mut self) -> Vec<Mutation> {
        std::mem::take(&mut self.batch)
    }

    pub(crate) fn set_root(&mut self, id: u64) {
        self.batch.push(Mutation::SetRoot(id));
    }

    /// The root node for the render `element` of view `owner`, whose last
    /// was `old`. `standalone`: nothing else will put a new root in the old
    /// one's place.
    pub(crate) fn root(
        &mut self,
        owner: u32,
        old: Option<Retained>,
        element: Element,
        standalone: bool,
    ) -> Retained {
        assert!(
            !matches!(element.kind, Kind::View(_)),
            "a render returns an element; place a view inside one"
        );
        let outer = std::mem::replace(&mut self.owner, owner);
        let root = match old {
            Some(old) if same_kind(&old, &element) => self.reconcile(old, element),
            old => {
                let root = self.create(element);
                if let Some(old) = old {
                    if standalone {
                        self.batch.push(Mutation::Replace((old.id, root.id)));
                    }
                    self.dispose(Kept::Node(old));
                }
                root
            }
        };
        self.owner = outer;
        root
    }

    /// Disposes what was in a position that something else now takes. A
    /// view placed elsewhere in this comparison lives on.
    fn dispose(&mut self, kept: Kept) {
        let node = match kept {
            Kept::Node(node) => node,
            Kept::View(view) if view.claimed() != self.pass => match view.take_root() {
                Some(root) => root,
                None => return,
            },
            Kept::View(_) => return,
        };
        self.batch.push(Mutation::Dispose(node.id));
        self.handlers.remove(&node.id);
        for child in node.children {
            self.dispose(child);
        }
    }

    /// What goes in a child position: `old` kept when the element is the
    /// same view or the same kind of element.
    fn place(&mut self, old: Option<Kept>, element: Element) -> Kept {
        if let Kind::View(view) = &element.kind {
            let view = view.clone();
            match old {
                Some(Kept::View(old)) if old.key() == view.key() => {}
                Some(old) => self.dispose(old),
                None => {}
            }
            view.sync(self, false);
            return Kept::View(view);
        }
        match old {
            Some(Kept::Node(node)) if same_kind(&node, &element) => {
                Kept::Node(self.reconcile(node, element))
            }
            old => {
                if let Some(old) = old {
                    self.dispose(old);
                }
                Kept::Node(self.create(element))
            }
        }
    }

    /// The children of node `id`: `olds` matched to `elements` by position.
    fn place_children(&mut self, id: u64, olds: Vec<Kept>, elements: Vec<Element>) -> Vec<Kept> {
        // Every view placed here is claimed first, so that finding one in
        // another position's old place does not dispose it.
        for element in &elements {
            if let Kind::View(view) = &element.kind {
                view.claim(self.pass);
            }
        }
        let before: Vec<u64> = olds.iter().map(Kept::id).collect();
        let mut olds = olds.into_iter();
        let kept: Vec<Kept> = elements
            .into_iter()
            .map(|element| {
                let old = olds.next();
                self.place(old, element)
            })
            .collect();
        for left in olds {
            self.dispose(left);
        }
        let after: Vec<u64> = kept.iter().map(Kept::id).collect();
        if before != after {
            self.batch.push(Mutation::SetChildren((id, after)));
        }
        kept
    }

    fn create(&mut self, element: Element) -> Retained {
        self.next_id += 1;
        let id = self.next_id;
        let (kind, component) = match &element.kind {
            Kind::Div => (WireKind::Element, None),
            Kind::Text => (WireKind::Text, None),
            Kind::Component(name) => (WireKind::Component(name.to_string()), Some(name.clone())),
            Kind::View(_) => unreachable!("a view is placed, not created"),
        };
        let is_text = matches!(kind, WireKind::Text);
        self.batch.push(Mutation::Create((id, kind)));
        let node = Retained {
            id,
            component,
            is_text,
            text: String::new(),
            style: Map::new(),
            props: Value::Null,
            listeners: Vec::new(),
            children: Vec::new(),
        };
        self.reconcile(node, element)
    }

    /// Updates `old`, which is the same kind of element, to `element`.
    fn reconcile(&mut self, mut old: Retained, element: Element) -> Retained {
        let id = old.id;
        if old.text != element.text {
            self.batch
                .push(Mutation::SetText((id, element.text.clone())));
            old.text = element.text;
        }
        if old.style != element.style {
            let style = Value::Object(element.style.clone()).to_string();
            self.batch.push(Mutation::SetStyle((id, style)));
            old.style = element.style;
        }
        if old.props != element.props {
            self.batch
                .push(Mutation::SetProps((id, element.props.to_string())));
            old.props = element.props;
        }
        if old.listeners != element.listeners {
            // The events cross only when one is added or dropped; which
            // callback each reaches stays here.
            let events = |listeners: &[(&'static str, u32)]| -> Vec<&'static str> {
                listeners.iter().map(|(event, _)| *event).collect()
            };
            if events(&old.listeners) != events(&element.listeners) {
                let events = events(&element.listeners);
                let events = events.into_iter().map(str::to_owned).collect();
                self.batch.push(Mutation::Listen((id, events)));
            }
            if element.listeners.is_empty() {
                self.handlers.remove(&id);
            } else {
                self.handlers
                    .insert(id, (self.owner, element.listeners.clone()));
            }
            old.listeners = element.listeners;
        }
        if let Some(focus) = element.focus {
            focus.0.set(id);
        }
        let olds = std::mem::take(&mut old.children);
        old.children = self.place_children(id, olds, element.children);
        old
    }
}

/// Whether node `old` can become `element` in place.
fn same_kind(old: &Retained, element: &Element) -> bool {
    match &element.kind {
        Kind::Div => !old.is_text && old.component.is_none(),
        Kind::Text => old.is_text,
        Kind::Component(name) => old.component.as_ref() == Some(name),
        Kind::View(_) => false,
    }
}
