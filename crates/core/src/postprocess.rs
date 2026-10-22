//! Size-oriented rewrites around Wizer snapshotting of a linked component.
//!
//! The two phases have deliberately different responsibilities:
//!
//! - [`prepare_snapshot`] runs before initialization. It gives the outer `init`
//!   export a globally unused name so Wizer can remove it without also removing
//!   core exports that component aliases still reference.
//! - [`cleanup`] runs after initialization, snapshotting, and import stubbing.
//!   It strips debug/build metadata and lets Walrus collect unreachable core
//!   code, including callees used only by explicitly marked setup functions.

use std::collections::BTreeSet;

use anyhow::{Context, Result, bail, ensure};
use wasm_encoder::reencode::{ReencodeComponent, RoundtripReencoder};
use wasm_encoder::{Component, Module, RawSection};
use wasmparser::{ComponentExternalKind, Parser, Payload};

/// Runtime-authored opt-in to post-snapshot body pruning.
///
/// See [`prune_init`] for metadata validation.
const INIT_ONLY_SECTION: &str = "componentize-qjs:init-only";

/// Rename the outer initializer so Wizer can remove it without harming core exports.
pub(crate) fn prepare_snapshot(input: &[u8]) -> Result<(Vec<u8>, String)> {
    if !Parser::is_component(input) {
        bail!("expected a wasm component");
    }

    let mut names = BTreeSet::new();

    for payload in Parser::new(0).parse_all(input) {
        match payload? {
            Payload::ExportSection(exports) => {
                for export in exports {
                    names.insert(export?.name);
                }
            }
            Payload::ComponentExportSection(exports) => {
                for export in exports {
                    names.insert(export?.name.name);
                }
            }
            _ => {}
        }
    }

    let mut name = String::from("componentize-qjs-init");
    while names.contains(name.as_str()) {
        name.push_str("-x");
    }

    let mut output = Component::new();
    let mut found = false;

    for (payload, id, data) in sections(input)? {
        let Payload::ComponentExportSection(exports) = payload else {
            output.section(&RawSection { id, data });
            continue;
        };

        let mut section = wasm_encoder::ComponentExportSection::new();
        for export in exports {
            let mut export = export?;
            let is_init = export.name.name == "init";

            if is_init && found {
                bail!("duplicate component init export");
            }

            if is_init && export.kind != ComponentExternalKind::Func {
                bail!("component init export must be a function");
            }

            if is_init {
                found = true;
                export.name.name = &name;
            }
            RoundtripReencoder.parse_component_export(&mut section, export)?;
        }
        output.section(&section);
    }

    ensure!(found, "missing component init export");
    Ok((output.finish(), name))
}

/// Recursively shrink an already snapshotted component.
pub(crate) fn cleanup(input: &[u8]) -> Result<Vec<u8>> {
    if !Parser::is_component(input) {
        bail!("expected a wasm component");
    }

    let mut output = Component::new();
    for (payload, id, data) in sections(input)? {
        let data: &[u8] = match payload {
            Payload::ModuleSection { .. } => &strip(data)?,
            Payload::ComponentSection { .. } => &cleanup(data)?,
            Payload::CustomSection(section) if should_strip(section.name()) => continue,
            _ => data,
        };
        output.section(&RawSection { id, data });
    }

    Ok(output.finish())
}

/// Strip disposable metadata and attempt garbage collection of one core module.
fn strip(input: &[u8]) -> Result<Vec<u8>> {
    let mut stripped = Module::new();
    for (payload, id, data) in sections(input)? {
        if let Payload::CustomSection(section) = payload
            && should_strip(section.name())
        {
            continue;
        }

        stripped.section(&RawSection { id, data });
    }

    let stripped = stripped.finish();
    let mut module = walrus::ModuleConfig::new()
        .generate_name_section(false)
        .generate_producers_section(false)
        .parse(&stripped)
        .context("failed to parse core module with Walrus")?;

    prune_init(&mut module)?;

    // GC traces table entries without changing their slots or active data offsets:
    // the snapshot contains C function pointers and pointers into linear memory.
    walrus::passes::gc::run(&mut module);

    let output = module.emit_wasm();
    let ret = if output.len() < stripped.len() {
        output
    } else {
        stripped
    };

    Ok(ret)
}

