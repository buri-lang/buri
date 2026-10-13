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
//! ## Where a style is extracted
//!
//! Only where it reaches a node. To the rest of the program a `Style` is an
//! ordinary enum: a match on one, or a comparison, sees the variant the source
//! wrote. [`Flow`] finds the positions whose value a node is given: a field of
//! a `ui/node` literal, an argument to a `ui/node` function, and whatever flows
//! into one of those through a local, a parameter, a result or a constant.
//! A style that folds where a node is given it is extracted right there, so
//! the `let`, field or function it came from keeps the variant it names.
//!
//! A node given a parameter, a field or a result instead has it wrapped in
//! `ui/style/classify`'s `classify`, which names each class the way this pass does and
//! looks it up among the `(name, class)` pairs this pass found for the values
//! that can reach it. Those values stay what the source wrote, and the sheet
//! holds their classes because [`survey`] counts each one where it's written.
//! When the pass saw every value that can reach a `classify`, a test build
//! stops on a name it found no class for: that is a compiler bug.
//!
//! ## What the runtime is left with
//!
//! A style that folded is replaced, in the typed tree, by the
//! `Style::Extracted` variant: a `Classes`, holding a list of `(slot, class)`
//! pairs. `Classes` has a private field, so only `ui/style` and this pass can
//! build one, which is what keeps the variant unwritable. The *slot* is the
//! conflict key — the property, its condition, and the edge or corner where the
//! property names one — so the runtime's last-wins resolution is a scan over
//! compiler-assigned pairs, choosing between classes that are already in the
//! stylesheet. It never builds one.
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
use crate::compiler::semantics::consteval::{bound, Env, Folder, Value};
use crate::compiler::semantics::layered::Layered;
use crate::compiler::semantics::resolve::{
    own_fn, own_type, BodyMap, ConstMap, ModuleScope, Walked,
};
use crate::compiler::semantics::typed::{self, ExprKind, Magnitude};
use crate::compiler::semantics::types::{
    ConstId, FnId, FuncIdx, LocalId, ModuleId, Tables, TraitId, Ty, TyConId, TyKind,
};
use crate::diagnostics::{invariant, Diagnostic, Diagnostics, Invariant as _, Span};
use crate::hash::{Map as HashMap, Set as HashSet};
use std::cell::RefCell;

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
/// `Pin(Edge, Length)`.
const STYLE_PIN: usize = 15;
/// `PaddingEdge(Edge, Length)`.
const STYLE_PADDING_EDGE: usize = 23;
/// `BorderWidth(Length)`.
const STYLE_BORDER_WIDTH: usize = 33;
/// `BorderEdge(Edge, Length)`.
const STYLE_BORDER_EDGE: usize = 34;
/// `RadiusCorner(Corner, Length)`.
const STYLE_RADIUS_CORNER: usize = 38;
/// `Shadow(Shadow)`.
const STYLE_SHADOW: usize = 40;
/// `Shadows([Shadow])`, which writes the same declaration and so shares
/// `Shadow`'s conflict slot.
const STYLE_SHADOWS: usize = 41;
/// `ListMarker(ListMarker)`.
const STYLE_LIST_MARKER: usize = 54;
/// `Bleed(Edge, Length)`, declared last because nothing else writes a margin.
const STYLE_BLEED: usize = 55;
/// `Animation`: the one property whose rules are guarded by the reader's motion
/// preference, and the one that needs keyframes beside its rules.
const STYLE_ANIMATION: usize = 62;

/// `ui/style`'s `State::Focus`, which is `:focus-visible`. The one state the
/// sheet says anything about beyond the class that names it.
const STATE_FOCUS: u8 = 1;

/// `ui/style`'s `Color`, for the two variants that carry an alpha: `Rgba`, and
/// the `Faded` token `alpha` answers for one.
const COLOR_RGBA: usize = 1;
const COLOR_FADED: usize = 5;

// `ui/node`'s `NodeKind`, whose variant order is load-bearing for the same
// reason and says so in its own comment. Only the ones that lower to an
// element a browser paints chrome on are named here.
const NODE_HEADING: usize = 2;
const NODE_BUTTON: usize = 5;
/// An `<a>`. `link` and `routeLink` are one variant now — the anchor a route
/// link renders is a link's — so this one index carries the anchor reset for
/// both.
const NODE_LINK: usize = 6;
const NODE_IMAGE: usize = 7;
const NODE_ICON: usize = 8;
const NODE_FIELD: usize = 9;
/// A `<input type="range">`, which draws a track and a thumb of its own. The
/// range chrome-reset lives inside the field reset — it is what sized the old
/// `FieldKind.Range` — so a slider asks for the same `field` reset even where
/// the program builds no text field.
const NODE_SLIDER: usize = 10;
const NODE_TOGGLE: usize = 11;
/// A picker, whose options are `<input type="radio">` drawing a dot of their
/// own, so it takes the same radio-group reset.
const NODE_PICKER: usize = 12;
/// A form's action, which is a `<button>` like `button`'s and takes the same
/// reset.
const NODE_SUBMIT: usize = 14;
const NODE_DIALOG: usize = 15;
/// `progress` lowers to a plain `<div>`, which the container reset already
/// reaches, so it needs no chrome cleared and is not named here. `disclosure`
/// lowers to `<details>`/`<summary>`, which a browser paints a marker and a
/// block layout on by itself.
const NODE_DISCLOSURE: usize = 16;
/// A file picker, whose control is a `<button>` like `button`'s and takes the
/// same reset. Its `<input type="file">` is `hidden`, so it needs none.
const NODE_FILE_PICKER: usize = 23;
/// A tooltip, whose bubble a browser has to be told where to put and to hide:
/// the reset's `display` beats a browser's own `[hidden]`.
const NODE_TOOLTIP: usize = 24;

/// `ui/node`'s `Role::List` and `Role::Separator`, the two roles that lower to
/// an element a browser paints something on by itself. A role is written at the
/// call site rather than inside `region`, so these are the literals the walk
/// looks for.
const ROLE_LIST: usize = 7;
const ROLE_SEPARATOR: usize = 10;
/// `ui/node`'s `Role::Table`, which lowers to a real `<table>`. A browser's own
/// sheet leaves `border-collapse: separate; border-spacing: 2px` standing, so a
/// 1px border on each box paints two pixels from its neighbour's and the rule a
/// design system draws across a row comes out broken. The reset collapses the
/// borders, which is the continuous line the headless painter already draws and
/// what every design system's base layer does.
const ROLE_TABLE: usize = 13;

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

/// How many conditions a property divides into: eight states counting "none",
/// by five breakpoints counting "none", which is what `Cond::code` numbers.
///
/// `ui/node`'s `slotState` divides a slot by this to read a condition back, so
/// a state added to `ui/style` moves both.
const CONDITIONS: u32 = 40;

/// How many sub-keys a property divides into: the four `Edge`s — which is also
/// the four `Corner`s — and the one a `Pin` keeps its `position` in.
const SUB_KEYS: u32 = 5;

/// The sub-key a `Pin`'s `position` sits in, past the four edges.
const PIN_FLOW: u32 = 4;

/// The conflict slot: the property, the sub-key where the property has one,
/// and the condition, packed into one number the runtime compares for equality.
///
/// The sub-key is what stops two `Pin`s from colliding. A property is usually
/// one declaration, so its variant is the whole key — but `Pin`, `PaddingEdge`
/// `BorderEdge` and `Bleed` name an *edge* and `RadiusCorner` names a *corner*,
/// and two of them naming different ones write different declarations and
/// compose. `ui/style` says so in each variant's documentation, and the sub-key
/// is how the slot says it too.
///
/// The one place the key is *not* the variant is `Shadows`, which writes the
/// same `box-shadow` `Shadow` does. Two spellings of one declaration are one
/// slot, so the last written wins rather than both landing on the element.
fn slot(variant: usize, sub: u32, cond: Cond) -> Option<u32> {
    let variant = if variant == STYLE_SHADOWS {
        STYLE_SHADOW
    } else {
        variant
    };
    let variant = u32::try_from(variant).ok()?;
    variant
        .checked_mul(SUB_KEYS)?
        .checked_add(sub)?
        .checked_mul(CONDITIONS)?
        .checked_add(cond.code())
}

/// The variable-backed lowering for a dynamic value the compiler could not fold
/// (#195): the class abbreviation that names it, and the CSS property the class
/// sets. A dynamic value is written inline as the custom property
/// `--buri-<abbreviation>`, and this rule — `<property>:var(--buri-<abbr>)` on
/// `.<abbr>-var` — reads it out, so the property comes from the stylesheet like
/// any other and an `At`/`On` rule can still override it at its breakpoint,
/// rather than an inline declaration outranking every rule at every width.
///
/// `Some` only for the properties whose declaration is one property with a
/// fixed name and one leaf value — the shape a parameter is threaded through,
/// which is the case #195 is about. An edge- or corner-named property, and one
/// that writes more than a single declaration, still degrades to an inline
/// declaration: its CSS property name is not fixed at compile time, so there is
/// no one class the variable could feed. `Shadows` shares `Shadow`'s slot and
/// class, so it is folded onto `Shadow` here the way `slot` folds it.
fn variable_property(variant: usize) -> Option<(&'static str, &'static str)> {
    let variant = if variant == STYLE_SHADOWS {
        STYLE_SHADOW
    } else {
        variant
    };
    let pair = match variant {
        12 => ("grow", "flex-grow"),
        13 => ("shrink", "flex-shrink"),
        14 => ("span", "grid-column"),
        17 => ("gap", "gap"),
        18 => ("gapx", "column-gap"),
        19 => ("gapy", "row-gap"),
        20 => ("p", "padding"),
        21 => ("px", "padding-inline"),
        22 => ("py", "padding-block"),
        24 => ("w", "width"),
        25 => ("h", "height"),
        26 => ("minw", "min-width"),
        27 => ("maxw", "max-width"),
        28 => ("minh", "min-height"),
        29 => ("maxh", "max-height"),
        30 => ("ar", "aspect-ratio"),
        31 => ("bg", "background-color"),
        32 => ("fg", "color"),
        35 => ("bc", "border-color"),
        37 => ("r", "border-radius"),
        39 => ("op", "opacity"),
        40 => ("sh", "box-shadow"),
        43 => ("fs", "font-size"),
        46 => ("lh", "line-height"),
        47 => ("ls", "letter-spacing"),
        59 => ("bdblur", "backdrop-filter"),
        60 => ("caret", "caret-color"),
        STYLE_ANIMATION => ("anim", "animation"),
        _ => return None,
    };
    Some(pair)
}

/// The sub-key a property's value falls in: the edge or corner it names, or
/// nothing. Both are four values, so one numbering serves both.
fn sub_key(variant: usize, args: &[Value]) -> u32 {
    match variant {
        STYLE_PIN | STYLE_PADDING_EDGE | STYLE_BORDER_EDGE | STYLE_RADIUS_CORNER | STYLE_BLEED => {
            args.first()
                .and_then(Value::as_variant)
                .and_then(|(edge, _)| u32::try_from(edge).ok())
                .unwrap_or(0)
        }
        _ => 0,
    }
}

/// `(class prefix, selector, min-width)`, in `Screen`'s declaration order.
/// Mobile-first: every one of these is a floor, so a larger tier overrides a
/// smaller one by being written later.
const SCREENS: [(&str, &str); 4] = [
    ("sm", "40rem"),
    ("md", "48rem"),
    ("lg", "64rem"),
    ("xl", "80rem"),
];

/// `(class prefix, selector suffix)`, in `State`'s declaration order.
const STATES: [(&str, &str); 7] = [
    ("hover", ":hover"),
    // `:focus-visible` rather than `:focus`, so a mouse press does not draw a
    // focus ring — which is what the vocabulary's `Focus` promises.
    ("focus", ":focus-visible"),
    // The container's state rather than the element's: something inside it has
    // the keyboard, which is what an input group rings on.
    ("focuswithin", ":focus-within"),
    ("active", ":active"),
    ("disabled", ":disabled"),
    ("checked", ":checked"),
    // An attribute rather than a pseudo-class, and the one state that is: a
    // program marks a control invalid and `:invalid` is a browser's own
    // verdict on its own constraints, which no program can reach. The
    // attribute is what a reader is told too, so the ring and the
    // announcement are one fact.
    ("invalid", "[aria-invalid=true]"),
];

// ---------------------------------------------------------------------------
// The pass
// ---------------------------------------------------------------------------

/// What one run of the pass found, kept apart by where it was found.
///
/// Apart because a run over a compilation that opens with a base
/// (`resolve::Base`) walks only what came after it, and the rules have to come
/// out in the order one run over everything would have found them in:
/// constants before bodies, and each in id order, which puts the base's first
/// within each. [`Styled::after`] is that merge.
#[derive(Default)]
pub struct Styled {
    /// The rules the constants produced, in walk order.
    pub const_rules: Vec<StyleRule>,
    /// The rules the bodies produced, in walk order.
    pub body_rules: Vec<StyleRule>,
    /// The classes each style that folded where no node is given it stands
    /// for, by where it was written: what [`survey`] counts in its place.
    pub sources: Sources,
    /// What the base's constants and bodies are once the pass has rewritten
    /// them, where it did. Empty for a run that is not a base's.
    pub consts: ConstMap,
    pub bodies: BodyMap,
}

/// The classes a style left standing as a value stands for, by its span.
pub type Sources = HashMap<Span, Vec<String>>;

impl Styled {
    /// The rules of a run over what came after `base`, merged with the base's,
    /// with the base's rewrites put back into the maps the run left them out
    /// of, and the two runs' sources together.
    ///
    /// A class names itself, so a rule both runs found is one rule, kept
    /// where it was found first.
    pub fn after(
        self,
        base: &Styled,
        bodies: &mut BodyMap,
        consts: &mut ConstMap,
    ) -> (Vec<StyleRule>, Sources) {
        for (id, body) in &base.bodies {
            bodies.insert(id, std::sync::Arc::clone(body));
        }
        for (id, init) in &base.consts {
            consts.insert(id, init.clone());
        }
        let mut seen: HashSet<String> = HashSet::default();
        let rules = base
            .const_rules
            .iter()
            .chain(&self.const_rules)
            .chain(&base.body_rules)
            .chain(&self.body_rules)
            .filter(|r| seen.insert(r.class.clone()))
            .cloned()
            .collect();
        let mut sources = base.sources.clone();
        sources.extend(self.sources);
        (rules, sources)
    }
}

