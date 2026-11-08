//! The monorepo's design system: a theme package, component packages built
//! on it and on each other, and a JavaScript test suite each.
//!
//! What real large workspaces' interfaces have that a few styled pages lack:
//!
//! * **Long style lists**: groups shared from the theme, `.At` breakpoints and
//!   `.On` states nested in them, token colours and spacing steps that fold
//!   at compile time, and styles a caller hands in.
//! * **Components over components**: each package draws on a few before it,
//!   and every web page puts several together. Style extraction follows each
//!   style to the node it reaches through all of them.
//! * **Suites that render**: each package's tests declare `backends: [JS]`,
//!   so each suite is a compilation of its own, as UI suites are.

use super::{quoted, File, Monorepo, Rng, WORDS};

/// A component a page or a later package can put on screen.
#[derive(Clone)]
pub struct Component {
    /// Its package's index.
    pub package: usize,
    pub kind: Kind,
    pub name: String,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    /// A titled surface around children, given its package's props.
    Panel,
    /// A short label in a tone.
    Badge,
    /// A button that runs a handler.
    Action,
    /// A badge per item.
    List,
}

pub const KINDS: [Kind; 4] = [Kind::Panel, Kind::Badge, Kind::Action, Kind::List];

const TONES: [&str; 10] = [
    "Surface", "Raised", "Sunken", "Ink", "Muted", "Accent", "Border", "Danger", "Success",
    "Warning",
];

pub fn package(k: usize) -> String {
    format!("//design/d{k:02}")
}

/// The theme every component is painted from.
fn theme(out: &mut Vec<File>) {
    let mut s = String::from(
        "//! The design system's tokens and the style groups every component shares.\n\n\
         from \"ui/style\" import { Color, Length, Style, token };\n\n\
         /// The colours a component is painted in, named by what they are for.\n\
         export enum Tone {\n",
    );
    for t in TONES {
        s.push_str(&format!("    {t},\n"));
    }
    s.push_str(
        "}\n\n\
         impl Tone {\n\
         \x20   /// The custom property the tone is, which is what a style holds.\n\
         \x20   export fn color(self): Color {\n\
         \x20       match (self) {\n",
    );
    for t in TONES {
        s.push_str(&format!(
            "            .{t} => token(\"theme\", \"{}\"),\n",
            t.to_lowercase()
        ));
    }
    s.push_str(
        "        }\n\
         \x20   }\n\
         }\n\n\
         /// `n` steps of the spacing scale.\n\
         export fn space(n: Int): Length {\n\
         \x20   .Px(4 * n)\n\
         }\n\n\
         /// Step `n` of the type scale.\n\
         export fn textSize(n: Int): Length {\n\
         \x20   .Px(12 + 2 * n)\n\
         }\n\n\
         /// A surface with a hairline border.\n\
         export fn surface(): [Style] {\n\
         \x20   [\n\
         \x20       .Background(Tone.Surface.color()),\n\
         \x20       .Foreground(Tone.Ink.color()),\n\
         \x20       .BorderWidth(.Px(1)),\n\
         \x20       .BorderColor(Tone.Border.color()),\n\
         \x20       .Radius(space(2)),\n\
         \x20   ]\n\
         }\n\n\
         /// Body text.\n\
         export fn body(): [Style] {\n\
         \x20   [.FontSize(textSize(1)), .LineHeight(1.5), .FontWeight(.Regular), .Foreground(Tone.Ink.color())]\n\
         }\n\n\
         /// What has the keyboard's attention.\n\
         export fn ring(): [Style] {\n\
         \x20   [.BorderColor(Tone.Accent.color()), .BorderWidth(.Px(2))]\n\
         }\n\n\
         /// A heading at step `n` of the type scale.\n\
         export fn heading(n: Int): [Style] {\n\
         \x20   [.FontSize(textSize(n + 2)), .FontWeight(.Semibold), .LineHeight(1.25)]\n\
         }\n",
    );
    out.push(File {
        path: "design/theme/theme.buri".into(),
        text: s,
    });
    out.push(File {
        path: "design/theme/lib.buri".into(),
        text: "//! The public surface of the theme.\n\n\
               from \"//design/theme/theme.buri\" export { body, color, heading, ring, space, surface, textSize, Tone };\n"
            .into(),
    });
    out.push(File {
        path: "design/theme/BUILD.buri".into(),
        text: "library {\n    sources: [\"theme.buri\"]\n    visibility: [\"//...\"]\n}\n".into(),
    });
}

