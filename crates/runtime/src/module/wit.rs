//! WIT-backed ES module resolution and native module definitions.

use std::cell::RefCell;

use heck::{ToLowerCamelCase, ToUpperCamelCase};
use rquickjs::loader::{Loader, Resolver};
use rquickjs::module::Declared;
use rquickjs::module::{Declarations, Exports, ModuleDef};
use rquickjs::{Ctx, Error, Module};
use wit_dylib_ffi::Wit;

use crate::wit_imports::{WitInterface, partition_imports};
use crate::{CtxExt, bindings, with_ctx};

// TODO(tandr): remove when rquickjs 0.12 is released
const _: () = {
    assert!(
        core::mem::size_of::<Module<'static>>() == core::mem::size_of::<Exports<'static>>(),
        "Module and Exports must have the same size"
    );
    assert!(
        core::mem::align_of::<Module<'static>>() == core::mem::align_of::<Exports<'static>>(),
        "Module and Exports must have the same alignment"
    );
};

/// Transient state used while declaring native WIT import modules.
#[derive(Default, rquickjs::JsLifetime)]
pub(crate) struct WitImportDeclarations(RefCell<Vec<Vec<String>>>);

impl WitImportDeclarations {
    fn push(&self, names: Vec<String>) {
        self.0.borrow_mut().push(names);
    }

    fn pop(&self) {
        self.0
            .borrow_mut()
            .pop()
            .expect("WIT module export declaration stack underflow");
    }

    fn current(&self) -> Option<Vec<String>> {
        self.0.borrow().last().cloned()
    }
}

pub(super) struct WitModuleResolver;

impl Resolver for WitModuleResolver {
    fn resolve<'js>(&mut self, ctx: &Ctx<'js>, base: &str, name: &str) -> rquickjs::Result<String> {
        if has_import_module(ctx.wit(), name) {
            Ok(name.to_string())
        } else {
            Err(Error::new_resolving(base, name))
        }
    }
}

pub(super) struct WitModuleLoader;

impl Loader for WitModuleLoader {
    fn load<'js>(&mut self, ctx: &Ctx<'js>, name: &str) -> rquickjs::Result<Module<'js, Declared>> {
        let iface = find_import_interface(ctx.wit(), name)
            .ok_or_else(|| Error::new_loading_message(name, "WIT import not found"))?;

        declare_import_module(ctx, name, &iface)
    }
}

struct WitImportModule;

impl ModuleDef for WitImportModule {
    fn declare<'js>(decl: &Declarations<'js>) -> rquickjs::Result<()> {
        let names = with_ctx(|ctx| ctx.wit_import_declarations().current()).ok_or_else(|| {
            Error::new_loading_message("WIT import", "WIT module exports were not declared")
        })?;

        decl.declare("default")?;
        for name in names {
            decl.declare(name)?;
        }

        Ok(())
    }

    fn evaluate<'js>(ctx: &Ctx<'js>, exports: &Exports<'js>) -> rquickjs::Result<()> {
        let module_name = module_name_from_exports(exports)?;
        let iface = find_import_interface(ctx.wit(), &module_name)
            .ok_or_else(|| Error::new_loading_message(module_name, "WIT import not found"))?;

        let obj = bindings::interface_to_js(ctx, &iface)?;
        freeze(ctx, obj.clone())?;

        exports.export("default", obj.clone())?;
        for name in export_names(&iface) {
            let value: rquickjs::Value = obj.get(name.as_str())?;
            exports.export(name, value)?;
        }

        Ok(())
    }
}

struct DeclaredExportsGuard<'js> {
    ctx: Ctx<'js>,
}

impl Drop for DeclaredExportsGuard<'_> {
    fn drop(&mut self) {
        self.ctx.wit_import_declarations().pop();
    }
}

fn find_import_interface(wit_def: Wit, specifier: &str) -> Option<WitInterface> {
    for (name, iface) in partition_imports(wit_def) {
        let Some(name) = name else {
            continue;
        };

        let name_no_version = name.split('@').next().unwrap_or(name);

        if specifier == name || specifier == name_no_version {
            return Some(iface);
        }
    }

    None
}

fn has_import_module(wit_def: Wit, specifier: &str) -> bool {
    find_import_interface(wit_def, specifier).is_some()
}

fn declare_import_module<'js>(
    ctx: &Ctx<'js>,
    name: &str,
    iface: &WitInterface,
) -> rquickjs::Result<Module<'js, Declared>> {
    ctx.wit_import_declarations().push(export_names(iface));

    let _guard = DeclaredExportsGuard { ctx: ctx.clone() };
    Module::declare_def::<WitImportModule, _>(ctx.clone(), name)
}

fn module_name_from_exports<'js>(exports: &Exports<'js>) -> rquickjs::Result<String> {
    // SAFETY: rquickjs exposes `Exports` as a newtype around `Module`; the
    // size/alignment assertions above guard this assumption until rquickjs
    // exposes module names directly to `ModuleDef`.
    let module: &Module<'js> = unsafe { core::mem::transmute(exports) };
    module.name()
}

fn export_names(iface: &WitInterface) -> Vec<String> {
    let mut names = Vec::new();

    names.extend(
        iface
            .funcs
            .iter()
            .map(|func| func.name().to_lower_camel_case()),
    );
    names.extend(
        iface
            .flags
            .iter()
            .map(|flags| flags.name().to_upper_camel_case()),
    );
    names.extend(
        iface
            .enums
            .iter()
            .map(|enum_ty| enum_ty.name().to_upper_camel_case()),
    );
    names.extend(
        iface
            .variants
            .iter()
            .map(|variant| variant.name().to_upper_camel_case()),
    );

    names
}

fn freeze<'js>(ctx: &Ctx<'js>, obj: rquickjs::Object<'js>) -> rquickjs::Result<()> {
    let object_ctor: rquickjs::Object = ctx.globals().get("Object")?;
    let freeze_fn: rquickjs::Function = object_ctor.get("freeze")?;
    freeze_fn.call::<_, rquickjs::Value>((obj,))?;
    Ok(())
}