/// Extracts every static style that reaches a node.
///
/// Rewrites `bodies` and `consts` in place, and answers the rules the module's
/// styles produced, in walk order, constants' first. A build merges the answers of every module
/// it links; since a class names itself, merging is a dedupe and never a
/// renumbering.
///
/// A compilation none of whose own modules imports from `ui/` returns
/// immediately, which is every program that is not a user interface, test
/// builds included — so this costs nothing at all for the rest of the corpus.
pub fn run(
    loaded: &Loaded,
    tables: &Tables,
    scopes: &Layered<ModuleScope>,
    bodies: &mut BodyMap,
    consts: &mut ConstMap,
    diags: &mut Diagnostics,
    walked: &Walked,
) -> (Styled, Option<TyConId>) {
    // `ui/style` declares both, so a program has both or neither.
    let (Some(style_con), Some(classes_con)) = (
        own_type(loaded, scopes, "ui/style", "Style"),
        own_type(loaded, scopes, "ui/style", "Classes"),
    ) else {
        return (Styled::default(), None);
    };
    // Only a module of the program's own that imports from `ui/` can build a
    // node, and only such a program loads `classify`. A test build without
    // one still loads `ui/style` through the test platform, but no style can
    // reach a node there.
    let Some((classify, classify_style)) = classifiers(loaded, scopes) else {
        return (Styled::default(), Some(style_con));
    };
    let extracted =
        own_fn(loaded, scopes, "ui/style", "extracted").or_ice("`ui/style` declares `extracted`");

    // The interpreter reads bodies and constants as they were *before* this
    // pass rewrote any of them: folding a call into an already-extracted style
    // would produce a value the flattener cannot read back. So the maps stay
    // untouched while the walk reads them, each rewrite is made on a copy of
    // the one declaration it changes, and the copies go back in at the end.
    // Copying the maps instead held a second `Arc` to every body, which made
    // the rewrite deep-copy what it changed anyway, and deep-copied every
    // constant whether it changed or not.
    let flow = Flow::of(
        tables,
        style_con,
        loaded.find("ui/node"),
        walked,
        bodies,
        consts,
    );
    let mut ex = Extractor {
        style_con,
        classes_con,
        color_con: own_type(loaded, scopes, "ui/style", "Color"),
        classify,
        classify_style,
        extracted,
        tables,
        original_bodies: bodies,
        original_consts: consts,
        rules: Vec::new(),
        recorded: HashSet::default(),
        diags,
        flow,
        owner: Owner::Fn(extracted),
        mode: Mode::Rewrite,
        wraps: false,
        directs: HashMap::default(),
        sources: HashMap::default(),
        tainted: HashSet::default(),
        extra: Edges::default(),
        wrapped: Vec::new(),
        leafy: HashSet::default(),
    };

    // In id order, which is the order the maps walk in, so the sheet's rule
    // order — and any diagnostic this reports — is the same on every run.
    //
    // A scoped analysis names the files it was asked about, and walks only
    // what they wrote. The rest of the closure is here to be *folded into*
    // those — which is what `original_bodies` above is — rather than to be
    // rewritten and reported on by a run that is not about it.
    //
    // Only what mentions a style is rewritten. Walking anything else changes
    // nothing, and leaving it alone keeps it shared rather than copied.
    let mut new_consts: Vec<(ConstId, typed::Expr)> = Vec::new();
    for (id, init) in walked.constants(tables, consts) {
        ex.alphas(init);
        if ex.mentions_style(init) {
            new_consts.push((id, init.clone()));
        }
    }
    let mut new_bodies: Vec<(FnId, typed::Body)> = Vec::new();
    for (id, body) in walked.functions(tables, bodies) {
        ex.alphas(&body.expr);
        if ex.mentions_style(&body.expr) {
            new_bodies.push((id, typed::Body::clone(body)));
        }
    }
    ex.walk(&mut new_consts, &mut new_bodies, true);
    ex.recorded.clear();
    let (const_rules, body_rules) = ex.walk(&mut new_consts, &mut new_bodies, false);
    ex.fill(&mut new_consts, &mut new_bodies);
    let styled = Styled {
        const_rules,
        body_rules,
        sources: ex.sources,
        ..Styled::default()
    };
    for (id, init) in new_consts {
        consts.insert(id, init);
    }
    for (id, body) in new_bodies {
        bodies.insert(id, std::sync::Arc::new(body));
    }
    (styled, Some(style_con))
}

/// How many times this process drew [`Flow`]'s graph of what reads what, over
/// every declaration a compilation reads. `BURI_PROFILE` prints it, because
/// unlike instructions it is the same on every run.
static GRAPHS_DRAWN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// See [`GRAPHS_DRAWN`].
pub fn graphs_drawn() -> u64 {
    GRAPHS_DRAWN.load(std::sync::atomic::Ordering::Relaxed)
}

/// `ui/style/classify`'s `classify` and `classifyStyle`: what a style a node is
/// given goes through where the compiler can't say which value it is. Not
/// there in a program none of whose own modules imports from `ui/`.
pub fn classifiers(loaded: &Loaded, scopes: &Layered<ModuleScope>) -> Option<(FnId, FnId)> {
    let module = "ui/style/classify";
    Some((
        own_fn(loaded, scopes, module, "classify")?,
        own_fn(loaded, scopes, module, "classifyStyle")?,
    ))
}

struct Extractor<'a> {
    style_con: TyConId,
    /// `ui/style`'s `Classes`, the private-field wrapper an `Extracted`
    /// payload is: only this pass and `ui/style` can build one.
    classes_con: TyConId,
    /// `ui/style`'s `Color`, which is how [`Extractor::alphas`] recognises one.
    color_con: Option<TyConId>,
    /// `ui/style/classify`'s `classify`, for a list of styles, and
    /// `classifyStyle`, for one.
    classify: FnId,
    classify_style: FnId,
    /// `ui/style`'s `extracted`, which `classify` is handed to build the
    /// `Classes` it can't.
    extracted: FnId,
    tables: &'a Tables,
    original_bodies: &'a BodyMap,
    original_consts: &'a ConstMap,
    rules: Vec<StyleRule>,
    recorded: HashSet<String>,
    diags: &'a mut Diagnostics,
    flow: Flow<'a>,
    /// The declaration whose locals the walk is in.
    owner: Owner,
    mode: Mode,
    /// Whether a style a node is given here that is not a literal goes
    /// through `classify`: not while checking, and not inside `ui/node`, whose
    /// callers are where its styles are written.
    wraps: bool,
    /// The `(name, class)` pairs of each style that folded where it was read
    /// as a demand rather than given to a node, by that demand.
    directs: HashMap<Demand, Vec<(String, String)>>,
    sources: Sources,
    /// The demands a value the compiler did not see can reach: one built from
    /// a value known only at run time, or answered by a function this run
    /// does not read.
    tainted: HashSet<Demand>,
    /// What each [`Demand::Leaf`] reads.
    extra: Edges,
    /// Each leaf, in the order its `classify` was written.
    wrapped: Vec<Demand>,
    /// The declarations holding one.
    leafy: HashSet<Owner>,
}

/// What a walk does with the styles it reaches.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Extracts what a node is given, and refuses what can't be.
    Rewrite,
    /// Refuses what [`Flow::reached`] says a node reads, and rewrites nothing.
    Checking,
    /// Extracts a declaration `Checking` walked, so refuses nothing.
    Quiet,
}

/// Where the value of an expression a walk visits goes.
#[derive(Clone, Copy)]
struct Sink {
    /// The demand it answers: [`Demand::Node`] where a node is given it.
    to: Demand,
    /// Whether a node is given it right where it is written, rather than
    /// through a local, a parameter, a result or a constant.
    site: bool,
}

