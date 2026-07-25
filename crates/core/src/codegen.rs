//! Code generation for the JS shim that bridges WIT types to the quickjs runtime.

use std::collections::{HashMap, HashSet};
use wit_dylib::metadata;
use wit_parser::{Resolve, Type, TypeDefKind, TypeId, TypeOwner};

/// Generate a JS shim from WIT metadata that sets up stream/future factories.
pub fn generate_shim(resolve: &Resolve, metadata: &wit_dylib::Metadata) -> String {
    let mut ctx = EmitContext::new(resolve, metadata);
    ctx.emit();
    ctx.output()
}

struct EmitContext<'a> {
    resolve: &'a Resolve,
    metadata: &'a wit_dylib::Metadata,
    lines: Vec<String>,
}

impl<'a> EmitContext<'a> {
    fn new(resolve: &'a Resolve, metadata: &'a wit_dylib::Metadata) -> Self {
        Self {
            resolve,
            metadata,
            lines: Vec::new(),
        }
    }

    fn line(&mut self, s: &str) {
        self.lines.push(s.to_string());
    }

    fn multiline(&mut self, s: &str) {
        self.lines.extend(s.lines().map(str::to_string));
    }

    fn output(self) -> String {
        self.lines.join("\n") + "\n"
    }

    fn emit(&mut self) {
        self.line("const wit = globalThis.wit = {};");

        let streams = self
            .metadata
            .streams
            .iter()
            .map(|stream| async_payload(self.resolve, stream.id, true))
            .collect::<Vec<_>>();

        if !streams.is_empty() {
            let (aliases, _) = async_aliases(self.resolve, self.metadata);
            self.emit_constructor("Stream", "__cqjs.makeStream", &streams, &aliases);
        }

        let futures = self
            .metadata
            .futures
            .iter()
            .map(|future| async_payload(self.resolve, future.id, false))
            .collect::<Vec<_>>();

        if !futures.is_empty() {
            let (_, aliases) = async_aliases(self.resolve, self.metadata);
            self.emit_constructor("Future", "__cqjs.makeFuture", &futures, &aliases);
        }
    }

    fn emit_constructor(
        &mut self,
        name: &str,
        native_fn: &str,
        types: &[Option<Type>],
        aliases: &[Vec<TypeId>],
    ) {
        if types.len() == 1 {
            self.line(&format!(
                "wit.{name} = function(type) {{ return {native_fn}(type ?? 0); }};"
            ));
        } else {
            self.multiline(&format!(
                r#"wit.{name} = function(type) {{
                  if (type === undefined) throw new Error('{name} type required, use wit.{name}.<TYPE>');
                  return {native_fn}(type);
                }};"#
            ));
        }

        self.line(&format!("wit.{name}.types = {{}};"));

        let primary_names = unique_const_names(self.resolve, types);
        let mut emitted = HashMap::<String, usize>::new();
        let mut used = HashSet::new();

        for (index, const_name) in primary_names.into_iter().enumerate() {
            self.emit_type_constant(name, &const_name, index);
            used.insert(const_name.clone());
            emitted.insert(const_name, index);
        }

        for (index, type_aliases) in aliases.iter().enumerate() {
            for alias in type_aliases {
                let local = typedef_const_name(self.resolve, *alias, ConstNameStyle::Local);
                let candidate = match emitted.get(&local) {
                    Some(existing) if *existing == index => continue,
                    Some(_) => typedef_const_name(self.resolve, *alias, ConstNameStyle::Qualified),
                    None => local,
                };

                if emitted
                    .get(&candidate)
                    .is_some_and(|existing| *existing == index)
                {
                    continue;
                }

                let const_name = unique_name(candidate, &mut used);
                self.emit_type_constant(name, &const_name, index);
                emitted.insert(const_name, index);
            }
        }

        if name == "Stream" {
            self.multiline(
                r#"wit.Stream.from = function(iterable, type) {
                      const { readable, writable } = wit.Stream(type);
                      const completion = (async () => {
                        try {
                          for await (const item of iterable) {
                            if (!await writable.writeIterableItem(item)) break;
                          }
                        } finally {
                          writable.drop();
                        }
                      })();
                      return { readable, completion };
                    };"#,
            );
        }
    }

    fn emit_type_constant(&mut self, constructor: &str, const_name: &str, index: usize) {
        self.line(&format!(
            "wit.{constructor}.{const_name} = {index}; wit.{constructor}.types.{const_name} = {index};"
        ));
    }
}

fn async_payload(resolve: &Resolve, id: TypeId, stream: bool) -> Option<Type> {
    match &resolve.types[id].kind {
        TypeDefKind::Stream(ty) if stream => *ty,
        TypeDefKind::Future(ty) if !stream => *ty,
        _ => unreachable!("metadata async type does not match WIT type"),
    }
}

fn async_aliases(
    resolve: &Resolve,
    metadata: &wit_dylib::Metadata,
) -> (Vec<Vec<TypeId>>, Vec<Vec<TypeId>>) {
    let mut streams = vec![Vec::new(); metadata.streams.len()];
    let mut futures = vec![Vec::new(); metadata.futures.len()];

    for (index, stream) in metadata.streams.iter().enumerate() {
        if resolve.types[stream.id].name.is_some() {
            streams[index].push(stream.id);
        }
    }
    for (index, future) in metadata.futures.iter().enumerate() {
        if resolve.types[future.id].name.is_some() {
            futures[index].push(future.id);
        }
    }

    for alias in &metadata.aliases {
        match resolve_metadata_async_type(metadata, alias.ty) {
            Some(MetadataAsyncType::Stream(index)) => streams[index].push(alias.id),
            Some(MetadataAsyncType::Future(index)) => futures[index].push(alias.id),
            None => {}
        }
    }

    (streams, futures)
}

#[derive(Clone, Copy)]
enum MetadataAsyncType {
    Stream(usize),
    Future(usize),
}

fn resolve_metadata_async_type(
    metadata: &wit_dylib::Metadata,
    mut ty: metadata::Type,
) -> Option<MetadataAsyncType> {
    let mut visited = HashSet::new();
    loop {
        match ty {
            metadata::Type::Stream(index) => return Some(MetadataAsyncType::Stream(index)),
            metadata::Type::Future(index) => return Some(MetadataAsyncType::Future(index)),
            metadata::Type::Alias(index) if visited.insert(index) => {
                ty = metadata.aliases[index].ty;
            }
            _ => return None,
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
