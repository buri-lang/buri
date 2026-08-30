//! Static style extraction: `ui/style` literals in, atomic classes out.
//!
//! `design/ui-reactivity.md` §Styling commits to two things that decide the
//! shape of this file. **Nothing is generated at run time, ever** — so every
//! class name and every rule in the stylesheet is written here, at compile
//! time. And **one atomic class per distinct property value, deduped across the
//! whole build** — so two modules that ask for the same padding must arrive at
//! the same class without having seen each other.
//!
//! ## Why a class name is derived rather than allocated
//!
//! A counter handing out `.c1`, `.c2` would make the name of a class depend on
//! what else the build contains, which is exactly the property that stops a
//! module from being compiled on its own. So a class is named after *what it
//! is*: the property, the condition it is scoped to, and an injective key for
//! the value. `.p-8` is eight pixels of padding wherever it was written, in
//! whichever module, in whichever order the build walked them. Dedup is then
//! not an algorithm at all — two identical rules are literally the same string
//! — and the link step is a merge that cannot disagree with itself, which is
//! what `buri build --check-reproducible` needs.
//!
//! ## What the runtime is left with
//!
//! A style that folded is replaced, in the typed tree, by the
//! `Style::Extracted` variant: a `Classes`, holding a list of `(slot, class)`
//! pairs. `Classes` has a private field, so only `ui/style` and this pass can
//! build one, which is what keeps the variant unwritable. The *slot* is
//! the conflict key — the property together with its condition — so the
//! runtime's last-wins resolution is a scan over compiler-assigned pairs,
//! choosing between classes that are already in the stylesheet. It never builds
//! one.
//!
//! ## What is refused, and what merely degrades
//!
//! A style the interpreter cannot reduce degrades to the inline tier, which is
//! where `Computed` already lives; that is the design's documented default and
//! it is what lets folding get better wave by wave with no correctness window.
//!
//! `On` and `At` are the exception, because a pseudo-class and a media query
//! have nowhere to degrade *to*: there is no inline form of `:hover`. Styles
//! under one of those are statically known or the program is rejected
//! (`style-not-static`).

use crate::compiler::modules::Loaded;
use crate::compiler::semantics::consteval::{Env, Folder, Value};
use crate::compiler::semantics::resolve::{ModuleScope, Sym};
use crate::compiler::semantics::typed::{self, ExprKind};
use crate::compiler::semantics::types::{ConstId, FnId, Tables, Ty, TyConId};
use crate::diagnostics::{Diagnostic, Diagnostics, Span};
use crate::hash::{Map as HashMap, Set as HashSet};

// ---------------------------------------------------------------------------
// The vocabulary's tag numbers.
//
// `ui/style`'s variant order is load-bearing and its module header says so:
// the JavaScript runtime reads a `Style` by its tag, and the stylesheet is
// written in property order so that a narrower property declared later
// overrides a broader one declared earlier. These constants are the other half
// of that contract.
// ---------------------------------------------------------------------------

const STYLE_GROUP: usize = 0;
const STYLE_ON: usize = 1;
const STYLE_AT: usize = 2;
const STYLE_WHEN: usize = 3;
const STYLE_COMPUTED: usize = 4;
const STYLE_EXTRACTED: usize = 5;
/// The first variant that is one property with one value.
const FIRST_PROPERTY: usize = 6;

/// One rule in the emitted stylesheet.
///
/// Carries what it is scoped to as well as its text, because the sheet is
/// grouped by media query and ordered by property, and neither can be
/// recovered from a rendered rule.
#[derive(Clone, Debug)]
pub struct StyleRule {
    /// The class name, derived from the property, the condition and the value.
    pub class: String,
    /// The property's variant index in `Style`, which is the order rules are
    /// written in.
    pub property: u16,
    /// The pseudo-class this rule is scoped to, as a `State` variant index.
    pub state: Option<u8>,
    /// The breakpoint this rule is scoped to, as a `Screen` variant index.
    pub screen: Option<u8>,
    /// Selector suffix and declarations, one entry per rule the property needs.
    /// Almost always one entry with an empty suffix; `Layout(.Layers)` is the
    /// case that needs two, because stacking is a fact about the children.
    pub blocks: Vec<(&'static str, String)>,
}

/// A condition a style is scoped to. Both halves may be set at once —
/// `At(.Medium, [.On(.Hover, [...])])` is a hover rule inside a media query.
///
/// One state and one breakpoint, never two of either: a nested `On` replaces
/// the state it was nested in, which is what `ui/style`'s own documentation
/// says, because two pseudo-classes at once is a rule the vocabulary has no
/// way to name.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
struct Cond {
    state: Option<u8>,
    screen: Option<u8>,
}

impl Cond {
    fn is_plain(self) -> bool {
        self.state.is_none() && self.screen.is_none()
    }

    /// The class-name prefix: breakpoint first, then state, so that a reader
    /// scanning a class attribute sees the coarser scope first.
    fn prefix(self) -> String {
        let mut out = String::new();
        if let Some(s) = self.screen {
            out.push_str(SCREENS.get(s as usize).map_or("", |s| s.0));
            out.push('_');
        }
        if let Some(s) = self.state {
            out.push_str(STATES.get(s as usize).map_or("", |s| s.0));
            out.push('_');
        }
        out
    }

