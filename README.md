# gpui-wit-plugin

Draws, with GPUI, views that WASI preview 2 components describe.

A plugin is a component exporting the `plugin` world of `wit/plugin.wit`. It
numbers the nodes of a tree itself and returns, from each call, the operations
that change that tree. The host keeps the tree and turns it into GPUI elements
when it renders.

- `crates/gpui-wit-plugin` — the host. Depends on `gpui`, `wasmtime` and
  `wasmtime-wasi` only. A node is a `div`, text, or a component the embedding
  application registered by name; a node's style is GPUI's own
  `StyleRefinement` as JSON, read by GPUI's own `Deserialize`. Component
  libraries (gpui-kit and the like) are layers on top: they register
  components, the host knows none.
- `crates/gpui-wit-plugin-sdk` — writes a plugin the way a GPUI view is
  written: `render` returns the whole tree, `cx.notify()` renders again,
  `cx.new` makes child views that render on their own, `cx.emit` /
  `cx.subscribe` connect them. Only the difference between two renders
  crosses to the host.
- `examples/todo` — a plugin using all of the above, and what the host's test
  loads.

What a plugin reaches beyond drawing is the embedding application's choice:
the WASI context it gives each plugin (preopened directories, environment)
and the interfaces it adds to the linker.

## Build

```sh
rustup target add wasm32-wasip2
cargo test --workspace        # builds examples/todo for wasm32-wasip2 itself
```

## What it costs

`cargo test --release -p gpui-wit-plugin -- --ignored --nocapture` times
the to-do example with 100 rows (about 500 nodes) on GPUI's test platform.
One run on the machine this was written on, median of each:

| | ms |
|---|---|
| The plugin's first call (reads its file, renders every row) | 1.0 |
| Applying what that call answered | 0.6 |
| A click on one row: the plugin's call | 0.02 |
| Applying what that call answered | 0.001 |
| Drawing the view again after it | 1.2 |
| Drawing the same tree from a view written against GPUI directly | 1.2 |

A change costs what it costs a GPUI view of the same size: the call across
the boundary is small beside the draw. A first render of the same shape
run natively, with no boundary to cross, takes 0.6 ms
(`cargo test --release -p gpui-wit-plugin-sdk -- --ignored --nocapture`),
0.2 ms of it building the elements, whose styles are JSON maps.

## Not there yet

- Plugins run on the main thread, with no limit on how long a call takes.
- The host draws a plugin as one GPUI view: any change draws the whole tree
  again, as it does for a GPUI view that is not split into cached child
  views. The plugin side already renders and sends only the views that
  changed.
- An element's events are `click`, `mouse_down`, `hover` and `key_down`;
  anything else is an event of a registered component. There is no drag
  and drop, no custom painting, and no list that draws only the rows in
  view.
