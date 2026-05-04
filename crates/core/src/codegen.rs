//! Code generation for the JS shim that bridges WIT types to the quickjs runtime.

use indexmap::IndexSet;
use std::collections::{HashMap, HashSet};
use wit_parser::{Resolve, Type, TypeDefKind, TypeId, TypeOwner, WorldId, WorldItem};

/// Generate a JS shim from WIT metadata that sets up stream/future factories.
pub fn generate_shim(resolve: &Resolve, world_id: WorldId) -> String {
    let mut ctx = EmitContext::new(resolve, world_id);
    ctx.emit();
    ctx.output()
}

struct EmitContext<'a> {
    resolve: &'a Resolve,
    world_id: WorldId,
    lines: Vec<String>,
    streams: IndexSet<Option<Type>>,
    futures: IndexSet<Option<Type>>,
    visited_types: HashSet<TypeId>,
}

impl<'a> EmitContext<'a> {
    fn new(resolve: &'a Resolve, world_id: WorldId) -> Self {
        Self {
            resolve,
            world_id,
            lines: Vec::new(),
            streams: IndexSet::new(),
            futures: IndexSet::new(),
            visited_types: HashSet::new(),
        }
    }

    fn line(&mut self, s: &str) {
        self.lines.push(s.to_string());
    }

    fn output(self) -> String {
        self.lines.join("\n") + "\n"
    }

    fn emit(&mut self) {
        let world = &self.resolve.worlds[self.world_id];

        for item in world.imports.values() {
            self.collect_from_world_item(item);
        }

        for item in world.exports.values() {
            if let WorldItem::Function(f) = item {
                self.collect_from_function(f);
            }
        }

        for item in world.exports.values() {
            if let WorldItem::Interface { id, .. } = item {
                for f in self.resolve.interfaces[*id].functions.values() {
                    self.collect_from_function(f);
                }
            }
        }

        self.line("const wit = globalThis.wit = {};");

        let streams: Vec<_> = self.streams.iter().copied().collect();
        if !streams.is_empty() {
            self.emit_constructor("Stream", "__cqjs.makeStream", &streams);
        }

        let futures: Vec<_> = self.futures.iter().copied().collect();
        if !futures.is_empty() {
            self.emit_constructor("Future", "__cqjs.makeFuture", &futures);
        }
    }

    fn emit_constructor(&mut self, name: &str, native_fn: &str, types: &[Option<Type>]) {
        if types.len() == 1 {
            self.line(&format!(
                "wit.{name} = function(type) {{ return {native_fn}(type ?? 0); }};"
            ));
        } else {
            self.line(&format!("wit.{name} = function(type) {{"));
            self.line(&format!(
                "  if (type === undefined) throw new Error('{name} type required, use wit.{name}.<TYPE>');"
            ));
            self.line(&format!("  return {native_fn}(type);"));
            self.line("};");
        }

        self.line(&format!("wit.{name}.types = {{}};"));
        for (index, const_name) in unique_const_names(self.resolve, types)
            .into_iter()
            .enumerate()
        {
            self.line(&format!(
                "wit.{name}.{const_name} = {index}; wit.{name}.types.{const_name} = {index};"
            ));
        }
    }

    fn collect_from_world_item(&mut self, item: &WorldItem) {
        match item {
            WorldItem::Function(f) => {
                self.collect_from_function(f);
            }
            WorldItem::Interface { id, .. } => {
                for f in self.resolve.interfaces[*id].functions.values() {
                    self.collect_from_function(f);
                }
            }
            WorldItem::Type(id) => {
                self.collect_from_type_id(*id);
            }
        }
    }

    fn collect_from_function(&mut self, func: &wit_parser::Function) {
        for (_, ty) in &func.params {
            self.collect_from_type(ty);
        }
        if let Some(result) = &func.result {
            self.collect_from_type(result);
        }
    }

    fn collect_from_type(&mut self, ty: &Type) {
        if let Type::Id(id) = ty {
            self.collect_from_type_id(*id);
        }
    }

    fn collect_from_type_id(&mut self, id: TypeId) {
        if !self.visited_types.insert(id) {
            return;
        }

        let typedef = &self.resolve.types[id];
        match &typedef.kind {
            TypeDefKind::Stream(elem) => {
                if let Some(elem) = elem {
                    self.collect_from_type(elem);
                }
                self.streams.insert(*elem);
            }
            TypeDefKind::Future(elem) => {
                if let Some(elem) = elem {
                    self.collect_from_type(elem);
                }
                self.futures.insert(*elem);
            }
            TypeDefKind::Record(r) => {
                let tys: Vec<_> = r.fields.iter().map(|f| f.ty).collect();
                for ty in &tys {
                    self.collect_from_type(ty);
                }
            }
            TypeDefKind::Tuple(t) => {
                let tys = t.types.clone();
                for ty in &tys {
                    self.collect_from_type(ty);
                }
            }
            TypeDefKind::Variant(v) => {
                let tys: Vec<_> = v.cases.iter().filter_map(|c| c.ty).collect();
                for ty in &tys {
                    self.collect_from_type(ty);
                }
            }
            TypeDefKind::Option(ty) => {
                let ty = *ty;
                self.collect_from_type(&ty);
            }
            TypeDefKind::Result(r) => {
                let ok = r.ok;
                let err = r.err;
                if let Some(ty) = &ok {
                    self.collect_from_type(ty);
                }
                if let Some(ty) = &err {
                    self.collect_from_type(ty);
                }
            }
            TypeDefKind::List(ty) => {
                let ty = *ty;
                self.collect_from_type(&ty);
            }
            TypeDefKind::Type(ty) => {
                let ty = *ty;
                self.collect_from_type(&ty);
            }
            _ => {}
        }
    }
}

