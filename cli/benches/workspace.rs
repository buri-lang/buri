//! The realistic workspace the throughput goals are measured on
//! (`design/PERFORMANCE.md` §6.82).
//!
//! A repository of libraries, native programs and pages, written from a fixed
//! seed, so two runs build the same bytes. Each library:
//!
//! * declares a trait, an enum and a generic record in its first module;
//! * uses that record at three types in every module, and at its own types
//!   from the libraries it depends on;
//! * fills the rest of each module with the realistic construct mix of
//!   `generate.rs`;
//! * has a test suite.
//!
//! Programs and pages each depend on a few libraries, so the libraries are
//! shared, and the pages' styles go through style extraction. Nothing large is
//! checked in: the workspace is regenerated wherever it's measured.

use super::generate::{chunk, laid_out, nonblank_lines, Params, Rng};

#[path = "monorepo.rs"]
mod monorepo;
use monorepo::Monorepo;

/// A workspace `--goals` can build: its name and its shape.
pub struct Size {
    pub name: &'static str,
    pub shape: Shape,
}

pub enum Shape {
    /// This file's library-heavy workspace.
    Libraries(Libraries),
    /// `monorepo.rs`'s application monorepo.
    Monorepo(Monorepo),
}

impl Size {
    /// Whether `--goals` also measures what `buri test //...` builds.
    pub fn tests(&self) -> bool {
        matches!(self.shape, Shape::Monorepo(_))
    }
}

/// How large a library workspace to write.
pub struct Libraries {
    pub libraries: usize,
    pub modules: usize,
    /// Non-blank lines a module is filled to.
    pub module_lines: usize,
    pub programs: usize,
    pub pages: usize,
    /// Libraries each program and page depends on.
    pub uses: usize,
}

/// The library workspace `--goals` measured first: about 260,000 lines with
/// the stdlib.
pub const FULL: Size = Size {
    name: "full",
    shape: Shape::Libraries(Libraries {
        libraries: 48,
        modules: 10,
        module_lines: 560,
        programs: 12,
        pages: 4,
        uses: 4,
    }),
};

/// What CI measures of it: Valgrind runs a build tens of times slower, and
/// the full size would take most of an hour there.
pub const CI: Size = Size {
    name: "ci",
    shape: Shape::Libraries(Libraries {
        libraries: 8,
        modules: 6,
        module_lines: 300,
        programs: 3,
        pages: 1,
        uses: 3,
    }),
};

/// The smallest library workspace with every shape in it, for the suite.
pub const TINY: Size = Size {
    name: "tiny",
    shape: Shape::Libraries(Libraries {
        libraries: 3,
        modules: 2,
        module_lines: 80,
        programs: 1,
        pages: 1,
        uses: 2,
    }),
};

/// What `--goals` measures by default: an application monorepo of about
/// 400,000 lines with the stdlib, shaped like a large real one.
pub const MONOREPO: Size = Size {
    name: "monorepo",
    shape: Shape::Monorepo(Monorepo {
        packages: 150,
        modules: 4,
        module_lines: 300,
        programs: 30,
        scripts: 8,
        pages: 12,
        fixtures: 12,
        protos: 8,
        messages: 16,
        calls: 1,
        ui_packages: 44,
        ui_modules: 3,
        components: 6,
        machines: 5,
    }),
};

/// What CI measures of it.
pub const MONOREPO_CI: Size = Size {
    name: "monorepo-ci",
    shape: Shape::Monorepo(Monorepo {
        packages: 16,
        modules: 3,
        module_lines: 200,
        programs: 2,
        scripts: 1,
        pages: 1,
        fixtures: 8,
        protos: 8,
        messages: 8,
        calls: 1,
        ui_packages: 6,
        ui_modules: 2,
        components: 4,
        machines: 5,
    }),
};