    /// The conflict slot's contribution. `0` in each half means "unconditional",
    /// so an unconditional rule and a hover rule never resolve against each
    /// other — which is the point: they apply at different times.
    fn code(self) -> u32 {
        let state = self.state.map_or(0, |s| u32::from(s).saturating_add(1));
        let screen = self.screen.map_or(0, |s| u32::from(s).saturating_add(1));
        state.saturating_mul(5).saturating_add(screen)
    }
}

/// `(class prefix, selector, min-width)`, in `Screen`'s declaration order.
/// Mobile-first: every one of these is a floor, so a larger tier overrides a
/// smaller one by being written later.
const SCREENS: [(&str, &str); 4] =
    [("sm", "40rem"), ("md", "48rem"), ("lg", "64rem"), ("xl", "80rem")];

/// `(class prefix, pseudo-class)`, in `State`'s declaration order.
const STATES: [(&str, &str); 5] = [
    ("hover", ":hover"),
    // `:focus-visible` rather than `:focus`, so a mouse press does not draw a
    // focus ring — which is what the vocabulary's `Focus` promises.
    ("focus", ":focus-visible"),
    ("active", ":active"),
    ("disabled", ":disabled"),
    ("checked", ":checked"),
];

// ---------------------------------------------------------------------------
// The pass
// ---------------------------------------------------------------------------

/// Extracts every static style in the compilation.
///
/// Rewrites `bodies` and `consts` in place, and answers the rules the module's
/// styles produced, in walk order. A build merges the answers of every module
/// it links; since a class names itself, merging is a dedupe and never a
/// renumbering.
///
/// A compilation that does not load `ui/style` returns immediately, which is
/// every program that is not a user interface — so this costs nothing at all
/// for the rest of the corpus.
pub fn run(
    loaded: &Loaded,
    tables: &Tables,
    scopes: &[ModuleScope],
    bodies: &mut HashMap<FnId, typed::Body>,
    consts: &mut HashMap<ConstId, typed::Expr>,
    diags: &mut Diagnostics,
    only: Option<&[crate::diagnostics::FileId]>,
) -> (Vec<StyleRule>, Option<TyConId>) {
    let Some(style_con) = style_constructor(loaded, scopes) else { return (Vec::new(), None) };
    let Some(classes_con) = ui_style_type(loaded, scopes, "Classes") else {
        return (Vec::new(), None);
    };

    // The interpreter reads bodies and constants as they were *before* this
    // pass rewrote any of them: folding a call into an already-extracted style
    // would produce a value the flattener cannot read back. Cloned rather than
    // threaded, because the rewrite needs `&mut` on the same two maps — and
    // paid for only by programs that have a `ui/style` to extract.
    let original_bodies = bodies.clone();
    let original_consts = consts.clone();

    let mut ex = Extractor {
        style_con,
        classes_con,
        tables,
        original_bodies: &original_bodies,
        original_consts: &original_consts,
        rules: Vec::new(),
        recorded: HashSet::default(),
        diags,
    };

    // Sorted, so the sheet's rule order — and any diagnostic this reports — is
    // the same on every run. A `HashMap`'s iteration order is stable here but
    // it is not *meaningful*, and reproducibility is compared byte for byte.
    //
    // A scoped analysis names the files it was asked about, and walks only
    // what they wrote. The rest of the closure is here to be *folded into*
    // those — which is what `original_bodies` above is — rather than to be
    // rewritten and reported on by a run that is not about it.
    let wanted = |file| only.is_none_or(|files| files.contains(&file));
    let mut const_ids: Vec<ConstId> = consts.keys().copied().collect();
    const_ids.sort_by_key(|c| c.index());
    for id in const_ids {
        if !wanted(tables.const_(id).span.file) {
            continue;
        }
        if let Some(init) = consts.get_mut(&id) {
            ex.walk(init, Cond::default());
        }
    }
    let mut fn_ids: Vec<FnId> = bodies.keys().copied().collect();
    fn_ids.sort_by_key(|f| f.index());
    for id in fn_ids {
        if !wanted(tables.fn_info(id).span.file) {
            continue;
        }
        if let Some(body) = bodies.get_mut(&id) {
            ex.walk(&mut body.expr, Cond::default());
        }
    }
    (ex.rules, Some(style_con))
}

/// `ui/style`'s `Style`, when this compilation loaded it.
fn style_constructor(loaded: &Loaded, scopes: &[ModuleScope]) -> Option<TyConId> {
    ui_style_type(loaded, scopes, "Style")
}

/// One of `ui/style`'s own types, by name, when this compilation loaded it.
fn ui_style_type(loaded: &Loaded, scopes: &[ModuleScope], name: &str) -> Option<TyConId> {
    let index = loaded.modules.iter().position(|m| m.path == "ui/style/lib.buri")?;
    match scopes.get(index)?.own.get(name)? {
        Sym::Ty(id) => Some(*id),
        _ => None,
    }
}

struct Extractor<'a> {
    style_con: TyConId,
    /// `ui/style`'s `Classes`, the private-field wrapper an `Extracted`
    /// payload is: only this pass and `ui/style` can build one.
    classes_con: TyConId,
    tables: &'a Tables,
    original_bodies: &'a HashMap<FnId, typed::Body>,
    original_consts: &'a HashMap<ConstId, typed::Expr>,
    rules: Vec<StyleRule>,
    recorded: HashSet<String>,
    diags: &'a mut Diagnostics,
}

