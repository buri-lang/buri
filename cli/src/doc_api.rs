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

use crate::ast::{self, Item, ParamKind};
use crate::compile::{Loaded, ModuleData, Role};
use crate::format;
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
            ItemKind::Const => "const",
            ItemKind::Context => "context",
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
pub struct Member {
    pub name: String,
    pub signature: String,
    pub docs: Vec<String>,
}

pub struct ApiItem {
    pub kind: ItemKind,
    pub name: String,
    /// The type a method hangs off, and the trait it satisfies if any.
    pub owner: Option<String>,
    pub via_trait: Option<String>,
    pub signature: String,
    pub docs: Vec<String>,
    pub members: Vec<Member>,
    /// The bounds on the `ctx` parameter — what this function may do to the
    /// world. Empty means it cannot do anything: no context, no effects.
    pub effects: Vec<String>,
}

impl ApiItem {
    /// `core/list.map`, `//lib/money.Cents`.
    pub fn path(&self, module: &str) -> String {
        format!("{module}.{}", self.name)
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
                let wanted = spec.name.name.as_str();
                let shown = spec.alias.as_ref().unwrap_or(&spec.name).name.clone();
                for found in from.iter().filter(|i| i.name == wanted) {
                    items.push(ApiItem {
                        kind: found.kind,
                        name: shown.clone(),
                        owner: found.owner.clone(),
                        via_trait: found.via_trait.clone(),
                        signature: found.signature.clone(),
                        docs: found.docs.clone(),
                        members: found
                            .members
                            .iter()
                            .map(|x| Member {
                                name: x.name.clone(),
                                signature: x.signature.clone(),
                                docs: x.docs.clone(),
                            })
                            .collect(),
                        effects: found.effects.clone(),
                    });
                }
                // A method is *not* pulled in just because its type was. The
                // re-export list is the surface, exactly: `toCents` is exported
                // by `cents.buri` so its neighbours can use it and left off
                // `lib.buri`, so `c.toCents()` does not resolve for an importer
                // — and must not appear on the page either.
            }
        }
        items.sort_by(|a, b| {
            a.kind
                .rank()
                .cmp(&b.kind.rank())
                .then_with(|| a.owner.cmp(&b.owner))
                .then_with(|| a.name.cmp(&b.name))
        });
        items.dedup_by(|a, b| a.name == b.name && a.owner == b.owner && a.kind == b.kind);
        out.push(ApiModule { path: m.path.clone(), docs: m.ast.docs.clone(), items });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// The standard library: every `core/...` module.
pub fn std_filter(m: &ModuleData) -> bool {
    matches!(m.role, Role::Std | Role::Platform) && m.path.starts_with("core/")
}