/// How a component of `kind` is put on screen, in a context `ctx` given
/// `Ui` and an allocator, with `label` as its text.
pub fn call(c: &Component, label: &str, on_press: &str) -> String {
    let n = &c.name;
    match c.kind {
        Kind::Panel => format!(
            "{n}(Props{} {{ title: \"{label}\", tone: .Raised, children: [], styles: .None }})",
            c.package
        ),
        Kind::Badge => format!("{n}(\"{label}\", .Accent)"),
        Kind::Action => format!("{n}(\"{label}\", {on_press})"),
        Kind::List => format!("{n}(ctx, [\"{label}\", \"more\"])"),
    }
}

/// Whether a call of `c` needs a context in scope.
fn needs_ctx(c: &Component) -> bool {
    c.kind == Kind::List
}

/// One component and its test.
#[allow(clippy::too_many_arguments)]
fn component(
    body: &mut String,
    tests: &mut String,
    k: usize,
    g: usize,
    kind: Kind,
    name: &str,
    word: &str,
    inner: &[&Component],
    rng: &mut Rng,
) {
    let a = 1 + rng.below(3);
    let b = 2 + rng.below(3);
    let tone = TONES[rng.below(TONES.len() as u64) as usize];
    let label = format!("{word} {}", rng.below(100));
    match kind {
        Kind::Panel => {
            let w = 480 + 40 * rng.below(8);
            let mut inner_calls = String::new();
            for c in inner.iter().filter(|c| !needs_ctx(c)) {
                // A panel inside a panel is handed what this one was.
                let made = match c.kind {
                    Kind::Panel => format!(
                        "{}(Props{} {{ title: props.title, tone: props.tone, children: props.children, styles: props.styles }})",
                        c.name, c.package
                    ),
                    _ => call(c, word, "fn(_c) => ()"),
                };
                inner_calls.push_str(&format!("        {made},\n"));
            }
            body.push_str(&format!(
                "/// A {word} panel: a titled surface around what it is given.\n\
                 export fn {name}<C>(props: Props{k}<C>): Node<C> {{\n\
                 \x20   let held: [Node<C>] = [\n\
                 {inner_calls}\
                 \x20       ui.text({{ content: .Const(\"{word}\"), styles: .Some([.Group(body()), .Foreground(props.tone.color())]) }}),\n\
                 \x20       ui.stack({{ styles: [.Layout(.Column), .Gap(space(1))], children: props.children }}),\n\
                 \x20   ];\n\
                 \x20   ui.stack({{\n\
                 \x20       styles: [\n\
                 \x20           .Group(shell{k}x{g}(frame{k}x{g})),\n\
                 \x20           .Layout(.Column),\n\
                 \x20           .Gap(space({a})),\n\
                 \x20           .Padding(space({b})),\n\
                 \x20           .At(.Medium, [.Padding(space({})), .Gap(space({}))]),\n\
                 \x20           .At(.Large, [.Layout(.Row), .MaxWidth(.Px({w})), .On(.Hover, [.Group(ring())])]),\n\
                 \x20           .On(.Hover, [.Background(Tone.{tone}.color()), .Group(ring())]),\n\
                 \x20           .On(.FocusWithin, ring()),\n\
                 \x20           .Group(props.styles.withDefault([])),\n\
                 \x20       ],\n\
                 \x20       children: [section{k}x{g}([.Gap(space({a}))], title{k}x{g}, props.title, held)],\n\
                 \x20   }})\n\
                 }}\n\n",
                b + 1,
                a + 1,
            ));
            tests.push_str(&format!(
                "test \"{name}: a {word} panel shows its title above what it holds\" {{\n\
                 \x20   let ctx = context {{ Allocator: alloc(), Ui: headless() }};\n\
                 \x20   let page = render(\n\
                 \x20       ctx,\n\
                 \x20       {name}(Props{k} {{\n\
                 \x20           title: \"{label}\",\n\
                 \x20           tone: .{tone},\n\
                 \x20           children: [ui.text({{ content: .Const(\"held\") }})],\n\
                 \x20           styles: .Some([.MinWidth(.Px({}))]),\n\
                 \x20       }}),\n\
                 \x20   );\n\
                 \x20   assert.isTrue(page.text().startsWith(\"{label}\"));\n\
                 \x20   assert.containsText(page.text(), \"held\");\n\
                 }}\n\n",
                200 + rng.below(100)
            ));
        }
        Kind::Badge => {
            let s = rng.below(3);
            body.push_str(&format!(
                "/// A {word} badge in `tone`.\n\
                 export fn {name}<C>(label: Str, tone: Tone): Node<C> {{\n\
                 \x20   ui.text({{\n\
                 \x20       content: .Const(label),\n\
                 \x20       styles: .Some([\n\
                 \x20           .Group(body()),\n\
                 \x20           .FontSize(textSize({s})),\n\
                 \x20           .PaddingX(space({b})),\n\
                 \x20           .PaddingY(space(1)),\n\
                 \x20           .Radius(.Px(999)),\n\
                 \x20           .Background(tone.color()),\n\
                 \x20           .At(.Small, [.FontSize(textSize(0)), .PaddingX(space({a}))]),\n\
                 \x20       ]),\n\
                 \x20   }})\n\
                 }}\n\n"
            ));
            tests.push_str(&format!(
                "test \"{name}: a {word} badge reads as its label\" {{\n\
                 \x20   let ctx = context {{ Allocator: alloc(), Ui: headless() }};\n\
                 \x20   let page = render(ctx, {name}(\"{label}\", .{tone}));\n\
                 \x20   assert.equal(page.text(), \"{label}\");\n\
                 }}\n\n"
            ));
        }
        Kind::Action => {
            body.push_str(&format!(
                "/// A button that does something to a {word}.\n\
                 export fn {name}<C>(label: Str, onPress: fn(C) => ()): Node<C> {{\n\
                 \x20   ui.button({{\n\
                 \x20       label: .Const(label),\n\
                 \x20       styles: [\n\
                 \x20           .Group(body()),\n\
                 \x20           .Padding(space({a})),\n\
                 \x20           .Background(Tone.Accent.color()),\n\
                 \x20           .Foreground(Tone.Surface.color()),\n\
                 \x20           .Radius(space(1)),\n\
                 \x20           .On(.Hover, [.Background(Tone.{tone}.color())]),\n\
                 \x20           .On(.Focus, ring()),\n\
                 \x20           .On(.Disabled, [.Opacity(0.5)]),\n\
                 \x20           .At(.Medium, [.PaddingX(space({b}))]),\n\
                 \x20       ],\n\
                 \x20       onPress: .Some(onPress),\n\
                 \x20   }})\n\
                 }}\n\n"
            ));
            tests.push_str(&format!(
                "test \"{name}: pressing the {word} button runs its handler once\" {{\n\
                 \x20   let ctx = context {{ Allocator: alloc(), Ui: headless(), Watch: observer() }};\n\
                 \x20   let presses = signal(ctx, 0);\n\
                 \x20   let page = render(ctx, {name}(\"{label}\", fn(c) => presses.update(c, fn(n) => n + 1)));\n\
                 \x20   page.press(\"{label}\");\n\
                 \x20   assert.equal(presses.get(ctx), 1);\n\
                 }}\n\n"
            ));
        }
        Kind::List => {
            let badge = inner
                .iter()
                .find(|c| c.kind == Kind::Badge)
                .map(|c| c.name.clone());
            let item = match badge {
                Some(badge) => format!("{badge}(item, .Muted)"),
                None => "ui.text({ content: .Const(item), styles: .Some(body()) })".to_string(),
            };
            body.push_str(&format!(
                "/// The {word}s, one line each.\n\
                 export fn {name}<C: Allocator>(ctx: C, items: [Str]): Node<C> {{\n\
                 \x20   ui.stack({{\n\
                 \x20       styles: [\n\
                 \x20           .Layout(.Column),\n\
                 \x20           .Gap(space({a})),\n\
                 \x20           .At(.Medium, [.Layout(.Row), .Wrap(true), .Gap(space({b}))]),\n\
                 \x20       ],\n\
                 \x20       children: items.map(ctx, fn(item) => {item}),\n\
                 \x20   }})\n\
                 }}\n\n"
            ));
            tests.push_str(&format!(
                "test \"{name}: every {word} is listed in order\" {{\n\
                 \x20   let ctx = context {{ Allocator: alloc(), Ui: headless() }};\n\
                 \x20   let page = render(ctx, {name}(ctx, [\"first\", \"{label}\"]));\n\
                 \x20   assert.equal(page.text(), \"first {label}\");\n\
                 }}\n\n"
            ));
        }
    }
}

