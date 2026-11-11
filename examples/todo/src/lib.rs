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
            .flex()
            .flex_row()
            .gap_2()
            .group("row")
            .child(
                div()
                    .flex_1()
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(0xf0f0f0)))
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
                    .opacity(0.4)
                    .group_hover("row", |style| style.opacity(1.))
                    .on_click(cx.listener(|this, _, cx| cx.emit(RowEvent::Removed(this.id))))
                    .child("remove"),
            )
    }
}

struct Todo {
    rows: Vec<Entity<Row>>,
    next: u32,
    focus: FocusHandle,
    scroll: ScrollHandle,
    /// Whether the pointer is over "add", and how often it was pressed.
    hovering: bool,
    presses: u32,
}

impl Todo {
    fn new(cx: &mut Cx<Self>) -> Self {
        let mut todo = Todo {
            rows: Vec::new(),
            next: 0,
            focus: FocusHandle::new(),
            scroll: ScrollHandle::new(),
            hovering: false,
            presses: 0,
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
        match name {
            "add" => self.add(payload.as_str().unwrap_or_default().to_owned(), cx),
            "fail" => panic!("told to fail"),
            _ => return,
        }
        cx.notify();
    }

    fn render(&mut self, cx: &mut Cx<Self>) -> Element {
        let left = (self.rows.iter())
            .filter(|row| row.read(|row| !row.done))
            .count();
        let label = match self.presses {
            0 => format!("{left} left"),
            presses => format!("{left} left, {presses} pressed"),
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .p_2()
            .gap_1()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x202020))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event, cx| {
                if event["key"] == "a" {
                    this.add(format!("item {}", this.next + 1), cx);
                    cx.notify();
                } else if event["key"] == "d" {
                    this.scroll.scroll_by(10., 0.);
                }
            }))
            .child(
                div()
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, cx| {
                        this.add(format!("item {}", this.next + 1), cx);
                        cx.notify();
                    }))
                    .on_mouse_down(cx.listener(|this, _, cx| {
                        this.presses += 1;
                        cx.notify();
                    }))
                    .on_hover(cx.listener(|this, over, cx| {
                        this.hovering = over == true;
                        cx.notify();
                    }))
                    .child(if self.hovering {
                        "add (hovering)"
                    } else {
                        "add"
                    }),
            )
            .child(component("badge", json!({ "label": label })).on(
                "pressed",
                cx.listener(|this, _, cx| {
                    this.rows.clear();
                    cx.notify();
                }),
            ))
            .children(self.rows.iter())
    }
}

export_view!(Todo::new);
