//! Helpers for grouping imported WIT items by interface.

use heck::{ToLowerCamelCase, ToUpperCamelCase};
use wit_dylib_ffi::{ImportFunction, Resource, Wit};

use crate::{DetHashMap, DetHashSet};

/// WIT import functions belonging to one interface, or to the root scope.
#[derive(Default)]
pub(crate) struct WitInterface {
    pub(crate) funcs: Vec<ImportFunction>,
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

/// Partition WIT import functions by interface name.
///
/// Interfaces are seeded from import functions (including resource
/// methods/constructors/statics). Type-only constructs (records, enums,
/// variants, flags) have no runtime representation and are not exposed, so
/// type-only interfaces do not become importable ES modules.
pub(crate) fn partition_imports(wit: Wit) -> DetHashMap<Option<&'static str>, WitInterface> {
    let mut ret: DetHashMap<_, WitInterface> = DetHashMap::default();

    for func in wit.iter_import_funcs() {
        ret.entry(func.interface()).or_default().funcs.push(func);
    }

    ret
}

/// Build root-scope bindings installed on `globalThis`.
///
/// Root import functions remain callable as globals.
pub(crate) fn root_bindings(wit: Wit) -> WitInterface {
    let mut root = WitInterface::default();

    for func in wit.iter_import_funcs() {
        if func.interface().is_none() {
            root.funcs.push(func);
        }
    }

    root
}