impl<'a> Extractor<'a> {
    fn folder(&self) -> Folder<'a> {
        Folder::new(self.tables, self.original_bodies, self.original_consts)
    }

    fn is_style(&self, ty: &Ty) -> bool {
        self.flow.is_style(ty)
    }

    fn is_style_list(&self, ty: &Ty) -> bool {
        self.flow.is_style_list(ty)
    }

    /// Refuses a style that has to be static and is not.
    ///
    /// One code, because it is one rule — `On` and `At` are compile-time only —
    /// and the message says which of the two ways a style failed to be static,
    /// because the fix differs: a closure has to move, a value has to be
    /// written out.
    fn refuse(&mut self, span: Span, message: &str) {
        if self.mode == Mode::Quiet {
            return;
        }
        self.diags
            .items
            .push(Diagnostic::templated("style-not-static", span).with_bind("problem", message));
    }

    /// Refuses every colour whose alpha is outside `0.0..=1.0`.
    ///
    /// Its own walk, before the extraction one, for two reasons. A colour has
    /// to be refused wherever it was written — inside a `Computed`'s closure
    /// as much as in a folded list — and the extraction walk reaches a folded
    /// style's colour only as part of the value it folded, which has no span
    /// of its own. And it fires **once**: a colour that folded is not descended
    /// into, and a bare `Const` reference is left to the declaration that wrote
    /// it, so a module-level colour is reported where it is written rather than
    /// once more at every use.
    fn alphas(&mut self, e: &typed::Expr) {
        if self.is_color(&e.ty) && !matches!(e.kind, ExprKind::Const(_)) {
            if let Some(value) = self.folder().eval(e, &Env::default()) {
                if let Some(a) = alpha_of(&value) {
                    if !(0.0..=1.0).contains(&a) {
                        self.diags.items.push(
                            Diagnostic::templated("alpha-out-of-range", e.span)
                                .with_bind("alpha", a.to_string()),
                        );
                    }
                }
                return;
            }
        }
        typed::children(e, &mut |child| self.alphas(child));
    }

    fn is_color(&self, ty: &Ty) -> bool {
        // `Color` takes no type arguments either.
        matches!(ty.kind(), TyKind::Con(id, _) if Some(*id) == self.color_con)
    }

    /// Whether anything under `e` is a style or a list of them: the only
    /// nodes [`Extractor::visit`] rewrites or reports on.
    fn mentions_style(&self, e: &typed::Expr) -> bool {
        if self.is_style(&e.ty) || self.is_style_list(&e.ty) {
            return true;
        }
        let mut found = false;
        typed::children(e, &mut |child| found = found || self.mentions_style(child));
        found
    }

    /// One expression, given to `sink` when it goes where a node reads.
    fn visit(&mut self, e: &mut typed::Expr, sink: Option<Sink>, cond: Cond) {
        if let Some(s) = sink {
            if self.is_style_list(&e.ty) {
                self.style_list(e, s, cond);
                return;
            }
            if self.is_style(&e.ty) {
                self.style(e, s, cond);
                return;
            }
        }
        self.descend(e, sink, cond);
    }

    /// The children of `e`, each going where [`Flow::positions`] says.
    ///
    /// A condition may not escape into an expression that is not a style:
    /// `On(.Hover, [f(x)])` where `f` answers a `Style` is handled by `style`,
    /// and everything else under a condition has already been refused there.
    fn descend(&mut self, e: &mut typed::Expr, sink: Option<Sink>, cond: Cond) {
        if let Some(s) = sink {
            self.opaque(e, s);
        }
        let carries = carries(e);
        let all = self.mode == Mode::Checking;
        let mut positions = self.flow.positions(e).into_iter();
        typed::children_mut(e, &mut |child| {
            let pos = positions.next().or_ice("`positions` names every child");
            let to = self.flow.sink(self.owner, &pos, sink, carries, all);
            self.visit(child, to, cond);
        });
    }

    /// Marks `s` as reachable by a value this run doesn't see, where `e` is a
    /// call whose answer it can't follow.
    fn opaque(&mut self, e: &typed::Expr, s: Sink) {
        let unseen = match &e.kind {
            ExprKind::Intrinsic { .. } => true,
            ExprKind::CallFn { func, .. } => !self.flow.params.contains_key(&func.id),
            _ => false,
        };
        self.tainted.extend(unseen.then_some(s.to));
    }

    /// Walks every declaration that mentions a style, and answers the rules
    /// the constants put in the sheet and those the functions did.
    fn walk(
        &mut self,
        consts: &mut [(ConstId, typed::Expr)],
        bodies: &mut [(FnId, typed::Body)],
        checking: bool,
    ) -> (Vec<StyleRule>, Vec<StyleRule>) {
        for (id, init) in consts.iter_mut() {
            let module = self.tables.const_(*id).module;
            self.declaration(
                Owner::Const(*id),
                Demand::Const(*id),
                module,
                init,
                checking,
            );
        }
        let const_rules = std::mem::take(&mut self.rules);
        for (id, body) in bodies.iter_mut() {
            let module = self.tables.fn_info(*id).module;
            self.declaration(
                Owner::Fn(*id),
                Demand::Ret(*id),
                module,
                &mut body.expr,
                checking,
            );
        }
        (const_rules, std::mem::take(&mut self.rules))
    }

    /// One declaration, whose value goes to `top`.
    ///
    /// A style extracted where a node is given it leaves what it read alone,
    /// but that still has to be a style the stylesheet can hold. So a
    /// declaration where that happens is walked twice: `checking` refuses what
    /// can't be, and then the extracting walk refuses nothing a second time.
    fn declaration(
        &mut self,
        owner: Owner,
        top: Demand,
        module: ModuleId,
        e: &mut typed::Expr,
        checking: bool,
    ) {
        let top = Pos::At(Some(top));
        let touched = self.flow.touched.contains(&owner)
            || self.flow.reaches(owner, &top, false, true)
                != self.flow.reaches(owner, &top, false, false);
        self.mode = match (checking, touched) {
            (true, false) => return,
            (true, true) => Mode::Checking,
            (false, true) => Mode::Quiet,
            (false, false) => Mode::Rewrite,
        };
        self.owner = owner;
        self.wraps = !checking && !self.flow.in_ui_node(module);
        let sink = self.flow.sink(owner, &top, None, false, checking);
        self.visit(e, sink, Cond::default());
    }

    /// A `[Style]` — the shape every widget takes and the shape both branches
    /// of a `When` have.
    fn style_list(&mut self, e: &mut typed::Expr, s: Sink, cond: Cond) {
        // Anything else here is a type error, already reported where it was written.
        if !self.is_style_list(&e.ty) {
            return;
        }
        if let Some(atoms) = self.fold_list(e, cond) {
            let span = e.span;
            // A list that extracted to nothing becomes nothing. `ui.row([], …)`
            // is the common case and it should cost the same as it did before
            // there was a stylesheet.
            if self.extracts_at(s) {
                let items = if atoms.is_empty() {
                    Vec::new()
                } else {
                    vec![self.extracted(&atoms, span)]
                };
                *e = typed::Expr::new(ExprKind::Array(items), e.ty, span);
            }
            self.keep(e, s, atoms);
            return;
        }
        if let ExprKind::Array(items) = &mut e.kind {
            for item in items {
                self.style(item, s, cond);
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
        self.leaf_or_descend(e, s, cond, self.classify);
    }

    /// One `Style`.
    fn style(&mut self, e: &mut typed::Expr, s: Sink, cond: Cond) {
        if !self.is_style(&e.ty) {
            return;
        }
        if let Some(atoms) = self.fold_one(e, cond) {
            if self.extracts_at(s) {
                *e = self.extracted(&atoms, e.span);
            }
            self.keep(e, s, atoms);
            return;
        }
        // It did not fold whole. Take the composition apart and try the pieces:
        // `Group([.Padding(.Px(8)), .Computed(f)])` extracts its padding and
        // leaves the closure alone.
        let span = e.span;
        let property = match &mut e.kind {
            ExprKind::EnumLit { variant, args, .. } => {
                match self.composition(span, variant, args, s, cond) {
                    Composed::Done => return,
                    Composed::Property(variant) => Some(variant),
                    Composed::Closure => None,
                }
            }
            _ => None,
        };
        if !cond.is_plain() {
            self.refuse(
                e.span,
                "this style is not statically known, and `On` and `At` exist \
                 only in the stylesheet",
            );
            return;
        }
        match property {
            // A dynamic value under no condition does not fail: it is written
            // inline as a custom property that a stylesheet rule reads (#195).
            // This puts that rule in the sheet; the bare property is left
            // standing for the runtime, which writes the `--buri-…` value out
            // and takes the class.
            Some(variant) => {
                self.variable_rule(variant);
                self.tainted.insert(s.to);
                self.descend(e, Some(s), cond);
            }
            None => self.leaf_or_descend(e, s, cond, self.classify_style),
        }
    }

    /// Whether a style that folded where `s` says becomes classes right
    /// there: where a node is given it, and anywhere in `ui/node`. Anywhere
    /// else it stays the value it is, for a match or a comparison to read, and
    /// becomes classes where a node is given it.
    fn extracts_at(&self, s: Sink) -> bool {
        self.mode != Mode::Checking && (s.site || !self.wraps)
    }

    /// Notes what a style that folded and stays a value stands for: the
    /// classes [`survey`] counts where it's written, and the names `classify`
    /// looks them up by where it's read.
    fn keep(&mut self, e: &typed::Expr, s: Sink, atoms: Vec<Atom>) {
        if self.extracts_at(s) || self.mode == Mode::Checking {
            return;
        }
        let classes: Vec<String> = atoms.iter().map(|a| a.class.clone()).collect();
        // A constant is inlined where it's read, with the spans it was
        // written with, so that is where `survey` meets it.
        if let ExprKind::Const(c) = e.kind {
            let init = self
                .original_consts
                .get(&c)
                .or_ice("a constant that folded has a value");
            self.sources.insert(init.span, classes.clone());
        }
        self.sources.insert(e.span, classes);
        self.directs
            .entry(s.to)
            .or_default()
            .extend(atoms.into_iter().map(|a| (a.name, a.class)));
    }

    /// A style a node is given whose value isn't known here: through
    /// `classify` where it can be, so each value it holds at run time becomes
    /// the classes the sheet has for it. A choice is descended rather than
    /// wrapped, so that each branch is extracted or wrapped on its own.
    fn leaf_or_descend(&mut self, e: &mut typed::Expr, s: Sink, cond: Cond, classify: FnId) {
        let choice = matches!(
            e.kind,
            ExprKind::If { .. } | ExprKind::Match { .. } | ExprKind::Block { .. }
        );
        if !self.leaf_site(s) || choice {
            self.descend(e, Some(s), cond);
            return;
        }
        let to = Demand::Leaf(self.wrapped.len());
        self.flow
            .edges(self.owner, e, Some(to), false, &mut self.extra);
        self.descend(e, Some(Sink { to, site: false }), cond);
        let span = e.span;
        let ty = e.ty;
        let value = std::mem::replace(e, typed::Expr::new(ExprKind::Error, ty, span));
        // `fill` writes the names it can be once every declaration is walked.
        *e = typed::Expr::new(
            ExprKind::CallFn {
                func: typed::Callee {
                    id: classify,
                    targs: Vec::new(),
                },
                args: vec![
                    value,
                    self.known(Vec::new(), span),
                    self.boolean(true, span),
                    self.make(span),
                ],
            },
            ty,
            span,
        );
        self.wrapped.push(to);
        self.leafy.insert(self.owner);
    }

    /// Whether a style a node is given where `s` says, and that is not a
    /// literal, goes through `classify`.
    fn leaf_site(&self, s: Sink) -> bool {
        s.site && self.wraps
    }

    /// Writes into each `classify` the names a value reaching it can have,
    /// and whether the compiler saw every one of those values.
    fn fill(&self, consts: &mut [(ConstId, typed::Expr)], bodies: &mut [(FnId, typed::Body)]) {
        let mut found = self.wrapped.iter().map(|leaf| self.names(*leaf));
        for (_, init) in consts
            .iter_mut()
            .filter(|(id, _)| self.leafy.contains(&Owner::Const(*id)))
        {
            self.fill_in(init, &mut found);
        }
        for (_, body) in bodies
            .iter_mut()
            .filter(|(id, _)| self.leafy.contains(&Owner::Fn(*id)))
        {
            self.fill_in(&mut body.expr, &mut found);
        }
        invariant(
            found.next().is_none(),
            "every leaf is one `classify` in a declaration that holds one",
        );
    }

    /// The `classify`s under `e`, in the order [`Extractor::leaf_or_descend`]
    /// wrote them: each one's children first. Only a declaration that holds
    /// one is filled in, so `classify` is loaded whenever this runs.
    fn fill_in(
        &self,
        e: &mut typed::Expr,
        found: &mut dyn Iterator<Item = (Vec<(String, String)>, bool)>,
    ) {
        typed::children_mut(e, &mut |child| self.fill_in(child, found));
        let span = e.span;
        let ExprKind::CallFn { func, args } = &mut e.kind else {
            return;
        };
        if ![self.classify, self.classify_style].contains(&func.id) {
            return;
        }
        let (known, strict) = found
            .next()
            .or_ice("every `classify` this pass wrote is one leaf");
        match (known.is_empty(), strict) {
            // Nothing the compiler saw can get here, so there is nothing to
            // look up: the value is the one the runtime writes inline.
            (true, false) => {
                *e = std::mem::take(args)
                    .into_iter()
                    .next()
                    .or_ice("`classify` is given the style first");
            }
            _ => {
                *args
                    .get_mut(1)
                    .or_ice("`classify` is given the names second") = self.known(known, span);
                *args
                    .get_mut(2)
                    .or_ice("`classify` is given strictness third") = self.boolean(strict, span);
            }
        }
    }

    /// The names a leaf's values can have, each with its class, and whether
    /// those are all the values it can hold.
    fn names(&self, leaf: Demand) -> (Vec<(String, String)>, bool) {
        let reach = closure(leaf, &[&self.flow.edges, &self.extra]);
        let strict = reach
            .iter()
            .all(|d| !self.tainted.contains(d) && !self.flow.open(*d));
        let mut known: Vec<(String, String)> = reach
            .iter()
            .filter_map(|d| self.directs.get(d))
            .flatten()
            .cloned()
            .collect();
        known.sort();
        known.dedup();
        (known, strict)
    }

    /// `[(name, class), …]`, as a typed expression.
    fn known(&self, pairs: Vec<(String, String)>, span: Span) -> typed::Expr {
        let text = self
            .tables
            .prim(crate::compiler::semantics::types::Prim::Str);
        let pair = Ty::tuple([text, text]);
        let items = pairs
            .into_iter()
            .map(|(name, class)| {
                typed::Expr::new(
                    ExprKind::Tuple(vec![
                        typed::Expr::new(ExprKind::Str(name), text, span),
                        typed::Expr::new(ExprKind::Str(class), text, span),
                    ]),
                    pair,
                    span,
                )
            })
            .collect();
        typed::Expr::new(ExprKind::Array(items), Ty::array(pair), span)
    }

    fn boolean(&self, value: bool, span: Span) -> typed::Expr {
        let ty = self
            .tables
            .prim(crate::compiler::semantics::types::Prim::Bool);
        typed::Expr::new(ExprKind::Bool(value), ty, span)
    }

    /// `ui/style`'s `extracted`, as the `fn([(Int, Str)]) => Style` that
    /// `classify` builds its `Classes` with.
    fn make(&self, span: Span) -> typed::Expr {
        let info = self.tables.fn_info(self.extracted);
        let ty = Ty::func(info.params.iter().map(|p| p.ty), info.ret);
        typed::Expr::new(
            ExprKind::FnRef(typed::Callee {
                id: self.extracted,
                targs: Vec::new(),
            }),
            ty,
            span,
        )
    }

    /// Records the variable-backed rule a dynamic value at no condition needs
    /// (#195), once per class. Nothing for a property whose value is threaded
    /// through more than one declaration or an edge-named one — those keep
    /// degrading to an inline declaration, as [`variable_property`] says.
    fn variable_rule(&mut self, variant: usize) {
        let Some((abbreviation, property)) = variable_property(variant) else {
            return;
        };
        let class = format!("{abbreviation}-var");
        if !self.recorded.insert(class.clone()) {
            return;
        }
        let folded = if variant == STYLE_SHADOWS {
            STYLE_SHADOW
        } else {
            variant
        };
        self.rules.push(StyleRule {
            class,
            property: u16::try_from(folded).unwrap_or(0),
            state: None,
            screen: None,
            blocks: vec![("", format!("{property}:var(--buri-{abbreviation})"))],
        });
    }

    /// Descends one composition variant, the `Style` literal at `span`.
    fn composition(
        &mut self,
        span: Span,
        variant: &mut usize,
        args: &mut Vec<typed::Expr>,
        s: Sink,
        cond: Cond,
    ) -> Composed {
        match *variant {
            STYLE_GROUP => {
                let Some(list) = args.first_mut() else {
                    return Composed::Done;
                };
                self.style_list(list, s, cond);
                Composed::Done
            }
            STYLE_ON | STYLE_AT => {
                // The condition is absorbed into whatever the list extracts to,
                // so where a node is given it the node itself is replaced by an
                // ordinary group. A `When` inside one survives: its two
                // branches carry the condition.
                let chosen = args
                    .first()
                    .and_then(|selector| self.folder().eval(selector, &Env::default()))
                    .and_then(|value| value.as_variant().map(|(index, _)| index))
                    .and_then(|index| u8::try_from(index).ok());
                let Some(chosen) = chosen else {
                    self.refuse(
                        span,
                        "which state or screen this applies to is not statically \
                         known",
                    );
                    return Composed::Done;
                };
                let inner = if *variant == STYLE_ON {
                    Cond {
                        state: Some(chosen),
                        screen: cond.screen,
                    }
                } else {
                    Cond {
                        state: cond.state,
                        screen: Some(chosen),
                    }
                };
                if let Some(list) = args.get_mut(1) {
                    self.style_list(list, s, inner);
                }
                if self.extracts_at(s) {
                    *variant = STYLE_GROUP;
                    args.remove(0);
                }
                Composed::Done
            }
            STYLE_WHEN => {
                let mut rest = args.iter_mut();
                // The `Prop<Bool>` is ordinary reactive data and may hold a
                // closure with styles of its own.
                if let Some(condition) = rest.next() {
                    self.visit(condition, None, Cond::default());
                }
                for branch in rest {
                    self.style_list(branch, s, cond);
                }
                Composed::Done
            }
            STYLE_COMPUTED => {
                if !cond.is_plain() {
                    self.refuse(
                        span,
                        "a `Computed` style may not appear under `On` or `At`: a \
                         closure cannot be scoped to a pseudo-class or to a media \
                         query",
                    );
                    return Composed::Done;
                }
                // A closure written here is extracted inside. One that comes
                // from elsewhere goes through `classify` whole, which wraps
                // it in one that extracts what it answers.
                let written =
                    matches!(args.first().map(|f| &f.kind), Some(ExprKind::Lambda { .. }));
                if !written && self.leaf_site(s) {
                    return Composed::Closure;
                }
                for arg in args {
                    self.visit(arg, Some(s), Cond::default());
                }
                Composed::Done
            }
            other => Composed::Property(other),
        }
    }

    /// `Style::Extracted(pairs)`, as a typed expression.
    fn extracted(&self, atoms: &[Atom], span: Span) -> typed::Expr {
        let int = self
            .tables
            .prim(crate::compiler::semantics::types::Prim::I64);
        let text = self
            .tables
            .prim(crate::compiler::semantics::types::Prim::Str);
        let pair = Ty::tuple([int, text]);
        let items: Vec<typed::Expr> = atoms
            .iter()
            .map(|a| {
                typed::Expr::new(
                    ExprKind::Tuple(vec![
                        typed::Expr::new(
                            ExprKind::Int(Magnitude::new(u128::from(a.slot)), false),
                            int,
                            span,
                        ),
                        typed::Expr::new(ExprKind::Str(a.class.clone()), text, span),
                    ]),
                    pair,
                    span,
                )
            })
            .collect();
        let list = typed::Expr::new(ExprKind::Array(items), Ty::array(pair), span);
        // The payload is a `Classes`, whose one field is private to
        // `ui/style`. Writing it here is what a program cannot do.
        let classes = typed::Expr::new(
            ExprKind::StructLit {
                con: self.classes_con,
                targs: Vec::new(),
                fields: vec![list],
            },
            Ty::con(self.classes_con, []),
            span,
        );
        typed::Expr::new(
            ExprKind::EnumLit {
                con: self.style_con,
                targs: Vec::new(),
                variant: STYLE_EXTRACTED,
                args: vec![classes],
            },
            Ty::con(self.style_con, []),
            span,
        )
    }

    fn fold_one(&mut self, e: &typed::Expr, cond: Cond) -> Option<Vec<Atom>> {
        let value = self.flow.fold(self.owner, e)?;
        let mut atoms = Vec::new();
        flatten(&value, cond, &mut atoms)?;
        Some(self.record(atoms))
    }

    fn fold_list(&mut self, e: &typed::Expr, cond: Cond) -> Option<Vec<Atom>> {
        let Value::Array(items) = self.flow.fold(self.owner, e)? else {
            return None;
        };
        let mut atoms = Vec::new();
        for item in &items {
            flatten(item, cond, &mut atoms)?;
        }
        Some(self.record(atoms))
    }

    /// Puts each atom's rule in the sheet, once per class.
    fn record(&mut self, atoms: Vec<(Atom, StyleRule)>) -> Vec<Atom> {
        let atoms = atoms
            .into_iter()
            .map(|(atom, rule)| {
                if self.recorded.insert(rule.class.clone()) {
                    self.rules.push(rule);
                }
                atom
            })
            .collect();
        resolve_conflicts(atoms)
    }
}

/// What [`Extractor::composition`] made of a `Style` literal.
enum Composed {
    /// It handled it.
    Done,
    /// A property, which didn't fold.
    Property(usize),
    /// A `Computed` a node is given whose closure was written elsewhere.
    Closure,
}

/// Whether a child of `e` that goes wherever `e` goes is given to a node
/// right there when `e` is: a part of a value `e` builds, or a branch of the
/// choice `e` is.
fn carries(e: &typed::Expr) -> bool {
    matches!(
        e.kind,
        ExprKind::Array(_)
            | ExprKind::Tuple(_)
            | ExprKind::EnumLit { .. }
            | ExprKind::StructLit { .. }
            | ExprKind::StructUpdate { .. }
            | ExprKind::Lambda { .. }
            | ExprKind::If { .. }
            | ExprKind::Match { .. }
            | ExprKind::Block { .. }
    )
}

/// One folded `Style` value into the atoms it is.
fn flatten(v: &Value, cond: Cond, out: &mut Vec<(Atom, StyleRule)>) -> Option<()> {
    let (variant, args) = v.as_variant()?;
    match variant {
        STYLE_GROUP => {
            let Value::Array(items) = args.first()? else {
                return None;
            };
            for item in items {
                flatten(item, cond, out)?;
            }
            Some(())
        }
        STYLE_ON => {
            let (state, _) = args.first()?.as_variant()?;
            let Value::Array(items) = args.get(1)? else {
                return None;
            };
            let inner = Cond {
                state: Some(u8::try_from(state).ok()?),
                screen: cond.screen,
            };
            for item in items {
                flatten(item, inner, out)?;
            }
            Some(())
        }
        STYLE_AT => {
            let (screen, _) = args.first()?.as_variant()?;
            let Value::Array(items) = args.get(1)? else {
                return None;
            };
            let inner = Cond {
                state: cond.state,
                screen: Some(u8::try_from(screen).ok()?),
            };
            for item in items {
                flatten(item, inner, out)?;
            }
            Some(())
        }
        // A `When` is a runtime choice between two static lists, and an
        // already-extracted style has no value left to read. Both make the
        // enclosing fold give up so that the descent handles them.
        STYLE_WHEN | STYLE_COMPUTED | STYLE_EXTRACTED => None,
        // Every other variant is one property with one value.
        _ => property(variant, args, cond, out),
    }
}

/// One property value: the classes it needs, each in its own slot, and the
/// rules they put in the sheet.
fn property(
    variant: usize,
    args: &[Value],
    cond: Cond,
    out: &mut Vec<(Atom, StyleRule)>,
) -> Option<()> {
    // Out of the flow is one fact about the element rather than one per
    // edge, so a pin keeps its `position` in a class of its own. Four pins
    // then write four insets and one `position`, and `Position` — the
    // property that owns `position`, declared after `Pin` and so written
    // later in the sheet — overrides it wherever an element has both.
    if variant == STYLE_PIN {
        let flow = (
            "pin",
            "out".to_owned(),
            vec![("", "position:absolute".to_owned())],
        );
        out.push(atom(variant, PIN_FLOW, cond, flow, "out")?);
    }
    let sub = sub_key(variant, args);
    let declared = declaration(variant, args, digest)?;
    let (_, name, _) = declaration(variant, args, preimage)
        .or_ice("a value that declares under one digest declares under any");
    out.push(atom(variant, sub, cond, declared, &name)?);
    Some(())
}

/// One class: its name, its slot, and the rule it puts in the sheet. `name` is
/// what `ui/style` spells the value as, which differs from the class's key
/// only where that holds a digest.
fn atom(
    variant: usize,
    sub: u32,
    cond: Cond,
    (abbreviation, key, blocks): Declaration,
    name: &str,
) -> Option<(Atom, StyleRule)> {
    let class = format!("{}{}-{}", cond.prefix(), abbreviation, key);
    let rule = StyleRule {
        class: class.clone(),
        property: u16::try_from(variant).ok()?,
        state: cond.state,
        screen: cond.screen,
        blocks,
    };
    let name = format!("{}{}-{}", cond.prefix(), abbreviation, name);
    Some((
        Atom {
            slot: slot(variant, sub, cond)?,
            class,
            name,
        },
        rule,
    ))
}

/// What reads what, each read marked when it's under a style extracted where
/// it stands.
#[derive(Default)]
struct Edges {
    reads: HashMap<Demand, Vec<(Demand, bool)>>,
    /// The declarations holding such a style.
    stops: HashSet<Owner>,
    /// The functions a call names.
    called: HashSet<FnId>,
    /// The functions read as values, which can then be called from anywhere.
    referenced: HashSet<FnId>,
    /// The locals a `let` or a pattern binds.
    bound: Vec<(Owner, LocalId)>,
}

impl Edges {
    fn add(&mut self, from: Demand, to: Demand, stopped: bool) {
        self.reads.entry(from).or_default().push((to, stopped));
    }
}

/// Everything `from` reads in `graphs`, past the marked reads; [`reach`]
/// goes through those too.
fn closure(from: Demand, graphs: &[&Edges]) -> HashSet<Demand> {
    let mut found = HashSet::default();
    let mut work = vec![from];
    while let Some(d) = work.pop() {
        if found.insert(d) {
            let reads = graphs.iter().flat_map(|g| g.reads.get(&d)).flatten();
            work.extend(reads.filter(|(_, stopped)| !stopped).map(|(to, _)| *to));
        }
    }
    found
}

/// Everything a node reads in `edges`, through the marked reads too, and
/// each local of a function in `fns` read there from outside that function:
/// not by its result or another of its locals.
fn reach(edges: &Edges, fns: &HashSet<FnId>) -> (HashSet<Demand>, Vec<(FnId, LocalId)>) {
    let mut found = HashSet::default();
    let mut outside = Vec::new();
    let mut work = vec![Demand::Node];
    while let Some(d) = work.pop() {
        if found.insert(d) {
            for (to, _) in edges.reads.get(&d).into_iter().flatten() {
                work.push(*to);
                if let Demand::Local(Owner::Fn(f), l) = *to {
                    let inside = d == Demand::Ret(f)
                        || matches!(d, Demand::Local(Owner::Fn(g), _) if g == f);
                    if !inside && fns.contains(&f) {
                        outside.push((f, l));
                    }
                }
            }
        }
    }
    (found, outside)
}

/// Whether `e` reads a local, a constant or a function: what a style
/// extracted where it stands would otherwise demand.
fn reads(e: &typed::Expr) -> bool {
    let mut found = matches!(
        e.kind,
        ExprKind::Local(_)
            | ExprKind::Const(_)
            | ExprKind::CallFn { .. }
            | ExprKind::FnRef(_)
            | ExprKind::CallTrait { .. }
    );
    typed::children(e, &mut |child| found = found || reads(child));
    found
}

/// What decides whether a position's value reaches a node.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Demand {
    /// A node is given it.
    Node,
    /// A local of one declaration, a parameter among them.
    Local(Owner, LocalId),
    /// A function's result.
    Ret(FnId),
    Const(ConstId),
    /// A trait method's result, whichever `impl` answers it.
    Method(TraitId, usize),
    /// A style a node is given whose value goes through `classify`.
    Leaf(usize),
}

