//! The API reference, rendered from what the compiler actually compiled.
//!
//! Nothing here is hand-written. A module's page is built from the AST the
//! front end just checked: the signatures are printed by the same code
//! `buri format` uses, the documentation is the `///` and `//!` text attached
//! to the declarations, and the effect bounds are read off the `ctx`
//! parameter. A signature on a page and a signature in the compiler cannot
//! disagree, because they are the same value.
//!
//! One renderer serves two sources — the embedded standard library and the
//! packages of whatever repository you are standing in — because they are the
//! same thing to the loader. That is what makes `buri docs //lib/money` work
//! in somebody else's repository without any of this knowing about it.

use crate::compiler::modules::{Loaded, ModuleData, Role};
use crate::formatting;
use crate::parsing::tree::{self, Item, ParamKind};
use std::fmt::Write as _;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ItemKind {
    Function,
    Method,
    Struct,
    Enum,
    Trait,
    Effect,
    TypeAlias,
    Const,
    Context,
}

impl ItemKind {
    pub fn label(self) -> &'static str {
        match self {
            ItemKind::Function => "fn",
            ItemKind::Method => "method",
            ItemKind::Struct => "struct",
            ItemKind::Enum => "enum",
            ItemKind::Trait => "trait",
            ItemKind::Effect => "effect",
            ItemKind::TypeAlias => "type",
            ItemKind::Const => "let",
            ItemKind::Context => "context",
        }
    }

    /// What a page calls the section holding items of this kind. Public
    /// because the website writes its own module pages — it needs headings a
    /// reader can link to — and two lists of the same nine words would drift.
    pub fn heading(self) -> &'static str {
        match self {
            ItemKind::Struct => "Structs",
            ItemKind::Enum => "Enums",
            ItemKind::TypeAlias => "Type aliases",
            ItemKind::Trait => "Traits",
            ItemKind::Effect => "Effects",
            ItemKind::Const => "Constants",
            ItemKind::Context => "Contexts",
            ItemKind::Method => "Methods",
            ItemKind::Function => "Functions",
        }
    }

    /// The order a reference lists them: what a type *is* before what it can
    /// do, and the free functions last.
    fn rank(self) -> u8 {
        match self {
            ItemKind::Struct | ItemKind::Enum => 0,
            ItemKind::TypeAlias => 1,
            ItemKind::Trait | ItemKind::Effect => 2,
            ItemKind::Const => 3,
            ItemKind::Context => 4,
            ItemKind::Method => 5,
            ItemKind::Function => 6,
        }
    }
}

/// A field, a variant, or a trait method — something that belongs to an item
/// rather than standing on its own.
#[derive(Clone)]
pub struct Member {
    pub name: String,
    pub signature: String,
    pub docs: Vec<String>,
}

/// What an item is, holding whatever only that kind of item has.
///
/// This used to be a `kind` beside an `owner`, a `via_trait`, a `members` list
/// and an `effects` list, with the correlation maintained by one constructor
/// and by nothing else. A method with no owner rendered as a free function —
/// which is the exact confusion `methods_name_their_receiver` exists to catch —
/// a struct could be `T.Foo`, and `members` meant fields, variants, or methods
/// depending on a field three lines up and had to be empty for the rest.
#[derive(Clone)]
pub enum Api {
    Struct { fields: Vec<Member> },
    Enum { variants: Vec<Member> },
    TypeAlias,
    Trait { methods: Vec<Member> },
    Effect { methods: Vec<Member> },
    Const,
    Context,
    /// A method: the type it hangs off, and the trait it satisfies if any.
    Method { owner: String, via_trait: Option<String>, effects: Vec<String> },
    Function { effects: Vec<String> },
}

impl Api {
    pub fn kind(&self) -> ItemKind {
        match self {
            Api::Struct { .. } => ItemKind::Struct,
            Api::Enum { .. } => ItemKind::Enum,
            Api::TypeAlias => ItemKind::TypeAlias,
            Api::Trait { .. } => ItemKind::Trait,
            Api::Effect { .. } => ItemKind::Effect,
            Api::Const => ItemKind::Const,
            Api::Context => ItemKind::Context,
            Api::Method { .. } => ItemKind::Method,
            Api::Function { .. } => ItemKind::Function,
        }
    }

