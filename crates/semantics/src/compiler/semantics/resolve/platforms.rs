//! What the checker asks of a repository's own platforms and effects.
//!
//! A platform rule under `//platform/` declares a host type and entries in its
//! `platform.buri`, and an output naming the platform is held to them. A custom
//! effect lives in an effect package under `//platform/effect/`, beside a test
//! implementation. Everything here runs once signatures and conformances are
//! settled, because every question is about a signature or an `impl`.

use super::*;
use crate::build::buildfile::{Backend, CustomPlatform};
use crate::build::workspace::{is_test_only_path, ModuleKind, ModuleLocation};
use crate::compiler::semantics::crossing::{self, Known};

/// Whether a module path is part of an effect package: under
/// `//platform/effect/`, and outside its `testing` surface. Only these, and the
/// bundled platform modules, declare an effect.
pub fn is_effect_package_path(path: &str) -> bool {
    path.starts_with("//platform/effect/") && !is_test_only_path(path)
}

/// Whether a module is part of an effect package, by its path or, for a
/// module standing in a package without a file of its own (a documented
/// example), by its package's.
pub fn is_effect_package_module(
    ws: Option<&dyn crate::build::workspace::Packages>,
    m: &crate::compiler::modules::ModuleData,
) -> bool {
    if m.role == Role::TestOnly || is_test_only_path(&m.path) {
        return false;
    }
    if is_effect_package_path(&m.path) {
        return true;
    }
    match (ws, m.pkg) {
        (Some(ws), Some(pkg)) => ws.package(pkg).path.starts_with("platform/effect/"),
        _ => false,
    }
}

/// The production structs a backend has no implementation of, as a host type
/// names them from `platform/host`. `NATIVE` runs the reactive graph for a
/// test implementation alone, so a native host has no `HostUi` or `HostWatch`.
fn lacking(backend: Backend) -> &'static [&'static str] {
    match backend {
        Backend::Js => &["HostListen", "HostTcp"],
        Backend::Native => &["HostUi", "HostWatch"],
    }
}

/// The host type a signature takes first, when that is a struct its
/// platform's `platform.buri` declares.
pub fn declared_host(tables: &Tables, platform: ModuleId, params: &[ParamInfo]) -> Option<TyConId> {
    let TyKind::Con(con, args) = (params.first()?.ty).kind() else {
        return None;
    };
    let tycon = tables.tycon(*con);
    (args.is_empty() && tycon.module == platform && matches!(tycon.def, TyDef::Struct { .. }))
        .then_some(*con)
}

/// The types of the platform's own that an entry's host reaches, each with the
/// methods it declares without a body, which the entry's `js` file implements.
/// The host's fields come first, then whatever those reach through their
/// fields, their methods' signatures and their methods' bodies, and the
/// platform's own functions those call: `web`'s `IndexedDb` through its
/// `HostStorage`. A struct only another entry's host reaches isn't this one's.
pub fn js_structs(
    tables: &Tables,
    bodies: &BodyMap,
    platform: ModuleId,
    host: TyConId,
) -> Vec<(TyConId, Vec<FnId>)> {
    let mut reach = Reach {
        tables,
        platform,
        cons: vec![host],
        fns: Vec::new(),
    };
    let (mut next_con, mut next_fn) = (0, 0);
    loop {
        if let Some(&con) = reach.cons.get(next_con) {
            next_con = next_con.saturating_add(1);
            let tycon = tables.tycon(con);
            let variants = tycon.variants().iter().flat_map(|v| &v.fields);
            for field in tycon.fields().iter().chain(variants) {
                reach.ty(field.ty);
            }
            let methods = tables.fns.entries().filter(|(_, f)| f.self_ty == Some(con));
            for (id, _) in methods {
                reach.fn_(id);
            }
        } else if let Some(&f) = reach.fns.get(next_fn) {
            next_fn = next_fn.saturating_add(1);
            let info = tables.fn_info(f);
            for p in &info.params {
                reach.ty(p.ty);
            }
            reach.ty(info.ret);
            if let Some(body) = bodies.get(&f) {
                typed::walk(&body.expr, &mut |e| {
                    reach.ty(e.ty);
                    match &e.kind {
                        typed::ExprKind::StructLit { con, .. }
                        | typed::ExprKind::EnumLit { con, .. } => reach.con(*con),
                        typed::ExprKind::CallFn { func, .. } | typed::ExprKind::FnRef(func) => {
                            reach.fn_(func.id);
                        }
                        _ => {}
                    }
                });
            }
        } else {
            break;
        }
    }
    let reached = reach.cons.into_iter().filter(|c| *c != host);
    reached
        .map(|con| (con, bodiless_methods(tables, con)))
        .collect()
}

