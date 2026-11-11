//! What crosses to the host, checked without one: the entry points a
//! plugin exports are called directly and their batches read.

use crate::{
    __event, __init, Cx, Element, Entity, Mutation, Render, Styled as _, Value, div, text,
};

fn count(batch: &[Mutation], matches: impl Fn(&Mutation) -> bool) -> usize {
    batch.iter().filter(|mutation| matches(mutation)).count()
}

fn created(batch: &[Mutation]) -> usize {
    count(batch, |mutation| matches!(mutation, Mutation::Create(_)))
}

fn disposed(batch: &[Mutation]) -> usize {
    count(batch, |mutation| matches!(mutation, Mutation::Dispose(_)))
}

/// The node that reports `event`, from the batch that made it listen.
fn listening(batch: &[Mutation], event: &str) -> u64 {
    (batch.iter())
        .find_map(|mutation| match mutation {
            Mutation::Listen((id, events)) if events.iter().any(|name| name == event) => Some(*id),
            _ => None,
        })
        .expect("a listening node")
}

/// A child view whose root is a `div` or, once clicked, text.
struct Flip(bool);

impl Render for Flip {
    fn render(&mut self, cx: &mut Cx<Self>) -> Element {
        if self.0 {
            return text("flipped");
        }
        div()
            .on_click(cx.listener(|flip, _, cx| {
                flip.0 = true;
                cx.notify();
            }))
            .child("flip")
    }
}

struct Around(Entity<Flip>);

impl Render for Around {
    fn render(&mut self, _: &mut Cx<Self>) -> Element {
        div().child("before").child(&self.0).child("after")
    }
}

#[test]
fn a_view_whose_root_changes_kind_takes_its_own_place() {
    let first = __init(|cx: &mut Cx<Around>| Around(cx.new(|_| Flip(false))));
    let old = listening(&first, "click");
    let batch = __event(old, "click", "null");

    // The view rendered alone: its new root replaces the old where it was,
    // and nothing about the parent crosses.
    let replaced = (batch.iter()).find_map(|mutation| match mutation {
        Mutation::Replace((from, to)) => Some((*from, *to)),
        _ => None,
    });
    let (from, to) = replaced.expect("a replace");
    assert_eq!(from, old);
    assert!(
        batch
            .iter()
            .any(|mutation| matches!(mutation, Mutation::Create((id, _)) if *id == to))
    );
    assert_eq!(created(&batch), 1);
    assert_eq!(disposed(&batch), 2, "the old root and its text");
    assert_eq!(
        count(&batch, |mutation| matches!(
            mutation,
            Mutation::SetChildren(_)
        )),
        0
    );
    // The old root's handler went with it.
    assert!(__event(old, "click", "null").is_empty());
}