/// The declaration a local belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Owner {
    Fn(FnId),
    Const(ConstId),
}

/// Where one child's value goes, next to its parent's.
#[derive(Clone)]
enum Pos {
    /// Wherever the parent's goes.
    Same,
    /// To this, or nowhere a node reads.
    At(Option<Demand>),
    /// Into these locals: a `let`'s value, or a match's scrutinee.
    Bound(Vec<LocalId>),
}

/// What a parameter stands for at a call.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Param {
    /// Every argument it is given: its type isn't generic.
    Fixed,
    /// Each call's own argument, until it is found to go further than its
    /// function's result: its type is generic and may hold a style.
    Passed,
    /// Each call's own argument: its type is generic and holds no style a
    /// node hasn't been given, or its function is in `ui/node`.
    Opaque,
}

/// The pattern a `let` binds and the value it binds it to.
type Let<'a> = (&'a typed::Pattern, &'a typed::Expr);

/// Which values reach a node, found before anything is rewritten.
///
/// It reads backwards from each node. A node's argument is demanded, and so is
/// whatever an expression in a demanded position reads: a local's value, a
/// called function's result, a parameter's argument at every call. A call
/// through a trait bound reads every `impl` of the method.
///
/// A style a node is given that folds where it's given is extracted right
/// there, so it demands nothing: a `let`, a field or a function it reads stays
/// an ordinary value for the rest of the program to match on.
struct Flow<'a> {
    tables: &'a Tables,
    style_con: TyConId,
    ui_node: Option<ModuleId>,
    bodies: &'a BodyMap,
    consts: &'a ConstMap,
    /// The parameters of each body this run reads.
    params: HashMap<FnId, Vec<(LocalId, Param)>>,
    /// The parameters of a generic type that go further than their
    /// function's result, as [`Flow::escaping`] finds them.
    escaping: HashSet<(FnId, LocalId)>,
    /// Each `let`-bound local of the declarations a fold has read: the pattern
    /// that binds it, and its value.
    lets: RefCell<HashMap<(Owner, LocalId), Let<'a>>>,
    /// The declarations whose `let`s are in `lets`.
    indexed: RefCell<HashSet<Owner>>,
    /// The value each local a fold has read folded to, if it did.
    statics: RefCell<HashMap<(Owner, LocalId), Option<Value>>>,
    /// What a node reads, past the styles extracted where a node is given them.
    demanded: HashSet<Demand>,
    /// What a node reads through those as well: every one of them still has to
    /// be a style the stylesheet can hold, so [`Mode::Checking`] walks these.
    reached: HashSet<Demand>,
    /// The declarations holding such a style, or a local only it reads.
    touched: HashSet<Owner>,
    /// What reads what.
    edges: Edges,
    /// The locals every value of which this run sees bound: a `let`'s or a
    /// pattern's, and the parameters of a function called only by name.
    seen: HashSet<(Owner, LocalId)>,
}