/// What [`js_structs`] has found so far, in the order it found it.
struct Reach<'a> {
    tables: &'a Tables,
    platform: ModuleId,
    cons: Vec<TyConId>,
    fns: Vec<FnId>,
}

impl Reach<'_> {
    fn con(&mut self, con: TyConId) {
        if self.tables.tycon(con).module == self.platform && !self.cons.contains(&con) {
            self.cons.push(con);
        }
    }

    fn fn_(&mut self, f: FnId) {
        if self.tables.fn_info(f).module == self.platform && !self.fns.contains(&f) {
            self.fns.push(f);
        }
    }

    fn ty(&mut self, ty: Ty) {
        match ty.kind() {
            TyKind::Con(con, args) => {
                self.con(*con);
                args.iter().for_each(|a| self.ty(*a));
            }
            TyKind::Array(t) => self.ty(*t),
            TyKind::Tuple(ts) => ts.iter().for_each(|t| self.ty(*t)),
            TyKind::Fn(ps, r) => {
                ps.iter().for_each(|p| self.ty(*p));
                self.ty(*r);
            }
            _ => {}
        }
    }
}

/// Every type of the platform's own with methods declared without a body,
/// each with them: what some entry's `js` file implements.
fn bodiless_structs(tables: &Tables, platform: ModuleId) -> Vec<(TyConId, Vec<FnId>)> {
    let cons = tables
        .tycons
        .entries()
        .filter(|(_, t)| t.module == platform);
    cons.map(|(id, _)| (id, bodiless_methods(tables, id)))
        .filter(|(_, methods)| !methods.is_empty())
        .collect()
}

/// The methods a platform's own production struct declares without a body.
fn bodiless_methods(tables: &Tables, con: TyConId) -> Vec<FnId> {
    let fns = tables.fns.entries();
    fns.filter(|(_, f)| f.self_ty == Some(con) && f.intrinsic)
        .map(|(id, _)| id)
        .collect()
}

impl<'a> Checker<'a> {
    /// Whether a module is a repository platform's `platform.buri`.
    pub(crate) fn is_platform_surface(&self, module: ModuleId) -> bool {
        let Some(ws) = self.ws else { return false };
        let path = &self.module(module).path;
        path.ends_with("/platform.buri")
            && matches!(
                ws.resolve_module(path),
                Ok(ModuleLocation::InPackage(m)) if m.kind == ModuleKind::PlatformSurface
            )
    }

    /// The repository platforms whose entry this function fills, one per
    /// output naming one, or `None` where no output of a repository platform
    /// enters through it.
    pub(super) fn custom_entries(
        &self,
        module: ModuleId,
        name: &str,
    ) -> Option<Vec<CustomPlatform>> {
        // A build is of one output, and only its own entry is its business.
        if let (Some(_), Some(built)) = (self.loaded.platform, self.loaded.entry.as_deref()) {
            return match (&self.loaded.custom, built == name) {
                (Some(c), true) => Some(vec![c.clone()]),
                _ => None,
            };
        }
        let (ws, pkg) = (self.ws?, self.module(module).pkg?);
        let target = TargetId {
            package: pkg,
            kind: RuleKind::Binary,
        };
        let mut out: Vec<CustomPlatform> = Vec::new();
        for c in ws
            .declared_entries(target)
            .into_iter()
            .filter(|e| e.name == name)
            .filter_map(|e| e.custom)
        {
            if !out
                .iter()
                .any(|o| o.label.value == c.label.value && o.point == c.point)
            {
                out.push(c);
            }
        }
        (!out.is_empty()).then_some(out)
    }

    /// The `platform.buri` a repository platform's label names, when this
    /// compilation loaded it.
    fn platform_module(&self, custom: &CustomPlatform) -> Option<ModuleId> {
        self.loaded
            .find(&format!("//{}/platform.buri", custom.package_path()))
    }

