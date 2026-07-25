//! Helpers for grouping imported WIT items by interface.

use heck::{ToLowerCamelCase, ToUpperCamelCase};
use wit_dylib_ffi::{ImportFunction, Resource, Wit};

use crate::{DetHashMap, DetHashSet, DetIndexMap};

/// WIT import functions belonging to one interface, or to the root scope.
#[derive(Default, rquickjs::JsLifetime)]
pub(crate) struct WitInterface {
    pub(crate) funcs: Vec<ImportFunction>,
}

/// Precomputed lookup table for root and interface-scoped WIT imports.
#[derive(Default, rquickjs::JsLifetime)]
pub(crate) struct WitImportRegistry {
    root: WitInterface,
    interfaces: Vec<WitInterface>,
    specifiers: DetHashMap<&'static str, usize>,
}

impl WitImportRegistry {
    pub(crate) fn new(wit: Wit) -> Self {
        let mut partitions: DetIndexMap<Option<&'static str>, WitInterface> =
            DetIndexMap::default();
        for func in wit.iter_import_funcs() {
            partitions
                .entry(func.interface())
                .or_default()
                .funcs
                .push(func);
        }

        let mut registry = Self::default();
        for (name, interface) in partitions {
            let Some(name) = name else {
                registry.root = interface;
                continue;
            };

            let index = registry.interfaces.len();
            registry.specifiers.insert(name, index);

            let unversioned = name.split('@').next().unwrap_or(name);
            registry.specifiers.entry(unversioned).or_insert(index);
            registry.interfaces.push(interface);
        }

        registry
    }

    pub(crate) fn get(&self, specifier: &str) -> Option<&WitInterface> {
        self.specifiers
            .get(specifier)
            .map(|&index| &self.interfaces[index])
    }

    pub(crate) fn root(&self) -> &WitInterface {
        &self.root
    }
}

/// Classification of a WIT function name by its canonical-ABI prefix.
#[derive(Clone, Copy)]
pub(crate) enum FuncKind<'a> {
    /// A freestanding function (no resource association).
    Freestanding,
    /// `[constructor]resource`.
    Constructor { resource: &'a str },
    /// `[method]resource.name`.
    Method { resource: &'a str, method: &'a str },
    /// `[static]resource.name`.
    Static { resource: &'a str, method: &'a str },
}

/// Classify a WIT function name
pub(crate) fn classify(name: &str) -> FuncKind<'_> {
    if let Some(resource) = name.strip_prefix("[constructor]") {
        FuncKind::Constructor { resource }
    } else if let Some(rest) = name.strip_prefix("[method]") {
        let (resource, method) = rest.split_once('.').unwrap_or((rest, ""));
        FuncKind::Method { resource, method }
    } else if let Some(rest) = name.strip_prefix("[static]") {
        let (resource, method) = rest.split_once('.').unwrap_or((rest, ""));
        FuncKind::Static { resource, method }
    } else {
        FuncKind::Freestanding
    }
}

/// Find an imported resource by interface and name.
pub(crate) fn find_resource(wit: Wit, interface: Option<&str>, name: &str) -> Option<Resource> {
    wit.iter_resources()
        .find(|r| r.interface() == interface && r.name() == name)
}

/// JS member names exposed by an interface object: freestanding functions in
/// lowerCamelCase plus one UpperCamelCase class per resource that has a
/// constructor, method, or static. Resource classes are emitted in first-seen
/// order and de-duplicated.
pub(crate) fn interface_member_names(iface: &WitInterface) -> Vec<String> {
    let mut names = Vec::new();
    let mut seen: DetHashSet<&str> = DetHashSet::default();

    for func in &iface.funcs {
        match classify(func.name()) {
            FuncKind::Freestanding => names.push(func.name().to_lower_camel_case()),
            FuncKind::Constructor { resource }
            | FuncKind::Method { resource, .. }
            | FuncKind::Static { resource, .. } => {
                if seen.insert(resource) {
                    names.push(resource.to_upper_camel_case());
                }
            }
        }
    }

    names
}