impl<'a> Flow<'a> {
    fn of(
        tables: &'a Tables,
        style_con: TyConId,
        ui_node: Option<ModuleId>,
        walked: &Walked,
        bodies: &'a BodyMap,
        consts: &'a ConstMap,
    ) -> Flow<'a> {
        let mut flow = Flow {
            tables,
            style_con,
            ui_node,
            bodies,
            consts,
            params: HashMap::default(),
            escaping: HashSet::default(),
            lets: RefCell::default(),
            indexed: RefCell::default(),
            statics: RefCell::default(),
            demanded: HashSet::default(),
            reached: HashSet::default(),
            touched: HashSet::default(),
            edges: Edges::default(),
            seen: HashSet::default(),
        };
        // The functions with a parameter that is `Param::Passed`.
        let mut passing: HashSet<FnId> = HashSet::default();
        for (id, body) in walked.unsettled(bodies) {
            let info = tables.fn_info(id);
            let ui = flow.in_ui_node(info.module);
            let kinds = body.params.iter().zip(&info.params).map(|(l, p)| {
                let kind = if !p.ty.has_params() {
                    Param::Fixed
                } else if !ui && flow.may_hold_style(p.ty) {
                    passing.insert(id);
                    Param::Passed
                } else {
                    Param::Opaque
                };
                (*l, kind)
            });
            let kinds = kinds.collect();
            flow.params.insert(id, kinds);
        }
        // A generic parameter found to go further than its function's result
        // is given each of its callers' arguments too, so the graph is drawn
        // again following it. That can send one of theirs further, so this
        // runs until nothing more does. Most programs have no such parameter
        // and draw it once.
        let mut edges = flow.graph(walked);
        loop {
            let (reached, roots) = reach(&edges, &passing);
            let grown = flow.escaping(&edges, roots);
            if grown.is_empty() {
                flow.reached = reached;
                break;
            }
            flow.escaping.extend(grown);
            edges = flow.graph(walked);
        }
        flow.demanded = closure(Demand::Node, &[&edges]);
        flow.touched = std::mem::take(&mut edges.stops);
        for d in flow.reached.difference(&flow.demanded) {
            if let Demand::Local(owner, _) = d {
                flow.touched.insert(*owner);
            }
        }
        // A function no one reads as a value is called only where its name is
        // written, so every argument it is given is one this run sees. A
        // method can be called through a bound from code this run doesn't
        // read, and a generic parameter only its result reads is not followed
        // at all.
        let closed = edges
            .called
            .iter()
            .filter(|id| !edges.referenced.contains(*id))
            .filter(|id| tables.fn_info(**id).impl_of.is_none())
            .filter_map(|id| flow.params.get(id).map(|params| (*id, params)));
        let this = &flow;
        let seen: Vec<(Owner, LocalId)> = closed
            .flat_map(|(id, params)| {
                let followed = params
                    .iter()
                    .filter(move |(l, kind)| this.follows(id, *l, *kind));
                followed.map(move |(l, _)| (Owner::Fn(id), *l))
            })
            .collect();
        flow.seen.extend(seen);
        flow.seen.extend(edges.bound.drain(..));
        flow.edges = edges;
        flow
    }

    /// What reads what, across every declaration this run reads.
    fn graph(&self, walked: &Walked) -> Edges {
        GRAPHS_DRAWN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut edges = Edges::default();
        for (id, init) in walked.unsettled_constants(self.consts) {
            self.edges(
                Owner::Const(id),
                init,
                Some(Demand::Const(id)),
                false,
                &mut edges,
            );
        }
        for (id, body) in walked.unsettled(self.bodies) {
            if let Some((t, m)) = self.tables.fn_info(id).impl_of {
                edges.add(Demand::Method(t, m), Demand::Ret(id), false);
            }
            self.edges(
                Owner::Fn(id),
                &body.expr,
                Some(Demand::Ret(id)),
                false,
                &mut edges,
            );
        }
        edges
    }

    /// The parameters of a generic type, not yet escaping, that a node reads
    /// through something other than their own function's result: a node
    /// itself, a constant, or another function's local, directly (`roots`)
    /// or through a local of their own function. Such a parameter is given
    /// every caller's argument wherever those go, as one whose type is not
    /// generic is.
    fn escaping(&self, edges: &Edges, roots: Vec<(FnId, LocalId)>) -> HashSet<(FnId, LocalId)> {
        let mut found: HashSet<(FnId, LocalId)> = HashSet::default();
        let mut work = roots;
        while let Some((f, l)) = work.pop() {
            if found.insert((f, l)) {
                let reads = edges.reads.get(&Demand::Local(Owner::Fn(f), l));
                work.extend(reads.into_iter().flatten().filter_map(|(to, _)| match to {
                    Demand::Local(Owner::Fn(g), m) if *g == f => Some((f, *m)),
                    _ => None,
                }));
            }
        }
        found.retain(|(f, l)| {
            !self.escaping.contains(&(*f, *l))
                && self
                    .params
                    .get(f)
                    .is_some_and(|ps| ps.contains(&(*l, Param::Passed)))
        });
        found
    }

    /// Whether a value of `ty` can hold a style no node has been given yet:
    /// not a bare type parameter, which only a generic helper's result
    /// hands on, and not a `ui/node` type, which a node built already.
    fn may_hold_style(&self, ty: Ty) -> bool {
        match ty.kind() {
            TyKind::Con(id, _) => !self.in_ui_node(self.tables.tycon(*id).module),
            TyKind::Array(elem) => self.may_hold_style(*elem),
            TyKind::Tuple(items) => items.iter().any(|t| self.may_hold_style(*t)),
            TyKind::Fn(_, ret) => self.may_hold_style(*ret),
            _ => false,
        }
    }

    /// Whether parameter `l` of `f` stands for every argument it is given
    /// rather than passing each call's on.
    fn follows(&self, f: FnId, l: LocalId, kind: Param) -> bool {
        match kind {
            Param::Fixed => true,
            Param::Passed => !self.escaping.is_empty() && self.escaping.contains(&(f, l)),
            Param::Opaque => false,
        }
    }

    /// Whether `d` is a local some value of which this run doesn't see.
    fn open(&self, d: Demand) -> bool {
        matches!(d, Demand::Local(o, l) if !self.seen.contains(&(o, l)))
    }

    /// Whether `e` is a style, or a list of them, that reads something and
    /// extracts whole where it stands.
    fn extracts(&self, owner: Owner, e: &typed::Expr) -> bool {
        if !(self.is_style(&e.ty) || self.is_style_list(&e.ty)) || !reads(e) {
            return false;
        }
        let Some(value) = self.fold(owner, e) else {
            return false;
        };
        // A list folds to an array, and a style never does.
        let items = match &value {
            Value::Array(items) => &items[..],
            one => std::slice::from_ref(one),
        };
        let mut atoms = Vec::new();
        items
            .iter()
            .all(|v| flatten(v, Cond::default(), &mut atoms).is_some())
    }

    fn is_style(&self, ty: &Ty) -> bool {
        // `Style` takes no type arguments, so its id alone names the type.
        matches!(ty.kind(), TyKind::Con(id, _) if *id == self.style_con)
    }

    fn is_style_list(&self, ty: &Ty) -> bool {
        matches!(ty.kind(), TyKind::Array(elem) if self.is_style(elem))
    }

    /// The value `e` denotes at compile time, reading the `let`s of `owner`.
    fn fold(&self, owner: Owner, e: &typed::Expr) -> Option<Value> {
        let lets = |l| self.static_local(owner, l);
        Folder::new(self.tables, self.bodies, self.consts).eval(e, &Env::reading(&lets))
    }

    fn static_local(&self, owner: Owner, l: LocalId) -> Option<Value> {
        let key = (owner, l);
        if let Some(known) = self.statics.borrow().get(&key) {
            return known.clone();
        }
        if self.indexed.borrow_mut().insert(owner) {
            let root = match owner {
                Owner::Fn(id) => self.bodies.get(&id).map(|body| &body.expr),
                Owner::Const(id) => self.consts.get(&id),
            };
            root.into_iter().for_each(|root| self.index(owner, root));
        }
        let (pattern, value) = *self.lets.borrow().get(&key)?;
        // A closure that names its own `let` gives up rather than recursing.
        self.statics.borrow_mut().insert(key, None);
        let folded = self.fold(owner, value).and_then(|v| bound(pattern, &v, l));
        self.statics.borrow_mut().insert(key, folded.clone());
        folded
    }

    fn index(&self, owner: Owner, e: &'a typed::Expr) {
        if let ExprKind::Block { stmts, .. } = &e.kind {
            for stmt in stmts {
                if let typed::Stmt::Let { pattern, value, .. } = stmt {
                    pattern.each_bind(&mut |l| {
                        self.lets.borrow_mut().insert((owner, l), (pattern, value));
                    });
                }
            }
        }
        typed::children(e, &mut |child| self.index(owner, child));
    }

    /// Records what `e` reads when its value goes to `ctx`, and the same for
    /// everything under it. Under a style `stopped` where a node is given it,
    /// only [`Flow::reached`] reads it.
    fn edges(
        &self,
        owner: Owner,
        e: &typed::Expr,
        ctx: Option<Demand>,
        stopped: bool,
        out: &mut Edges,
    ) {
        let stops = !stopped && ctx.is_some() && self.extracts(owner, e);
        if stops {
            out.stops.insert(owner);
        }
        let stopped = stopped || stops;
        match &e.kind {
            ExprKind::CallFn { func, .. } => {
                out.called.insert(func.id);
            }
            ExprKind::FnRef(func) => {
                out.referenced.insert(func.id);
            }
            _ => {}
        }
        if let Some(d) = ctx {
            let read = match &e.kind {
                ExprKind::Local(l) => Some(Demand::Local(owner, *l)),
                ExprKind::Const(c) => Some(Demand::Const(*c)),
                ExprKind::CallFn { func, .. } | ExprKind::FnRef(func) => {
                    let f = func.id;
                    self.params.contains_key(&f).then_some(Demand::Ret(f))
                }
                ExprKind::CallTrait {
                    trait_id, method, ..
                } => Some(Demand::Method(*trait_id, *method)),
                _ => None,
            };
            if let Some(read) = read {
                out.add(d, read, stopped);
            }
        }
        let mut positions = self.positions(e).into_iter();
        typed::children(e, &mut |child| {
            let child_ctx = match positions.next().or_ice("`positions` names every child") {
                Pos::Same => ctx,
                Pos::At(d) => d,
                // Any one of them demanded demands the first, which stands for
                // the value they were all bound from.
                Pos::Bound(locals) => locals.first().map(|first| {
                    out.bound.extend(locals.iter().map(|l| (owner, *l)));
                    let first = Demand::Local(owner, *first);
                    for l in locals.iter().skip(1) {
                        out.add(Demand::Local(owner, *l), first, stopped);
                    }
                    first
                }),
            };
            self.edges(owner, child, child_ctx, stopped, out);
        });
    }

    /// Whether a child at `pos` reaches a node, given whether its parent's
    /// value does: through a style extracted where it's given too when `all`.
    fn reaches(&self, owner: Owner, pos: &Pos, sink: bool, all: bool) -> bool {
        let set = if all { &self.reached } else { &self.demanded };
        match pos {
            Pos::Same => sink,
            Pos::At(d) => d.is_some_and(|d| set.contains(&d)),
            Pos::Bound(locals) => locals
                .first()
                .is_some_and(|l| set.contains(&Demand::Local(owner, *l))),
        }
    }

    /// Where a child at `pos` sends its value when its parent's goes to
    /// `parent`, if a node reads it: through a style extracted where it's
    /// given too when `all`. A child a node is given right where its parent
    /// is, `carries` says, is given it right there too.
    fn sink(
        &self,
        owner: Owner,
        pos: &Pos,
        parent: Option<Sink>,
        carries: bool,
        all: bool,
    ) -> Option<Sink> {
        let set = if all { &self.reached } else { &self.demanded };
        match pos {
            Pos::Same => parent.map(|p| Sink {
                to: p.to,
                site: p.site && carries,
            }),
            Pos::At(d) => d.filter(|d| set.contains(d)).map(|to| Sink {
                to,
                site: to == Demand::Node,
            }),
            Pos::Bound(locals) => locals
                .first()
                .map(|l| Demand::Local(owner, *l))
                .filter(|d| set.contains(d))
                .map(|to| Sink { to, site: false }),
        }
    }

    fn in_ui_node(&self, module: ModuleId) -> bool {
        self.ui_node == Some(module)
    }

    /// Where each child of `e` sends its value, in [`typed::children`]'s
    /// order.
    fn positions(&self, e: &typed::Expr) -> Vec<Pos> {
        let all = |pos: Pos, n: usize| vec![pos; n];
        let built = |con: TyConId| {
            if self.in_ui_node(self.tables.tycon(con).module) {
                Pos::At(Some(Demand::Node))
            } else {
                Pos::Same
            }
        };
        match &e.kind {
            // A closure answers what its body does, which may be any of its
            // arguments.
            ExprKind::CallValue { args, .. } => std::iter::once(Pos::Same)
                .chain(args.iter().map(|_| Pos::Same))
                .collect(),
            ExprKind::CallFn { func, args } => {
                let f = func.id;
                let info = self.tables.fn_info(f);
                if self.in_ui_node(info.module) {
                    return all(Pos::At(Some(Demand::Node)), args.len());
                }
                match self.params.get(&f) {
                    // A parameter whose type is generic carries every caller's
                    // values, so where only its result reads it, it can't say
                    // which of them reach a node. It passes this call's on,
                    // the way an unread function does.
                    Some(params) => params
                        .iter()
                        .map(|&(l, kind)| {
                            if self.follows(f, l, kind) {
                                Pos::At(Some(Demand::Local(Owner::Fn(f), l)))
                            } else {
                                Pos::Same
                            }
                        })
                        .chain(std::iter::repeat(Pos::At(None)))
                        .take(args.len())
                        .collect(),
                    // A function this run doesn't read: what it answers may
                    // be any of its arguments.
                    None => all(Pos::Same, args.len()),
                }
            }
            // A method called through a bound answers what its `impl` does,
            // which may be any of its arguments.
            ExprKind::CallTrait { args, .. }
            | ExprKind::Intrinsic { args, .. }
            | ExprKind::Tuple(args)
            | ExprKind::Array(args) => all(Pos::Same, args.len()),
            ExprKind::StructLit {
                con, fields: args, ..
            }
            | ExprKind::EnumLit { con, args, .. } => all(built(*con), args.len()),
            ExprKind::StructUpdate { con, updates, .. } => std::iter::once(built(*con))
                .chain(updates.iter().map(|_| built(*con)))
                .collect(),
            ExprKind::Field { .. }
            | ExprKind::TupleIndex { .. }
            | ExprKind::Try { .. }
            | ExprKind::Lambda { .. } => {
                vec![Pos::Same]
            }
            ExprKind::Index { .. } => vec![Pos::Same, Pos::At(None)],
            ExprKind::Block { stmts, tail } => stmts
                .iter()
                .map(|s| match s {
                    typed::Stmt::Let { pattern, .. } => {
                        let mut locals = Vec::new();
                        pattern.binds(&mut locals);
                        Pos::Bound(locals)
                    }
                    typed::Stmt::Expr(_) => Pos::At(None),
                })
                .chain(tail.iter().map(|_| Pos::Same))
                .collect(),
            ExprKind::If { .. } => vec![Pos::At(None), Pos::Same, Pos::Same],
            ExprKind::Match { arms, .. } => {
                let mut locals = Vec::new();
                for arm in arms {
                    arm.pattern.binds(&mut locals);
                }
                let mut out = vec![Pos::Bound(locals)];
                for arm in arms {
                    out.extend(arm.guard.iter().map(|_| Pos::At(None)));
                    out.push(Pos::Same);
                }
                out
            }
            // Nothing else hands a child's value on: a condition, an
            // operator, a template, a context.
            _ => {
                let mut out = Vec::new();
                typed::children(e, &mut |_| out.push(Pos::At(None)));
                out
            }
        }
    }
}

/// One `(slot, class)` pair, which is what reaches the runtime.
#[derive(Clone, Debug)]
struct Atom {
    slot: u32,
    class: String,
    /// What `ui/style` names the class by.
    name: String,
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
///
/// A [`Reset`] opens it, where the program builds one of those elements.
pub fn stylesheet(rules: &[StyleRule], used: &HashSet<String>, reset: Reset) -> String {
    // `rules` names each class once: one extractor records a class once, and
    // `Styled::after` dedupes a base's rules against the run's.
    let mut unique: Vec<&StyleRule> = Vec::new();
    for rule in rules {
        // A rule nobody reaches is not shipped. Extraction walks every checked
        // body, including the ones monomorphization then drops, and a
        // stylesheet carrying a library's unused styles is the thing atomic
        // classes exist to avoid.
        if used.contains(&rule.class) {
            unique.push(rule);
        }
    }
    unique.sort_by(|a, b| {
        (a.screen, a.property, a.state, &a.class).cmp(&(b.screen, b.property, b.state, &b.class))
    });

    let mut out = reset.rules();
    out.push_str(&focus_ring(&unique));
    out.push_str(&marker_anchor(&unique));
    out.push_str(&keyframes(&unique));
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
        let state = rule
            .state
            .and_then(|s| STATES.get(s as usize))
            .map_or("", |s| s.1);
        for (suffix, declarations) in &rule.blocks {
            let text = format!(".{}{state}{suffix}{{{declarations}}}", rule.class);
            // An animation runs only for a reader who has not asked for less
            // motion. One line, so the painter skips it whole.
            if rule.property as usize == STYLE_ANIMATION {
                out.push_str(&format!("{MOTION_ALLOWED}{{{text}}}\n"));
            } else {
                out.push_str(&text);
                out.push('\n');
            }
        }
    }
    if open.is_some_and(|s| s.is_some()) {
        out.push_str("}\n");
    }
    out
}

/// Takes the platform's focus ring away from the elements that draw one of
/// their own, and from no others.
///
/// A browser paints its own `outline` on whatever is focused, over anything a
/// program put there — so a designed ring and Chrome's blue rectangle render at
/// once, and `ui/style` has no `Outline` to turn one of them off with. Naming
/// the classes rather than writing `:where(:focus-visible){outline:none}` is
/// what keeps a control that styles nothing visibly focusable, which is the
/// whole reason `State.Focus` is `:focus-visible` in the first place.
///
/// A ring is a shadow or a border: the two things this vocabulary can draw one
/// with. A focus style that only fades or recolours adds to the platform's ring
/// rather than replacing it, so the platform's stays. So does a ring that only
/// applies from a breakpoint upwards, because there is a width at which it
/// paints nothing.
///
/// `State::FocusWithin` is deliberately not here. The platform's outline lands
/// on the descendant that has the keyboard, not on the container that rings
/// itself around it, so taking it away needs a descendant selector — a shape
/// this sheet writes nowhere else. An input group therefore rings twice, once
/// around the group and once around the field inside it.
fn focus_ring(rules: &[&StyleRule]) -> String {
    let mut classes: Vec<&str> = rules
        .iter()
        .filter(|r| r.state == Some(STATE_FOCUS) && r.screen.is_none())
        .filter(|r| {
            matches!(
                r.property as usize,
                STYLE_BORDER_WIDTH | STYLE_BORDER_EDGE | STYLE_SHADOW | STYLE_SHADOWS
            )
        })
        .map(|r| r.class.as_str())
        .collect();
    if classes.is_empty() {
        return String::new();
    }
    classes.sort_unstable();
    let selectors = classes
        .iter()
        .map(|c| format!(".{c}"))
        .collect::<Vec<_>>()
        .join(",");
    format!(":where({selectors}):focus-visible{{outline:none}}\n")
}