impl<'a> Extractor<'a> {
    fn folder(&self) -> Folder<'a> {
        Folder::new(self.tables, self.original_bodies, self.original_consts)
    }

    fn is_style(&self, ty: &Ty) -> bool {
        matches!(ty, Ty::Con(id, args) if *id == self.style_con && args.is_empty())
    }

    fn is_style_list(&self, ty: &Ty) -> bool {
        matches!(ty, Ty::Array(elem) if self.is_style(elem))
    }

    /// Refuses a style that has to be static and is not.
    ///
    /// One code, because it is one rule — `On` and `At` are compile-time only —
    /// and the message says which of the two ways a style failed to be static,
    /// because the fix differs: a closure has to move, a value has to be
    /// written out.
    fn refuse(&mut self, span: Span, message: &str) {
        self.diags
            .items
            .push(Diagnostic::templated("style-not-static", span).with_bind("problem", message));
    }

    /// The generic descent: anything that is not itself a style.
    fn walk(&mut self, e: &mut typed::Expr, cond: Cond) {
        if self.is_style_list(&e.ty) {
            self.style_list(e, cond);
            return;
        }
        if self.is_style(&e.ty) {
            self.style(e, cond);
            return;
        }
        // A condition may not escape into an expression that is not a style:
        // `On(.Hover, [f(x)])` where `f` answers a `Style` is handled by
        // `style`, and everything else under a condition has already been
        // refused there.
        typed::children_mut(e, &mut |child| self.walk(child, cond));
    }

    /// A `[Style]` — the shape every widget takes and the shape both branches
    /// of a `When` have.
    fn style_list(&mut self, e: &mut typed::Expr, cond: Cond) {
        if let Some(atoms) = self.fold_list(e, cond) {
            let span = e.span;
            // A list that extracted to nothing becomes nothing. `ui.row([], …)`
            // is the common case and it should cost the same as it did before
            // there was a stylesheet.
            let items = if atoms.is_empty() {
                Vec::new()
            } else {
                vec![self.extracted(atoms, span)]
            };
            *e = typed::Expr::new(ExprKind::Array(items), e.ty.clone(), span);
            return;
        }
        if let ExprKind::Array(items) = &mut e.kind {
            for item in items {
                self.style(item, cond);
            }
            return;
        }
        if !cond.is_plain() {
            self.refuse(
                e.span,
                "this list of styles is not statically known, and `On` and `At` \
                 exist only in the stylesheet",
            );
            return;
        }
        typed::children_mut(e, &mut |child| self.walk(child, cond));
    }

    /// One `Style`.
    fn style(&mut self, e: &mut typed::Expr, cond: Cond) {
        if let Some(atoms) = self.fold_one(e, cond) {
            let span = e.span;
            *e = self.extracted(atoms, span);
            return;
        }
        // It did not fold whole. Take the composition apart and try the pieces:
        // `Group([.Padding(.Px(8)), .Computed(f)])` extracts its padding and
        // leaves the closure alone.
        let composition = match &e.kind {
            ExprKind::EnumLit { con, variant, .. } if *con == self.style_con => Some(*variant),
            _ => None,
        };
        if let Some(variant) = composition {
            if self.composition(e, variant, cond) {
                return;
            }
        }
        if !cond.is_plain() {
            self.refuse(
                e.span,
                "this style is not statically known, and `On` and `At` exist \
                 only in the stylesheet",
            );
            return;
        }
        typed::children_mut(e, &mut |child| self.walk(child, cond));
    }

    /// Descends one composition variant. Answers whether it handled the node.
    fn composition(&mut self, e: &mut typed::Expr, variant: usize, cond: Cond) -> bool {
        match variant {
            STYLE_GROUP => {
                let ExprKind::EnumLit { args, .. } = &mut e.kind else { return false };
                let Some(list) = args.first_mut() else { return false };
                self.style_list(list, cond);
                true
            }
            STYLE_ON | STYLE_AT => {
                // The condition is absorbed into whatever the list extracts to,
                // so the node itself is replaced by an ordinary group. A `When`
                // inside one survives: its two branches carry the condition.
                let chosen = match &e.kind {
                    ExprKind::EnumLit { args, .. } => args
                        .first()
                        .and_then(|selector| self.folder().eval(selector, &Env::default()))
                        .and_then(|value| value.as_variant().map(|(index, _)| index))
                        .and_then(|index| u8::try_from(index).ok()),
                    _ => return false,
                };
                let Some(chosen) = chosen else {
                    self.refuse(
                        e.span,
                        "which state or screen this applies to is not statically \
                         known",
                    );
                    return true;
                };
                let inner = if variant == STYLE_ON {
                    Cond { state: Some(chosen), screen: cond.screen }
                } else {
                    Cond { state: cond.state, screen: Some(chosen) }
                };
                if let ExprKind::EnumLit { args, .. } = &mut e.kind {
                    if let Some(list) = args.get_mut(1) {
                        self.style_list(list, inner);
                    }
                }
                if let ExprKind::EnumLit { variant, args, .. } = &mut e.kind {
                    *variant = STYLE_GROUP;
                    args.remove(0);
                }
                true
            }
            STYLE_WHEN => {
                if let ExprKind::EnumLit { args, .. } = &mut e.kind {
                    let mut rest = args.iter_mut();
                    // The `Prop<Bool>` is ordinary reactive data and may hold a
                    // closure with styles of its own.
                    if let Some(condition) = rest.next() {
                        typed::children_mut(condition, &mut |child| self.walk(child, Cond::default()));
                    }
                    for branch in rest {
                        self.style_list(branch, cond);
                    }
                    return true;
                }
                false
            }
            STYLE_COMPUTED => {
                if !cond.is_plain() {
                    self.refuse(
                        e.span,
                        "a `Computed` style may not appear under `On` or `At`: a \
                         closure cannot be scoped to a pseudo-class or to a media \
                         query",
                    );
                    return true;
                }
                let ExprKind::EnumLit { args, .. } = &mut e.kind else { return false };
                for arg in args {
                    typed::children_mut(arg, &mut |child| self.walk(child, Cond::default()));
                }
                true
            }
            _ => false,
        }
    }

    /// `Style::Extracted(pairs)`, as a typed expression.
    fn extracted(&self, atoms: Vec<Atom>, span: Span) -> typed::Expr {
        let int = self.tables.prim(crate::compiler::semantics::types::Prim::I64);
        let text = self.tables.prim(crate::compiler::semantics::types::Prim::Str);
        let pair = Ty::Tuple(vec![int.clone(), text.clone()]);
        let items: Vec<typed::Expr> = atoms
            .into_iter()
            .map(|a| {
                typed::Expr::new(
                    ExprKind::Tuple(vec![
                        typed::Expr::new(
                            ExprKind::Int(u128::from(a.slot), false),
                            int.clone(),
                            span,
                        ),
                        typed::Expr::new(ExprKind::Str(a.class), text.clone(), span),
                    ]),
                    pair.clone(),
                    span,
                )
            })
            .collect();
        let list = typed::Expr::new(
            ExprKind::Array(items),
            Ty::Array(Box::new(pair)),
            span,
        );
        // The payload is a `Classes`, whose one field is private to
        // `ui/style`. Writing it here is what a program cannot do.
        let classes = typed::Expr::new(
            ExprKind::StructLit {
                con: self.classes_con,
                targs: Vec::new(),
                fields: vec![list],
            },
            Ty::Con(self.classes_con, Vec::new()),
            span,
        );
        typed::Expr::new(
            ExprKind::EnumLit {
                con: self.style_con,
                targs: Vec::new(),
                variant: STYLE_EXTRACTED,
                args: vec![classes],
            },
            Ty::Con(self.style_con, Vec::new()),
            span,
        )
    }

    fn fold_one(&mut self, e: &typed::Expr, cond: Cond) -> Option<Vec<Atom>> {
        let value = self.folder().eval(e, &Env::default())?;
        let mut atoms = Vec::new();
        self.flatten(&value, cond, &mut atoms)?;
        Some(resolve_conflicts(atoms))
    }

    fn fold_list(&mut self, e: &typed::Expr, cond: Cond) -> Option<Vec<Atom>> {
        let Value::Array(items) = self.folder().eval(e, &Env::default())? else { return None };
        let mut atoms = Vec::new();
        for item in &items {
            self.flatten(item, cond, &mut atoms)?;
        }
        Some(resolve_conflicts(atoms))
    }

    /// One folded `Style` value into the atoms it is.
    fn flatten(&mut self, v: &Value, cond: Cond, out: &mut Vec<Atom>) -> Option<()> {
        let (variant, args) = v.as_variant()?;
        match variant {
            STYLE_GROUP => {
                let Value::Array(items) = args.first()? else { return None };
                for item in items {
                    self.flatten(item, cond, out)?;
                }
                Some(())
            }
            STYLE_ON => {
                let (state, _) = args.first()?.as_variant()?;
                let Value::Array(items) = args.get(1)? else { return None };
                let inner = Cond { state: Some(u8::try_from(state).ok()?), screen: cond.screen };
                for item in items {
                    self.flatten(item, inner, out)?;
                }
                Some(())
            }
            STYLE_AT => {
                let (screen, _) = args.first()?.as_variant()?;
                let Value::Array(items) = args.get(1)? else { return None };
                let inner = Cond { state: cond.state, screen: Some(u8::try_from(screen).ok()?) };
                for item in items {
                    self.flatten(item, inner, out)?;
                }
                Some(())
            }
            // A `When` is a runtime choice between two static lists, and an
            // already-extracted style has no value left to read. Both make the
            // enclosing fold give up so that the descent handles them.
            STYLE_WHEN | STYLE_COMPUTED | STYLE_EXTRACTED => None,
            _ if variant >= FIRST_PROPERTY => {
                let atom = self.property(variant, args, cond)?;
                out.push(atom);
                Some(())
            }
            _ => None,
        }
    }

    /// One property value: its class, its slot, and the rule it puts in the
    /// sheet.
    fn property(&mut self, variant: usize, args: &[Value], cond: Cond) -> Option<Atom> {
        let (abbreviation, key, blocks) = declaration(variant, args)?;
        let class = format!("{}{}-{}", cond.prefix(), abbreviation, key);
        let property = u16::try_from(variant).ok()?;
        if self.recorded.insert(class.clone()) {
            self.rules.push(StyleRule {
                class: class.clone(),
                property,
                state: cond.state,
                screen: cond.screen,
                blocks,
            });
        }
        let slot = u32::try_from(variant).ok()?.checked_mul(30)?.checked_add(cond.code())?;
        Some(Atom { slot, class })
    }
}