/// What a module's panels are built from: style constants, a function that
/// adds the caller's styles to the theme's surface, and a titled block given
/// its styles as arguments.
fn helpers(k: usize, g: usize, word: &str, rng: &mut Rng) -> String {
    let n = format!("{k}x{g}");
    let max = 320 + 32 * rng.below(8);
    let r = 2 + rng.below(3);
    format!(
        "/// How wide a {word} panel gets: most of a phone, a fixed width above.\n\
         let frame{n}: [Style] = [.MaxWidth(.Percent(92.0)), .At(.Small, [.MaxWidth(.Px({max}))])];\n\n\
         /// A {word} panel's heading.\n\
         let title{n}: [Style] = [.FontSize(textSize(1)), .LineHeight(1.0), .FontWeight(.Medium)];\n\n\
         /// The theme's surface, with `styles` saying how wide it gets.\n\
         fn shell{n}(styles: [Style]): [Style] {{\n\
         \x20   [.Group(surface()), .Width(.Full), .Radius(space({r})), .Clip(false), .Group(styles)]\n\
         }}\n\n\
         /// A heading over `children`: `styles` is the block's spacing and\n\
         /// `titleStyles` the size the heading is set at.\n\
         fn section{n}<C>(styles: [Style], titleStyles: [Style], title: Str, children: [Node<C>]): Node<C> {{\n\
         \x20   ui.stack({{\n\
         \x20       styles: [.Layout(.Column), .Gap(space(2)), .Group(styles)],\n\
         \x20       children: [\n\
         \x20           ui.text({{ content: .Const(title), headingLevel: .Some(2), styles: .Some(titleStyles) }}),\n\
         \x20           ui.stack({{ styles: [.Layout(.Column), .Gap(space(1))], children: children }}),\n\
         \x20       ],\n\
         \x20   }})\n\
         }}\n\n"
    )
}