fn items_of(module: &ast::Module) -> Vec<ApiItem> {
    let mut out = Vec::new();
    for item in &module.items {
        match item {
            Item::Fn(d) if d.exported => out.push(function(d, None, None)),
            Item::Struct(d) if d.exported => out.push(structure(d)),
            Item::Enum(d) if d.exported => out.push(enumeration(d)),
            Item::Trait(d) if d.exported => out.push(trait_or_effect(d)),
            Item::TypeAlias(d) if d.exported => out.push(ApiItem {
                kind: ItemKind::TypeAlias,
                name: d.name.name.clone(),
                owner: None,
                via_trait: None,
                signature: format!("type {} = {}", d.name.name, format::ty(&d.ty)),
                docs: d.docs.clone(),
                members: Vec::new(),
                effects: Vec::new(),
            }),
            Item::Const(d) if d.exported => out.push(ApiItem {
                kind: ItemKind::Const,
                name: d.name.name.clone(),
                owner: None,
                via_trait: None,
                signature: format!("const {}: {}", d.name.name, format::ty(&d.ty)),
                docs: d.docs.clone(),
                members: Vec::new(),
                effects: Vec::new(),
            }),
            Item::Context(d) if d.exported => out.push(ApiItem {
                kind: ItemKind::Context,
                name: d.name.name.clone(),
                owner: None,
                via_trait: None,
                signature: format!("context {}", d.name.name),
                docs: d.docs.clone(),
                members: Vec::new(),
                effects: Vec::new(),
            }),
            Item::Impl(d) => {
                let owner = format::ty(&d.self_ty);
                let via = d.trait_ty.as_ref().map(format::ty);
                for m in &d.methods {
                    // A trait's methods are visible wherever the type is, so
                    // conformance methods are listed even though they carry no
                    // `export` of their own.
                    if m.exported || via.is_some() {
                        out.push(function(m, Some(owner.clone()), via.clone()));
                    }
                }
            }
            _ => {}
        }
    }
    out.sort_by(|a, b| {
        a.kind
            .rank()
            .cmp(&b.kind.rank())
            .then_with(|| a.owner.cmp(&b.owner))
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

fn function(d: &ast::FnDecl, owner: Option<String>, via_trait: Option<String>) -> ApiItem {
    ApiItem {
        kind: if d.is_method() || owner.is_some() { ItemKind::Method } else { ItemKind::Function },
        name: d.name.name.clone(),
        owner,
        via_trait,
        signature: format::signature(d),
        docs: d.docs.clone(),
        members: Vec::new(),
        effects: effects_of(d),
    }
}

/// The bounds on the `ctx` parameter, which is the whole of what a function
/// may do to the world. Reading them off the signature is the point: purity is
/// the absence of this list, not an annotation somebody had to remember.
fn effects_of(d: &ast::FnDecl) -> Vec<String> {
    let Some(ctx) = d.params.iter().find(|p| p.kind == ParamKind::CtxParam) else {
        return Vec::new();
    };
    let name = format::ty(&ctx.ty);
    d.generics
        .iter()
        .find(|g| g.name.name == name)
        .map(|g| g.bounds.iter().map(format::ty).collect())
        .unwrap_or_else(|| vec![name])
}

/// Per-field and per-variant visibility is a declaration detail. A reference
/// lists what is exported and nothing else, so repeating the keyword on every
/// line is noise a reader has to skip.
fn strip_export(sig: &str) -> String {
    let mut out = sig.strip_prefix("export ").unwrap_or(sig).to_string();
    for opener in ["{ ", ", "] {
        out = out.replace(&format!("{opener}export "), opener);
    }
    out
}

fn structure(d: &ast::StructDecl) -> ApiItem {
    let members = match &d.body {
        ast::StructBody::Record(fields) => fields
            .iter()
            .filter(|f| f.exported)
            .map(|f| Member {
                name: f.name.name.clone(),
                signature: strip_export(&format::field_decl(f)),
                docs: f.docs.clone(),
            })
            .collect(),
        ast::StructBody::Tuple(fields) => fields
            .iter()
            .enumerate()
            .filter(|(_, f)| f.exported)
            .map(|(i, f)| Member {
                name: i.to_string(),
                signature: format!("{i}: {}", format::ty(&f.ty)),
                docs: Vec::new(),
            })
            .collect(),
    };
    ApiItem {
        kind: ItemKind::Struct,
        name: d.name.name.clone(),
        owner: None,
        via_trait: None,
        signature: format!("struct {}{}", d.name.name, format::generics(&d.generics)),
        docs: d.docs.clone(),
        members,
        effects: Vec::new(),
    }
}

fn enumeration(d: &ast::EnumDecl) -> ApiItem {
    ApiItem {
        kind: ItemKind::Enum,
        name: d.name.name.clone(),
        owner: None,
        via_trait: None,
        signature: format!("enum {}{}", d.name.name, format::generics(&d.generics)),
        docs: d.docs.clone(),
        members: d
            .variants
            .iter()
            .filter(|v| v.exported)
            .map(|v| Member {
                name: v.name.name.clone(),
                signature: strip_export(&format::variant(v)),
                docs: v.docs.clone(),
            })
            .collect(),
        effects: Vec::new(),
    }
}

fn trait_or_effect(d: &ast::TraitDecl) -> ApiItem {
    ApiItem {
        kind: if d.is_effect { ItemKind::Effect } else { ItemKind::Trait },
        name: d.name.name.clone(),
        owner: None,
        via_trait: None,
        signature: format!(
            "{} {}{}",
            if d.is_effect { "effect" } else { "trait" },
            d.name.name,
            format::generics(&d.generics)
        ),
        docs: d.docs.clone(),
        members: d
            .methods
            .iter()
            .map(|m| Member {
                name: m.name.name.clone(),
                signature: format::signature(m),
                docs: m.docs.clone(),
            })
            .collect(),
        effects: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

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
        if last != Some(item.kind) {
            let _ = writeln!(out, "## {}\n", heading(item.kind));
            if matches!(item.kind, ItemKind::Method | ItemKind::Function) {
                let _ = writeln!(
                    out,
                    "*Pure* means the function takes no context parameter, so it cannot \
                     allocate, read, write, or observe anything — the guarantee is the \
                     absence of an argument rather than an annotation.\n"
                );
            }
            last = Some(item.kind);
        }
        write_item(&mut out, item);
    }
    out
}

fn heading(kind: ItemKind) -> &'static str {
    match kind {
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

fn write_item(out: &mut String, item: &ApiItem) {
    let title = match (&item.owner, &item.via_trait) {
        (Some(owner), Some(via)) => format!("{owner}.{} — via {via}", item.name),
        (Some(owner), None) => format!("{owner}.{}", item.name),
        (None, _) => item.name.clone(),
    };
    let _ = writeln!(out, "### {title}\n");
    let _ = writeln!(out, "```buri sig\n{}\n```\n", item.signature);

    // The effect line is the one fact a reader most often wants and least
    // wants to derive from a signature, so it is stated on every function —
    // but tersely, because it appears on all of them. What "Pure" means is
    // explained once, under the section heading.
    if !item.effects.is_empty() {
        let _ = writeln!(out, "Effects: `{}`\n", item.effects.join("` · `"));
    } else if matches!(item.kind, ItemKind::Method | ItemKind::Function) {
        let _ = writeln!(out, "Pure.\n");
    }

    for line in &item.docs {
        let _ = writeln!(out, "{line}");
    }
    if !item.docs.is_empty() {
        out.push('\n');
    }

    if !item.members.is_empty() {
        for m in &item.members {
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
    use crate::diag::SourceMap;

    fn stdlib() -> Vec<ApiModule> {
        let mut map = SourceMap::new();
        let analysis = crate::driver::analyze_stdlib(&mut map);
        assert!(!analysis.diags.has_errors(), "the standard library must check");
        from_loaded(&analysis.loaded, &std_filter)
    }

    #[test]
    fn every_std_module_renders_something() {
        let modules = stdlib();
        assert_eq!(modules.len(), crate::stdlib::MODULES.len());
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
        assert_eq!(map_fn.effects, vec!["Alloc"], "map allocates and says so");

        let (_, len) = find_item(&modules, "core/list.len").expect("core/list.len");
        assert!(len.effects.is_empty(), "len is pure");

        let (_, alloc) = find_item(&modules, "core/cap.Alloc").expect("core/cap.Alloc");
        assert_eq!(alloc.kind, ItemKind::Effect);
        assert!(alloc.members.iter().any(|m| m.name == "allocate"));
    }

    /// A method's page names the type it hangs off. Without that, `get` and
    /// `get` from two modules are indistinguishable in a listing.
    #[test]
    fn methods_name_their_receiver() {
        let modules = stdlib();
        let list = modules.iter().find(|m| m.path == "core/list").unwrap();
        let get = list.items.iter().find(|i| i.name == "get").unwrap();
        assert_eq!(get.owner.as_deref(), Some("[T]"));
    }
}