/// The smallest monorepo with every shape in it, for the suite.
pub const MONOREPO_TINY: Size = Size {
    name: "monorepo-tiny",
    shape: Shape::Monorepo(Monorepo {
        packages: 3,
        modules: 2,
        module_lines: 60,
        programs: 1,
        scripts: 1,
        pages: 1,
        fixtures: 2,
        protos: 3,
        messages: 3,
        calls: 1,
        ui_packages: 2,
        ui_modules: 1,
        components: 4,
        machines: 2,
    }),
};

pub const SIZES: [&Size; 6] = [&MONOREPO, &MONOREPO_CI, &MONOREPO_TINY, &FULL, &CI, &TINY];

pub struct File {
    /// Relative to the repository root.
    pub path: String,
    pub text: String,
}

/// Every file of the workspace at `size`, in a fixed order.
pub fn files(size: &Size) -> Vec<File> {
    match &size.shape {
        Shape::Libraries(l) => libraries(l),
        Shape::Monorepo(m) => monorepo::files(m),
    }
}

fn libraries(size: &Libraries) -> Vec<File> {
    let p = Params::default();
    let mut rng = Rng::new(0x5745_4f52_4b53_5041);
    let mut aux = Rng::new(0x4155_5849_4c49_4152);
    let mut out = vec![File {
        path: "REPO.buri".into(),
        text: "# A generated workspace (cli/benches/workspace.rs).\n\
               lint {\n    check_during_build: true\n}\n"
            .into(),
    }];
    let mut deps: Vec<Vec<usize>> = Vec::new();
    for k in 0..size.libraries {
        let mut mine = Vec::new();
        for _ in 0..(k.min(3)) {
            let d = rng.below(k as u64) as usize;
            if !mine.contains(&d) {
                mine.push(d);
            }
        }
        mine.sort_unstable();
        library(&mut out, size, k, &mine, &p, &mut rng, &mut aux);
        deps.push(mine);
    }
    for j in 0..size.programs {
        let used = draw(&mut rng, size);
        program(&mut out, j, &used);
    }
    for j in 0..size.pages {
        let used = draw(&mut rng, size);
        page(&mut out, j, &used);
    }
    for f in &mut out {
        if f.path.ends_with(".buri") && !f.path.ends_with("BUILD.buri") && f.path != "REPO.buri" {
            f.text = laid_out(&f.path, std::mem::take(&mut f.text));
        }
    }
    out
}

/// `uses` distinct libraries, in order.
fn draw(rng: &mut Rng, size: &Libraries) -> Vec<usize> {
    let mut used = Vec::new();
    while used.len() < size.uses.min(size.libraries) {
        let d = rng.below(size.libraries as u64) as usize;
        if !used.contains(&d) {
            used.push(d);
        }
    }
    used.sort_unstable();
    used
}

fn lib(k: usize) -> String {
    format!("//lib/l{k:03}")
}