    fn shown(&self, ty: &Ty) -> String {
        show(&self.tables, None, &[], ty)
    }

    /// `fn fetch(host: CloudflareHost, request: Request): Response`.
    fn signature_text(&self, name: &str, info: &FnInfo) -> String {
        let params: Vec<String> = info
            .params
            .iter()
            .map(|p| format!("{}: {}", p.name, self.shown(&p.ty)))
            .collect();
        format!(
            "fn {name}({}): {}",
            params.join(", "),
            self.shown(&info.ret)
        )
    }

    /// One function against the entry a repository platform declares for it:
    /// the same host first, then the same parameters and the same answer.
    pub(super) fn check_custom_entry(
        &mut self,
        fid: FnId,
        d: &tree::FnDecl,
        name: &str,
        custom: &CustomPlatform,
    ) {
        let Some(platform) = self.platform_module(custom) else {
            return;
        };
        let label = custom.label.value.clone();
        let Some(Sym::Fn(decl)) = self.scope(platform).own.get(&custom.point).cloned() else {
            let point = &custom.point;
            self.templated("platform-entry-undeclared", d.span)
                .bind("entry", name.to_string())
                .bind("point", point.clone())
                .bind("platform", label.clone())
                .fix(format!(
                    "declare it in `{label}`'s `platform.buri`, as `export fn {point}(host: H, ...): ...;`"
                ));
            return;
        };
        let want = self.tables.fn_info(decl).clone();
        let info = self.tables.fn_info(fid).clone();
        self.refuse_entry_generics(&info, d, name);
        let host = declared_host(&self.tables, platform, &want.params);
        let wanted = self.signature_text(name, &want);
        match (info.params.first(), host) {
            (None, Some(host)) => {
                let host_name = self.tables.tycon(host).name.clone();
                let fix = format!(
                    "take the platform's host and bind its fields:\n     export {wanted} {{\n         \
                     ...\n     }}"
                );
                self.templated("entry-missing-host", d.span)
                    .bind("entry", name.to_string())
                    .fix(fix)
                    .notes
                    .push(format!(
                        "the host type is `{label}`'s: `from \"{label}\" import {{ {host_name} }};`"
                    ));
                return;
            }
            (Some(param), Some(host)) => {
                let host_ty = Ty::con(host, []);
                if param.ty != host_ty && !param.ty.is_error() {
                    let host_name = self.tables.tycon(host).name.clone();
                    let taken = self.shown(&param.ty);
                    self.templated("entry-host-mismatch", param.span)
                        .bind("entry", name.to_string())
                        .bind("taken", taken)
                        .bind("platform", label.clone())
                        .bind("wanted", host_name.clone())
                        .fix(format!(
                            "take `{host_name}`, imported with `from \"{label}\" import \
                             {{ {host_name} }};`"
                        ));
                    return;
                }
            }
            _ => {}
        }
        let skip = usize::from(host.is_some());
        // An unknown type on either side was reported where it was written.
        let agree = |a: &Ty, b: &Ty| a == b || a.is_error() || b.is_error();
        let same = info.params.len() == want.params.len()
            && info
                .params
                .iter()
                .zip(&want.params)
                .skip(skip)
                .all(|(a, b)| agree(&a.ty, &b.ty))
            && agree(&info.ret, &want.ret);
        if !same {
            self.templated("entry-signature-mismatch", d.span)
                .bind("entry", name.to_string())
                .bind(
                    "requirement",
                    format!("have the signature `{label}` declares, `{wanted}`"),
                )
                .fix(format!("write it `{wanted}`"));
        } else if custom.js.is_none()
            && !(host.is_some()
                && want.params.len() == 1
                && (self.is_program_answer(&want.ret) || want.ret.is_error()))
        {
            // Nothing but the backend calls an entry without a `js` file, and
            // the backend passes the host alone and reads a `Result<(), Str>`.
            let point = &custom.point;
            self.templated("entry-signature-mismatch", d.span)
                .bind("entry", name.to_string())
                .bind(
                    "requirement",
                    "take only its host and answer `Result<(), Str>`: its platform gives it no \
                     `js` file, so it starts itself",
                )
                .fix(format!(
                    "declare `{point}` in `{label}` as `fn {point}(host: H): Result<(), Str>`, or \
                     give its `entry` a `js` file that calls it"
                ));
        }
        // What a `js` file hands an entry and reads back crosses the table,
        // whether or not the declaration takes a host.
        if custom.backend == Backend::Js && custom.js.is_some() {
            self.check_crossing_of(&want, skip, &format!("`{}`", custom.point));
        }
        if let Some(host) = host {
            self.check_host_fields(custom, platform, host);
            if custom.backend == Backend::Js {
                self.check_js_methods(platform);
            }
        }
    }

