//! Helpers for grouping imported WIT items by interface.

use std::collections::{HashMap, HashSet};

use wit_dylib_ffi::{
    Alias, Enum, FixedLengthList, Flags, Future, ImportFunction, List, Record, Stream, Tuple, Type,
    Variant, Wit, WitOption, WitResult,
};

/// WIT imports belonging to one interface, or to the root scope.
#[derive(Default)]
pub(crate) struct WitInterface {
    pub(crate) funcs: Vec<ImportFunction>,
    pub(crate) flags: Vec<Flags>,
    pub(crate) enums: Vec<Enum>,
    pub(crate) variants: Vec<Variant>,
}

/// Partition WIT imports by interface name.
///
/// `wit-dylib-ffi` exposes functions with import/export direction, but exposes
/// type definitions as one shared table. Seed partitions from actual import
/// functions, then attach only type constants referenced by those imports so
/// export-only interfaces don't become importable ES modules.
pub(crate) fn partition_imports(wit: Wit) -> HashMap<Option<&'static str>, WitInterface> {
    let mut ret: HashMap<_, WitInterface> = HashMap::new();
    let mut referenced_types = ReferencedTypes::default();

    for func in wit.iter_import_funcs() {
        collect_import_func_types(&mut referenced_types, func);
        ret.entry(func.interface()).or_default().funcs.push(func);
    }

    for flags in wit.iter_flags() {
        if referenced_types.flags.contains(&flags) && ret.contains_key(&flags.interface()) {
            ret.entry(flags.interface()).or_default().flags.push(flags);
        }
    }
    for enum_ty in wit.iter_enums() {
        if referenced_types.enums.contains(&enum_ty) && ret.contains_key(&enum_ty.interface()) {
            ret.entry(enum_ty.interface())
                .or_default()
                .enums
                .push(enum_ty);
        }
    }
    for variant in wit.iter_variants() {
        if referenced_types.variants.contains(&variant) && ret.contains_key(&variant.interface()) {
            ret.entry(variant.interface())
                .or_default()
                .variants
                .push(variant);
        }
    }

    ret
}

/// Build root-scope bindings installed on `globalThis`.
///
/// Root import functions remain callable as globals for backwards
/// compatibility, while root-scope flags/enums/variants used by root imports or
/// exports are exposed so user export implementations can inspect those values.
pub(crate) fn root_bindings(wit: Wit) -> WitInterface {
    let mut root = WitInterface::default();
    let mut referenced_types = ReferencedTypes::default();

    for func in wit.iter_import_funcs() {
        if func.interface().is_some() {
            continue;
        }
        collect_import_func_types(&mut referenced_types, func);
        root.funcs.push(func);
    }

    for func in wit.iter_export_funcs() {
        if func.interface().is_some() {
            continue;
        }
        for param in func.params() {
            referenced_types.collect(param);
        }
        if let Some(result) = func.result() {
            referenced_types.collect(result);
        }
    }

    for flags in wit.iter_flags() {
        if flags.interface().is_none() && referenced_types.flags.contains(&flags) {
            root.flags.push(flags);
        }
    }
    for enum_ty in wit.iter_enums() {
        if enum_ty.interface().is_none() && referenced_types.enums.contains(&enum_ty) {
            root.enums.push(enum_ty);
        }
    }
    for variant in wit.iter_variants() {
        if variant.interface().is_none() && referenced_types.variants.contains(&variant) {
            root.variants.push(variant);
        }
    }

    root
}

fn collect_import_func_types(referenced_types: &mut ReferencedTypes, func: ImportFunction) {
    for param in func.params() {
        referenced_types.collect(param);
    }
    if let Some(result) = func.result() {
        referenced_types.collect(result);
    }
}

#[derive(Default)]
struct ReferencedTypes {
    flags: HashSet<Flags>,
    enums: HashSet<Enum>,
    variants: HashSet<Variant>,
    records: HashSet<Record>,
    tuples: HashSet<Tuple>,
    options: HashSet<WitOption>,
    results: HashSet<WitResult>,
    lists: HashSet<List>,
    fixed_length_lists: HashSet<FixedLengthList>,
    futures: HashSet<Future>,
    streams: HashSet<Stream>,
    aliases: HashSet<Alias>,
}

impl ReferencedTypes {
    fn collect(&mut self, ty: Type) {
        match ty {
            Type::Record(record) => {
                if self.records.insert(record) {
                    for (_, field_ty) in record.fields() {
                        self.collect(field_ty);
                    }
                }
            }
            Type::Tuple(tuple) => {
                if self.tuples.insert(tuple) {
                    for ty in tuple.types() {
                        self.collect(ty);
                    }
                }
            }
            Type::Variant(variant) => {
                if self.variants.insert(variant) {
                    for (_, payload_ty) in variant.cases() {
                        if let Some(payload_ty) = payload_ty {
                            self.collect(payload_ty);
                        }
                    }
                }
            }
            Type::Flags(flags) => {
                self.flags.insert(flags);
            }
            Type::Enum(enum_ty) => {
                self.enums.insert(enum_ty);
            }
            Type::Option(option) => {
                if self.options.insert(option) {
                    self.collect(option.ty());
                }
            }
            Type::Result(result) => {
                if self.results.insert(result) {
                    if let Some(ok) = result.ok() {
                        self.collect(ok);
                    }
                    if let Some(err) = result.err() {
                        self.collect(err);
                    }
                }
            }
            Type::List(list) => {
                if self.lists.insert(list) {
                    self.collect(list.ty());
                }
            }
            Type::FixedLengthList(list) => {
                if self.fixed_length_lists.insert(list) {
                    self.collect(list.ty());
                }
            }
            Type::Future(future) => {
                if self.futures.insert(future)
                    && let Some(ty) = future.ty()
                {
                    self.collect(ty);
                }
            }
            Type::Stream(stream) => {
                if self.streams.insert(stream)
                    && let Some(ty) = stream.ty()
                {
                    self.collect(ty);
                }
            }
            Type::Alias(alias) => {
                if self.aliases.insert(alias) {
                    self.collect(alias.ty());
                }
            }
            Type::U8
            | Type::U16
            | Type::U32
            | Type::U64
            | Type::S8
            | Type::S16
            | Type::S32
            | Type::S64
            | Type::Bool
            | Type::Char
            | Type::F32
            | Type::F64
            | Type::String
            | Type::ErrorContext
            | Type::Own(_)
            | Type::Borrow(_) => {}
        }
    }
}