/// Makes each child of a marked list the box its `::after` marker hangs from.
/// `:where()` keeps it weightless, so a child's own `Position` still wins.
fn marker_anchor(rules: &[&StyleRule]) -> String {
    let mut classes: Vec<&str> = rules
        .iter()
        .filter(|r| r.property as usize == STYLE_LIST_MARKER)
        .filter(|r| r.class.ends_with("lm-disc") || r.class.ends_with("lm-decimal"))
        .map(|r| r.class.as_str())
        .collect();
    if classes.is_empty() {
        return String::new();
    }
    classes.sort_unstable();
    let selectors = classes
        .iter()
        .map(|c| format!(".{c}"))
        .collect::<Vec<_>>()
        .join(",");
    format!(":where({selectors})>*{{position:relative}}\n")
}

/// The keyframes the sheet's animations name, each once, and only those a rule
/// runs. A variable-backed rule (`anim-var`) can run either, so it asks for both.
fn keyframes(rules: &[&StyleRule]) -> String {
    let animated: Vec<&str> = rules
        .iter()
        .filter(|r| r.property as usize == STYLE_ANIMATION)
        .map(|r| r.class.as_str())
        .collect();
    ANIMATIONS
        .iter()
        .filter(|(key, _, _)| {
            animated
                .iter()
                .any(|class| class.ends_with("anim-var") || class.ends_with(&format!("anim-{key}")))
        })
        .map(|(_, _, frames)| format!("{frames}\n"))
        .collect()
}

/// Everything a browser paints on an element by itself that no atomic class
/// can get under: the size, the weight and the margins on a heading, the bevel
/// on a button, the blue underline on a link, the border and the inner shadow
/// on a field, the disc and the forty-pixel indent on a list, the inset border
/// on a separator, and the baseline a picture sits on.
///
/// A class says what one property is and nothing about the rest, so the sheet
/// has to say it once, up front, for the elements the program actually builds.
/// `:where()` holds the selectors, which makes the reset weigh nothing in the
/// cascade — every class beats it, whatever order they land in.
///
/// One rule is about the box model rather than about any element: a `Width` is
/// the whole box, which is the box the headless painter measures.
///
/// What comes out is also what the headless painter already draws: no padding
/// nobody asked for, the surrounding font, no margin the scene document has
/// no counterpart for, and no marker beside a list item. A toggle's box is
/// cleared like every other control now that the widget draws its own mark:
/// `appearance:none` erases a browser's tick, and the two rules below put back
/// a tick and a thumb this vocabulary can paint.
#[derive(Clone, Copy, Default, Debug)]
pub struct Reset {
    /// The artifact builds a tree at all. The box model belongs to the
    /// vocabulary rather than to any one element, so it is asked once.
    pub tree: bool,
    pub heading: bool,
    pub button: bool,
    pub link: bool,
    pub field: bool,
    pub toggle: bool,
    pub list: bool,
    pub separator: bool,
    pub image: bool,
    pub dialog: bool,
    pub radiogroup: bool,
    pub disclosure: bool,
    pub table: bool,
    pub tooltip: bool,
}

/// The declarations a control drops. `font` and `color` are inherited rather
/// than cleared, because a control that does not say what it looks like should
/// look like the text around it.
const CONTROL_RESET: &str =
    "appearance:none;background:none;border:0;padding:0;font:inherit;color:inherit";

/// A slider's track: a bar a quarter of the control's height, across the
/// middle of it, in the control's own colour.
///
/// A background image rather than a background colour, so a `Background` on the
/// range is the box behind the bar rather than a fight with it. The painter
/// draws the same rectangle, unrounded, because a gradient has no corners.
const TRACK_BAR: &str = "background-image:linear-gradient(currentColor,currentColor);\
    background-size:100% 25%;background-repeat:no-repeat;background-position:center";

/// A slider's thumb: a disc one line across, in the control's own colour.
///
/// One line rather than the control's height, because a percentage height on a
/// slider thumb resolves against nothing in either engine. The painter pins it
/// to the same line, so a taller track has the same thumb in both.
const THUMB: &str = "appearance:none;-webkit-appearance:none;width:1rem;height:1rem;\
    border-radius:9999px;background-color:currentColor";

/// How much of the element's own foreground a hint is painted at. The painter
/// carries the same number as `PLACEHOLDER_FADE`, because a browser's default
/// placeholder colour differs by engine and neither is one a golden may rest
/// on.
const PLACEHOLDER_FADE: &str = "0.5";

/// A checkbox's tick, as the shape that masks the box's own colour.
///
/// A mask rather than a picture, because the mark takes the control's
/// `Foreground` and a picture would carry a colour of its own. The geometry is
/// the painter's `mark:tick` exactly — a stroke through the same three points
/// of the same sixteen-unit square — so the two renderers draw one tick.
const TICK_MASK: &str = "mask-image:url(\"data:image/svg+xml,\
    %3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'%3E\
    %3Cpath d='M3.5 8.5 6.5 11.5 12.5 4.5' fill='none' stroke='%23000' \
    stroke-width='2.5' stroke-linecap='round' stroke-linejoin='round'/%3E\
    %3C/svg%3E\");mask-size:contain;mask-repeat:no-repeat;mask-position:center";

/// A checked radio's dot: a disc half the box across, centred, in the control's
/// own colour. Half rather than the whole box because a radio's mark sits inside
/// its ring, which is what tells it apart from a checkbox at a glance. The
/// painter draws the same disc from the scene's `mark:dot`, so a browser and it
/// draw one dot.
const DOT: &str = "width:50%;height:50%;border-radius:9999px;background-color:currentColor";

impl Reset {
    fn rules(self) -> String {
        let mut out = String::new();
        if self.tree {
            // A `Width` is the whole box, padding and border inside it — which
            // is what the headless painter lays out and what every design
            // system this vocabulary is aimed at is written against. CSS's own
            // initial `content-box` would hang a padded, bordered child out of
            // its parent by exactly its padding.
            out.push_str("*,*::before,*::after{box-sizing:border-box}\n");
            // A border is off until a program gives an edge a width. CSS's
            // initial `border-width: medium` is 3px, held back only by the
            // initial `border-style: none`, so the moment a program sets a
            // style — a whole-box `BorderStyle(.Dashed)` beside a single
            // `BorderEdge`, say — every edge that was never given a width draws
            // a 3px line the caller never asked for, where the headless painter
            // draws a border only on the edge that has a width. Zeroing the
            // width here is what Tailwind's Preflight does for the same reason:
            // a style alone is nothing, and only a `Width`/`BorderEdge` draws a
            // line, on both renderers.
            out.push_str("*,*::before,*::after{border-width:0}\n");
            // A word too long for its box breaks mid-word rather than running
            // past the edge. The headless painter has broken one that way all
            // along — it shapes with a glyph-level fallback once a word will
            // not fit — where CSS's own initial `overflow-wrap: normal` lets
            // the word overflow, so the two renderers drew a long word two
            // different ways. `break-word` rather than `anywhere` because the
            // min-content width stays the longest word, not the widest letter:
            // the painter measures the same, so an `fr` track holds open to its
            // widest word rather than collapsing to a letter.
            out.push_str("*{overflow-wrap:break-word}\n");
            // The document is what the tree is mounted into, and a browser's
            // own sheet puts eight pixels around it. Nothing a program writes
            // can reach that: this vocabulary has no margin, a margin is not
            // inherited, and `mount` renders into `<body>` rather than into a
            // box the program made. Left alone it frames every page, on every
            // route, in a band of the browser's white.
            out.push_str(":where(body){margin:0}\n");
            // `Layout`'s documented default: a container that names none
            // stacks its children downwards, because a document is a column.
            // Without this a container is CSS's `display:block`, where
            // `AlignMain` and `AlignCross` are computed and do nothing — and
            // the headless painter has laid a container out as a flex column
            // all along. The four table elements are left out: a browser's own
            // table layout is what the scene document mirrors for them.
            out.push_str(
                ":where(div,nav,main,header,footer,aside,article,search,ul,li,hr,form,a,\
                 button){display:flex;flex-direction:column}\n",
            );
        }
        if self.heading {
            // A level is an outline position, not a size, so the size and the
            // weight belong to the styles and the margin to nobody: this
            // vocabulary has no margins, and the scene document the headless
            // painter reads has no counterpart for one.
            out.push_str(
                ":where(h1,h2,h3,h4,h5,h6){font-size:inherit;font-weight:inherit;margin:0}\n",
            );
        }
        if self.button {
            out.push_str(&format!(":where(button){{{CONTROL_RESET}}}\n"));
        }
        if self.link {
            out.push_str(":where(a){color:inherit;text-decoration:none}\n");
        }
        if self.field {
            // A checkbox is a toggle's, never a field's: it has a reset of its
            // own below, because it is the one control that draws a mark.
            out.push_str(&format!(
                ":where(input:not([type=checkbox]),textarea){{{CONTROL_RESET}}}\n"
            ));
            // A range is the other control that draws itself. The line above
            // has already taken a browser's own slider away, so these put back
            // one this vocabulary can paint: a track twelve lines wide and one
            // line tall, a bar a quarter of that height across the middle of
            // it, and a round thumb the browser slides along it. All three are
            // `currentColor`, so `Foreground` is the one property that paints a
            // slider, and the painter draws the same three from the scene's
            // `range:` — which is why the sizes are here rather than left to a
            // browser, whose own differ by a few pixels each.
            out.push_str(&format!(
                ":where(input[type=range]){{width:12rem;height:1rem;flex-shrink:0;\
                 {TRACK_BAR}}}\n"
            ));
            // Two rules and not one selector list: an unknown pseudo-element
            // throws the whole selector away, so a browser that knows only one
            // of these has to meet it on its own.
            out.push_str(&format!(
                ":where(input[type=range])::-webkit-slider-thumb{{{THUMB}}}\n"
            ));
            out.push_str(&format!(
                ":where(input[type=range])::-moz-range-thumb{{border:0;{THUMB}}}\n"
            ));
            // The hint inside an empty box. A browser's own placeholder colour
            // is its own — grey in one engine, a faded text colour in another
            // — and neither is a colour the painter can guess, so the sheet
            // pins it: the element's own foreground at `PLACEHOLDER_FADE`. The
            // painter multiplies the same fraction into the same colour, which
            // is what makes the two renderers draw one hint.
            out.push_str(&format!(
                ":where(input,textarea)::placeholder{{color:inherit;opacity:{PLACEHOLDER_FADE}}}\n"
            ));
        }
        if self.field || self.toggle || self.radiogroup {
            // The `<label>` wrapping a control and its text. A browser lays it
            // out as an inline box, which is not a box a surrounding row can
            // measure and not a box the headless painter has any flow for. One
            // wrapping row says the same thing to both, and `around` beats it
            // — every class does. A radio group's options are labels of this
            // shape too.
            out.push_str(
                ":where(label){display:flex;flex-direction:row;flex-wrap:wrap;\
                 align-items:center}\n",
            );
        }
        if self.toggle {
            // The box the widget draws its mark in. It is a row that centres
            // what is in it, one line square, and it does not shrink — with
            // `appearance:none` a browser gives an unsized checkbox no size at
            // all, and an invisible control is worse than a browser's own.
            out.push_str(&format!(
                ":where(input[type=checkbox]){{{CONTROL_RESET};display:flex;\
                 flex-direction:row;align-items:center;justify-content:flex-start;\
                 width:1rem;height:1rem;flex-shrink:0}}\n"
            ));
            // Where the mark sits. It is what makes a switch's thumb travel,
            // and it is nothing to a tick, which fills the box.
            out.push_str(":where(input[type=checkbox]:checked){justify-content:flex-end}\n");
            // The two marks. A switch always has its thumb; a checkbox has its
            // tick only when it is on. Both take the box's own colour, which is
            // the one property that paints them.
            out.push_str(
                ":where(input[type=checkbox][role=switch])::before{content:\"\";\
                 height:100%;aspect-ratio:1;border-radius:9999px;\
                 background-color:currentColor}\n",
            );
            out.push_str(&format!(
                ":where(input[type=checkbox]:not([role=switch]):checked)::before\
                 {{content:\"\";width:100%;height:100%;\
                 background-color:currentColor;{TICK_MASK}}}\n"
            ));
        }
        if self.radiogroup {
            // A radio draws a mark like a checkbox, so it clears the same chrome
            // and takes the same box: one line square, centred, and no shrink,
            // because with `appearance:none` a browser gives an unsized radio no
            // size at all. The one thing it keeps is the round outline a radio
            // has always had — the ring the dot sits in.
            out.push_str(&format!(
                ":where(input[type=radio]){{{CONTROL_RESET};display:flex;\
                 flex-direction:row;align-items:center;justify-content:center;\
                 width:1rem;height:1rem;flex-shrink:0;border-radius:9999px}}\n"
            ));
            // The dot, only when it is on. It takes the box's own colour, the
            // one property that paints it, and the painter draws the same disc
            // from the scene's `mark:dot`.
            out.push_str(&format!(
                ":where(input[type=radio]:checked)::before{{content:\"\";{DOT}}}\n"
            ));
        }
        if self.list {
            // `ListMarker` is how a list asks for a marker back, and it is a
            // class, so it beats this wherever the two meet.
            out.push_str(":where(ul,ol){margin:0;padding:0;list-style:none}\n");
        }
        if self.separator {
            // A browser's `<hr>` is a 1px `inset` border on all four edges and
            // an automatic inline margin, so a separator asked for one pixel
            // paints three in two tones and shrinks inside a column. A break
            // between sections is a rule the caller paints.
            out.push_str(":where(hr){border:0;margin:0}\n");
        }
        if self.dialog {
            // A browser's own `<dialog>` is a bordered, padded, `canvas`-white
            // card with a width and a height nobody asked for. What it keeps
            // is the half no style can say: `position:fixed`, the automatic
            // margins that centre it, and the top layer `showModal` puts it
            // in. The panel's own look is the styles'.
            // `overflow:visible` undoes the UA's `dialog:modal{overflow:auto}`,
            // which clips focus rings and pinned panels the painter draws whole.
            out.push_str(
                ":where(dialog){border:0;padding:0;background:none;color:inherit;\
                 max-width:none;max-height:none;overflow:visible}\n",
            );
            // Open, it is a container like every other one. Shut, it keeps a
            // browser's `display:none`, which is why this hangs off `[open]`
            // rather than joining the reset's own list of containers.
            out.push_str(":where(dialog[open]){display:flex;flex-direction:column}\n");
            // The scrim, which is the widget's own drawing rather than
            // anything a program wrote. `ui/node`'s `scrim` says the same
            // black at half strength to the painter, so the two renderers draw
            // one backdrop.
            out.push_str(":where(dialog)::backdrop{background-color:rgba(0,0,0,0.5)}\n");
        }
        if self.disclosure {
            // A `<details>` is a block and a `<summary>` is a `list-item` with
            // a triangle a browser draws for itself — none of which the scene
            // document has a counterpart for. So both become containers like
            // every other one, and the marker is taken away in the two spellings
            // a browser has for it: `list-style` reaches one engine and the
            // pseudo-element the other. What is left is the summary's text and
            // the caller's own children, which is what the painter draws.
            out.push_str(":where(details,summary){display:flex;flex-direction:column}\n");
            out.push_str(":where(summary){list-style:none}\n");
            out.push_str(":where(summary)::-webkit-details-marker{display:none}\n");
        }
        if self.tooltip {
            // The bubble sits below its trigger at the start edge, out of the
            // flow so it covers what follows rather than moving it, on one
            // line — `ui/node`'s `bubbleScene` says the same to the painter.
            // The wrapper is what it is placed against.
            out.push_str(":where(:has(>[role=tooltip])){position:relative}\n");
            out.push_str(
                ":where([role=tooltip]){position:absolute;inset-block-start:100%;\
                 inset-inline-start:0;text-wrap:nowrap;z-index:1}\n",
            );
            // Hidden is hidden whatever a class says its `display` is.
            out.push_str("[role=tooltip][hidden]{display:none!important}\n");
        }
        if self.table {
            // A browser's own sheet rules a table `border-collapse: separate`
            // with a 2px `border-spacing`, so a 1px border on each box sits two
            // pixels from its neighbour's and a rule drawn across a row comes
            // out broken. Collapsing the borders joins each box's edge to the
            // next, which is the continuous line the headless painter draws
            // edge to edge. There is no way back: a collapsed table is what a
            // data table is, and no style names `border-spacing`.
            out.push_str(":where(table){border-collapse:collapse}\n");
            // A browser pads each `<td>` and `<th>` by a pixel and centres or
            // baseline-aligns what is in it. The painter does neither.
            out.push_str(":where(td,th){padding:0;vertical-align:top}\n");
        }
        if self.image {
            // A picture and an inlined `<svg>` are the two leaves a browser
            // leaves inline, so each sits on the text baseline with a descender
            // gap under it. Every other leaf here is a block, and the headless
            // painter has no inline flow at all.
            out.push_str(":where(img,svg){display:block}\n");
        }
        out
    }
}