    /// Refuses generic parameters on an entry, which the platform calls.
    pub(super) fn refuse_entry_generics(&mut self, info: &FnInfo, d: &tree::FnDecl, name: &str) {
        if !info.generics.is_empty() {
            self.templated("entry-signature-mismatch", d.span)
                .bind("entry", name.to_string())
                .bind("requirement", "declare no generic parameters")
                .fix(format!(
                    "drop them: `{name}` is called by the platform, so there is nothing to infer \
                     them from"
                ));
        }
    }

    /// Whether `ty` is `Result<(), Str>`, what a program entry answers.
    pub(super) fn is_program_answer(&self, ty: &Ty) -> bool {
        let str_ty = self.tables.prim(Prim::Str);
        matches!(ty.kind(), TyKind::Con(id, [ok, err])
            if self.result_con.as_ref() == Some(id) && *ok == Ty::UNIT && *err == str_ty)
    }

    /// Every field of a host type is a production struct its backend has: the
    /// backend's own from `platform/host`, or the platform's own, which only a
    /// `js` file can implement.
    fn check_host_fields(&mut self, custom: &CustomPlatform, platform: ModuleId, host: TyConId) {
        let label = custom.label.value.clone();
        let fields = self.tables.tycon(host).fields().to_vec();
        for field in fields {
            if field.ty.is_error() {
                continue;
            }
            // A production struct: one of the platform's own or `platform/host`'s.
            let production = match field.ty.kind() {
                TyKind::Con(con, args) => {
                    let tycon = self.tables.tycon(*con);
                    // A built-in type, `Int` say, has no module.
                    let from_host = self
                        .loaded
                        .modules
                        .get(tycon.module.index())
                        .is_some_and(|m| m.path == standard_library::HOST_STRUCTS_MODULE);
                    let a_struct = args.is_empty()
                        && matches!(tycon.def, TyDef::Struct { .. })
                        && (tycon.module == platform || from_host);
                    a_struct.then_some((*con, from_host, tycon.fields().is_empty()))
                }
                _ => None,
            };
            let Some((con, from_host, true)) = production else {
                self.refuse_host_field(&field, production.is_some());
                continue;
            };
            let struct_name = self.tables.tycon(con).name.clone();
            if from_host {
                if lacking(custom.backend).contains(&struct_name.as_str()) {
                    let effects: Vec<String> =
                        standard_library::effects_of_host_struct(&struct_name)
                            .iter()
                            .map(|e| format!("`{e}`"))
                            .collect();
                    if self.already(field.span, "effect-not-on-backend") {
                        continue;
                    }
                    self.templated("effect-not-on-backend", field.span)
                        .bind("field", field.name.clone())
                        .bind("struct", struct_name.clone())
                        .bind("backend", custom.backend.proto())
                        .bind("effects", effects.join(" and "))
                        .bind("platform", label.clone());
                }
            } else if custom.backend == Backend::Native
                && !bodiless_methods(&self.tables, con).is_empty()
                && !self.already(field.span, "custom-effect-outside-js")
            {
                self.templated("custom-effect-outside-js", field.span)
                    .bind("field", field.name.clone())
                    .bind("struct", struct_name.clone())
                    .bind("platform", label.clone());
            }
        }
    }

    /// A host field that isn't a production struct, once per field: a struct
    /// with fields of its own, or anything else.
    fn refuse_host_field(&mut self, field: &FieldInfo, has_fields: bool) {
        if self.already(field.span, "host-field-not-production") {
            return;
        }
        let shown = self.shown(&field.ty);
        let fix = if has_fields {
            format!("drop `{shown}`'s fields: the CLI builds the host, and has nothing to fill them with")
        } else {
            String::from(
                "use a struct from `platform/host`, or declare one with no fields in `platform.buri`; \
                 hand anything else to the entry as a parameter",
            )
        };
        self.templated("host-field-not-production", field.span)
            .bind("field", field.name.clone())
            .bind("type", shown)
            .bind("fix", fix);
    }