/// One `(slot, class)` pair, which is what reaches the runtime.
#[derive(Clone, Debug)]
struct Atom {
    slot: u32,
    class: String,
}

/// Last wins, per slot, resolved here rather than at run time — which is what
/// the design means by "when both sides of a merge are literals it resolves at
/// compile time".
fn resolve_conflicts(atoms: Vec<Atom>) -> Vec<Atom> {
    let mut out: Vec<Atom> = Vec::with_capacity(atoms.len());
    for atom in atoms {
        match out.iter_mut().find(|a| a.slot == atom.slot) {
            Some(existing) => *existing = atom,
            None => out.push(atom),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The stylesheet
// ---------------------------------------------------------------------------

/// The whole stylesheet, from every module's rules.
///
/// Three orderings, and each is a decision rather than a convenience:
///
/// * **By breakpoint, ascending, unconditional first.** Mobile-first: a rule
///   inside `@media (min-width: 48rem)` overrides the same property outside one
///   because it is written later, so a program never needs a max-width query.
/// * **By property, in `Style`'s declaration order.** Atomic classes all have
///   the same specificity, so between two properties that touch the same CSS
///   declaration — `Padding` and `PaddingX` — position in the sheet is what
///   decides. `ui/style` declares the narrower one later for exactly this.
/// * **By class name within a property**, so the sheet is a stable text.
pub fn stylesheet(rules: &[StyleRule], used: &HashSet<String>) -> String {
    let mut unique: Vec<&StyleRule> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::default();
    for rule in rules {
        // A rule nobody reaches is not shipped. Extraction walks every checked
        // body, including the ones monomorphization then drops, and a
        // stylesheet carrying a library's unused styles is the thing atomic
        // classes exist to avoid.
        if used.contains(&rule.class) && seen.insert(rule.class.as_str()) {
            unique.push(rule);
        }
    }
    unique.sort_by(|a, b| {
        (a.screen, a.property, a.state, &a.class).cmp(&(b.screen, b.property, b.state, &b.class))
    });

    let mut out = String::new();
    let mut open: Option<Option<u8>> = None;
    for rule in unique {
        if open != Some(rule.screen) {
            if open.is_some_and(|s| s.is_some()) {
                out.push_str("}\n");
            }
            if let Some(screen) = rule.screen {
                let width = SCREENS.get(screen as usize).map_or("0", |s| s.1);
                out.push_str(&format!("@media (min-width:{width}){{\n"));
            }
            open = Some(rule.screen);
        }
        let state = rule.state.and_then(|s| STATES.get(s as usize)).map_or("", |s| s.1);
        for (suffix, declarations) in &rule.blocks {
            out.push_str(&format!(".{}{state}{suffix}{{{declarations}}}\n", rule.class));
        }
    }
    if open.is_some_and(|s| s.is_some()) {
        out.push_str("}\n");
    }
    out
}

// ---------------------------------------------------------------------------
// Property -> CSS
// ---------------------------------------------------------------------------

/// What one property lowers to: the class abbreviation that names it, the
/// injective key for its value, and the `(selector suffix, declarations)`
/// blocks it renders to.
type Declaration = (&'static str, String, Vec<(&'static str, String)>);

/// A property's class abbreviation, the injective key for its value, and the
/// blocks it renders to.
///
/// Every key here is injective *within its property*, and the abbreviation is
/// unique across properties, so two different styles can never be handed the
/// same class name. Where a value is open-ended — a font name, a grid's tracks,
/// a shadow — the key is a hash of the rendered declaration, which is injective
/// by the same argument every content-addressed name in this toolchain rests
/// on.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per property in `ui/style`, in the same order, so that \
              the vocabulary and its lowering are read side by side. Splitting \
              it by section would put the ordering contract in two places."
)]
fn declaration(variant: usize, args: &[Value]) -> Option<Declaration> {
    let one = |property: &str, value: &str| vec![("", format!("{property}:{value}"))];
    let first = args.first();
    match variant {
        // arrangement
        6 => {
            let (which, inner) = first?.as_variant()?;
            match which {
                0 => Some(("lay", "col".into(), one("display", "flex;flex-direction:column"))),
                1 => Some(("lay", "row".into(), one("display", "flex;flex-direction:row"))),
                2 => {
                    let Value::Array(tracks) = inner.first()? else { return None };
                    let mut rendered = Vec::new();
                    for track in tracks {
                        rendered.push(track_value(track)?);
                    }
                    let columns = rendered.join(" ");
                    Some((
                        "lay",
                        format!("grid-{}", digest(&columns)),
                        one("display", &format!("grid;grid-template-columns:{columns}")),
                    ))
                }
                _ => Some((
                    "lay",
                    "layers".into(),
                    // Children share one cell, and the order they were written
                    // in is the order they stack in. The second block is why a
                    // rule carries a selector suffix at all.
                    vec![("", "display:grid".into()), (">*", "grid-area:1/1".into())],
                )),
            }
        }
        7 => {
            let (css, key) = align(first?)?;
            Some(("am", key.into(), one("justify-content", css)))
        }
        8 => {
            let (css, key) = align(first?)?;
            Some(("ac", key.into(), one("align-items", css)))
        }
        9 => {
            let (css, key) = align(first?)?;
            Some(("as", key.into(), one("align-self", css)))
        }
        10 => {
            let on = first?.as_bool()?;
            let css = if on { "wrap" } else { "nowrap" };
            Some(("wrap", css.into(), one("flex-wrap", css)))
        }
        11 => {
            let (which, _) = first?.as_variant()?;
            match which {
                0 => Some(("scroll", "x".into(), one("overflow-x", "auto"))),
                1 => Some(("scroll", "y".into(), one("overflow-y", "auto"))),
                _ => Some(("scroll", "both".into(), one("overflow", "auto"))),
            }
        }
        12 => count("grow", "flex-grow", first?),
        13 => count("shrink", "flex-shrink", first?),
        14 => {
            let n = first?.as_int()?;
            Some(("span", integer_key(n), one("grid-column", &format!("span {n}"))))
        }
        15 => {
            let (property, edge) = edge_property("inset", first?)?;
            let (length, key) = length(args.get(1)?)?;
            Some((
                "pin",
                format!("{edge}-{key}"),
                one("position", &format!("absolute;{property}:{length}")),
            ))
        }
        16 => {
            let (which, _) = first?.as_variant()?;
            match which {
                0 => Some(("pos", "flow".into(), one("position", "relative"))),
                1 => Some(("pos", "sticky".into(), one("position", "sticky"))),
                _ => Some(("pos", "viewport".into(), one("position", "fixed"))),
            }
        }

        // spacing
        17 => spacing("gap", "gap", first?),
        18 => spacing("gapx", "column-gap", first?),
        19 => spacing("gapy", "row-gap", first?),
        20 => spacing("p", "padding", first?),
        21 => spacing("px", "padding-inline", first?),
        22 => spacing("py", "padding-block", first?),
        23 => {
            let (property, edge) = edge_property("padding", first?)?;
            let (length, key) = length(args.get(1)?)?;
            Some(("pe", format!("{edge}-{key}"), one(&property, &length)))
        }

        // size
        24 => spacing("w", "width", first?),
        25 => spacing("h", "height", first?),
        26 => spacing("minw", "min-width", first?),
        27 => spacing("maxw", "max-width", first?),
        28 => spacing("minh", "min-height", first?),
        29 => spacing("maxh", "max-height", first?),
        30 => {
            let ratio = number(first?.as_float()?)?;
            Some(("ar", number_key(&ratio), one("aspect-ratio", &ratio)))
        }

        // paint
        31 => {
            let (css, key) = colour(first?)?;
            Some(("bg", key, one("background-color", &css)))
        }
        32 => {
            let (css, key) = colour(first?)?;
            Some(("fg", key, one("color", &css)))
        }
        33 => {
            let (css, key) = length(first?)?;
            // A width on its own draws a solid border; `BorderStyle` is
            // declared later and overrides this.
            Some(("bw", key, one("border-style", &format!("solid;border-width:{css}"))))
        }
        34 => {
            let (css, key) = colour(first?)?;
            Some(("bc", key, one("border-color", &css)))
        }
        35 => {
            let (which, _) = first?.as_variant()?;
            let css = ["none", "solid", "dashed"].get(which)?;
            Some(("bs", (*css).into(), one("border-style", css)))
        }
        36 => spacing("r", "border-radius", first?),
        37 => {
            let value = number(first?.as_float()?)?;
            Some(("op", number_key(&value), one("opacity", &value)))
        }
        38 => {
            let Value::Struct(fields) = first? else { return None };
            let (x, _) = length(fields.first()?)?;
            let (y, _) = length(fields.get(1)?)?;
            let (blur, _) = length(fields.get(2)?)?;
            let (spread, _) = length(fields.get(3)?)?;
            let (colour, _) = colour(fields.get(4)?)?;
            let css = format!("{x} {y} {blur} {spread} {colour}");
            Some(("sh", digest(&css), one("box-shadow", &css)))
        }

        // text
        39 => {
            let (which, inner) = first?.as_variant()?;
            let stack = match which {
                0 => "ui-sans-serif,system-ui,sans-serif".to_string(),
                1 => "ui-serif,Georgia,serif".to_string(),
                2 => "ui-monospace,SFMono-Regular,monospace".to_string(),
                _ => {
                    let name = inner.first()?.as_str()?;
                    // A name that would need escaping is refused rather than
                    // escaped: it degrades to an inline style, and the
                    // stylesheet never contains text this did not construct.
                    if name.is_empty()
                        || !name.chars().all(|c| c.is_ascii_alphanumeric() || " _-".contains(c))
                    {
                        return None;
                    }
                    format!("\"{name}\",ui-sans-serif,sans-serif")
                }
            };
            Some(("ff", digest(&stack), one("font-family", &stack)))
        }
        40 => spacing("fs", "font-size", first?),
        41 => {
            let (which, _) = first?.as_variant()?;
            let (css, key) = [("400", "regular"), ("500", "medium"), ("600", "semibold"), ("700", "bold")]
                .get(which)
                .copied()?;
            Some(("fw", key.into(), one("font-weight", css)))
        }
        42 => {
            let on = first?.as_bool()?;
            let css = if on { "italic" } else { "normal" };
            Some(("it", css.into(), one("font-style", css)))
        }
        43 => {
            let value = number(first?.as_float()?)?;
            Some(("lh", number_key(&value), one("line-height", &value)))
        }
        44 => spacing("ls", "letter-spacing", first?),
        45 => {
            let (_, key) = align(first?)?;
            // Text has no leftover room to distribute, so every distribution
            // means justified.
            let css = match key {
                "start" | "center" | "end" => key,
                _ => "justify",
            };
            Some(("ta", key.into(), one("text-align", css)))
        }
        46 => {
            let (which, _) = first?.as_variant()?;
            let (css, key) = [
                ("none", "aswritten"),
                ("uppercase", "upper"),
                ("lowercase", "lower"),
                ("capitalize", "capitalize"),
            ]
            .get(which)
            .copied()?;
            Some(("tc", key.into(), one("text-transform", css)))
        }
        47 => {
            let (which, _) = first?.as_variant()?;
            let (css, key) =
                [("none", "none"), ("underline", "underline"), ("line-through", "strike")]
                    .get(which)
                    .copied()?;
            Some(("tl", key.into(), one("text-decoration-line", css)))
        }
        48 => {
            let (which, _) = first?.as_variant()?;
            let css = ["wrap", "nowrap", "balance"].get(which)?;
            Some(("tw", (*css).into(), one("text-wrap", css)))
        }
        49 => {
            let lines = first?.as_int()?;
            if lines <= 0 {
                return Some((
                    "trunc",
                    "0".into(),
                    one("-webkit-line-clamp", "none;overflow:visible"),
                ));
            }
            Some((
                "trunc",
                integer_key(lines),
                one(
                    "display",
                    &format!(
                        "-webkit-box;-webkit-box-orient:vertical;\
                         -webkit-line-clamp:{lines};overflow:hidden"
                    ),
                ),
            ))
        }

        // interaction
        50 => {
            let (which, _) = first?.as_variant()?;
            let (css, key) = [
                ("auto", "default"),
                ("pointer", "pointer"),
                ("text", "text"),
                ("not-allowed", "notallowed"),
            ]
            .get(which)
            .copied()?;
            Some(("cur", key.into(), one("cursor", css)))
        }
        _ => None,
    }
}

