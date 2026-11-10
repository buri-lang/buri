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
4. **Text.** `shape.text`, and a token bound to `.Inherit`.
5. **Interaction.** A shape with a `label` joins the focus order with that name; `onFocus` on shapes; `onPointerMove` on the drawing, in drawing units. Both in the browser and in `platform/effect/testing`'s `render`. About 250 turns.
6. **The `each` gaps** listed under phase 3, then close #332.

## Status

Phases 1 to 5 are done.

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
- Docs: the guide's drawing paragraph says props update in place.

Phase 3 added:

- `ui_shape.buri`: `rect`, `circle`, `group` (with `offset`, as `translate(x y)`) and keyed `each`. `Shape` gained its children and an `each`'s rows (`Rows`: a key and a row per index). `built(ctx, c)` puts an `each`'s rows where it was, and `keys(ctx)` reads every key so a watcher can follow the lists.
- `ui_node.buri`: `.Drawing` holds the artwork as a builder over `C` plus its keys. A watcher can't capture a `C`, so `emitNode` rebuilds the drawing's region whenever the keys change (the runtime hands the region its `C`), and a nested watcher patches the markup.
- `runtime.js`: `$tree_row`, `$tree_reconcile` and `$tree_each` take the `render` function, so `$tree_shape` reconciles an `each` with the same code as `ui.each`.
- `semantics::reactive`: `shape.each` is a builder; `load-in-reactive-builder` names it.
- A group with no offset adds no element, in the browser and in the scene. `drawing` builds its rows and keys through one such group, which keeps `ui_node` free of list walks of its own.
- The check-cost goals rows are tight: every line in `ui_node`/`ui_shape` is checked for every UI program. To pay for this phase, `styles::Flow::edges` walks an expression's children without collecting them first (about 2% of check in a small UI program). Measure stdlib growth with `BURI_PROFILE=1 buri build` on a mounted page before running the goals check.
- Tests: four conformance tests in `tree.buri` (rect/circle, group, each, a row's own props), `language::pages::a_drawing_s_keyed_shapes_move_with_their_items_and_keep_their_elements`, UI case `a_drawing_paints_bars_and_dots`, reject case `shape_each_row_loads`.
- Docs: the guide's chart example.

Known gaps:

- Native `render` rebuilds every row when the keys change; the browser keeps rows whose keys stay.
- A row's index is the one it was built with, so it goes stale after a reorder in the browser (as with `ui.each`).
- An `each` inside another `each`'s row isn't followed natively: `keys` reads only the rows built at the top level.
- Duplicate keys abort in the browser but not natively (as with `ui.each`).

Phase 4 added:

- `shape.text({ at, content })`: `Shape`'s sixth field holds the words. `svg` escapes them; `runtime.js` binds `shape[5]` as a text node. The text is centred on `x` (`text-anchor='middle'`), `font-size='5'` drawing units, `fill='currentColor'`.
- `image.rs` paints `text` as one run (no `tspan`): `paint.rs`'s `lettering` shapes it with `cosmic_text` and fills each glyph's `SwashCache` outline with `tiny_skia`. Space is collapsed the way a page does it. Image parsing takes `&mut Fonts`, because `paint_with` already borrows `FACES`. This also draws text in a `data:` SVG image.
- A token's paint is `var(--ns-name,currentColor)`. A token bound to `.Inherit` puts `inherit` at `:root`, which a page treats as unset, so the fallback applies. `parse_variables` drops `inherit` bindings and any binding whose `var()` chain doesn't resolve, so the painter agrees: a drawing paints in the text's colour and a `.Background` stays clear.
- Tests: conformance `tree.buri` "a text is centred on its point, …", `language::pages::a_drawing_s_text_is_centred_on_its_point_and_follows_its_words_in_place`, UI cases `a_drawing_paints_its_labels` and `a_token_bound_to_inherit_paints_as_inherit`.
- Docs: the guide's text example, and the snapshot list says SVG text paints.

Phase 5 added:

- `shape.rect` takes `label` and `onFocus`. `Rect` is now `Rect<C>`. Path and circle stay label-free: their configs aren't generic, and making them generic breaks `let area = shape.path({…})` (`untyped-struct-literal`), as the tree tests showed.
- `Shape`'s seventh field is `Option<Stop<C>>`: the label and the `onFocus`. `svg` never writes it, so a label is never in the painted artwork. `labels(ctx)` reads every labelled shape's element name and label.
- `runtime.js` `$tree_shape`: a stop sets `tabindex='0'`, `role='img'`, binds `aria-label`, and wires `onFocus` through `$tree_events` (`focusin`/`focusout`).
- `ui.drawing` takes `onPointerMove`. `$tree_drawing` scales `x`/`y` by the `viewBox` over `getBoundingClientRect()`; viewport coordinates and `overRow` pass through. `render` has no layout, so the box spans the units and `x` is the `x` a test moves to.
- Native `render`: `.Drawing`'s artwork closure now answers `(markup, stops)`. `stops(builder)` (`ui_node` `standIn`, built in `drawing`, so only programs that draw compile it) labels the `svg` with its `alt` and opens an empty element per labelled shape inside the drawing's region, with `relabel` and `attachFocus`. So `tab`, `focus` and `focused` reach the bars, and `pointerMove("<alt>", …)` reaches the drawing. They show in `markup()` as `e 1 ` lines.
- `onFocus` is the browser's alone, like every node's `onFocus` (`registerEvents` doc).
- Tests: conformance `tree.buri` "a labelled shape is a stop in the focus order, named by its label" and "a drawing hears the pointer in its own units"; `language::pages::a_drawing_s_labelled_shapes_take_the_focus_and_are_read_by_their_label` and `a_drawing_hears_the_pointer_in_its_own_units`. `harness::browser`'s `fire` knows `pointermove`.
- Docs: the guide's labelled-bar and pointer examples.

More known gaps:

- Native stops are rebuilt with the rows when the keys change, so the focus on a bar is lost when its list changes.

Phase 6 starts here:

1. The `each` gaps under phase 3's known gaps: keep native rows whose keys stay, a row's stale index after a reorder, nested `each` keys natively, duplicate keys aborting natively. Native stops ride on the same rebuild, so keeping rows should keep them too.
2. Then close #332.
