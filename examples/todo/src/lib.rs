//! A to-do list, written the way a GPUI view is.
//!
//! Each row is a child view with its own state: toggling one renders that
//! row and, because the count changed, the list around it, but no other
//! row. The first rows come from the file `TODO_FILE` names, read through
//! WASI inside whatever directory the host preopened. The `badge` is a
//! component the host registered.

use gpui_wit_plugin_sdk::*;

struct Row {
    id: u32,
    title: String,
    done: bool,
}

enum RowEvent {
    Toggled,
    Removed(u32),
}

impl Render for Row {
    fn render(&mut self, cx: &mut Cx<Self>) -> Element {
        let mark = if self.done { "[x]" } else { "[ ]" };
        div()
            .flex_row()
            .gap(8.)
            .child(
                div()
                    .flex_1()
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, cx| {
                        this.done = !this.done;
                        cx.emit(RowEvent::Toggled);
                        cx.notify();
                    }))
                    .child(format!("{mark} {}", self.title)),
            )
            .child(
                div()
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, cx| cx.emit(RowEvent::Removed(this.id))))
                    .child("remove"),
            )
    }
}

struct Todo {
    rows: Vec<Entity<Row>>,
    next: u32,
    focus: FocusHandle,
}

impl Todo {
    fn new(cx: &mut Cx<Self>) -> Self {
        let mut todo = Todo {
            rows: Vec::new(),
            next: 0,
            focus: FocusHandle::new(),
        };
        let initial = std::env::var("TODO_FILE")
            .ok()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .unwrap_or_default();
        for title in initial.lines() {
            todo.add(title.to_owned(), cx);
        }
        todo.focus.focus();
        todo
    }

    fn add(&mut self, title: String, cx: &mut Cx<Self>) {
        self.next += 1;
        let id = self.next;
        let row = cx.new(|_| Row {
            id,
            title,
            done: false,
        });
        cx.subscribe(&row, |this, event: &RowEvent, cx| {
            if let RowEvent::Removed(id) = event {
                this.rows.retain(|row| row.read(|row| row.id != *id));
            }
            cx.notify();
        });
        self.rows.push(row);
    }
}

impl Render for Todo {
    fn host_event(&mut self, name: &str, payload: &Value, cx: &mut Cx<Self>) {
        if name == "add" {
            self.add(payload.as_str().unwrap_or_default().to_owned(), cx);
            cx.notify();
        }
    }

    fn render(&mut self, cx: &mut Cx<Self>) -> Element {
        let left = (self.rows.iter())
            .filter(|row| row.read(|row| !row.done))
            .count();
        div()
            .flex_col()
            .size_full()
            .p(8.)
            .gap(4.)
            .bg("#ffffff")
            .text_color("#202020")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event, cx| {
                if event["key"] == "a" {
                    this.add(format!("item {}", this.next + 1), cx);
                    cx.notify();
                }
            }))
            .child(
                div()
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, cx| {
                        this.add(format!("item {}", this.next + 1), cx);
                        cx.notify();
                    }))
                    .child("add"),
            )
            .child(
                component("badge", json!({ "label": format!("{left} left") })).on(
                    "pressed",
                    cx.listener(|this, _, cx| {
                        this.rows.clear();
                        cx.notify();
                    }),
                ),
            )
            .children(self.rows.iter())
    }
}

export_view!(Todo::new);
