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

Phases 1 and 2 are done.

Phase 1 (393c5a197) added:

- `sources/ui_shape.buri`: `Segment` (`MoveTo`, `LineTo`, `Close`), `Shape<C>` with its reader `svg(ctx)`, and `path`. Every `Color` lowers, tokens included (`var(--ns-name)`, and `color-mix(…)` for `Faded`).
- `ui_node.buri`: `drawing`, `NodeKind::Drawing` (tag 25), and the `drawn` helper both `emitNode` and `nodeLines` use.
- `runtime.js`: `$tree_drawing` and `$tree_shape`, behind `$tree_drawing_hook`. Attributes are set with `setAttribute`, and `fill`/`stroke` as style properties. Each prop goes through `$tree_bind`, so the browser updates `.Cell` and `.Computed` props.
- `cli/runtime/image.rs`: the painter paints a colour's own alpha (`rgba()`, `#rgba`, `#rrggbbaa`, `rgb(… / a)`). c66aea4d7 added its UI case, `an_icon_paints_a_colour_s_own_alpha`. No other paint path had the bug: the scene's colours are all `rgba(r,g,b,a)` written by the compiler.

Phase 2 added:

- `emitNode`'s `.Drawing` arm opens the `svg` once and registers one watcher, which re-reads the artwork and the styles and patches the body in place. The element keeps its identity. `describe` still reads once, as it does for every node.
- `paint.rs`'s `raster` puts the themes' values in place of every `var()` in an `icon:` artwork before painting. `image.rs`'s `ink` reads `color-mix(in srgb,<colour> <n>%,transparent)`, nested up to `MIX_DEPTH`, which is what `.Faded` becomes.
- Tests:
  - conformance: `tree.buri` "a drawing follows its size, segments, paint and styles, and stays the same element"
  - `language::pages::a_drawing_follows_its_size_outline_and_paint_in_place`
  - UI case `a_drawing_paints_tokens_light_and_dark`: each token picture shares a golden with the same card in its theme's colours
  - `image::tests::a_colour_mixed_with_transparent_is_that_colour_faded`
- Docs: the guide's drawing paragraph says props update in place and tokens follow the theme.

Phase 3 starts here:

1. `shape.rect`, `shape.circle` and `shape.text` lower in `ui_shape.buri` the way `path` does. `image.rs` paints `rect` and `circle` but not `text`, so a text run needs the painter to draw SVG `text`, or `drawn` to lower it to something it can draw.
2. `shape.group` with `offset` lowers to `<g transform='translate(…)'>`, which `image.rs` reads. `Shape` has no children yet (its fields are the element, its attributes and its paint), so a group needs a children field, `svg(ctx)` must write them, and `$tree_shape` must build them.
3. Keyed `shape.each` reconciles like `ui.each` in the browser. The native watcher re-serialises the whole artwork on any change, which is enough for `render` and snapshots. Add `each` to `semantics::reactive`'s builders.
4. A token bound to `.Inherit` puts `inherit` in the artwork, which `image.rs` paints as nothing. Nothing does that yet.
