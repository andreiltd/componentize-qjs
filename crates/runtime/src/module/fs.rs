//! Filesystem-backed ES module resolution for build-time imports.

use std::path::Path;

use oxc_resolver::{ResolveOptions, Resolver as OxcResolver};
use rquickjs::loader::{ImportAttributes, Loader, Resolver};
use rquickjs::module::Declared;
use rquickjs::{Ctx, Error, Module};

pub(super) struct FsModuleResolver {
    resolver: OxcResolver,
}

impl FsModuleResolver {
    pub(super) fn new() -> Self {
        Self {
            resolver: OxcResolver::new(ResolveOptions {
                condition_names: vec!["import".into(), "default".into()],
                extensions: vec![".mjs".into(), ".js".into()],
                main_fields: vec!["module".into(), "main".into()],
                node_path: false,
                symlinks: false,
                ..ResolveOptions::default()
            }),
        }
    }
}

impl Resolver for FsModuleResolver {
    fn resolve<'js>(
        &mut self,
        _ctx: &Ctx<'js>,
        base: &str,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> rquickjs::Result<String> {
        let base_path = Path::new(base);
        if !base_path.is_absolute() {
            return Err(Error::new_resolving_message(
                base,
                name,
                "filesystem imports require an entry file path",
            ));
        }

        let resolution = self.resolver.resolve_file(base_path, name).map_err(|err| {
            Error::new_resolving_message(base, name, format!("filesystem module not found: {err}"))
        })?;
        let resolved = resolution.path();

        resolved.to_str().map(str::to_owned).ok_or_else(|| {
            Error::new_resolving_message(
                base,
                name,
                format!("resolved path is not valid UTF-8: {}", resolved.display()),
            )
        })
    }
}

pub(super) struct FsModuleLoader;

impl Loader for FsModuleLoader {
    fn load<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> rquickjs::Result<Module<'js, Declared>> {
        let path = Path::new(name);
        if !path.is_absolute() {
            return Err(Error::new_loading(name));
        }
        if !is_javascript_path(path) {
            return Err(Error::new_loading_message(
                name,
                "unsupported JavaScript module extension",
            ));
        }

        let source = std::fs::read_to_string(path).map_err(|err| {
            Error::new_loading_message(name, format!("failed to read JavaScript module: {err}"))
        })?;

        Module::declare(ctx.clone(), name, source)
    }
}

fn is_javascript_path(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("mjs" | "js")
    )
}