    /// The type a method hangs off. Only a method has one.
    pub fn owner(&self) -> Option<&str> {
        match self {
            Api::Method { owner, .. } => Some(owner),
            _ => None,
        }
    }

    /// The fields, variants, or methods listed under the item. The kinds that
    /// have none cannot be given any.
    pub fn members(&self) -> &[Member] {
        match self {
            Api::Struct { fields } => fields,
            Api::Enum { variants } => variants,
            Api::Trait { methods } | Api::Effect { methods } => methods,
            _ => &[],
        }
    }

    /// The bounds on the `ctx` parameter — what this function may do to the
    /// world. Empty means it cannot do anything: no context, no effects. Only
    /// a function or a method has a `ctx` parameter to read them off.
    pub fn effects(&self) -> &[String] {
        match self {
            Api::Method { effects, .. } | Api::Function { effects } => effects,
            _ => &[],
        }
    }

    /// Whether the item is one for which "Pure" is a statement about it.
    pub fn is_callable(&self) -> bool {
        matches!(self, Api::Method { .. } | Api::Function { .. })
    }
}

#[derive(Clone)]
pub struct ApiItem {
    pub api: Api,
    pub name: String,
    pub signature: String,
    pub docs: Vec<String>,
}

impl ApiItem {
    /// `core/list.map`, `//lib/money.Cents`.
    pub fn path(&self, module: &str) -> String {
        format!("{module}.{}", self.name)
    }

    pub fn kind(&self) -> ItemKind {
        self.api.kind()
    }
}

pub struct ApiModule {
    pub path: String,
    pub docs: Vec<String>,
    pub items: Vec<ApiItem>,
}

// ---------------------------------------------------------------------------
// Building
// ---------------------------------------------------------------------------