#[derive(Clone, Copy)]
enum ConstNameStyle {
    Local,
    Qualified,
}

fn type_const_name(resolve: &Resolve, ty: Option<&Type>, style: ConstNameStyle) -> String {
    match ty {
        None => "UNIT".to_string(),
        Some(Type::Bool) => "BOOL".to_string(),
        Some(Type::U8) => "U8".to_string(),
        Some(Type::S8) => "S8".to_string(),
        Some(Type::U16) => "U16".to_string(),
        Some(Type::S16) => "S16".to_string(),
        Some(Type::U32) => "U32".to_string(),
        Some(Type::S32) => "S32".to_string(),
        Some(Type::U64) => "U64".to_string(),
        Some(Type::S64) => "S64".to_string(),
        Some(Type::F32) => "F32".to_string(),
        Some(Type::F64) => "F64".to_string(),
        Some(Type::Char) => "CHAR".to_string(),
        Some(Type::String) => "STRING".to_string(),
        Some(Type::ErrorContext) => "ERROR_CONTEXT".to_string(),
        Some(Type::Id(id)) => typedef_const_name(resolve, *id, style),
    }
}

fn typedef_const_name(resolve: &Resolve, id: TypeId, style: ConstNameStyle) -> String {
    let typedef = &resolve.types[id];

    if let Some(name) = &typedef.name {
        return match style {
            ConstNameStyle::Local => const_ident(name),
            ConstNameStyle::Qualified => {
                let prefix = match typedef.owner {
                    TypeOwner::Interface(interface) => resolve.id_of(interface),
                    TypeOwner::World(world) => Some(resolve.worlds[world].name.clone()),
                    TypeOwner::None => None,
                };

                match prefix {
                    Some(prefix) => const_ident(&format!("{prefix}-{name}")),
                    None => const_ident(name),
                }
            }
        };
    }

    // Build type name recursively, e.g. OPTION_U32, RESULT_STRING_VOID, etc.
    match &typedef.kind {
        TypeDefKind::Option(inner) => {
            format!("OPTION_{}", type_const_name(resolve, Some(inner), style))
        }
        TypeDefKind::Tuple(t) => {
            let inner: Vec<String> = t
                .types
                .iter()
                .map(|t| type_const_name(resolve, Some(t), style))
                .collect();
            format!("TUPLE_{}", inner.join("_"))
        }
        TypeDefKind::Result(r) => {
            let ok =
                r.ok.as_ref()
                    .map(|t| type_const_name(resolve, Some(t), style))
                    .unwrap_or("VOID".to_string());
            let err = r
                .err
                .as_ref()
                .map(|t| type_const_name(resolve, Some(t), style))
                .unwrap_or("VOID".to_string());
            format!("RESULT_{ok}_{err}")
        }
        TypeDefKind::List(inner) => {
            format!("LIST_{}", type_const_name(resolve, Some(inner), style))
        }
        TypeDefKind::Future(inner) => {
            let inner = inner
                .as_ref()
                .map(|t| type_const_name(resolve, Some(t), style))
                .unwrap_or("UNIT".to_string());
            format!("FUTURE_{inner}")
        }
        TypeDefKind::Stream(inner) => {
            let inner = inner
                .as_ref()
                .map(|t| type_const_name(resolve, Some(t), style))
                .unwrap_or("UNIT".to_string());
            format!("STREAM_{inner}")
        }
        TypeDefKind::Type(inner) => type_const_name(resolve, Some(inner), style),
        _ => "OTHER".to_string(),
    }
}

fn unique_const_names(resolve: &Resolve, types: &[Option<Type>]) -> Vec<String> {
    let base_names: Vec<_> = types
        .iter()
        .map(|ty| type_const_name(resolve, ty.as_ref(), ConstNameStyle::Local))
        .collect();
    let mut counts = HashMap::<String, usize>::new();
    for name in &base_names {
        *counts.entry(name.clone()).or_default() += 1;
    }

    let mut used = HashSet::new();
    base_names
        .into_iter()
        .zip(types.iter())
        .map(|(base, ty)| {
            let candidate = if counts[base.as_str()] > 1 {
                type_const_name(resolve, ty.as_ref(), ConstNameStyle::Qualified)
            } else {
                base
            };

            unique_name(candidate, &mut used)
        })
        .collect()
}

fn unique_name(candidate: String, used: &mut HashSet<String>) -> String {
    if used.insert(candidate.clone()) {
        return candidate;
    }

    let mut suffix = 2;
    loop {
        let name = format!("{candidate}_{suffix}");
        if used.insert(name.clone()) {
            return name;
        }
        suffix += 1;
    }
}

fn const_ident(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_uppercase());
        } else {
            out.push('_');
        }
    }

    if out.as_bytes().first().is_some_and(|b| b.is_ascii_digit()) {
        out.insert(0, '_');
    }

    out
}