    /// Whether an error with this code is already reported at this span, so a
    /// platform two outputs name is told about once.
    fn already(&self, span: Span, code: &str) -> bool {
        self.diags
            .items
            .iter()
            .any(|d| d.span == span && d.code.as_deref() == Some(code))
    }

    /// The parameters after the first `skip`, and the answer, of a signature a
    /// `js` file meets, against the crossing table.
    fn check_crossing_of(&mut self, info: &FnInfo, skip: usize, what: &str) {
        let known = Known {
            request: self.known_types.get("Request").copied(),
            response: self.known_types.get("Response").copied(),
        };
        for p in info.params.iter().skip(skip) {
            if let Err(bad) = crossing::classify(&self.tables, known, &p.ty, false) {
                let shown = self.shown(&bad);
                if !self.already(p.span, "type-not-crossable") {
                    self.templated("type-not-crossable", p.span)
                        .bind("type", shown)
                        .bind("place", format!("{what}'s parameter `{}`", p.name));
                }
            }
        }
        if let Err(bad) = crossing::classify(&self.tables, known, &info.ret, true) {
            let shown = self.shown(&bad);
            if !self.already(info.span, "type-not-crossable") {
                self.templated("type-not-crossable", info.span)
                    .bind("type", shown)
                    .bind("place", format!("{what}'s answer"));
            }
        }
    }

    /// Every method a platform's `js` files implement, against the table.
    ///
    /// Bodies aren't checked yet, so which entry's file implements which is
    /// the build's question (`js_structs`); every one crosses either way.
    fn check_js_methods(&mut self, platform: ModuleId) {
        for (con, methods) in bodiless_structs(&self.tables, platform) {
            let struct_name = self.tables.tycon(con).name.clone();
            for method in methods {
                let info = self.tables.fn_info(method).clone();
                // A generic method is told once, however many entries reach it.
                if let Some(generic) = info.generics.first() {
                    if !self.already(info.span, "type-not-crossable") {
                        self.templated("type-not-crossable", info.span)
                            .bind("type", generic.name.clone())
                            .bind("place", format!("`{struct_name}.{}`", info.name));
                    }
                    continue;
                }
                self.check_crossing_of(&info, 1, &format!("`{struct_name}.{}`", info.name));
            }
        }
    }

    /// Every effect a repository declares has a test implementation beside it,
    /// in its package's `testing` surface.
    ///
    /// Answered where the answer can be read: a package with no `testing`
    /// block has none anywhere, and one with a block is asked once its surface
    /// is loaded, which is whenever the effect package itself is built,
    /// tested or linted.
    pub(super) fn check_effect_test_implementations(&mut self) {
        let Some(ws) = self.ws else { return };
        let mut missing: Vec<(Span, String, String)> = Vec::new();
        for (id, t) in self.tables.traits.entries() {
            if !t.is_effect {
                continue;
            }
            let module = self.module(t.module);
            let (Some(pkg), true) = (module.pkg, is_effect_package_module(Some(ws), module)) else {
                continue;
            };
            let package = ws.package(pkg);
            let has_testing = package
                .build
                .library
                .as_ref()
                .is_some_and(|l| l.testing.is_some());
            let surface = self
                .loaded
                .find(&package.module_path("testing/lib.buri"))
                .is_some();
            if has_testing && !surface {
                continue;
            }
            let implemented = self.tables.impls.keys().any(|(tr, con)| {
                *tr == id
                    && self
                        .loaded
                        .modules
                        .get(self.tables.tycon(*con).module.index())
                        .is_some_and(|m| m.pkg == Some(pkg) && is_test_only_path(&m.path))
            });
            if !implemented {
                missing.push((t.span, t.name.clone(), package.label()));
            }
        }
        for (span, effect, package) in missing {
            self.templated("effect-missing-test-implementation", span)
                .bind("effect", effect)
                .bind("package", package);
        }
    }
}