/// A property whose only value is a `Length`.
fn spacing(abbreviation: &'static str, property: &str, value: &Value) -> Option<Declaration> {
    let (css, key) = length(value)?;
    Some((abbreviation, key, vec![("", format!("{property}:{css}"))]))
}

/// A property whose only value is a count.
fn count(abbreviation: &'static str, property: &str, value: &Value) -> Option<Declaration> {
    let n = value.as_int()?;
    Some((abbreviation, integer_key(n), vec![("", format!("{property}:{n}"))]))
}

/// One logical edge, as the CSS property it names under a prefix.
fn edge_property(prefix: &str, value: &Value) -> Option<(String, &'static str)> {
    let (which, _) = value.as_variant()?;
    let (suffix, key) = [
        ("block-start", "top"),
        ("block-end", "bottom"),
        ("inline-start", "start"),
        ("inline-end", "end"),
    ]
    .get(which)
    .copied()?;
    Some((format!("{prefix}-{suffix}"), key))
}

fn length(value: &Value) -> Option<(String, String)> {
    let (which, args) = value.as_variant()?;
    match which {
        0 => {
            let n = args.first()?.as_int()?;
            Some((format!("{n}px"), integer_key(n)))
        }
        1 => {
            let n = number(args.first()?.as_float()?)?;
            Some((format!("{n}rem"), format!("r{}", number_key(&n))))
        }
        2 => {
            let n = number(args.first()?.as_float()?)?;
            Some((format!("{n}%"), format!("pc{}", number_key(&n))))
        }
        3 => Some(("auto".into(), "auto".into())),
        4 => Some(("100%".into(), "full".into())),
        _ => None,
    }
}

