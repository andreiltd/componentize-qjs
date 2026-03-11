//! Code generation for the JS shim that bridges WIT types to the quickjs runtime.

use std::collections::HashSet;
use wit_parser::{Resolve, Type, TypeDefKind, TypeId, WorldId, WorldItem};

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
    streams: HashSet<Option<Type>>,
    futures: HashSet<Option<Type>>,
}

impl<'a> EmitContext<'a> {
    fn new(resolve: &'a Resolve, world_id: WorldId) -> Self {
        Self {
            resolve,
            world_id,
            lines: Vec::new(),
            streams: HashSet::new(),
            futures: HashSet::new(),
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

        for item in world.imports.values().chain(world.exports.values()) {
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

        self.line("globalThis.wit = {};");

        let streams: Vec<_> = self.streams.iter().copied().collect();
        if !streams.is_empty() {
            self.emit_constructor("Stream", "__componentize_make_stream", &streams);
        }

        let futures: Vec<_> = self.futures.iter().copied().collect();
        if !futures.is_empty() {
            self.emit_constructor("Future", "__componentize_make_future", &futures);
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
        for (index, elem_ty) in types.iter().enumerate() {
            let const_name = type_const_name(self.resolve, elem_ty.as_ref());
            self.line(&format!(
                "wit.{name}.{const_name} = {index}; wit.{name}.types.{const_name} = {index};"
            ));
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
        let typedef = &self.resolve.types[id];
        match &typedef.kind {
            TypeDefKind::Stream(elem) => {
                self.streams.insert(*elem);
                if let Some(elem) = elem {
                    self.collect_from_type(elem);
                }
            }
            TypeDefKind::Future(elem) => {
                self.futures.insert(*elem);
                if let Some(elem) = elem {
                    self.collect_from_type(elem);
                }
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

fn type_const_name(resolve: &Resolve, ty: Option<&Type>) -> String {
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
        Some(Type::Id(id)) => typedef_const_name(resolve, *id),
    }
}

fn typedef_const_name(resolve: &Resolve, id: TypeId) -> String {
    let typedef = &resolve.types[id];

    if let Some(name) = &typedef.name {
        return name.to_uppercase().replace('-', "_");
    }

    // Build type name recursively, e.g. OPTION_U32, RESULT_STRING_VOID, etc.
    match &typedef.kind {
        TypeDefKind::Option(inner) => {
            format!("OPTION_{}", type_const_name(resolve, Some(inner)))
        }
        TypeDefKind::Tuple(t) => {
            let inner: Vec<String> = t
                .types
                .iter()
                .map(|t| type_const_name(resolve, Some(t)))
                .collect();
            format!("TUPLE_{}", inner.join("_"))
        }
        TypeDefKind::Result(r) => {
            let ok =
                r.ok.as_ref()
                    .map(|t| type_const_name(resolve, Some(t)))
                    .unwrap_or("VOID".to_string());
            let err = r
                .err
                .as_ref()
                .map(|t| type_const_name(resolve, Some(t)))
                .unwrap_or("VOID".to_string());
            format!("RESULT_{ok}_{err}")
        }
        TypeDefKind::List(inner) => {
            format!("LIST_{}", type_const_name(resolve, Some(inner)))
        }
        TypeDefKind::Type(inner) => type_const_name(resolve, Some(inner)),
        _ => "OTHER".to_string(),
    }
}
