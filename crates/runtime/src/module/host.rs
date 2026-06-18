use rquickjs::loader::{ImportAttributes, Loader, Resolver};
use rquickjs::module::Declared;
use rquickjs::{Ctx, Error, Module};

use crate::init::local::init::module_loader;

pub(super) struct HostModuleResolver;

impl Resolver for HostModuleResolver {
    fn resolve<'js>(
        &mut self,
        _ctx: &Ctx<'js>,
        base: &str,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> rquickjs::Result<String> {
        module_loader::resolve(base, name)
            .map_err(|err| Error::new_resolving_message(base, name, err))
    }
}

pub(super) struct HostModuleLoader;

impl Loader for HostModuleLoader {
    fn load<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> rquickjs::Result<Module<'js, Declared>> {
        let source =
            module_loader::load(name).map_err(|err| Error::new_loading_message(name, err))?;
        Module::declare(ctx.clone(), name, source)
    }
}