fn colour(value: &Value) -> Option<(String, String)> {
    let (which, args) = value.as_variant()?;
    let channel = |v: Option<&Value>| -> Option<u8> {
        u8::try_from(v?.as_int()?).ok()
    };
    match which {
        0 => {
            let (r, g, b) =
                (channel(args.first())?, channel(args.get(1))?, channel(args.get(2))?);
            Some((format!("rgb({r},{g},{b})"), format!("{r:02x}{g:02x}{b:02x}")))
        }
        1 => {
            let (r, g, b) =
                (channel(args.first())?, channel(args.get(1))?, channel(args.get(2))?);
            let a = number(args.get(3)?.as_float()?)?;
            Some((
                format!("rgba({r},{g},{b},{a})"),
                format!("{r:02x}{g:02x}{b:02x}a{}", number_key(&a)),
            ))
        }
        // A design token. It lowers to a custom property rather than to a
        // colour, which is the whole of what makes a theme cost nothing: the
        // class is decided here, at compile time, and installing or switching a
        // theme only ever changes what `var(--cardlib-surface)` reads as.
        2 => {
            let Value::Struct(parts) = args.first()? else { return None };
            let namespace = parts.first()?.as_str()?;
            let name = parts.get(1)?.as_str()?;
            Some((format!("var(--{namespace}-{name})"), token_key(namespace, name)))
        }
        3 => Some(("transparent".into(), "none".into())),
        4 => Some(("inherit".into(), "inherit".into())),
        _ => None,
    }
}

