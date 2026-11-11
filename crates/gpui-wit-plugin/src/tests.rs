use std::{ops::Deref, path::PathBuf, process::Command, rc::Rc};

use gpui::{
    AppContext as _, Entity, InteractiveElement, IntoElement, Modifiers, ParentElement,
    SharedString, StatefulInteractiveElement, TestAppContext, VisualTestContext, div, point, px,
};
use serde_json::Value;
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtxBuilder};

use super::{PluginView, Runtime};

/// Builds the example plugin `name` and returns its component.
fn example(name: &str) -> PathBuf {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name);
    let status = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["build", "--release", "--target", "wasm32-wasip2"])
        .current_dir(&directory)
        .status()
        .expect("cargo");
    assert!(status.success(), "building {name}");
    directory.join(format!("target/wasm32-wasip2/release/{name}.wasm"))
}

/// The tree a plugin has described, one node per line, as `id text`.
fn outline(view: &Entity<PluginView>, cx: &mut VisualTestContext) -> Vec<String> {
    view.update(cx, |view, _| {
        (view.outline().iter())
            .map(|(id, line)| format!("{id} {line}"))
            .collect()
    })
}

/// Clicks the element whose text is `label`, as the element would report it.
fn click(view: &Entity<PluginView>, label: &str, cx: &mut VisualTestContext) {
    view.update_in(cx, |view, _, cx| {
        let (id, _) = (view.nodes.iter())
            .find(|(_, node)| {
                (node.children.iter()).any(|child| view.nodes[child].text.as_ref() == label)
            })
            .expect("the element");
        view.dispatch(*id, "click", &Value::Null, cx);
    });
}

