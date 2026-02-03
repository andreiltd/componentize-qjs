//! WASI import stubbing for snapshotted components.
//!
//! The approach:
//! 1. Decode the snapshotted component to extract its WIT world
//! 2. Create a "stub world" where the WASI imports become exports
//! 3. Use `dummy_module` to generate a core module with trap implementations
//! 4. Encode it as a stub component
//! 5. Use `wac-graph` to compose the stub into the original component

use anyhow::{bail, Context, Result};
use indexmap::IndexMap;
use wac_graph::{plug, types::Package, CompositionGraph, EncodeOptions};
use wit_component::{dummy_module, embed_component_metadata, ComponentEncoder, StringEncoding};
use wit_parser::decoding::{decode, DecodedWasm};
use wit_parser::{Docs, ManglingAndAbi, Resolve, Stability, World, WorldItem, WorldKey};

/// Stub all WASI imports in a component, producing a self-contained component.
pub fn stub_wasi_imports(component: &[u8]) -> Result<Vec<u8>> {
    let decoded = decode(component).context("failed to decode component WIT")?;
    let (resolve, world_id) = match decoded {
        DecodedWasm::Component(resolve, world_id) => (resolve, world_id),
        _ => bail!("expected a component, got a WIT package"),
    };

    let world = &resolve.worlds[world_id];

    let wasi_imports: IndexMap<WorldKey, WorldItem> = world
        .imports
        .clone()
        .into_iter()
        .filter(|(key, _)| resolve.name_world_key(key).starts_with("wasi:"))
        .collect();

    if wasi_imports.is_empty() {
        return Ok(component.to_vec());
    }

    let stub_component = make_stub_component(&resolve, world, &wasi_imports)
        .context("failed to build stub component")?;

    let mut graph = CompositionGraph::new();

    let orig_pkg = Package::from_bytes("original", None, component.to_vec(), graph.types_mut())
        .context("failed to register original component")?;

    let stub_pkg = Package::from_bytes("stubs", None, stub_component, graph.types_mut())
        .context("failed to register stub component")?;

    let orig_id = graph.register_package(orig_pkg)?;
    let stub_id = graph.register_package(stub_pkg)?;

    plug(&mut graph, vec![stub_id], orig_id)?;

    graph
        .encode(EncodeOptions::default())
        .context("failed to encode composed component")
}

/// Build a component that exports trap implementations for the given WASI imports.
fn make_stub_component(
    resolve: &Resolve,
    original_world: &World,
    wasi_imports: &IndexMap<WorldKey, WorldItem>,
) -> Result<Vec<u8>> {
    let mut stub_resolve = resolve.clone();
    let stub_world_id = stub_resolve.worlds.alloc(World {
        name: "wasi-stubs".to_string(),
        imports: IndexMap::new(),
        exports: wasi_imports.clone(),
        package: original_world.package,
        docs: Docs::default(),
        stability: Stability::default(),
        includes: Vec::new(),
        include_names: Vec::new(),
    });

    let mut core_module = dummy_module(&stub_resolve, stub_world_id, ManglingAndAbi::Standard32);

    embed_component_metadata(
        &mut core_module,
        &stub_resolve,
        stub_world_id,
        StringEncoding::UTF8,
    )
    .context("failed to embed component metadata in stub module")?;

    ComponentEncoder::default()
        .module(&core_module)
        .unwrap()
        .validate(true)
        .encode()
        .context("failed to encode stub component")
}