/// The dependencies of component package `k`: up to three of the eight
/// before it.
fn dependencies(rng: &mut Rng, k: usize) -> Vec<usize> {
    let mut deps = Vec::new();
    let want = k.min(1 + rng.below(3) as usize);
    let window = k.min(8) as u64;
    while deps.len() < want {
        let d = k - 1 - rng.below(window) as usize;
        if !deps.contains(&d) {
            deps.push(d);
        }
    }
    deps.sort_unstable();
    deps
}

/// Writes the theme and every component package; the components a page can
/// use, by package.
pub fn files(out: &mut Vec<File>, m: &Monorepo, rng: &mut Rng) -> Vec<Vec<Component>> {
    let mut all: Vec<Vec<Component>> = Vec::new();
    if m.ui_packages == 0 {
        return all;
    }
    theme(out);
    for k in 0..m.ui_packages {
        let deps = dependencies(rng, k);
        let dir = format!("design/d{k:02}");
        let mine = library(out, m, k, &dir, &deps, &all, rng);
        out.push(File {
            path: format!("{dir}/BUILD.buri"),
            text: format!(
                "library {{\n    sources: [{}]\n    dependencies: [{}]\n    visibility: [\"//...\"]\n\n    test {{\n        sources: [{}]\n        backends: [JS]\n    }}\n}}\n",
                quoted((1..=m.ui_modules).map(|g| format!("c{g}.buri"))),
                quoted(
                    std::iter::once("//design/theme".to_string())
                        .chain(deps.iter().map(|d| package(*d)))
                ),
                quoted((1..=m.ui_modules).map(|g| format!("test/c{g}_test.buri"))),
            ),
        });
        all.push(mine);
    }
    all
}

