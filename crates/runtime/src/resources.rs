//! Exported resource table: maps `rep` indices to JS objects.
//!
//! When JS defines a resource type and exports it, the component model needs
//! a table mapping internal representation indices (`rep`) to the js objects
//! that back them.

use std::cell::RefCell;
use std::collections::HashMap;

use rquickjs::{JsLifetime, Persistent, Value};
use wit_dylib_ffi::Resource;

use crate::CtxExt;

/// A borrowed imported resource handle that must be dropped when the call ends.
pub(crate) struct BorrowedResource {
    pub(crate) handle: u32,
    pub(crate) drop_fn: unsafe extern "C" fn(u32),
}

/// Table mapping `rep` indices to JS objects for exported resources.
#[derive(Default, JsLifetime)]
pub(crate) struct ResourceTable {
    inner: RefCell<Inner>,
}

#[derive(Default)]
struct Inner {
    map: HashMap<usize, Persistent<Value<'static>>>,
    next_rep: usize,
}

impl ResourceTable {
    /// Insert a JS object, returning its `rep` index.
    pub(crate) fn insert(&self, val: Persistent<Value<'static>>) -> usize {
        let mut inner = self.inner.borrow_mut();
        let rep = inner.next_rep;
        inner.next_rep += 1;
        inner.map.insert(rep, val);
        rep
    }

    /// Get a cloned persistent handle by `rep` index for borrow lookups.
    pub(crate) fn get(&self, rep: usize) -> Persistent<Value<'static>> {
        self.inner
            .borrow()
            .map
            .get(&rep)
            .expect("resource not found")
            .clone()
    }

    /// Remove and return a js object by `rep` index.
    pub(crate) fn remove(&self, rep: usize) -> Persistent<Value<'static>> {
        self.inner
            .borrow_mut()
            .map
            .remove(&rep)
            .expect("resource not found")
    }
}

/// Extract the canonical handle from an imported resource wrapper object.
pub(crate) fn imported_resource_to_handle(val: &Value<'_>) -> u32 {
    val.as_object()
        .and_then(|obj| obj.get::<_, u32>("componentize_js_handle").ok())
        .expect("expected resource wrapper with componentize_js_handle")
}

/// Convert a js object to a canonical handle for an exported resource.
pub(crate) fn exported_resource_to_handle<'js>(
    ctx: &rquickjs::Ctx<'js>,
    ty: Resource,
    val: &Value<'js>,
) -> u32 {
    let obj = val.as_object().expect("expected resource object");
    if let Ok(handle) = obj.get::<_, u32>("componentize_js_handle") {
        return handle;
    }

    let rep = ctx.resources().insert(Persistent::save(ctx, val.clone()));
    let new_fn = ty.new().expect("exported resource must have new()");
    let handle = unsafe { new_fn(rep) };

    obj.set("componentize_js_handle", handle).unwrap();
    handle
}
