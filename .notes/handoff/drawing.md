# Drawings: `ui.drawing` and `ui/shape` (issue #332)

The approved interface is the last comment on #332: four examples (sparkline, themed area chart, keyed bars, pointer marker). Build only what they use. Gradients, clip paths, measuring, canvas and arcs stay out.

## How it fits together

- `ui/shape` (`sources/ui_shape.buri`) owns `Segment`, the opaque `Shape<C>` and its constructors. `ui/node` imports it; it never imports `ui/node`.
- A shape is stored already lowered: an SVG element name, its attributes as `Prop<Str>`, and its children. Every rule (segments to `d`, a colour to a paint, an offset to a `transform`) runs in Buri when the shape is built, so every back end draws the same thing.
- `Shape.svg(scope)` writes a shape out as SVG markup. It's the one reader `ui/node` needs, the way `Classes.pairs()` is for `ui/style`.
- `NodeKind::Drawing` is appended last (tag 25). The browser (`runtime.js` `$tree_drawing`, behind `$tree_drawing_hook` and `Program::drawings`) builds real SVG elements and binds each attribute prop. `describe` and the native `render` document write one `svg` element carrying `icon:<svg …>`, which `cli/runtime/image.rs` already paints.
- `preserveAspectRatio='none'`: the drawing stretches to the box its styles give it. That's also what the painter does with any SVG.

## Phases

Each phase ends at a commit landed with `/tmp/buri-land.sh`.

1. **Drawing and static path.** `ui.drawing({ alt, size, styles, shapes })`, `shape.path({ segments, fill, stroke, strokeWidth })`, and `Segment` `MoveTo`/`LineTo`/`Close`. Colours: `Rgb`, `Rgba`, `Inherit` (`currentColor`), `Transparent`. Renders in the browser, in `render` (both back ends) and in snapshots. A UI snapshot case, happy-dom page tests, conformance tests, docs. About 250 turns.
2. **Reactivity and tokens.** `.Cell` and `.Computed` segments, size and colours update in place: the browser already binds each prop, and the native `render` document needs a watcher that patches the drawing's body. `style.token` and `alpha` (`Token`, `Faded`) as fill and stroke, following the theme in the browser and in the painter, light and dark. About 250 turns.
3. **More shapes.** `shape.rect`, `shape.circle`, `shape.text` (a run of text in the drawing), `shape.group` with `offset`, and keyed `shape.each` (reconciled like `ui.each` in the browser and in `render`; add it to `semantics::reactive`'s builders). About 300 turns.
4. **Interaction.** A shape with a `label` joins the focus order with that name; `onFocus` on shapes; `onPointerMove` on the drawing, in drawing units. Both in the browser and in `platform/effect/testing`'s `render`. Close #332. About 250 turns.

## Status

Phase 1 is done. What landed:

- `sources/ui_shape.buri`: `Segment` (`MoveTo`, `LineTo`, `Close`), `Shape<C>` with its reader `svg(ctx)`, and `path`. Every `Color` already lowers, tokens included (`var(--ns-name)`, and `color-mix(…)` for `Faded`).
- `ui_node.buri`: `drawing`, `NodeKind::Drawing` (tag 25), and the `drawn` helper both `emitNode` and `nodeLines` use.
- `runtime.js`: `$tree_drawing` and `$tree_shape`, behind `$tree_drawing_hook`. Attributes are set with `setAttribute`, and `fill`/`stroke` as style properties. Each prop goes through `$tree_bind`, so the browser already updates `.Cell` and `.Computed` props.
- `cli/runtime/image.rs`: the painter now paints a colour's own alpha (`rgba()`, `#rgba`, `#rrggbbaa`, `rgb(… / a)`). Before this it dropped it.
- Tests:
  - conformance: `tree.buri` "drawings" section and `render.buri` "a drawing and its paths"
  - `language::pages::a_drawing_is_svg_its_paths_are_painted_and_its_alt_names_it`
  - UI case `a_drawing_paints_its_paths`
  - `image::tests::a_colour_s_own_alpha_is_painted`
- Docs: the guide's drawing paragraph in `user-interfaces.md`, and the `ui/shape` link in the reference.

Phase 2 starts here:

1. The native `render` document reads a drawing's props once. `emitNode`'s `.Drawing` arm needs a watcher that rewrites the `icon` declaration when `size`, segments or paint change, following how other reactive leaves in `emitNode` register one.
2. Write e2e tests for updates in `tree.buri`, in the browser (`pages.rs`), and in `render`.
3. The painter has no `var()`, so a token fill paints nothing in a snapshot. Make `drawn` resolve tokens against the theme for the scene, or teach `image.rs` the scene's token values. Then add a light and a dark UI case.