#[gpui::test]
fn a_plugin_draws_and_hears_events(cx: &mut TestAppContext) {
    let data = std::env::temp_dir().join(format!("gpui-wit-plugin-{}", std::process::id()));
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("todos.txt"), "write\nreview\n").unwrap();

    let mut runtime = Runtime::new().unwrap();
    runtime.register("badge", |props, _, _| {
        let emit = props.emit.clone();
        let label = props.props["label"].as_str().unwrap_or_default().to_owned();
        div()
            .id(props.id)
            .debug_selector(|| "badge".into())
            .on_click(move |_, _, cx| emit.emit("pressed", Value::Null, cx))
            .child(SharedString::from(label))
            .into_any_element()
    });
    let runtime = Rc::new(runtime);
    let component = runtime.load(example("todo")).unwrap();
    let mut wasi = WasiCtxBuilder::new();
    wasi.env("TODO_FILE", "/data/todos.txt")
        .preopened_dir(&data, "/data", DirPerms::READ, FilePerms::READ)
        .unwrap();

    let instance = runtime
        .instantiate(&component, wasi.build(), Box::new(()))
        .unwrap();
    let window = cx.add_window(|_, cx| PluginView::new(&runtime, instance, cx));
    let view = window.root(cx).unwrap();
    let cx = &mut VisualTestContext::from_window(*window.deref(), cx);
    cx.run_until_parked();

    // The rows came from the file, read through WASI.
    let first = outline(&view, cx);
    let texts = |lines: &[String]| -> Vec<String> {
        (lines.iter())
            .map(|line| line.split_once(' ').unwrap().1.to_owned())
            .collect()
    };
    assert_eq!(
        texts(&first),
        [
            "add",
            r#"<badge {"label":"2 left"}>"#,
            "[ ] write",
            "remove",
            "[ ] review",
            "remove"
        ]
    );

    // Toggling a row changes that row and the count; every node stays.
    click(&view, "[ ] write", cx);
    let toggled = outline(&view, cx);
    assert_eq!(
        texts(&toggled)[1..3],
        [r#"<badge {"label":"1 left"}>"#, "[x] write"]
    );
    let ids = |lines: &[String]| -> Vec<String> {
        (lines.iter())
            .map(|line| line.split_once(' ').unwrap().0.to_owned())
            .collect()
    };
    assert_eq!(ids(&first), ids(&toggled));

    // A real click on the first element, and a key while focused.
    cx.simulate_click(point(px(12.), px(12.)), Modifiers::none());
    cx.simulate_keystrokes("a");
    let added = outline(&view, cx);
    assert_eq!(
        texts(&added)[6..],
        ["[ ] item 3", "remove", "[ ] item 4", "remove"]
    );
    assert_eq!(ids(&first), ids(&added)[..6]);

    // A row removes itself by telling the list; the rows after it stay.
    view.update_in(cx, |view, _, cx| {
        let remove = ids(&added)[3].parse::<u64>().unwrap();
        let (id, _) = (view.nodes.iter())
            .find(|(_, node)| node.children.contains(&remove))
            .unwrap();
        view.dispatch(*id, "click", &Value::Null, cx);
    });
    let removed = outline(&view, cx);
    assert_eq!(texts(&removed)[2..4], ["[ ] review", "remove"]);
    assert_eq!(ids(&removed)[2..], ids(&added)[4..]);
    let live = view.update(cx, |view, _| view.nodes.len());
    assert_eq!(live, 1 + 2 + 1 + 3 * 5, "disposed nodes are forgotten");

    // Something the application tells the plugin, and its word that the
    // call has returned.
    let returned = Rc::new(std::cell::Cell::new(0));
    let count = returned.clone();
    let _subscription = cx.update(|_, cx| {
        cx.subscribe(&view, move |_, _: &super::Returned, _| {
            count.set(count.get() + 1)
        })
    });
    view.update_in(cx, |view, _, cx| {
        view.dispatch(0, "add", &Value::from("told"), cx);
    });
    assert_eq!(texts(&outline(&view, cx)).last().unwrap(), "remove");
    assert!(texts(&outline(&view, cx)).contains(&"[ ] told".to_owned()));
    assert_eq!(returned.get(), 1);

    // The plugin scrolls its own element, once there is more than fits.
    let offset = |cx: &mut VisualTestContext| {
        view.update(cx, |view, _| {
            let root = &view.nodes[&view.root.unwrap()];
            root.scroll.as_ref().expect("a scrolling root").offset().y
        })
    };
    for row in 0..80 {
        view.update_in(cx, |view, _, cx| {
            view.dispatch(0, "add", &Value::from(format!("row {row}")), cx);
        });
    }
    cx.run_until_parked();
    assert_eq!(offset(cx), px(0.));
    cx.simulate_keystrokes("d d");
    assert_eq!(offset(cx), px(-20.));

    // An event of a registered component, from a real click on it.
    let badge = cx.debug_bounds("badge").expect("the badge");
    cx.simulate_click(badge.center(), Modifiers::none());
    assert_eq!(
        texts(&outline(&view, cx)),
        ["add", r#"<badge {"label":"0 left"}>"#]
    );
}

/// A plugin that traps is drawn as what it failed with, and is left alone.
#[gpui::test]
fn a_plugin_that_fails_is_drawn_as_its_error(cx: &mut TestAppContext) {
    let runtime = Rc::new(Runtime::new().unwrap());
    let component = runtime.load(example("todo")).unwrap();
    let instance = runtime
        .instantiate(&component, WasiCtxBuilder::new().build(), Box::new(()))
        .unwrap();
    let window = cx.add_window(|_, cx| PluginView::new(&runtime, instance, cx));
    let view = window.root(cx).unwrap();
    let cx = &mut VisualTestContext::from_window(*window.deref(), cx);
    view.update_in(cx, |view, _, cx| view.dispatch(0, "fail", &Value::Null, cx));
    let failed = view.update(cx, |view, _| view.outline());
    assert_eq!(failed.len(), 1);
    assert!(failed[0].1.contains("unreachable"), "{}", failed[0].1);
    view.update_in(cx, |view, _, cx| {
        view.dispatch(0, "add", &Value::from("late"), cx)
    });
    assert_eq!(view.update(cx, |view, _| view.outline()), failed);
}

/// Times a list of 100 rows: `cargo test --release -- --ignored --nocapture`.
#[gpui::test]
#[ignore = "a measurement, not a check"]
fn measure_a_hundred_rows(cx: &mut TestAppContext) {
    use std::time::Instant;

    let data = std::env::temp_dir().join(format!("gpui-wit-plugin-bench-{}", std::process::id()));
    std::fs::create_dir_all(&data).unwrap();
    let rows: String = (0..100).map(|row| format!("row {row}\n")).collect();
    std::fs::write(data.join("todos.txt"), rows).unwrap();
    let runtime = Rc::new(Runtime::new().unwrap());
    let component = runtime.load(example("todo")).unwrap();
    let cx = &mut VisualTestContext::from_window(*cx.add_window(|_, _| gpui::Empty).deref(), cx);

    // What a change costs, apart: the plugin's own call, applying what it
    // answered, and drawing the view again.
    let (mut inits, mut first_applies, mut calls, mut applies, mut draws) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for _ in 0..30 {
        let mut wasi = WasiCtxBuilder::new();
        wasi.env("TODO_FILE", "/data/todos.txt")
            .preopened_dir(&data, "/data", DirPerms::READ, FilePerms::READ)
            .unwrap();
        let instance = (runtime.instantiate(&component, wasi.build(), Box::new(()))).unwrap();
        // The same two steps `PluginView::new` takes, timed apart.
        let crate::Instance { mut store, plugin } = instance;
        let started = Instant::now();
        let batch = plugin.call_init(&mut store).unwrap();
        inits.push(started.elapsed());
        let instance = crate::Instance { store, plugin };
        let started = Instant::now();
        let view = cx.new(|cx| {
            let mut view = PluginView::unstarted(&runtime, instance, cx);
            view.apply(batch, cx);
            view
        });
        first_applies.push(started.elapsed());
        let window = cx.add_window(|_, _| Holder(view.clone()));
        let cx = &mut VisualTestContext::from_window(*window.deref(), cx);
        cx.run_until_parked();
        for round in 0..20 {
            let label = if round % 2 == 0 {
                "[ ] row 50"
            } else {
                "[x] row 50"
            };
            let started = Instant::now();
            let (call, apply) = view.update_in(cx, |view, _, cx| {
                let (id, _) = (view.nodes.iter())
                    .find(|(_, node)| {
                        (node.children.iter()).any(|child| view.nodes[child].text.as_ref() == label)
                    })
                    .expect("the row");
                let started = Instant::now();
                let batch = (view.plugin)
                    .call_event(&mut view.store, *id, "click", "null")
                    .unwrap();
                let call = started.elapsed();
                let started = Instant::now();
                view.apply(batch, cx);
                cx.notify();
                (call, started.elapsed())
            });
            cx.run_until_parked();
            draws.push(started.elapsed().saturating_sub(call + apply));
            calls.push(call);
            applies.push(apply);
        }
    }
    // The same tree drawn by a view written against GPUI directly, for what
    // drawing it costs without a plugin.
    let native = cx.new(|_| Native(false));
    let window = cx.add_window(|_, _| NativeHolder(native.clone()));
    let mut natives = Vec::new();
    {
        let cx = &mut VisualTestContext::from_window(*window.deref(), cx);
        cx.run_until_parked();
        for _ in 0..600 {
            let started = Instant::now();
            native.update(cx, |native, cx| {
                native.0 = !native.0;
                cx.notify();
            });
            cx.run_until_parked();
            natives.push(started.elapsed());
        }
    }
    for (name, mut times) in [
        ("native draw", natives),
        ("init call", inits),
        ("init apply", first_applies),
        ("call", calls),
        ("apply", applies),
        ("rest (find, draw)", draws),
    ] {
        times.sort();
        let ms = |time: std::time::Duration| time.as_secs_f64() * 1000.;
        println!(
            "{name}: best {:.3} ms, median {:.3} ms",
            ms(times[0]),
            ms(times[times.len() / 2])
        );
    }
}

/// A window's root that draws a plugin's view.
struct Holder(Entity<PluginView>);

impl gpui::Render for Holder {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        self.0.clone()
    }
}

