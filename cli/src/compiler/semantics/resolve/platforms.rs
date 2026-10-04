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
    ws: Option<&crate::build::workspace::Workspace>,
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

impl<'a> Checker<'a> {
    /// Whether a module is a repository platform's `platform.buri`.
    pub(super) fn is_platform_surface(&self, module: ModuleId) -> bool {
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
    pub(super) fn custom_entries(&self, module: ModuleId, name: &str) -> Option<Vec<CustomPlatform>> {
        // A build is of one output, and only its own entry is its business.
        if let (Some(_), Some(built)) = (self.loaded.platform, self.loaded.entry.as_deref()) {
            return match (&self.loaded.custom, built == name) {
                (Some(c), true) => Some(vec![c.clone()]),
                _ => None,
            };
        }
        let (ws, pkg) = (self.ws?, self.module(module).pkg?);
        let target = TargetId { package: pkg, kind: RuleKind::Binary };
        let mut out: Vec<CustomPlatform> = Vec::new();
        for c in ws.declared_entries(target).into_iter().filter(|e| e.name == name).filter_map(|e| e.custom) {
            if !out.iter().any(|o| o.label.value == c.label.value && o.point == c.point) {
                out.push(c);
            }
        }
        (!out.is_empty()).then_some(out)
    }

    /// The `platform.buri` a repository platform's label names, when this
    /// compilation loaded it.
    fn platform_module(&self, custom: &CustomPlatform) -> Option<ModuleId> {
        self.loaded.find(&format!("//{}/platform.buri", custom.package_path()))
    }

    /// The host type a signature takes first, when that is a struct its
    /// platform's `platform.buri` declares.
    fn declared_host(&self, platform: ModuleId, params: &[ParamInfo]) -> Option<TyConId> {
        let TyKind::Con(con, args) = (params.first()?.ty).kind() else { return None };
        let tycon = self.tables.tycon(*con);
        (args.is_empty() && tycon.module == platform && matches!(tycon.def, TyDef::Struct { .. }))
            .then_some(*con)
    }

    fn shown(&self, ty: &Ty) -> String {
        show(&self.tables, None, &[], ty)
    }

    /// `fn fetch(host: CloudflareHost, request: Request): Response`.
    fn signature_text(&self, name: &str, info: &FnInfo) -> String {
        let params: Vec<String> =
            info.params.iter().map(|p| format!("{}: {}", p.name, self.shown(&p.ty))).collect();
        format!("fn {name}({}): {}", params.join(", "), self.shown(&info.ret))
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
        let Some(platform) = self.platform_module(custom) else { return };
        let Some(Sym::Fn(decl)) = self.scope(platform).own.get(&custom.point).cloned() else { return };
        let want = self.tables.fn_info(decl).clone();
        let info = self.tables.fn_info(fid).clone();
        let label = custom.label.value.clone();
        if !info.generics.is_empty() {
            self.templated("entry-signature-mismatch", d.span)
                .bind("entry", name.to_string())
                .bind("requirement", "declare no generic parameters")
                .fix(format!(
                    "drop them: `{name}` is called by the platform, so there is nothing to infer \
                     them from"
                ));
        }
        let host = self.declared_host(platform, &want.params);
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
        let same = info.params.len() == want.params.len()
            && info.params.iter().zip(&want.params).skip(skip).all(|(a, b)| a.ty == b.ty || a.ty.is_error())
            && (info.ret == want.ret || info.ret.is_error());
        if !same {
            self.templated("entry-signature-mismatch", d.span)
                .bind("entry", name.to_string())
                .bind("requirement", format!("have the signature `{label}` declares, `{wanted}`"))
                .fix(format!("write it `{wanted}`"));
        } else if custom.js.is_none() && !self.starts_itself(&want, host.is_some()) {
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
                self.check_js_methods(platform, host);
            }
        }
    }