/// Which of those elements an enum literal builds.
///
/// The same question as [`Reached::themes`] and asked the same way: `NodeKind`
/// is `ui/node`'s private enum, so a literal of it was written inside that
/// module's own constructors and nowhere else. A list is the exception —
/// `region` takes its role as a parameter, so what names one is a `Role::List`
/// literal, and that is written at the call site.
fn reset_in(
    con: TyConId,
    variant: usize,
    node_con: TyConId,
    role_con: Option<TyConId>,
    out: &mut Reset,
) {
    if con == node_con {
        out.tree = true;
        match variant {
            NODE_HEADING => out.heading = true,
            NODE_BUTTON => out.button = true,
            NODE_LINK => out.link = true,
            // A slider is a `<input type="range">`, whose track and thumb
            // the field reset clears and re-draws, so it asks for the same
            // reset a field does.
            NODE_FIELD | NODE_SLIDER => out.field = true,
            NODE_TOGGLE => out.toggle = true,
            NODE_IMAGE | NODE_ICON => out.image = true,
            NODE_SUBMIT | NODE_FILE_PICKER => out.button = true,
            NODE_DIALOG => out.dialog = true,
            NODE_PICKER => out.radiogroup = true,
            NODE_DISCLOSURE => out.disclosure = true,
            NODE_TOOLTIP => out.tooltip = true,
            _ => {}
        }
    }
    if Some(con) == role_con {
        match variant {
            ROLE_LIST => out.list = true,
            ROLE_SEPARATOR => out.separator = true,
            ROLE_TABLE => out.table = true,
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Property -> CSS
// ---------------------------------------------------------------------------

/// What one property lowers to: the class abbreviation that names it, the
/// injective key for its value, and the `(selector suffix, declarations)`
/// blocks it renders to.
type Declaration = (&'static str, String, Vec<(&'static str, String)>);

/// How a key names a value with no bounded spelling: [`digest`] in a class,
/// [`preimage`] in the name `ui/style`'s `classify` looks the class up by.
type Digest = fn(&str) -> String;

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
fn declaration(variant: usize, args: &[Value], digest: Digest) -> Option<Declaration> {
    let one = |property: &str, value: &str| vec![("", format!("{property}:{value}"))];
    let first = args.first();
    match variant {
        // arrangement
        6 => {
            let (which, inner) = first?.as_variant()?;
            match which {
                0 => Some((
                    "lay",
                    "col".into(),
                    one("display", "flex;flex-direction:column"),
                )),
                1 => Some((
                    "lay",
                    "row".into(),
                    one("display", "flex;flex-direction:row"),
                )),
                // Reversed: the document keeps the order it was written in and
                // only the paint runs backwards, which is the whole reason a
                // component that is handed its children can ask for one.
                2 => Some((
                    "lay",
                    "colrev".into(),
                    one("display", "flex;flex-direction:column-reverse"),
                )),
                3 => Some((
                    "lay",
                    "rowrev".into(),
                    one("display", "flex;flex-direction:row-reverse"),
                )),
                4 => {
                    let Value::Array(tracks) = inner.first()? else {
                        return None;
                    };
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
            let (css, key) = cross_align(first?)?;
            Some(("ac", key.into(), one("align-items", css)))
        }
        9 => {
            let (css, key) = cross_align(first?)?;
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
            Some((
                "span",
                integer_key(n),
                one("grid-column", &format!("span {n}")),
            ))
        }
        // Only the inset: the `position` a pin needs is the `pin-out` class
        // `property` writes beside this one, so that four edges compose.
        15 => {
            let (property, edge) = edge_property("inset", first?)?;
            let (length, key) = length(args.get(1)?)?;
            Some(("pin", format!("{edge}-{key}"), one(&property, &length)))
        }
        16 => {
            let (which, _) = first?.as_variant()?;
            match which {
                0 => Some(("pos", "flow".into(), one("position", "relative"))),
                1 => Some(("pos", "sticky".into(), one("position", "sticky"))),
                2 => Some(("pos", "viewport".into(), one("position", "fixed"))),
                // `PinAnchor`: out of every ancestor's clip. `position: fixed`
                // is the browser's own way to lift a positioned box out of every
                // scroll container, and the `--buri-pin-anchor` marker — a
                // custom property a browser ignores — tells the snapshot painter
                // to keep the box against its `Flow` ancestor rather than the
                // page, so the golden shows it hanging below its trigger (#168).
                _ => Some((
                    "pos",
                    "anchor".into(),
                    one("position", "fixed;--buri-pin-anchor:1"),
                )),
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
            let (css, key) = colour(first?, digest)?;
            Some(("bg", key, one("background-color", &css)))
        }
        32 => {
            let (css, key) = colour(first?, digest)?;
            Some(("fg", key, one("color", &css)))
        }
        33 => {
            let (css, key) = border_width_length(first?)?;
            // A width on its own draws a solid border; `BorderStyle` is
            // declared later and overrides this.
            Some((
                "bw",
                key,
                one("border-style", &format!("solid;border-width:{css}")),
            ))
        }
        // The same solid a whole-box width implies, on one edge. `BorderStyle`
        // is written later and overrides it, exactly as it does for `bw`.
        34 => {
            let (property, edge) = edge_property("border", first?)?;
            let (css, key) = border_width_length(args.get(1)?)?;
            Some((
                "be",
                format!("{edge}-{key}"),
                one(
                    &format!("{property}-style"),
                    &format!("solid;{property}-width:{css}"),
                ),
            ))
        }
        35 => {
            let (css, key) = colour(first?, digest)?;
            Some(("bc", key, one("border-color", &css)))
        }
        36 => {
            let (which, _) = first?.as_variant()?;
            let css = ["none", "solid", "dashed"].get(which)?;
            Some(("bs", (*css).into(), one("border-style", css)))
        }
        37 => {
            let (css, key) = radius_length(first?)?;
            Some(("r", key, one("border-radius", &css)))
        }
        38 => {
            let (property, corner) = corner_property(first?)?;
            let (css, key) = radius_length(args.get(1)?)?;
            Some(("rc", format!("{corner}-{key}"), one(property, &css)))
        }
        39 => {
            let value = number(first?.as_float()?)?;
            Some(("op", number_key(&value), one("opacity", &value)))
        }
        40 => {
            let css = shadow(first?)?;
            Some(("sh", digest(&css), one("box-shadow", &css)))
        }
        // The layers in the order they were written, which is the order a
        // browser paints them — first over last. One layer here renders what
        // `Shadow` renders, so the two arrive at one class and one rule.
        41 => {
            let Value::Array(layers) = first? else {
                return None;
            };
            let mut rendered = Vec::with_capacity(layers.len());
            for layer in layers {
                rendered.push(shadow(layer)?);
            }
            let css = rendered.join(",");
            Some(("sh", digest(&css), one("box-shadow", &css)))
        }

        // text
        42 => {
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
                        || !name
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || " _-".contains(c))
                    {
                        return None;
                    }
                    format!("\"{name}\",ui-sans-serif,sans-serif")
                }
            };
            Some(("ff", digest(&stack), one("font-family", &stack)))
        }
        43 => spacing("fs", "font-size", first?),
        44 => {
            let (which, _) = first?.as_variant()?;
            let (css, key) = [
                ("400", "regular"),
                ("500", "medium"),
                ("600", "semibold"),
                ("700", "bold"),
            ]
            .get(which)
            .copied()?;
            Some(("fw", key.into(), one("font-weight", css)))
        }
        45 => {
            let on = first?.as_bool()?;
            let css = if on { "italic" } else { "normal" };
            Some(("it", css.into(), one("font-style", css)))
        }
        46 => {
            let value = number(first?.as_float()?)?;
            Some(("lh", number_key(&value), one("line-height", &value)))
        }
        47 => spacing("ls", "letter-spacing", first?),
        48 => {
            let (_, key) = align(first?)?;
            // Text has no leftover room to distribute, so every distribution
            // means justified.
            let css = match key {
                "start" | "center" | "end" => key,
                _ => "justify",
            };
            Some(("ta", key.into(), one("text-align", css)))
        }
        49 => {
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
        50 => {
            let (which, _) = first?.as_variant()?;
            let (css, key) = [
                ("none", "none"),
                ("underline", "underline"),
                ("line-through", "strike"),
            ]
            .get(which)
            .copied()?;
            Some(("tl", key.into(), one("text-decoration-line", css)))
        }
        51 => {
            let (which, _) = first?.as_variant()?;
            let css = ["wrap", "nowrap", "balance"].get(which)?;
            Some(("tw", (*css).into(), one("text-wrap", css)))
        }
        52 => {
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
        53 => {
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

        // lists
        54 => {
            let (which, _) = first?.as_variant()?;
            // The type, not the shorthand: the reset already cleared the
            // indent, and a marker asked for here must not put it back.
            //
            // A browser draws `::marker` only on `display:list-item`, and the
            // reset makes every item a flex box. So `::after` draws the
            // painter's marker instead, sized by its MARKER_DISC and MARKER_GAP.
            let blocks = match which {
                0 => vec![
                    ("", "list-style-type:none".into()),
                    (">*::after", "content:none".into()),
                ],
                1 => vec![
                    ("", "list-style-type:disc".into()),
                    (
                        ">*::after",
                        "content:\"\";position:absolute;top:calc(0.5lh - 0.175em);\
                         right:calc(100% + 0.4em);width:0.35em;height:0.35em;\
                         border-radius:9999px;background-color:currentColor;\
                         white-space:normal"
                            .into(),
                    ),
                ],
                2 => vec![
                    ("", "list-style-type:decimal;counter-reset:buri-item".into()),
                    (">*", "counter-increment:buri-item".into()),
                    (
                        ">*::after",
                        "content:counter(buri-item) \".\";position:absolute;top:0;\
                         right:calc(100% + 0.4em);width:auto;height:auto;\
                         border-radius:0;background-color:transparent;white-space:nowrap"
                            .into(),
                    ),
                ],
                _ => return None,
            };
            let key = *["none", "disc", "decimal"].get(which)?;
            Some(("lm", key.into(), blocks))
        }

        // out of the box the container put the child in
        55 => {
            let (property, edge) = edge_property("margin", first?)?;
            let value = args.get(1)?;
            let (_, key) = length(value)?;
            Some((
                "bleed",
                format!("{edge}-{key}"),
                one(&property, &outwards(value)?),
            ))
        }

        // transform
        56 => {
            let (x, x_key) = length(first?)?;
            let (y, y_key) = length(args.get(1)?)?;
            // A `-` separates the two keys, and no length's key holds one, so
            // the pair is injective the way each half is.
            Some((
                "tr",
                format!("{x_key}-{y_key}"),
                one("transform", &format!("translate({x},{y})")),
            ))
        }

        // what is painted outside the box, and what the pointer does with it
        57 => {
            let on = first?.as_bool()?;
            // `clip` rather than `hidden`: both stop the paint, and only
            // `hidden` also makes a scroll container a keyboard can land in.
            let css = if on { "clip" } else { "visible" };
            Some(("clip", css.into(), one("overflow", css)))
        }
        58 => {
            let on = first?.as_bool()?;
            let css = if on { "none" } else { "auto" };
            Some(("pass", css.into(), one("pointer-events", css)))
        }
        // The blur that separates a modal's panel from the page behind it. Only
        // what is painted behind the box is blurred; the box paints over it.
        59 => {
            let (css, key) = length(first?)?;
            Some((
                "bdblur",
                key,
                one("backdrop-filter", &format!("blur({css})")),
            ))
        }
        // The caret on its own, so a field with transparent text — the mirror a
        // syntax-highlighted `<input>` is drawn under — still has a visible one.
        // A colour like `Foreground`'s, which is the property that otherwise
        // reaches the caret.
        60 => {
            let (css, key) = colour(first?, digest)?;
            Some(("caret", key, one("caret-color", &css)))
        }
        // Out of sight, in the tree: the "sr-only" clip. A one-pixel box out of
        // the flow with its overflow clipped away, so the element paints
        // nothing and takes no space while its text stays for the accessible
        // name. The `--buri-visually-hidden` marker — a custom property a
        // browser ignores — is what tells the snapshot painter to paint nothing
        // rather than a clipped pixel (#188).
        61 => Some((
            "vh",
            "on".into(),
            vec![(
                "",
                "position:absolute;width:1px;height:1px;padding:0;margin:-1px;\
                 overflow:hidden;clip:rect(0,0,0,0);white-space:nowrap;border:0;\
                 --buri-visually-hidden:1"
                    .to_owned(),
            )],
        )),
        // How the element moves. The keyframes are not here: `stylesheet`
        // writes each one once, beside the rules that run it, and guards those
        // rules by the reader's motion preference.
        STYLE_ANIMATION => {
            let (which, _) = first?.as_variant()?;
            let (key, css, _) = *ANIMATIONS.get(which)?;
            Some(("anim", key.into(), one("animation", css)))
        }
        _ => None,
    }
}

/// Each `Animation`, in declaration order: its class key, the `animation` it
/// lowers to, and the keyframes that animation names. `ui/node`'s `declare` and
/// `runtime.js`'s `$TREE_MOTION` write the same values.
///
/// `Pulse` is Tailwind's `animate-pulse`. `Spin` turns with `rotate` rather than
/// `transform`, so it composes with a `Translate` instead of replacing it.
const ANIMATIONS: [(&str, &str, &str); 2] = [
    (
        "pulse",
        "buri-pulse 2s cubic-bezier(0.4,0,0.6,1) infinite",
        "@keyframes buri-pulse{50%{opacity:0.5}}",
    ),
    (
        "spin",
        "buri-spin 1s linear infinite",
        "@keyframes buri-spin{to{rotate:360deg}}",
    ),
];

/// The query an animation rule sits inside, so a reader whose platform asks for
/// reduced motion sees the element still.
const MOTION_ALLOWED: &str = "@media (prefers-reduced-motion:no-preference)";

/// A bleed's length, as the margin it writes.
///
/// A bleed is a distance *outwards*, so the margin is its negation. Nothing
/// goes the other way: an inward margin is the space between things, which
/// belongs to the container, so a negative distance and `.Auto` — which is no
/// distance at all — both bleed nothing.
fn outwards(value: &Value) -> Option<String> {
    let (which, args) = value.as_variant()?;
    let out = |n: f64, unit: &str| -> Option<String> {
        let rendered = number(n)?;
        Some(if n > 0.0 {
            format!("-{rendered}{unit}")
        } else {
            "0px".to_owned()
        })
    };
    match which {
        0 => {
            let n = args.first()?.as_int()?;
            Some(if n > 0 {
                format!("-{n}px")
            } else {
                "0px".to_owned()
            })
        }
        1 => out(args.first()?.as_float()?, "rem"),
        2 => out(args.first()?.as_float()?, "em"),
        3 => out(args.first()?.as_float()?, "%"),
        4 => Some("0px".to_owned()),
        5 => Some("-100%".to_owned()),
        _ => None,
    }
}

/// A colour's alpha, where it has one. `Rgb`, `Transparent` and `Inherit` do
/// not, and a token's is decided by whatever theme is installed.
fn alpha_of(colour: &Value) -> Option<f64> {
    let (which, args) = colour.as_variant()?;
    match which {
        COLOR_RGBA => args.get(3)?.as_float(),
        COLOR_FADED => args.get(1)?.as_float(),
        _ => None,
    }
}

/// One `Shadow`, as the four lengths and the colour a `box-shadow` layer is.
fn shadow(value: &Value) -> Option<String> {
    let Value::Struct(fields) = value else {
        return None;
    };
    let (x, _) = length(fields.first()?)?;
    let (y, _) = length(fields.get(1)?)?;
    let (blur, _) = length(fields.get(2)?)?;
    let (spread, _) = length(fields.get(3)?)?;
    let (colour, _) = colour(fields.get(4)?, digest)?;
    Some(format!("{x} {y} {blur} {spread} {colour}"))
}

/// A property whose only value is a `Length`.
fn spacing(abbreviation: &'static str, property: &str, value: &Value) -> Option<Declaration> {
    let (css, key) = length(value)?;
    Some((abbreviation, key, vec![("", format!("{property}:{css}"))]))
}

/// A property whose only value is a count.
fn count(abbreviation: &'static str, property: &str, value: &Value) -> Option<Declaration> {
    let n = value.as_int()?;
    Some((
        abbreviation,
        integer_key(n),
        vec![("", format!("{property}:{n}"))],
    ))
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

/// One logical corner, as the CSS radius property it names.
///
/// CSS spells a corner as the two logical edges that meet at it, block edge
/// first: `border-start-end-radius` is the top edge's end. `Corner` says the
/// same thing in the same order, so this is a rename rather than a mapping.
fn corner_property(value: &Value) -> Option<(&'static str, &'static str)> {
    let (which, _) = value.as_variant()?;
    [
        ("border-start-start-radius", "topstart"),
        ("border-start-end-radius", "topend"),
        ("border-end-start-radius", "bottomstart"),
        ("border-end-end-radius", "bottomend"),
    ]
    .get(which)
    .copied()
}

/// A `Length` as the radius it names.
///
/// `.Full` is a pill on every box — `9999px`, the way Tailwind's `rounded-full`
/// is — rather than the `100%` a length would otherwise lower to. A percentage
/// radius resolves per axis, so `100%` is an ellipse of the box on anything not
/// square, which is never what a caller reaching for a pill means. Every other
/// length is the radius it is, so `Percent(50)` stays the ellipse for a caller
/// who wants one.
fn radius_length(value: &Value) -> Option<(String, String)> {
    if value.as_variant()?.0 == 5 {
        return Some(("9999px".into(), "full".into()));
    }
    length(value)
}

/// A border width accepts only an absolute length. A percentage, `.Auto` or
/// `.Full` is not a width a browser keeps: it drops the declaration but leaves
/// the `border-style: solid` beside it, so the box draws the CSS `medium`
/// default — a 3px line nobody asked for — where the painter drew nothing. So a
/// border given a length that is not a width is no border at all, on both
/// renderers.
fn border_width_length(value: &Value) -> Option<(String, String)> {
    match value.as_variant()?.0 {
        // Px, Rem, Em — the absolute lengths a border width is measured in.
        0..=2 => length(value),
        // Percent, Auto, Full — not a width; the whole border is dropped.
        _ => None,
    }
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
            Some((format!("{n}em"), format!("e{}", number_key(&n))))
        }
        3 => {
            let n = number(args.first()?.as_float()?)?;
            Some((format!("{n}%"), format!("pc{}", number_key(&n))))
        }
        4 => Some(("auto".into(), "auto".into())),
        5 => Some(("100%".into(), "full".into())),
        _ => None,
    }
}

fn colour(value: &Value, digest: Digest) -> Option<(String, String)> {
    let (which, args) = value.as_variant()?;
    let channel = |v: Option<&Value>| -> Option<u8> { u8::try_from(v?.as_int()?).ok() };
    match which {
        0 => {
            let (r, g, b) = (
                channel(args.first())?,
                channel(args.get(1))?,
                channel(args.get(2))?,
            );
            Some((
                format!("rgb({r},{g},{b})"),
                format!("{r:02x}{g:02x}{b:02x}"),
            ))
        }
        1 => {
            let (r, g, b) = (
                channel(args.first())?,
                channel(args.get(1))?,
                channel(args.get(2))?,
            );
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
            let Value::Struct(parts) = args.first()? else {
                return None;
            };
            let namespace = parts.first()?.as_str()?;
            let name = parts.get(1)?.as_str()?;
            Some((
                format!("var(--{namespace}-{name})"),
                token_key(namespace, name, digest),
            ))
        }
        3 => Some(("transparent".into(), "none".into())),
        4 => Some(("inherit".into(), "inherit".into())),
        // A token, faded. Blending it here is impossible — nothing knows what
        // a token is worth until a theme is installed — so the token stays a
        // token inside a `color-mix` and follows the theme like any `var()`.
        5 => {
            let Value::Struct(parts) = args.first()? else {
                return None;
            };
            let namespace = parts.first()?.as_str()?;
            let name = parts.get(1)?.as_str()?;
            let a = number(args.get(1)?.as_float()?)?;
            let percent = number(args.get(1)?.as_float()? * 100.0)?;
            Some((
                format!("color-mix(in srgb,var(--{namespace}-{name}) {percent}%,transparent)"),
                format!("{}a{}", token_key(namespace, name, digest), number_key(&a)),
            ))
        }
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
fn token_key(namespace: &str, name: &str, digest: Digest) -> String {
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

/// The cross-axis reading of an `Align`, for `AlignCross` and `AlignSelf`.
///
/// The three distributions — `space-between`, `space-around`, `space-evenly` —
/// have no cross-axis meaning: `align-items:space-between` is not a legal
/// declaration, so a browser drops it and `align-items` stays `normal`, which
/// for a flex container is stretch. The extractor writes the stretch a browser
/// lands on rather than a declaration a browser will throw away, so the sheet,
/// the scene document and the painter all agree by construction. The four that
/// do have a cross-axis meaning are themselves.
fn cross_align(value: &Value) -> Option<(&'static str, &'static str)> {
    match align(value)? {
        ("space-between" | "space-around" | "space-evenly", _) => Some(("stretch", "stretch")),
        pair => Some(pair),
    }
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

/// What `ui/style` names a digested value by, since it has no hash of its own:
/// the text, behind a `#` no class holds.
fn preimage(text: &str) -> String {
    format!("#{text}")
}

// ---------------------------------------------------------------------------
// Walking
// ---------------------------------------------------------------------------

/// The `ui` types a walk over a program reads, each `None` where the
/// compilation did not load it.
#[derive(Clone, Copy)]
pub struct UiTypes {
    pub style: Option<TyConId>,
    pub theme: Option<TyConId>,
    pub node: Option<TyConId>,
    pub role: Option<TyConId>,
}

/// What a walk over the functions a program kept found out about its user
/// interface.
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
    /// a style a node is given and it could evaluate *in place* into an
    /// `Extracted`, so a property variant that survives to here is one no node
    /// is given, or one the compiler could not fold and which degrades to
    /// inline rather than failing.
    ///
    /// An over-approximation is safe and an under-approximation is not, so the
    /// question is asked of the tags rather than of what a tag might hold: a
    /// property inside a `Computed`'s closure is one of these too, and counting
    /// it twice costs nothing. Complete because a `Style` has no constructor
    /// but an enum literal — a value reaching a `Computed` from anywhere at all
    /// was written down somewhere in the program.
    pub inline: bool,
    /// Whether an `Animation` is among those, asked the same way. Only then
    /// does the inline lowering need to know one.
    pub inline_animations: bool,
    /// Whether the program builds a `ui/theme` `Theme`.
    ///
    /// Asked the way [`Reached::inline`] is: a `Theme` is an opaque struct
    /// with a private field, wrapping a private enum, and the only two
    /// literals of it are written inside `themed` and `switching`. A program
    /// that monomorphized neither can hand `mount` nothing but an empty list,
    /// and the whole theme half of the runtime — resolution, rendering, the
    /// `:root` block, the switch's computation — is unreachable.
    pub themes: bool,
    /// Whether the program can build an icon.
    ///
    /// Asked the way `themes` is, and for the same reason: the renderer
    /// reaches `$tree_icon` through a hole rather than by name, so the parser
    /// and the two allow lists — 2.5 KB of them — ship only in an artifact
    /// that has artwork in it. `NodeKind` is `ui/node`'s private enum, so a
    /// literal of it was written inside that module's own constructors and
    /// nowhere else.
    pub icons: bool,
    /// Which elements a browser paints something on the program builds, which
    /// is which reset rules the sheet opens with.
    pub reset: Reset,
}

/// Reads an expression and everything under it into `out`: the styles it
/// names, and whether it builds a theme, an icon, or which elements.
///
/// Read after monomorphization, over the functions a program actually kept.
/// One walk for every question, each asked only of a type the compilation
/// loaded, so a program that is not a user interface finds nothing.
///
/// A style left standing as a value, which [`run`] noted in `sources`, counts
/// as the classes it stands for. A call to a `classify` in `unchecked` is told
/// the compiler did not see every value it can be given, so a build that is
/// not a test's never stops on one.
pub fn survey(
    e: &mut typed::Expr,
    ui: UiTypes,
    sources: &Sources,
    unchecked: &[FuncIdx],
    out: &mut Reached,
) {
    if let Some(classes) = sources.get(&e.span) {
        out.classes.extend(classes.iter().cloned());
        return;
    }
    match &mut e.kind {
        ExprKind::EnumLit {
            con, variant, args, ..
        } => {
            if Some(*con) == ui.style {
                collect(*variant, args, out);
            }
            if let Some(node_con) = ui.node {
                out.icons |= *con == node_con && *variant == NODE_ICON;
                reset_in(*con, *variant, node_con, ui.role, &mut out.reset);
            }
        }
        ExprKind::StructLit { con, .. } if Some(*con) == ui.theme => out.themes = true,
        ExprKind::CallFunc { func: f, args } if unchecked.contains(f) => {
            let strict = args
                .get_mut(2)
                .or_ice("`classify` is given strictness third");
            strict.kind = ExprKind::Bool(false);
        }
        _ => {}
    }
    typed::children_mut(e, &mut |child| survey(child, ui, sources, unchecked, out));
}

/// The styles one `Style` literal names, gathered for [`Reached`].
fn collect(variant: usize, args: &[typed::Expr], out: &mut Reached) {
    match variant {
        STYLE_EXTRACTED => {
            // `Extracted(Classes([(slot, class), …]))`: one field, and
            // the list is inside it.
            let list = match args.first() {
                Some(typed::Expr {
                    kind: ExprKind::StructLit { fields, .. },
                    ..
                }) => fields.first(),
                _ => None,
            };
            if let Some(typed::Expr {
                kind: ExprKind::Array(items),
                ..
            }) = list
            {
                // A `Classes` literal is the extractor's: a program cannot
                // write one, its field being private.
                for item in items {
                    let class = match &item.kind {
                        ExprKind::Tuple(pair) => match pair.get(1) {
                            Some(typed::Expr {
                                kind: ExprKind::Str(class),
                                ..
                            }) => Some(class),
                            _ => None,
                        },
                        _ => None,
                    };
                    out.classes.insert(
                        class
                            .or_ice("the extractor writes each class as a `(slot, class)` pair")
                            .clone(),
                    );
                }
            }
        }
        // `Group` is transparent and `When` holds two lists that were
        // extracted like any other; neither is a property of its own.
        // `On` and `At` exist only in the stylesheet: one a node is given is
        // extracted, or goes through `classify`, which makes it classes.
        STYLE_GROUP | STYLE_WHEN | STYLE_ON | STYLE_AT => {}
        other => {
            out.inline = true;
            out.inline_animations |= other == STYLE_ANIMATION;
            // A dynamic value that lowers to a variable-backed rule
            // (#195) names that rule's class here, so the sheet keeps it
            // the way it keeps a class an `Extracted` pair names.
            if let Some((abbreviation, _)) = variable_property(other) {
                out.classes.insert(format!("{abbreviation}-var"));
            }
        }
    }
}
