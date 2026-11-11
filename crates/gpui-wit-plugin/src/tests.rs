use std::{ops::Deref, path::PathBuf, process::Command, rc::Rc};

use gpui::{
    Entity, InteractiveElement, IntoElement, Modifiers, ParentElement, SharedString,
    StatefulInteractiveElement, TestAppContext, VisualTestContext, div, point, px,
};
use serde_json::Value;
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtxBuilder};

use super::{Kind, PluginView, Runtime};

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
    fn walk(view: &PluginView, id: u64, lines: &mut Vec<String>) {
        let node = &view.nodes[&id];
        match &node.kind {
            Kind::Text => lines.push(format!("{id} {}", node.text)),
            Kind::Component(name) => lines.push(format!("{id} <{name} {}>", node.props)),
            Kind::Element => {}
        }
        for child in &node.children {
            walk(view, *child, lines);
        }
    }
    view.update(cx, |view, _| {
        assert_eq!(view.error, None);
        let mut lines = Vec::new();
        walk(view, view.root.expect("a root"), &mut lines);
        lines
    })
}

/// Clicks the element whose text is `label`, as the element would report it.
fn click(view: &Entity<PluginView>, label: &str, cx: &mut VisualTestContext) {
    view.update_in(cx, |view, window, cx| {
        let (id, _) = (view.nodes.iter())
            .find(|(_, node)| {
                (node.children.iter()).any(|child| view.nodes[child].text.as_ref() == label)
            })
            .expect("the element");
        view.dispatch(*id, "click", &Value::Null, window, cx);
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
            .on_click(move |_, window, cx| emit.emit("pressed", Value::Null, window, cx))
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
    let window = cx.add_window(|window, cx| PluginView::new(&runtime, instance, window, cx));
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
    view.update_in(cx, |view, window, cx| {
        let remove = ids(&added)[3].parse::<u64>().unwrap();
        let (id, _) = (view.nodes.iter())
            .find(|(_, node)| node.children.contains(&remove))
            .unwrap();
        view.dispatch(*id, "click", &Value::Null, window, cx);
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
    view.update_in(cx, |view, window, cx| {
        view.dispatch(0, "add", &Value::from("told"), window, cx);
    });
    assert_eq!(texts(&outline(&view, cx)).last().unwrap(), "remove");
    assert!(texts(&outline(&view, cx)).contains(&"[ ] told".to_owned()));
    assert_eq!(returned.get(), 1);

    // An event of a registered component, from a real click on it.
    let badge = cx.debug_bounds("badge").expect("the badge");
    cx.simulate_click(badge.center(), Modifiers::none());
    assert_eq!(
        texts(&outline(&view, cx)),
        ["add", r#"<badge {"label":"0 left"}>"#]
    );
}