    /// Whether a declaration has the program signature: the host alone, and a
    /// `Result<(), Str>` answer.
    fn starts_itself(&mut self, want: &FnInfo, has_host: bool) -> bool {
        let str_ty = self.tables.prim(Prim::Str);
        let answers = match want.ret.kind() {
            TyKind::Con(id, args) => {
                self.result_con.as_ref() == Some(id)
                    && matches!(args, [ok, err] if *ok == Ty::UNIT && *err == str_ty)
            }
            _ => false,
        };
        has_host && want.params.len() == 1 && (answers || want.ret.is_error())
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
            let production = match field.ty.kind() {
                TyKind::Con(con, args) => {
                    let tycon = self.tables.tycon(*con);
                    let home = self.loaded.modules.get(tycon.module.index()).map(|m| m.path.as_str());
                    let a_struct = args.is_empty()
                        && matches!(tycon.def, TyDef::Struct { .. })
                        && (tycon.module == platform || home == Some(standard_library::HOST_STRUCTS_MODULE));
                    a_struct.then_some(tycon.fields().is_empty())
                }
                _ => None,
            };
            if production != Some(true) {
                if !self.already(field.span, "host-field-not-production") {
                    let shown = self.shown(&field.ty);
                    let fix = match production {
                        Some(_) => format!(
                            "drop `{shown}`'s fields: the CLI builds the host, and has nothing to fill them with"
                        ),
                        None => String::from(
                            "use a struct from `platform/host`, or declare one with no fields in `platform.buri`; \
                             hand anything else to the entry as a parameter",
                        ),
                    };
                    self.templated("host-field-not-production", field.span)
                        .bind("field", field.name.clone())
                        .bind("type", shown)
                        .bind("fix", fix);
                }
                continue;
            }
            let TyKind::Con(con, _) = field.ty.kind() else { continue };
            let tycon = self.tables.tycon(*con);
            let Some(module) = self.loaded.modules.get(tycon.module.index()) else { continue };
            let struct_name = tycon.name.clone();
            if module.path == standard_library::HOST_STRUCTS_MODULE {
                if lacking(custom.backend).contains(&struct_name.as_str()) {
                    let effects: Vec<String> = standard_library::effects_of_host_struct(&struct_name)
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
            } else if tycon.module == platform
                && custom.backend == Backend::Native
                && self.has_bodiless_methods(*con)
                && !self.already(field.span, "custom-effect-outside-js")
            {
                self.templated("custom-effect-outside-js", field.span)
                    .bind("field", field.name.clone())
                    .bind("struct", struct_name.clone())
                    .bind("platform", label.clone());
            }
        }
    }

    /// Whether an error with this code is already reported at this span, so a
    /// platform two outputs name is told about once.
    fn already(&self, span: Span, code: &str) -> bool {
        self.diags.items.iter().any(|d| d.span == span && d.code.as_deref() == Some(code))
    }

    /// The methods a platform's own production struct declares without a
    /// body, which its `js` file implements.
    pub(super) fn bodiless_methods(&self, con: TyConId) -> Vec<FnId> {
        self.tables
            .fns
            .iter()
            .enumerate()
            .filter(|(_, f)| f.self_ty == Some(con) && f.intrinsic)
            .map(|(i, _)| FnId(i as u32))
            .collect()
    }

    fn has_bodiless_methods(&self, con: TyConId) -> bool {
        !self.bodiless_methods(con).is_empty()
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

    /// Every method a `js` file implements for this host, against the table.
    fn check_js_methods(&mut self, platform: ModuleId, host: TyConId) {
        let fields = self.tables.tycon(host).fields().to_vec();
        for field in fields {
            let TyKind::Con(con, _) = field.ty.kind() else { continue };
            if self.tables.tycon(*con).module != platform {
                continue;
            }
            let struct_name = self.tables.tycon(*con).name.clone();
            for method in self.bodiless_methods(*con) {
                let info = self.tables.fn_info(method).clone();
                let generic = info.generics.first().map(|g| g.name.clone());
                if let Some(param) = generic.filter(|_| !self.already(info.span, "type-not-crossable")) {
                    self.templated("type-not-crossable", info.span)
                        .bind("type", param)
                        .bind("place", format!("`{struct_name}.{}`", info.name));
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
        for (i, t) in self.tables.traits.iter().enumerate() {
            if !t.is_effect {
                continue;
            }
            let Some(module) = self.loaded.modules.get(t.module.index()) else { continue };
            let (Some(pkg), true) = (module.pkg, is_effect_package_module(Some(ws), module)) else { continue };
            let package = ws.package(pkg);
            let has_testing = package.build.library.as_ref().is_some_and(|l| l.testing.is_some());
            let surface = self.loaded.find(&package.module_path("testing/lib.buri")).is_some();
            if has_testing && !surface {
                continue;
            }
            let id = TraitId(i as u32);
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