/// The to-do example's tree with 100 rows, built from GPUI's own elements.
struct Native(bool);

impl gpui::Render for Native {
    fn render(&mut self, _: &mut gpui::Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        use gpui::Styled as _;
        let toggled = self.0;
        div()
            .id("root")
            .flex()
            .flex_col()
            .size_full()
            .p(px(8.))
            .gap(px(4.))
            .bg(gpui::white())
            .text_color(gpui::black())
            .overflow_y_scroll()
            .on_key_down(cx.listener(|_, _: &gpui::KeyDownEvent, _, _| {}))
            .child(
                div()
                    .id("add")
                    .cursor_pointer()
                    .on_click(cx.listener(|_, _, _, _| {}))
                    .child("add"),
            )
            .child(div().id("badge").child("100 left"))
            .children((0..100usize).map(|row| {
                let mark = if toggled && row == 50 { "[x]" } else { "[ ]" };
                div()
                    .id(row * 3)
                    .flex()
                    .flex_row()
                    .gap(px(8.))
                    .child(
                        div()
                            .id(row * 3 + 1)
                            .flex_1()
                            .cursor_pointer()
                            .on_click(cx.listener(|_, _, _, _| {}))
                            .child(format!("{mark} row {row}")),
                    )
                    .child(
                        div()
                            .id(row * 3 + 2)
                            .cursor_pointer()
                            .on_click(cx.listener(|_, _, _, _| {}))
                            .child("remove"),
                    )
            }))
    }
}

struct NativeHolder(Entity<Native>);

impl gpui::Render for NativeHolder {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        self.0.clone()
    }
}