struct Row(&'static str);

impl Render for Row {
    fn render(&mut self, _: &mut Cx<Self>) -> Element {
        div().child(self.0)
    }
}

struct List(Vec<Entity<Row>>);

impl Render for List {
    fn host_event(&mut self, name: &str, _: &Value, cx: &mut Cx<Self>) {
        match name {
            "rotate" => self.0.rotate_left(1),
            "prepend" => self.0.insert(0, cx.new(|_| Row("new"))),
            "drop-first" => drop(self.0.remove(0)),
            _ => return,
        }
        cx.notify();
    }

    fn render(&mut self, _: &mut Cx<Self>) -> Element {
        div().children(self.0.iter())
    }
}

fn list() -> Vec<u64> {
    let batch =
        __init(|cx: &mut Cx<List>| List(["a", "b", "c"].map(|name| cx.new(|_| Row(name))).into()));
    children(&batch).expect("the list's children")
}

/// The children the last `SetChildren` of `batch` gave the list.
fn children(batch: &[Mutation]) -> Option<Vec<u64>> {
    (batch.iter().rev()).find_map(|mutation| match mutation {
        Mutation::SetChildren((_, children)) if children.len() > 1 => Some(children.clone()),
        _ => None,
    })
}

#[test]
fn views_that_move_keep_their_nodes() {
    let rows = list();
    let batch = __event(0, "rotate", "null");
    assert_eq!(children(&batch), Some(vec![rows[1], rows[2], rows[0]]));
    assert_eq!(batch.len(), 1, "only the order crosses");
}

#[test]
fn a_view_put_in_front_leaves_the_others_as_they_are() {
    let rows = list();
    let batch = __event(0, "prepend", "null");
    let after = children(&batch).expect("the new order");
    assert_eq!(after[1..], rows);
    assert_eq!(created(&batch), 2, "the new row and its text");
    assert_eq!(disposed(&batch), 0);
}

#[test]
fn a_view_taken_out_is_disposed_and_the_rest_stay() {
    let rows = list();
    let batch = __event(0, "drop-first", "null");
    assert_eq!(children(&batch), Some(rows[1..].to_vec()));
    assert_eq!(created(&batch), 0);
    assert_eq!(disposed(&batch), 2, "the row and its text");
}

struct Hears(Vec<Entity<Row>>);

impl Render for Hears {
    fn host_event(&mut self, _: &str, _: &Value, cx: &mut Cx<Self>) {
        // Replaces its only row, hearing the new one.
        let row = cx.new(|_| Row("row"));
        cx.subscribe(&row, |_, _: &(), _| {});
        self.0 = vec![row];
        cx.notify();
    }

    fn render(&mut self, _: &mut Cx<Self>) -> Element {
        div().children(self.0.iter())
    }
}

#[test]
fn hearing_views_that_are_gone_does_not_pile_up() {
    __init(|_: &mut Cx<Hears>| Hears(Vec::new()));
    for _ in 0..10 {
        __event(0, "replace", "null");
    }
    let root = crate::ROOT.with(|root| root.borrow().clone()).unwrap();
    assert_eq!(root.subscription_count(), 1, "only the row in place");
    let heard = crate::SUBSCRIBERS.with(|subscribers| subscribers.borrow().len());
    assert_eq!(heard, 1);
}

struct StyledRow(usize);

impl Render for StyledRow {
    fn render(&mut self, cx: &mut Cx<Self>) -> Element {
        div()
            .flex()
            .flex_row()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .cursor_pointer()
                    .on_click(cx.listener(|_, _, _| {}))
                    .child(format!("[ ] row {}", self.0)),
            )
            .child(
                div()
                    .cursor_pointer()
                    .on_click(cx.listener(|_, _, _| {}))
                    .child("remove"),
            )
    }
}

struct Hundred(Vec<Entity<StyledRow>>);

impl Render for Hundred {
    fn render(&mut self, _: &mut Cx<Self>) -> Element {
        div()
            .flex()
            .flex_col()
            .size_full()
            .p_2()
            .gap_1()
            .children(self.0.iter())
    }
}

/// Times a first render of 100 rows without a boundary to cross:
/// `cargo test --release -p gpui-wit-plugin-sdk -- --ignored --nocapture`.
#[test]
#[ignore = "a measurement, not a check"]
fn measure_a_first_render() {
    let mut times = Vec::new();
    for _ in 0..200 {
        let started = std::time::Instant::now();
        let batch = __init(|cx: &mut Cx<Hundred>| {
            Hundred((0..100).map(|row| cx.new(|_| StyledRow(row))).collect())
        });
        times.push(started.elapsed());
        assert!(batch.len() > 1000);
    }
    println!(
        "sizes: StyleRefinement {}, Element {}",
        std::mem::size_of::<gpui::StyleRefinement>(),
        std::mem::size_of::<Element>()
    );
    times.sort();
    println!("first render: best {:?}, median {:?}", times[0], times[100]);

    // Building the elements alone, and serializing their styles alone.
    let mut cx = Cx::<StyledRow> {
        key: 0,
        notified: false,
        handlers: Vec::new(),
        subscriptions: std::collections::HashMap::new(),
    };
    let mut builds = Vec::new();
    let mut serials = Vec::new();
    for _ in 0..200 {
        let started = std::time::Instant::now();
        let rows: Vec<Element> = (0..100).map(|row| StyledRow(row).render(&mut cx)).collect();
        builds.push(started.elapsed());
        let started = std::time::Instant::now();
        let mut bytes = 0;
        for row in &rows {
            bytes += serde_json::to_string(&row.style).unwrap().len();
            for child in &row.children {
                bytes += serde_json::to_string(&child.style).unwrap().len();
            }
        }
        serials.push(started.elapsed());
        assert!(bytes > 0);
    }
    builds.sort();
    serials.sort();
    println!(
        "build: {:?}, serialize styles: {:?}",
        builds[100], serials[100]
    );
}