fn quoted(xs: impl Iterator<Item = String>) -> String {
    xs.map(|x| format!("\"{x}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

#[allow(clippy::too_many_arguments)]
fn library(
    out: &mut Vec<File>,
    size: &Libraries,
    k: usize,
    deps: &[usize],
    p: &Params,
    rng: &mut Rng,
    aux: &mut Rng,
) {
    let dir = format!("lib/l{k:03}");
    let first = k * size.modules;
    let last = first + size.modules - 1;
    out.push(File {
        path: format!("{dir}/BUILD.buri"),
        text: format!(
            "library {{\n    sources: [{}]\n    dependencies: [{}]\n    visibility: [\"//...\"]\n\n    test {{\n        sources: [\"test/t.buri\"]\n    }}\n}}\n",
            quoted((0..=size.modules).map(|g| format!("m{g}.buri"))),
            quoted(deps.iter().map(|d| lib(*d))),
        ),
    });
    out.push(File {
        path: format!("{dir}/m0.buri"),
        text: shapes(k),
    });
    for g in 0..size.modules {
        let i = first + g;
        out.push(File {
            path: format!("{dir}/m{}.buri", g + 1),
            text: module(size, k, g, i, deps, p, rng, aux),
        });
    }
    let stocks: Vec<String> = (first..=last).map(|i| format!("stock{i}")).collect();
    out.push(File {
        path: format!("{dir}/lib.buri"),
        text: format!(
            "//! The public surface of {lib}.\n\n\
             from \"{lib}/m0.buri\" export {{ full, Grade{k}, grade{k}, heft{k}, Pool{k}, pool{k}, put, rank{k}, size, Weigh{k} }};\n\n\
             from \"{lib}/m{m}.buri\" export {{ announce{k}, entry{k} }};\n\n{}",
            (0..size.modules)
                .map(|g| format!(
                    "from \"{}/m{}.buri\" export {{ {} }};\n",
                    lib(k),
                    g + 1,
                    stocks[g]
                ))
                .collect::<String>(),
            lib = lib(k),
            m = size.modules,
        ),
    });
    let mut t = String::from(
        "from \"core/testing/assert\" import * as assert;\n\
         from \"platform/effect/testing\" import { alloc };\n",
    );
    t.push_str(&format!(
        "from \"{}\" import {{ {}, Grade{k}, grade{k}, pool{k}, rank{k} }};\n\n",
        lib(k),
        stocks.join(", ")
    ));
    t.push_str(&format!(
        "test \"a pool fills to its cap\" {{\n\
         \x20   let ctx = alloc();\n\
         \x20   let full = pool{k}<Int>(2).put(ctx, 1).put(ctx, 2);\n\
         \x20   assert.isTrue(full.full());\n\
         \x20   assert.equal(grade{k}(full.size()), Grade{k}.Mid(2));\n\
         \x20   assert.equal(rank{k}(Grade{k}.Low), 0);\n\
         }}\n\n"
    ));
    for (g, s) in stocks.iter().enumerate() {
        t.push_str(&format!(
            "test \"module {g} stocks its items\" {{\n\
             \x20   assert.isTrue({s}(alloc()) > 0);\n\
             }}\n\n"
        ));
    }
    out.push(File {
        path: format!("{dir}/test/t.buri"),
        text: t,
    });
}

/// A library's first module: its trait, its enum and its generic record.
fn shapes(k: usize) -> String {
    format!(
        "//! The shapes library l{k:03} is built on.\n\n\
         from \"core/list\" import * as list;\n\
         from \"platform/effect\" import {{ Allocator }};\n\n\
         /// Anything a pool can weigh.\n\
         export trait Weigh{k} {{\n\
         \x20   fn weight(self): Int;\n\
         }}\n\n\
         /// How full a pool is.\n\
         derive Equal, Show for Grade{k};\n\
         export enum Grade{k} {{\n\
         \x20   Low,\n\
         \x20   Mid(Int),\n\
         \x20   High {{ score: Int, note: Str }},\n\
         }}\n\n\
         /// A bounded list of items.\n\
         export struct Pool{k}<T> {{\n\
         \x20   export items: [T],\n\
         \x20   export cap: Int,\n\
         }}\n\n\
         impl<T> Pool{k}<T> {{\n\
         \x20   /// The pool with `item` added.\n\
         \x20   export fn put<C: Allocator>(self, ctx: C, item: T): Pool{k}<T> {{\n\
         \x20       Pool{k} {{ items: self.items.push(ctx, item), cap: self.cap }}\n\
         \x20   }}\n\n\
         \x20   export fn size(self): Int {{\n\
         \x20       self.items.length()\n\
         \x20   }}\n\n\
         \x20   export fn full(self): Bool {{\n\
         \x20       self.items.length() >= self.cap\n\
         \x20   }}\n\
         }}\n\n\
         /// An empty pool.\n\
         export fn pool{k}<T>(cap: Int): Pool{k}<T> {{\n\
         \x20   Pool{k} {{ items: list.empty<T>(), cap: cap }}\n\
         }}\n\n\
         /// The weight of every item.\n\
         export fn heft{k}<T: Weigh{k}>(xs: [T]): Int {{\n\
         \x20   xs.fold(fn(sum, x) => sum + x.weight(), 0)\n\
         }}\n\n\
         export fn grade{k}(n: Int): Grade{k} {{\n\
         \x20   if (n == 0) {{\n\
         \x20       .Low\n\
         \x20   }} else if (n < {cut}) {{\n\
         \x20       .Mid(n)\n\
         \x20   }} else {{\n\
         \x20       .High {{ score: n, note: \"full\" }}\n\
         \x20   }}\n\
         }}\n\n\
         export fn rank{k}(g: Grade{k}): Int {{\n\
         \x20   match (g) {{\n\
         \x20       .Low => 0,\n\
         \x20       .Mid(n) => n,\n\
         \x20       .High {{ score, note }} => score + note.length(),\n\
         \x20   }}\n\
         }}\n",
        cut = 3 + k % 5,
    )
}

/// Module `g` of library `k`, numbered `i` across the workspace.
#[allow(clippy::too_many_arguments)]
fn module(
    size: &Libraries,
    k: usize,
    g: usize,
    i: usize,
    deps: &[usize],
    p: &Params,
    rng: &mut Rng,
    aux: &mut Rng,
) -> String {
    let last = g + 1 == size.modules;
    let mut s = format!("//! Module {g} of library l{k:03}.\n\n");
    if last {
        s.push_str("from \"core/io\" import * as io;\n");
    }
    s.push_str(&format!(
        "from \"core/list\" import * as list;\n\
         from \"core/str\" import * as str;\n\
         from \"platform/effect\" import {{ {} }};\n\
         from \"{lib}/m0.buri\" import {{ grade{k}, heft{k}, pool{k}, rank{k}, Weigh{k} }};\n",
        if last {
            "Allocator, IoError, Stdout"
        } else {
            "Allocator"
        },
        lib = lib(k),
    ));
    if g > 0 {
        s.push_str(&format!(
            "from \"{}/m{g}.buri\" import {{ reach{} }};\n",
            lib(k),
            i - 1
        ));
    }
    for d in deps {
        s.push_str(&format!(
            "from \"{}\" import {{ entry{d}, pool{d} }};\n",
            lib(*d)
        ));
    }
    s.push_str(&format!(
        "\n/// An item this module stocks.\n\
         derive Equal, Show for Item{i};\n\
         struct Item{i} {{\n\
         \x20   export name: Str,\n\
         \x20   export size: Int,\n\
         }}\n\n\
         impl Weigh{k} for Item{i} {{\n\
         \x20   fn weight(self): Int {{\n\
         \x20       self.size * {w}\n\
         \x20   }}\n\
         }}\n\n\
         /// Pools of this module's items, of numbers and of names.\n\
         export fn stock{i}<C: Allocator>(ctx: C): Int {{\n\
         \x20   let items = pool{k}<Item{i}>(4)\n\
         \x20       .put(ctx, Item{i} {{ name: \"a\", size: 2 }})\n\
         \x20       .put(ctx, Item{i} {{ name: \"b\", size: {i} }});\n\
         \x20   let counts = pool{k}<Int>(8).put(ctx, 1).put(ctx, 2);\n\
         \x20   let names = pool{k}<Str>(2).put(ctx, str.format(ctx, \"item {i}\"));\n\
         \x20   heft{k}(items.items) + counts.size() + names.size() + rank{k}(grade{k}(items.size()))\n\
         }}\n\n\
         /// The items' names, counted.\n\
         fn tally{i}<C: Allocator>(ctx: C): Int {{\n\
         \x20   let none: [Item{i}] = list.empty<Item{i}>();\n\
         \x20   let all = none.push(ctx, Item{i} {{ name: \"c\", size: 1 }});\n\
         \x20   all.map(ctx, fn(x) => x.name).length() + all.length()\n\
         }}\n\n",
        w = i % 7 + 1,
    ));
    let mut borrowed = String::new();
    for d in deps {
        s.push_str(&format!(
            "/// This module's items in library l{d:03}'s pool.\n\
             fn borrow{i}_{d}<C: Allocator>(ctx: C): Int {{\n\
             \x20   pool{d}<Item{i}>(1).put(ctx, Item{i} {{ name: \"d\", size: {d} }}).size() + entry{d}(ctx)\n\
             }}\n\n"
        ));
        borrowed.push_str(&format!("        + borrow{i}_{d}(ctx)\n"));
    }
    let weights = p.weights();
    let total: u32 = weights.iter().sum();
    let mut probes = String::new();
    let mut counter = 0usize;
    while nonblank_lines(&s) < size.module_lines {
        let mut r = rng.below(u64::from(total)) as u32;
        let mut kind = weights.len() - 1;
        for (n, w) in weights.iter().enumerate() {
            if r < *w {
                kind = n;
                break;
            }
            r -= *w;
        }
        let n = counter.to_string();
        let mut c = String::new();
        chunk(kind, &mut c, i, &n, p, rng, aux);
        s.push_str(&private(&c, &format!("probe{i}_{n}")));
        probes.push_str(&format!("        + probe{i}_{n}(ctx)\n"));
        counter += 1;
    }
    s.push_str(&format!(
        "/// Everything in this module, from one call.\n\
         {}fn reach{i}<C: Allocator>(ctx: C): Int {{\n\
         \x20   stock{i}(ctx)\n\
         \x20       + tally{i}(ctx)\n{borrowed}{probes}{prev}\
         }}\n",
        if last { "" } else { "export " },
        prev = if g > 0 {
            format!("        + reach{}(ctx)\n", i - 1)
        } else {
            String::new()
        },
    ));
    if last {
        s.push_str(&format!(
            "\n/// Everything in library l{k:03}, from one call.\n\
             export fn entry{k}<C: Allocator>(ctx: C): Int {{\n\
             \x20   reach{i}(ctx)\n\
             }}\n\n\
             /// Says what library l{k:03} weighs.\n\
             export fn announce{k}<C: Allocator + Stdout>(ctx: C): Result<(), IoError> {{\n\
             \x20   io.println(ctx, str.format(ctx, \"l{k:03}: ${{entry{k}(ctx)}}\"))\n\
             }}\n"
        ));
    }
    s
}

/// One of `generate.rs`'s declaration groups as this module's own: nothing in
/// it is exported, since only `reach` calls it, and a `probe` that never uses
/// its context asks for no allocator.
fn private(chunk: &str, probe: &str) -> String {
    let mut out = String::new();
    for line in chunk.lines() {
        out.push_str(line.strip_prefix("export ").unwrap_or(line));
        out.push('\n');
    }
    if out.contains("\n  let _ = ctx;\n") {
        out = out.replace(
            &format!("fn {probe}<C: Allocator>(ctx: C)"),
            &format!("fn {probe}<C>(ctx: C)"),
        );
    }
    out
}

/// A native program over `used` libraries.
fn program(out: &mut Vec<File>, j: usize, used: &[usize]) {
    let dir = format!("cmd/p{j}");
    out.push(File {
        path: format!("{dir}/BUILD.buri"),
        text: format!(
            "binary {{\n    sources: [\"work.buri\"]\n    dependencies: [{}]\n\n    outputs: [\n        {{ platform: \"native\" }},\n    ]\n}}\n",
            quoted(used.iter().map(|d| lib(*d))),
        ),
    });
    out.push(File {
        path: format!("{dir}/main.buri"),
        text: format!(
            "from \"native\" import {{ NativeHost }};\n\
             from \"platform/effect\" import {{ Allocator, Stdout }};\n\
             from \"//cmd/p{j}/work.buri\" import {{ work{j} }};\n\n\
             export fn main(host: NativeHost): Result<(), Str> {{\n\
             \x20   let ctx = context {{\n\
             \x20       Allocator: host.alloc,\n\
             \x20       Stdout: host.stdout,\n\
             \x20   }};\n\
             \x20   match (work{j}(ctx)) {{\n\
             \x20       .Ok(_) => .Ok(()),\n\
             \x20       .Err(_) => .Err(\"could not print\"),\n\
             \x20   }}\n\
             }}\n"
        ),
    });
    let mut w = String::from("from \"platform/effect\" import { Allocator, IoError, Stdout };\n");
    for d in used {
        w.push_str(&format!(
            "from \"{}\" import {{ announce{d}, entry{d} }};\n",
            lib(*d)
        ));
    }
    w.push_str(&format!(
        "\n/// Announces each library and adds up what they weigh.\n\
         export fn work{j}<C: Allocator + Stdout>(ctx: C): Result<Int, IoError> {{\n"
    ));
    for d in used {
        w.push_str(&format!("    let _ = announce{d}(ctx)?;\n"));
    }
    w.push_str(&format!(
        "    .Ok({})\n}}\n",
        used.iter()
            .map(|d| format!("entry{d}(ctx)"))
            .collect::<Vec<_>>()
            .join(" + ")
    ));
    out.push(File {
        path: format!("{dir}/work.buri"),
        text: w,
    });
}

/// A page over `used` libraries, with styles.
fn page(out: &mut Vec<File>, j: usize, used: &[usize]) {
    let dir = format!("web/w{j}");
    out.push(File {
        path: format!("{dir}/BUILD.buri"),
        text: format!(
            "binary {{\n    dependencies: [{}]\n\n    outputs: [\n        {{ platform: \"web\" }},\n    ]\n}}\n",
            quoted(used.iter().map(|d| lib(*d))),
        ),
    });
    let mut m = String::from(
        "from \"core/str\" import * as str;\n\
         from \"platform/effect\" import { Allocator, Ui };\n\
         from \"ui/node\" import * as ui;\n\
         from \"ui/node\" import { Node };\n\
         from \"ui/style\" import { Style };\n\
         from \"web\" import { WebHost };\n",
    );
    for d in used {
        m.push_str(&format!("from \"{}\" import {{ entry{d} }};\n", lib(*d)));
    }
    m.push_str(&format!(
        "\nlet panel{j}: Style = .Group([\n\
         \x20   .Layout(.Column),\n\
         \x20   .Gap(.Px({gap})),\n\
         \x20   .Padding(.Rem(0.75)),\n\
         \x20   .Radius(.Px(8)),\n\
         \x20   .BorderWidth(.Px(1)),\n\
         ]);\n\n\
         let label{j}: Style = .Group([\n\
         \x20   .FontWeight(.Semibold),\n\
         \x20   .Truncate(2),\n\
         ]);\n\n\
         /// One line per library.\n\
         fn row{j}<C: Allocator>(ctx: C, name: Str, weight: Int): Node<C> {{\n\
         \x20   ui.stack({{\n\
         \x20       styles: [.Layout(.Row), .Group([label{j}])],\n\
         \x20       children: [ui.text({{ content: .Const(str.format(ctx, \"${{name}}: ${{weight}}\")) }})],\n\
         \x20   }})\n\
         }}\n\n\
         export fn main(host: WebHost): Result<(), Str> {{\n\
         \x20   let ctx = context {{\n\
         \x20       Allocator: host.alloc,\n\
         \x20       Ui: host.ui,\n\
         \x20   }};\n\
         \x20   ui.mount(ctx, ui.stack({{\n\
         \x20       styles: [panel{j}],\n\
         \x20       children: [\n",
        gap = 4 + j % 4,
    ));
    for d in used {
        m.push_str(&format!(
            "            row{j}(ctx, \"l{d:03}\", entry{d}(ctx)),\n"
        ));
    }
    m.push_str("        ],\n    }), [])\n}\n");
    out.push(File {
        path: format!("{dir}/main.buri"),
        text: m,
    });
}