/// A token reference as a class-name fragment.
///
/// A namespace and a name that are plain letters and digits become part of the
/// class, so that `.bg-t_cardlib_surface` says what it is on sight. Anything
/// else falls back to a digest of the pair, because a class name has a smaller
/// alphabet than a string does and two different tokens must never be handed
/// one class. The separator is a byte no Buri identifier holds, so the digest
/// cannot be made to collide by choosing names that run together.
fn token_key(namespace: &str, name: &str) -> String {
    let plain = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric());
    if plain(namespace) && plain(name) {
        format!("t_{namespace}_{name}")
    } else {
        format!("t_{}", digest(&format!("{namespace}\u{1}{name}")))
    }
}

fn align(value: &Value) -> Option<(&'static str, &'static str)> {
    let (which, _) = value.as_variant()?;
    [
        ("flex-start", "start"),
        ("center", "center"),
        ("flex-end", "end"),
        ("stretch", "stretch"),
        ("space-between", "between"),
        ("space-around", "around"),
        ("space-evenly", "evenly"),
    ]
    .get(which)
    .copied()
}

fn track_value(value: &Value) -> Option<String> {
    let (which, args) = value.as_variant()?;
    match which {
        0 => {
            let n = args.first()?.as_int()?;
            Some(format!("{n}fr"))
        }
        1 => Some(length(args.first()?)?.0),
        _ => Some("auto".into()),
    }
}