/// Component package `k`'s modules, surface and tests; its components.
fn library(
    out: &mut Vec<File>,
    m: &Monorepo,
    k: usize,
    dir: &str,
    deps: &[usize],
    all: &[Vec<Component>],
    rng: &mut Rng,
) -> Vec<Component> {
    let mut mine: Vec<Component> = Vec::new();
    let mut surface = format!("//! The public surface of design package d{k:02}.\n\n");
    for g in 0..m.ui_modules {
        let word = WORDS[(k * 7 + g) % WORDS.len()];
        let mut body = String::new();
        let mut tests = String::new();
        // What this module's panels put inside them: a component of each
        // dependency, and the last this package declared.
        let mut borrowed: Vec<Component> = deps
            .iter()
            .filter_map(|d| {
                let theirs = &all[*d];
                let i = rng.below(theirs.len() as u64) as usize;
                theirs.get(i).cloned()
            })
            .collect();
        let start = mine.len();
        for j in 0..m.components {
            // Every kind early, so the smallest monorepo writes them all.
            let kind = match KINDS.get(j) {
                Some(kind) => *kind,
                None => KINDS[rng.below(KINDS.len() as u64) as usize],
            };
            let prefix = match kind {
                Kind::Panel => "panel",
                Kind::Badge => "badge",
                Kind::Action => "action",
                Kind::List => "list",
            };
            let name = format!("{prefix}{k}x{g}x{j}");
            let mut inner: Vec<&Component> = borrowed.iter().collect();
            if let Some(last) = mine[start..].iter().rev().find(|c| c.kind != Kind::Panel) {
                inner.push(last);
            }
            if let Some(last) = mine[start..].iter().rev().find(|c| c.kind == Kind::Panel) {
                inner.push(last);
            }
            component(&mut body, &mut tests, k, g, kind, &name, word, &inner, rng);
            mine.push(Component {
                package: k,
                kind,
                name,
            });
        }
        body.insert_str(0, &helpers(k, g, word, rng));
        let declared = &mine[start..];
        let mut s = format!("//! {word} components of design package d{k:02}.\n\n");
        if body.contains("<C: Allocator>") {
            s.push_str("from \"platform/effect\" import { Allocator };\n");
        }
        s.push_str("from \"ui/node\" import * as ui;\nfrom \"ui/node\" import { Node };\n");
        s.push_str("from \"ui/style\" import { Style };\n");
        let used: Vec<&str> = [
            "body", "heading", "ring", "space", "surface", "textSize", "Tone",
        ]
        .into_iter()
        .filter(|t| body.contains(&format!("{t}(")) || (*t == "Tone" && body.contains(t)))
        .collect();
        s.push_str(&format!(
            "from \"//design/theme\" import {{ {} }};\n",
            used.join(", ")
        ));
        if g == 0 {
            body.insert_str(
                0,
                &format!(
                    "/// What a panel of this package is given.\n\
                     export struct Props{k}<C> {{\n\
                     \x20   export title: Str,\n\
                     \x20   export tone: Tone,\n\
                     \x20   export children: [Node<C>],\n\
                     \x20   export styles: Option<[Style]>,\n\
                     }}\n\n"
                ),
            );
        } else {
            s.push_str(&format!(
                "from \"{}/c1.buri\" import {{ Props{k} }};\n",
                package(k)
            ));
        }
        borrowed.sort_by_key(|c| c.package);
        for d in deps {
            let names: Vec<String> = borrowed
                .iter()
                .filter(|c| c.package == *d && body.contains(&format!("{}(", c.name)))
                .flat_map(|c| {
                    let mut v = vec![c.name.clone()];
                    if c.kind == Kind::Panel {
                        v.push(format!("Props{d}"));
                    }
                    v
                })
                .collect();
            if !names.is_empty() {
                let mut names = names;
                names.sort();
                names.dedup();
                s.push_str(&format!(
                    "from \"{}\" import {{ {} }};\n",
                    package(*d),
                    names.join(", ")
                ));
            }
        }
        s.push('\n');
        s.push_str(&body);
        out.push(File {
            path: format!("{dir}/c{}.buri", g + 1),
            text: s,
        });
        let mut exports: Vec<String> = declared.iter().map(|c| c.name.clone()).collect();
        if g == 0 {
            exports.push(format!("Props{k}"));
        }
        exports.sort();
        surface.push_str(&format!(
            "from \"{}/c{}.buri\" export {{ {} }};\n",
            package(k),
            g + 1,
            exports.join(", ")
        ));
        let mut names: Vec<String> = declared.iter().map(|c| c.name.clone()).collect();
        names.push(format!("Props{k}"));
        names.sort();
        out.push(File {
            path: format!("{dir}/test/c{}_test.buri", g + 1),
            text: format!(
                "from \"core/testing/assert\" import * as assert;\n\
                 from \"platform/effect\" import {{ Allocator, Ui, Watch }};\n\
                 from \"platform/effect/testing\" import {{ alloc, headless, observer, render }};\n\
                 from \"ui/node\" import * as ui;\n\
                 from \"ui/signal\" import {{ signal }};\n\
                 from \"{}\" import {{ {} }};\n\n{tests}",
                package(k),
                names.join(", ")
            ),
        });
    }
    out.push(File {
        path: format!("{dir}/lib.buri"),
        text: surface,
    });
    mine
}