/// Every module of a loaded compilation that `keep` accepts.
///
/// A library's public surface is written as a list of re-exports in its
/// `lib.buri`, so those are followed: the page for `//lib/money` shows the
/// items it re-exports, not the two `from ... export` lines, because the
/// re-export list *is* the API and a reader wants the API.
pub fn from_loaded(loaded: &Loaded, keep: &dyn Fn(&ModuleData) -> bool) -> Vec<ApiModule> {
    // Every module's own items first, so a re-export can be resolved against a
    // module the filter excluded — which is the normal case, since the modules
    // behind a surface are internal.
    let mut owned: Vec<(String, Vec<ApiItem>)> = Vec::new();
    for m in &loaded.modules {
        owned.push((m.path.clone(), items_of(&m.ast)));
    }

    let mut out: Vec<ApiModule> = Vec::new();
    for m in loaded.modules.iter().filter(|m| keep(m)) {
        let mut items = items_of(&m.ast);
        for item in &m.ast.items {
            let Item::ReExport(r) = item else { continue };
            let Some((_, from)) = owned.iter().find(|(p, _)| *p == r.path) else { continue };
            for spec in &r.specs {
                let wanted = m.ast.tree.name(spec.name);
                let shown = m.ast.tree.name(spec.local()).to_string();
                for found in from.iter().filter(|i| i.name == wanted) {
                    items.push(ApiItem { name: shown.clone(), ..found.clone() });
                }
                // A method is *not* pulled in just because its type was. The
                // re-export list is the surface, exactly: `toCents` is exported
                // by `cents.buri` so its neighbours can use it and left off
                // `lib.buri`, so `c.toCents()` does not resolve for an importer
                // — and must not appear on the page either.
            }
        }
        items.sort_by(|a, b| sort_key(a).cmp(&sort_key(b)));
        items.dedup_by(|a, b| sort_key(a) == sort_key(b));
        out.push(ApiModule { path: m.path.clone(), docs: m.ast.docs.clone(), items });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// The standard library: every module under one of its reserved roots.
///
/// The role alone is not enough — a documentation example is loaded with
/// `Role::Std` so that a fence may show a signature without a body — so this
/// asks the path too.
pub fn std_filter(m: &ModuleData) -> bool {
    matches!(m.role, Role::Std | Role::Platform)
        && crate::compiler::standard_library::is_std_path(&m.path)
}

fn items_of(module: &tree::Module) -> Vec<ApiItem> {
    let t = &module.tree;
    let mut out = Vec::new();
    for item in &module.items {
        match item {
            Item::Fn(d) if d.exported => out.push(function(t, d, None, None)),
            Item::Struct(d) if d.exported => out.push(structure(t, d)),
            Item::Enum(d) if d.exported => out.push(enumeration(t, d)),
            Item::Trait(d) if d.exported => out.push(trait_or_effect(t, d)),
            Item::TypeAlias(d) if d.exported => out.push(ApiItem {
                api: Api::TypeAlias,
                name: t.name(d.name).to_string(),
                signature: format!("type {} = {}", t.name(d.name), formatting::type_text(t, d.ty)),
                docs: d.docs.clone(),
            }),
            Item::Let(d) if d.exported => out.push(ApiItem {
                api: Api::Const,
                name: t.name(d.name).to_string(),
                signature: format!("let {}: {}", t.name(d.name), formatting::type_text(t, d.ty)),
                docs: d.docs.clone(),
            }),
            Item::Context(d) if d.exported => out.push(ApiItem {
                api: Api::Context,
                name: t.name(d.name).to_string(),
                signature: format!("context {}", t.name(d.name)),
                docs: d.docs.clone(),
            }),
            Item::Impl(d) => {
                let owner = formatting::type_text(t, d.self_ty);
                let via = d.trait_ty.map(|x| formatting::type_text(t, x));
                for m in &d.methods {
                    // A trait's methods are visible wherever the type is, so
                    // conformance methods are listed even though they carry no
                    // `export` of their own.
                    if m.exported || via.is_some() {
                        out.push(function(t, m, Some(owner.clone()), via.clone()));
                    }
                }
            }
            _ => {}
        }
    }
    out.sort_by(|a, b| sort_key(a).cmp(&sort_key(b)));
    out
}

/// A reference lists what a type *is* before what it can do, then by receiver,
/// then by name. Also the identity two re-exports of one item are deduplicated
/// on, which is why it is one function rather than two orderings to keep in
/// step.
fn sort_key(i: &ApiItem) -> (u8, Option<&str>, &str) {
    (i.kind().rank(), i.api.owner(), &i.name)
}

/// A free function, or a method on `owner`. There is no third case: the one
/// thing a method has that a function does not is the type it hangs off, and it
/// is not optional.
fn function(
    t: &crate::parsing::flat::Tree,
    d: &tree::FnDecl,
    owner: Option<String>,
    via_trait: Option<String>,
) -> ApiItem {
    let effects = effects_of(t, d);
    ApiItem {
        api: match owner {
            Some(owner) => Api::Method { owner, via_trait, effects },
            None => Api::Function { effects },
        },
        name: t.name(d.name).to_string(),
        signature: formatting::signature(t, d),
        docs: d.docs.clone(),
    }
}

/// The bounds on the `ctx` parameter, which is the whole of what a function
/// may do to the world. Reading them off the signature is the point: purity is
/// the absence of this list, not an annotation somebody had to remember.
fn effects_of(t: &crate::parsing::flat::Tree, d: &tree::FnDecl) -> Vec<String> {
    let Some(ctx) = d.params.iter().find(|p| p.kind == ParamKind::CtxParam) else {
        return Vec::new();
    };
    let Some(ty) = ctx.written_type() else { return Vec::new() };
    let name = formatting::type_text(t, ty);
    d.generics
        .iter()
        .find(|g| t.name(g.name) == name)
        .map(|g| t.type_list(g.bounds).iter().map(|b| formatting::type_text(t, *b)).collect())
        .unwrap_or_else(|| vec![name])
}

/// Per-field visibility is a declaration detail. A reference lists what is
/// exported and nothing else, so repeating the keyword on every line is noise
/// a reader has to skip.
fn strip_export(sig: &str) -> String {
    sig.strip_prefix("export ").unwrap_or(sig).to_string()
}

fn structure(t: &crate::parsing::flat::Tree, d: &tree::StructDecl) -> ApiItem {
    let fields = match &d.body {
        tree::StructBody::Record(fields) => fields
            .iter()
            .filter(|f| f.exported)
            .map(|f| Member {
                name: t.name(f.name).to_string(),
                signature: strip_export(&formatting::field_decl(t, f)),
                docs: f.docs.clone(),
            })
            .collect(),
        tree::StructBody::Tuple(fields) => fields
            .iter()
            .enumerate()
            .filter(|(_, f)| f.exported)
            .map(|(i, f)| Member {
                name: i.to_string(),
                signature: format!("{i}: {}", formatting::type_text(t, f.ty)),
                docs: Vec::new(),
            })
            .collect(),
    };
    ApiItem {
        api: Api::Struct { fields },
        name: t.name(d.name).to_string(),
        signature: format!("struct {}{}", t.name(d.name), formatting::generics(t, &d.generics)),
        docs: d.docs.clone(),
    }
}

fn enumeration(t: &crate::parsing::flat::Tree, d: &tree::EnumDecl) -> ApiItem {
    ApiItem {
        api: Api::Enum {
            // Every variant of an exported enum is exported, so every one of
            // them is listed.
            variants: d
                .variants
                .iter()
                .map(|v| Member {
                    name: t.name(v.name).to_string(),
                    signature: formatting::variant(t, v),
                    docs: v.docs.clone(),
                })
                .collect(),
        },
        name: t.name(d.name).to_string(),
        signature: format!("enum {}{}", t.name(d.name), formatting::generics(t, &d.generics)),
        docs: d.docs.clone(),
    }
}

fn trait_or_effect(t: &crate::parsing::flat::Tree, d: &tree::TraitDecl) -> ApiItem {
    let methods = d
        .methods
        .iter()
        .map(|m| Member {
            name: t.name(m.name).to_string(),
            signature: formatting::signature(t, m),
            docs: m.docs.clone(),
        })
        .collect();
    ApiItem {
        api: if d.is_effect { Api::Effect { methods } } else { Api::Trait { methods } },
        name: t.name(d.name).to_string(),
        signature: format!(
            "{} {}{}",
            if d.is_effect { "effect" } else { "trait" },
            t.name(d.name),
            formatting::generics(t, &d.generics)
        ),
        docs: d.docs.clone(),
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// What the absence of a context parameter promises, said once under the
/// heading rather than on each of the two hundred functions below it. Public
/// for the website's own module pages, so that the terminal and the site make
/// the reader the same promise in the same words.
pub const PURITY: &str = "*Pure* means the function takes no context parameter, so it cannot \
                          allocate, read, write, or observe anything — the guarantee is the \
                          absence of an argument rather than an annotation.";

/// One module as a markdown page.
pub fn render(m: &ApiModule) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# {}\n", m.path);
    for line in &m.docs {
        let _ = writeln!(out, "{line}");
    }
    if !m.docs.is_empty() {
        out.push('\n');
    }
    if m.items.is_empty() {
        out.push_str("This module exports nothing.\n");
        return out;
    }

    let mut last: Option<ItemKind> = None;
    for item in &m.items {
        if last != Some(item.kind()) {
            let _ = writeln!(out, "## {}\n", item.kind().heading());
            if item.api.is_callable() {
                let _ = writeln!(out, "{PURITY}\n");
            }
            last = Some(item.kind());
        }
        write_item(&mut out, item);
    }
    out
}

fn write_item(out: &mut String, item: &ApiItem) {
    let title = match &item.api {
        Api::Method { owner, via_trait: Some(via), .. } => {
            format!("{owner}.{} — via {via}", item.name)
        }
        Api::Method { owner, via_trait: None, .. } => format!("{owner}.{}", item.name),
        _ => item.name.clone(),
    };
    let _ = writeln!(out, "### {title}\n");
    let _ = writeln!(out, "```buri sig\n{}\n```\n", item.signature);

    // The effect line is the one fact a reader most often wants and least
    // wants to derive from a signature, so it is stated on every function —
    // but tersely, because it appears on all of them. What "Pure" means is
    // explained once, under the section heading.
    let effects = item.api.effects();
    if !effects.is_empty() {
        let _ = writeln!(out, "Effects: `{}`\n", effects.join("` · `"));
    } else if item.api.is_callable() {
        let _ = writeln!(out, "Pure.\n");
    }

    for line in &item.docs {
        let _ = writeln!(out, "{line}");
    }
    if !item.docs.is_empty() {
        out.push('\n');
    }

    if !item.api.members().is_empty() {
        for m in item.api.members() {
            let _ = writeln!(out, "- `{}`{}", m.signature, member_doc(&m.docs));
        }
        out.push('\n');
    }
}

fn member_doc(docs: &[String]) -> String {
    if docs.is_empty() {
        String::new()
    } else {
        format!(" — {}", docs.join(" "))
    }
}

/// `core/list.map` — a single item, for `buri docs core/list.map`.
pub fn find_item<'a>(modules: &'a [ApiModule], path: &str) -> Option<(&'a ApiModule, &'a ApiItem)> {
    let (module, name) = path.rsplit_once('.')?;
    let m = modules.iter().find(|m| m.path == module)?;
    // A method may be written `core/list.map` or `core/list.[T].map`; both
    // resolve, because a reader knows the name before they know the receiver.
    let item = m
        .items
        .iter()
        .find(|i| i.name == name)
        .or_else(|| m.items.iter().find(|i| i.path(module) == path))?;
    Some((m, item))
}

/// One item as a page of its own.
pub fn render_item(m: &ApiModule, item: &ApiItem) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# {}.{}\n", m.path, item.name);
    let _ = writeln!(out, "From `{}`.\n", m.path);
    write_item(&mut out, item);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::SourceMap;

    fn stdlib() -> Vec<ApiModule> {
        let mut map = SourceMap::new();
        let analysis = crate::compiler::driver::analyze_stdlib(&mut map);
        assert!(!analysis.diagnostics.has_errors(), "the standard library must check");
        from_loaded(&analysis.loaded, &std_filter)
    }

    #[test]
    fn every_std_module_renders_something() {
        let modules = stdlib();
        assert_eq!(modules.len(), crate::compiler::standard_library::MODULES.len());
        for m in &modules {
            let page = render(m);
            assert!(page.contains(&m.path), "{} has no title", m.path);
            assert!(
                !m.docs.is_empty(),
                "{} has no `//!` module documentation",
                m.path
            );
        }
    }

    /// The signatures on the page are the ones the compiler read, so a few
    /// spot checks are enough to know the pipeline is wired up.
    #[test]
    fn the_signatures_are_the_real_ones() {
        let modules = stdlib();
        let (_, map_fn) = find_item(&modules, "core/list.map").expect("core/list.map");
        assert!(map_fn.signature.contains("map<B, C: Alloc>"), "{}", map_fn.signature);
        assert_eq!(map_fn.api.effects(), ["Alloc".to_string()], "map allocates and says so");

        let (_, len) = find_item(&modules, "core/list.len").expect("core/list.len");
        assert!(len.api.effects().is_empty(), "len is pure");

        let (_, alloc) = find_item(&modules, "core/effect.Alloc").expect("core/effect.Alloc");
        assert_eq!(alloc.kind(), ItemKind::Effect);
        assert!(alloc.api.members().iter().any(|m| m.name == "allocate"));
    }

    /// A method's page names the type it hangs off. Without that, `get` and
    /// `get` from two modules are indistinguishable in a listing.
    #[test]
    fn methods_name_their_receiver() {
        let modules = stdlib();
        let list = modules.iter().find(|m| m.path == "core/list").unwrap();
        let get = list.items.iter().find(|i| i.name == "get").unwrap();
        assert_eq!(get.api.owner(), Some("[T]"));
    }
}