/// Consume explicit runtime metadata and replace one-time function bodies with traps.
fn prune_init(module: &mut walrus::Module) -> Result<()> {
    let Some(section) = module.customs.remove_raw(INIT_ONLY_SECTION) else {
        return Ok(());
    };

    if module.customs.remove_raw(INIT_ONLY_SECTION).is_some() {
        bail!("duplicate initialization metadata");
    }

    let Some((&version, data)) = section.data.split_first() else {
        bail!("empty initialization-only metadata");
    };

    if version != 1 {
        bail!("unsupported initialization-only metadata version {version}");
    }

    let Some(data) = data.strip_suffix(&[0]) else {
        bail!("initialization-only export list is not NUL-terminated");
    };

    let data = std::str::from_utf8(data).context("invalid initialization-only export name")?;
    let mut names = BTreeSet::new();
    let mut funcs = BTreeSet::new();

    for name in data.split('\0') {
        if name.is_empty() || !names.insert(name) {
            bail!("empty or duplicate initialization-only export name");
        }

        let id = module
            .exports
            .get_func(name)
            .with_context(|| format!("init export {name} must be a function"))?;

        funcs.insert(id);
    }

    for export in module.exports.iter() {
        if let walrus::ExportItem::Function(id) = export.item
            && funcs.contains(&id)
            && !names.contains(export.name.as_str())
        {
            let name = export.name.as_str();
            bail!("initialization-only function also exported as {name}");
        }
    }

    for id in funcs {
        let walrus::FunctionKind::Local(function) = &mut module.funcs.get_mut(id).kind else {
            bail!("initialization-only export must be a defined function");
        };

        let entry = function.entry_block();
        let trap = vec![(walrus::ir::Unreachable {}.into(), Default::default())];
        function.block_mut(entry).instrs = trap;
    }

    Ok(())
}

/// Identify debug, provenance, and stale linker metadata that can be discarded.
fn should_strip(name: &str) -> bool {
    match name {
        "name"
        | "component-name"
        | "producers"
        | "external_debug_info"
        | "sourceMappingURL"
        | "linking"
        | "metadata.code.branch_hint" => true,
        _ => name.starts_with(".debug_") || name.starts_with("reloc."),
    }
}