/// A float, rendered the one way this compiler renders one.
///
/// Not finite means not foldable: there is no CSS for a NaN, and picking one
/// would be inventing a value the program did not write.
fn number(v: f64) -> Option<String> {
    v.is_finite().then(|| format!("{v}"))
}

/// A rendered number as a class-name fragment: a decimal point is not legal in
/// a class name unescaped, and a minus sign reads as a range.
fn number_key(rendered: &str) -> String {
    rendered.replace('.', "_").replace('-', "n")
}

fn integer_key(n: i128) -> String {
    if n < 0 {
        format!("n{}", n.unsigned_abs())
    } else {
        format!("{n}")
    }
}

/// Eight hex digits of the declaration, for a value with no bounded spelling.
fn digest(text: &str) -> String {
    let full = crate::build::sha256::hash_bytes(text.as_bytes());
    full.get(..8).unwrap_or(&full).to_string()
}

// ---------------------------------------------------------------------------
// Walking
// ---------------------------------------------------------------------------

/// What a walk over the functions a program kept found out about its styles.
#[derive(Default)]
pub struct Reached {
    /// The classes the program's already-extracted styles name, so that the
    /// stylesheet holds the rules a program reaches and no others.
    pub classes: HashSet<String>,
    /// Whether any style in the program can reach the **inline tier** — the
    /// run-time lowering of one property to a `style=` declaration, which the
    /// runtime spells `$tree_declare` and which is 3.5 KB of an artifact.
    ///
    /// True for a `Computed`, whose list is built per run and so cannot be
    /// extracted, and for any bare property left standing: extraction rewrites
    /// a style it could evaluate *in place* into an `Extracted`, so a property
    /// variant that survives to here is one the compiler could not fold and
    /// which degrades to inline rather than failing.
    ///
    /// An over-approximation is safe and an under-approximation is not, so the
    /// question is asked of the tags rather than of what a tag might hold: a
    /// property inside a `Computed`'s closure is one of these too, and counting
    /// it twice costs nothing. Complete because a `Style` has no constructor
    /// but an enum literal — a value reaching a `Computed` from anywhere at all
    /// was written down somewhere in the program.
    pub inline: bool,
}

/// The styles an expression names, gathered for [`Reached`].
///
/// Read after monomorphization, over the functions a program actually kept.
pub fn collect(e: &mut typed::Expr, style_con: TyConId, out: &mut Reached) {
    if let ExprKind::EnumLit { con, variant, args, .. } = &e.kind {
        if *con == style_con {
            match *variant {
                STYLE_EXTRACTED => {
                    // `Extracted(Classes([(slot, class), …]))`: one field, and
                    // the list is inside it.
                    let list = match args.first() {
                        Some(typed::Expr { kind: ExprKind::StructLit { fields, .. }, .. }) => {
                            fields.first()
                        }
                        _ => None,
                    };
                    if let Some(typed::Expr { kind: ExprKind::Array(items), .. }) = list {
                        for item in items {
                            let ExprKind::Tuple(pair) = &item.kind else { continue };
                            if let Some(typed::Expr { kind: ExprKind::Str(class), .. }) =
                                pair.get(1)
                            {
                                out.classes.insert(class.clone());
                            }
                        }
                    }
                }
                // `Group` is transparent and `When` holds two lists that were
                // extracted like any other; neither is a property of its own.
                // `On` and `At` exist only in the stylesheet — one that reached
                // the runtime aborts there — so they are not the inline tier
                // either, and the checker has already refused the shapes that
                // could produce one.
                STYLE_GROUP | STYLE_WHEN | STYLE_ON | STYLE_AT => {}
                _ => out.inline = true,
            }
        }
    }
    typed::children_mut(e, &mut |child| collect(child, style_con, out));
}

/// Whether an expression builds a `ui/theme` `Theme`.
///
/// The same question as [`Reached::inline`] and asked the same way: a `Theme`
/// is an opaque struct with a private field, wrapping a private enum, and the
/// only two literals of it are written inside `themed` and `switching`. A
/// program that monomorphized neither can hand `mount` nothing but an empty
/// list, and the whole theme half of the runtime — resolution, rendering, the
/// `:root` block, the switch's computation — is unreachable.
pub fn builds_a_theme(e: &mut typed::Expr, theme_con: TyConId) -> bool {
    if matches!(&e.kind, ExprKind::StructLit { con, .. } if *con == theme_con) {
        return true;
    }
    let mut found = false;
    typed::children_mut(e, &mut |child| found |= builds_a_theme(child, theme_con));
    found
}