/// Return top-level sections as `(parsed payload, section ID, raw contents)`.
fn sections(input: &[u8]) -> Result<Vec<(Payload<'_>, u8, &[u8])>> {
    let mut sections = Vec::new();
    let mut depth = 0;

    for payload in Parser::new(0).parse_all(input) {
        let payload = payload?;
        let top_level = depth == 0;

        match payload {
            Payload::ModuleSection { .. } | Payload::ComponentSection { .. } => depth += 1,
            Payload::End(_) if !top_level => depth -= 1,
            _ => {}
        }

        if top_level && let Some((id, range)) = payload.as_section() {
            let data = &input[range.start as usize..range.end as usize];
            sections.push((payload, id, data));
        }
    }

    Ok(sections)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_encoder::ComponentSectionId;
    use wasmparser::{Validator, WasmFeatures};
    use wasmtime::Store;
    use wasmtime::component::{Component as WasmtimeComponent, Linker};
    use wasmtime_wizer::{WasmtimeWizerComponent, Wizer};

    fn checked_cleanup(input: &[u8]) -> Vec<u8> {
        Validator::new_with_features(WasmFeatures::all())
            .validate_all(input)
            .unwrap();

        let output = cleanup(input).unwrap();
        Validator::new_with_features(WasmFeatures::all())
            .validate_all(&output)
            .unwrap();

        assert!(output.len() <= input.len());
        assert_eq!(output, cleanup(&output).unwrap());
        output
    }

    fn call_component(component: &[u8], name: &str) -> u32 {
        let engine = wasmtime::Engine::default();
        let component = WasmtimeComponent::new(&engine, component).unwrap();

        let mut store = Store::new(&engine, ());
        let instance = Linker::new(&engine)
            .instantiate(&mut store, &component)
            .unwrap();

        let run = instance
            .get_typed_func::<(), (u32,)>(&mut store, name)
            .unwrap();

        run.call(&mut store, ()).unwrap().0
    }

    #[test]
    fn renames_only_the_outer_initializer_and_preserves_type_ascriptions() {
        let prefix = wat::parse_str(
            r#"(component
                (@custom "component-name" "\ff")
                (component $nested
                    (core module $m
                        (@custom "name" "\ff")
                        (memory (export "componentize-qjs-init") 1)
                        (func (export "init") (result i32) i32.const 42))
                    (core instance $i (instantiate $m))
                    (core func $init (alias core export $i "init"))
                    (func (export "init") (result u32) (canon lift (core func $init))))
                (export "nested" (component $nested))
                (type $init-type (func))
                (core module $m (func (export "init")))
                (core instance $i (instantiate $m))
                (core func $init (alias core export $i "init"))
                (func (type $init-type) (canon lift (core func $init))))"#,
        )
        .unwrap();

        let with_export = |name| {
            use wasm_encoder::{ComponentExportSection, ComponentTypeRef};

            let mut exports = ComponentExportSection::new();
            let kind = wasm_encoder::ComponentExportKind::Func;
            let ty = Some(ComponentTypeRef::Func(0));

            exports.export(name, kind, 0, ty);

            let mut suffix = Component::new();
            suffix.section(&exports);

            let mut component = prefix.clone();
            component.extend_from_slice(&suffix.finish()[Component::HEADER.len()..]);
            component
        };

        let input = with_export("init");
        let (output, name) = prepare_snapshot(&input).unwrap();

        assert_eq!(name, "componentize-qjs-init-x");
        assert_eq!(output, with_export(&name));

        Validator::new_with_features(WasmFeatures::all())
            .validate_all(&output)
            .unwrap();
    }

    #[test]
    fn rejects_invalid_outer_initializers() {
        for (input, message) in [
            (
                r#"(component (core module $m) (export "init" (core module $m)))"#,
                "component init export must be a function",
            ),
            (
                r#"(component
                    (core module $m (func (export "init")))
                    (core instance $i (instantiate $m))
                    (core func $init (alias core export $i "init"))
                    (func $init (canon lift (core func $init)))
                    (export "init" (func $init))
                    (export "init" (func $init)))"#,
                "duplicate component init export",
            ),
            (
                r#"(component
                    (component
                        (core module $m (func (export "init")))
                        (core instance $i (instantiate $m))
                        (core func $init (alias core export $i "init"))
                        (func (export "init") (canon lift (core func $init)))))"#,
                "missing component init export",
            ),
        ] {
            let input = wat::parse_str(input).unwrap();
            let error = prepare_snapshot(&input).unwrap_err();
            assert!(format!("{error:#}").contains(message), "{error:#}");
        }
    }

    #[tokio::test]
    async fn wizer_removes_only_the_renamed_component_initializer() {
        let input = wat::parse_str(
            r#"(component
                (core module $m
                    (@custom "componentize-qjs:init-only" "\01init\00")
                    (global $state (mut i32) (i32.const 0))
                    (func (export "init") i32.const 41 global.set $state)
                    (func (export "componentize-qjs-init") (result i32)
                        global.get $state i32.const 1 i32.add))
                (core instance $i (instantiate $m))
                (core func $init (alias core export $i "init"))
                (func $init (canon lift (core func $init)))
                (export "init" (func $init))
                (core func $run (alias core export $i "componentize-qjs-init"))
                (func $run (result u32) (canon lift (core func $run)))
                (export "run" (func $run))
                (export "componentize-qjs-init-x" (func $run)))"#,
        )
        .unwrap();

        let (prepared, name) = prepare_snapshot(&input).unwrap();
        assert_eq!(name, "componentize-qjs-init-x-x");

        Validator::new_with_features(WasmFeatures::all())
            .validate_all(&prepared)
            .unwrap();

        let mut wizer = Wizer::new();
        wizer.init_func(&name);

        let (cx, instrumented) = wizer.instrument_component(&prepared).unwrap();
        let engine = wasmtime::Engine::default();
        let component = WasmtimeComponent::new(&engine, instrumented).unwrap();

        let mut store = Store::new(&engine, ());
        let instance = Linker::new(&engine)
            .instantiate_async(&mut store, &component)
            .await
            .unwrap();

        let init = instance
            .get_typed_func::<(), ()>(&mut store, &name)
            .unwrap();

        init.call_async(&mut store, ()).await.unwrap();
        let snapshot = wizer
            .snapshot_component(
                &cx,
                &mut WasmtimeWizerComponent {
                    store: &mut store,
                    instance,
                },
            )
            .await
            .unwrap();

        let output = checked_cleanup(&snapshot);
        let component = WasmtimeComponent::new(&engine, &output).unwrap();

        assert!(component.get_export(None, "init").is_none());
        assert!(component.get_export(None, &name).is_none());
        assert_eq!(call_component(&output, "run"), 42);
        assert_eq!(call_component(&output, "componentize-qjs-init-x"), 42);
    }

    #[test]
    fn collects_dead_code_in_nested_components_without_removing_exports() {
        let input = wat::parse_str(format!(
            r#"(component
                (component $nested
                    (core module $m
                        (func $dead {})
                        (func (export "init") (result i32) i32.const 7)
                        (func (export "__wasm_apply_data_relocs") (result i32) i32.const 42))
                    (core instance $i (instantiate $m))
                    (core func $init (alias core export $i "init"))
                    (core func $run (alias core export $i "__wasm_apply_data_relocs"))
                    (func (export "init") (result u32) (canon lift (core func $init)))
                    (func (export "run") (result u32) (canon lift (core func $run))))
                (instance $nested (instantiate $nested))
                (alias export $nested "init" (func $init))
                (alias export $nested "run" (func $run))
                (export "init" (func $init))
                (export "run" (func $run)))"#,
            "nop ".repeat(4096)
        ))
        .unwrap();

        let (prepared, name) = prepare_snapshot(&input).unwrap();
        assert_eq!(call_component(&prepared, &name), 7);
        assert_eq!(call_component(&prepared, "run"), 42);

        let output = checked_cleanup(&input);
        assert!(input.len() - output.len() >= 4096);
        assert_eq!(call_component(&output, "init"), 7);
        assert_eq!(call_component(&output, "run"), 42);
    }

    #[test]
    fn preserves_imported_table_slots_snapshot_data_and_start_effects() {
        let input = wat::parse_str(format!(
            r#"(component
                (core module $host
                    (type $sig (func (result i32)))
                    (table (export "table") 5 funcref)
                    (memory 1)
                    (data (i32.const 0) "\03\00\00\00")
                    (global $state (mut i32) (i32.const 0))
                    (func $start i32.const 5 global.set $state)
                    (start $start)
                    (func (export "run") (result i32)
                        i32.const 0 i32.load
                        call_indirect (type $sig)
                        global.get $state i32.add))
                (core module $callbacks
                    (import "host" "table" (table 5 funcref))
                    (func $dead {})
                    (func $answer (result i32) i32.const 37)
                    (func $other (result i32) i32.const 99)
                    (elem (i32.const 3) $answer $other))
                (core instance $host (instantiate $host))
                (core instance (instantiate $callbacks (with "host" (instance $host))))
                (core func $run (alias core export $host "run"))
                (func (export "run") (result u32) (canon lift (core func $run))))"#,
            "nop ".repeat(4096)
        ))
        .unwrap();

        let output = checked_cleanup(&input);
        assert!(input.len() - output.len() >= 4096);
        assert_eq!(call_component(&output, "run"), 42);
    }

    #[test]
    fn removes_unused_function_imports() {
        let input = wat::parse_str(
            r#"(module
                (import "unused" "function" (func $unused))
                (func $dead call $unused)
                (func (export "run") (result i32) i32.const 42))"#,
        )
        .unwrap();

        let output = strip(&input).unwrap();
        let engine = wasmtime::Engine::default();
        let module = wasmtime::Module::new(&engine, output).unwrap();

        let mut store = Store::new(&engine, ());
        let instance = wasmtime::Instance::new(&mut store, &module, &[]).unwrap();

        assert_eq!(
            instance
                .get_typed_func::<(), i32>(&mut store, "run")
                .unwrap()
                .call(&mut store, ())
                .unwrap(),
            42
        );
    }

    #[test]
    fn prunes_marked_bodies_and_callees_without_changing_table_slots() {
        let input = wat::parse_str(format!(
            r#"(module
                (@custom "componentize-qjs:init-only"
                    "\01init\00init-alias\00__wasm_apply_data_relocs\00")
                (type $sig (func (result i32)))
                (import "host" "setup" (func $setup))
                (func $leaf call $setup {})
                (func $init (export "init") call $leaf)
                (export "init-alias" (func $init))
                (func $reloc (export "__wasm_apply_data_relocs") call $leaf)
                (func $answer (type $sig) i32.const 42)
                (table 6 funcref)
                (elem (i32.const 3) $init $answer $reloc)
                (memory 1)
                (data (i32.const 0) "\04\00\00\00")
                (func (export "run") (result i32)
                    i32.const 0 i32.load call_indirect (type $sig)))"#,
            "nop ".repeat(4096)
        ))
        .unwrap();

        let size = input.len();
        let output = strip(&input).unwrap();

        assert!(size - output.len() >= 4096);
        assert_eq!(output, strip(&output).unwrap());

        assert!(!Parser::new(0).parse_all(&output).any(|payload| {
            matches!(payload.unwrap(), Payload::CustomSection(s) if s.name() == INIT_ONLY_SECTION)
        }));

        let engine = wasmtime::Engine::default();
        let module = wasmtime::Module::new(&engine, output).unwrap();

        let mut store = Store::new(&engine, ());
        let instance = wasmtime::Instance::new(&mut store, &module, &[]).unwrap();

        assert_eq!(
            instance
                .get_typed_func::<(), i32>(&mut store, "run")
                .unwrap()
                .call(&mut store, ())
                .unwrap(),
            42
        );

        for name in ["init", "init-alias", "__wasm_apply_data_relocs"] {
            let error = instance
                .get_typed_func::<(), ()>(&mut store, name)
                .unwrap()
                .call(&mut store, ())
                .unwrap_err();
            assert_eq!(
                error.downcast_ref::<wasmtime::Trap>(),
                Some(&wasmtime::Trap::UnreachableCodeReached)
            );
        }
    }

    #[test]
    fn rejects_invalid_initialization_metadata() {
        for (data, expected) in [
            ("", "empty initialization-only metadata"),
            (
                r"\00init\00",
                "unsupported initialization-only metadata version 0",
            ),
            (
                r"\02init\00",
                "unsupported initialization-only metadata version 2",
            ),
            (r"\02", "unsupported initialization-only metadata version 2"),
            (r"\01", "not NUL-terminated"),
            (r"\01init", "not NUL-terminated"),
            (r"\01\ff\00", "invalid initialization-only export name"),
            (r"\01\00", "empty or duplicate"),
            (r"\01init\00init\00", "empty or duplicate"),
            (r"\01missing\00", "init export missing must be a function"),
        ] {
            let input = wat::parse_str(format!(
                r#"(module
                    (@custom "componentize-qjs:init-only" "{data}")
                    (func (export "init")))"#
            ))
            .unwrap();
            let error = strip(&input).unwrap_err();
            assert!(format!("{error:#}").contains(expected), "{error:#}");
        }

        for (declarations, expected) in [
            (
                r#"(import "host" "init" (func $init))
                   (export "init" (func $init))"#,
                "must be a defined function",
            ),
            (
                r#"(func $init (export "init"))
                   (export "run" (func $init))"#,
                "also exported as run",
            ),
            (r#"(memory (export "init") 1)"#, "must be a function"),
            (
                r#"(func (export "init"))
                   (@custom "componentize-qjs:init-only" "\01init\00")"#,
                "duplicate initialization metadata",
            ),
        ] {
            let input = wat::parse_str(format!(
                r#"(module
                    (@custom "componentize-qjs:init-only" "\01init\00")
                    {declarations})"#
            ))
            .unwrap();

            let error = strip(&input).unwrap_err();
            assert!(format!("{error:#}").contains(expected), "{error:#}");
        }
    }

    #[test]
    fn strips_build_metadata_but_preserves_other_custom_sections() {
        use std::borrow::Cow;
        use wasm_encoder::CustomSection;

        let mut module = Module::new();
        for name in [
            "name",
            "producers",
            ".debug_info",
            "external_debug_info",
            "sourceMappingURL",
            "linking",
            "reloc.CODE",
            "metadata.code.branch_hint",
            "dylink.0",
            "target_features",
            "dep-v0",
            "component-type:fixture",
        ] {
            module.section(&CustomSection {
                name: Cow::Borrowed(name),
                data: Cow::Borrowed(b""),
            });
        }
        let mut nested = Component::new();
        nested.section(&wasm_encoder::ModuleSection(&module));

        let mut component = Component::new();
        component.section(&wasm_encoder::NestedComponentSection(&nested));

        for name in ["component-name", "producers", "application-metadata"] {
            component.section(&CustomSection {
                name: Cow::Borrowed(name),
                data: Cow::Borrowed(b"metadata"),
            });
        }

        let output = checked_cleanup(&component.finish());
        let sections: Vec<_> = Parser::new(0)
            .parse_all(&output)
            .filter_map(|payload| match payload.unwrap() {
                Payload::CustomSection(section) => Some((section.name(), section.data())),
                _ => None,
            })
            .collect();

        assert_eq!(
            sections,
            [
                ("dylink.0", &b""[..]),
                ("target_features", &b""[..]),
                ("dep-v0", &b""[..]),
                ("component-type:fixture", &b""[..]),
                ("application-metadata", &b"metadata"[..]),
            ]
        );
    }

    #[test]
    fn rejects_invalid_core_modules_and_non_components() {
        let module = wat::parse_str(r#"(module (func (export "run") call 9))"#).unwrap();
        let mut component = Component::new();
        component.section(&RawSection {
            id: ComponentSectionId::CoreModule.into(),
            data: &module,
        });

        let err = cleanup(&component.finish()).unwrap_err();
        assert!(err.to_string().contains("failed to parse core module"));
        assert!(err.chain().count() > 1, "lost the underlying Walrus error");

        let mut truncated = Component::new().finish();
        truncated.push(ComponentSectionId::CoreModule.into());

        assert!(
            cleanup(&truncated)
                .unwrap_err()
                .downcast_ref::<wasmparser::Error>()
                .is_some()
        );

        assert!(cleanup(&module).is_err());
        assert!(cleanup(b"not wasm").is_err());

        assert!(prepare_snapshot(&Component::new().finish()).is_err());
        assert!(prepare_snapshot(&module).is_err());
    }
}
