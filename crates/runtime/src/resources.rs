//! Native ownership of imported resources and the exported JS object table.
//!
//! Imported resource finalizers only enqueue owned handles. The runtime must
//! drain those handles outside QuickJS evaluations and jobs.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use rquickjs::class::{Class, JsClass, Readable, Trace};
use rquickjs::function::{Constructor, This};
use rquickjs::{Ctx, Exception, Function, JsLifetime, Object, Persistent, Value};
use wit_dylib_ffi::Resource;

use crate::{CtxExt, DetHashMap, symbol_dispose};

#[derive(Default)]
struct ResourceDropQueue {
    pending: RefCell<VecDeque<(Resource, u32)>>,
    draining: Cell<bool>,
}

#[derive(Clone, Copy)]
enum Ownership {
    Owned(u32),
    Borrowed(u32),
    Transferring,
    Transferred,
    Disposed,
    Expired,
}

struct ImportedResourceState {
    ty: Resource,
    ownership: Cell<Ownership>,
    loans: Cell<usize>,
    drops: Rc<ResourceDropQueue>,
}

impl ImportedResourceState {
    fn take_owned(&self) -> Option<(Resource, u32)> {
        let Ownership::Owned(handle) = self.ownership.get() else {
            return None;
        };
        self.ownership.set(Ownership::Disposed);
        Some((self.ty, handle))
    }

    fn validate(&self, ty: Resource, owned: bool) -> Result<u32, &'static str> {
        if self.ty != ty {
            return Err("imported resource has the wrong WIT resource type");
        }
        match self.ownership.get() {
            Ownership::Owned(_) if owned && self.loans.get() != 0 => {
                Err("cannot transfer ownership of a resource while it is borrowed")
            }
            Ownership::Owned(handle) => Ok(handle),
            Ownership::Borrowed(handle) if !owned => Ok(handle),
            Ownership::Borrowed(_) => Err("cannot transfer ownership of a borrowed resource"),
            Ownership::Transferring => Err("resource ownership transfer is in progress"),
            Ownership::Transferred => Err("resource ownership has already been transferred"),
            Ownership::Disposed => Err("resource has been disposed"),
            Ownership::Expired => Err("borrowed resource has expired"),
        }
    }
}

impl Drop for ImportedResourceState {
    fn drop(&mut self) {
        // This can run inside QuickJS's GC: no JS values or host calls here.
        if let Some(resource) = self.take_owned() {
            self.drops.pending.borrow_mut().push_back(resource);
        }
    }
}

/// Opaque, type-checked state for an imported resource's JS wrapper.
#[derive(Trace, JsLifetime)]
pub(crate) struct ImportedResource {
    #[qjs(skip_trace)]
    state: Rc<ImportedResourceState>,
}

impl<'js> JsClass<'js> for ImportedResource {
    const NAME: &'static str = "ImportedResource";
    type Mutable = Readable;

    fn prototype(ctx: &Ctx<'js>) -> rquickjs::Result<Option<Object<'js>>> {
        let prototype = Object::new(ctx.clone())?;
        let dispose = Function::new(ctx.clone(), dispose_imported_resource)?;
        prototype.set("drop", dispose.clone())?;
        prototype.set(symbol_dispose(ctx)?, dispose)?;
        Ok(Some(prototype))
    }

    fn constructor(_ctx: &Ctx<'js>) -> rquickjs::Result<Option<Constructor<'js>>> {
        Ok(None)
    }
}

/// Keeps a canonical imported borrow alive until its call ends.
///
/// Dropping the guard expires any retained JS wrapper before releasing the
/// canonical borrowed handle. That release does not destroy the borrowed owner.
#[must_use = "keep this guard alive until the borrowed resource's call ends"]
pub(crate) struct BorrowedResource {
    state: Rc<ImportedResourceState>,
    handle: u32,
}

impl Drop for BorrowedResource {
    fn drop(&mut self) {
        assert_eq!(
            self.state.loans.get(),
            0,
            "borrowed resource still used by an outstanding import at the end of its call"
        );
        self.state.ownership.set(Ownership::Expired);
        unsafe { self.state.ty.drop()(self.handle) };
    }
}

/// Retains native owner state while an outbound call borrows its handle.
///
/// Live loans prevent disposal and ownership transfer, even across suspension.
/// This does not extend an incoming canonical borrow's lifetime: its original
/// `BorrowedResource` guard must also outlive every call that uses it.
#[must_use = "keep this loan alive until the borrowing import completes"]
pub(crate) struct ImportedResourceLoan {
    state: Rc<ImportedResourceState>,
}

impl Drop for ImportedResourceLoan {
    fn drop(&mut self) {
        self.state.loans.set(self.state.loans.get() - 1);
    }
}

/// Restores an unconsumed handle after a partial write or cancelled import.
#[must_use = "commit consumed transfers before releasing their call context"]
pub(crate) struct ImportedResourceTransfer {
    state: Rc<ImportedResourceState>,
    handle: u32,
}

impl ImportedResourceTransfer {
    pub(crate) fn commit(self) {
        self.state.ownership.set(Ownership::Transferred);
    }
}

impl Drop for ImportedResourceTransfer {
    fn drop(&mut self) {
        if matches!(self.state.ownership.get(), Ownership::Transferring) {
            self.state.ownership.set(Ownership::Owned(self.handle));
        }
    }
}

/// Native disposal prototype inherited by each imported WIT resource prototype.
pub(crate) fn imported_resource_prototype<'js>(ctx: &Ctx<'js>) -> rquickjs::Result<Object<'js>> {
    Class::<ImportedResource>::prototype(ctx)?.ok_or_else(|| {
        Exception::throw_type(ctx, "native imported resource prototype is unavailable")
    })
}

fn make_imported<'js>(ctx: &Ctx<'js>, resource: ImportedResource) -> rquickjs::Result<Value<'js>> {
    let prototype = ctx.resource_classes().prototype(resource.state.ty.index());
    let prototype = match prototype {
        Some(prototype) => prototype.restore(ctx)?,
        None => imported_resource_prototype(ctx)?,
    };
    Class::instance_proto(resource, prototype).map(Class::into_value)
}

/// Take ownership of an imported handle, including on wrapper creation failure.
///
/// Failed construction queues the handle for the next safe-point drain.
pub(crate) fn make_imported_owned<'js>(
    ctx: &Ctx<'js>,
    ty: Resource,
    handle: u32,
) -> rquickjs::Result<Value<'js>> {
    let state = Rc::new(ImportedResourceState {
        ty,
        ownership: Cell::new(Ownership::Owned(handle)),
        loans: Cell::new(0),
        drops: Rc::clone(&ctx.resource_classes().drops),
    });
    make_imported(ctx, ImportedResource { state })
}

/// Wrap a canonical imported borrow and return its call-lifetime guard.
///
/// Failed construction releases the borrowed handle immediately.
pub(crate) fn make_imported_borrowed<'js>(
    ctx: &Ctx<'js>,
    ty: Resource,
    handle: u32,
) -> rquickjs::Result<(Value<'js>, BorrowedResource)> {
    let state = Rc::new(ImportedResourceState {
        ty,
        ownership: Cell::new(Ownership::Borrowed(handle)),
        loans: Cell::new(0),
        drops: Rc::clone(&ctx.resource_classes().drops),
    });
    let guard = BorrowedResource {
        state: Rc::clone(&state),
        handle,
    };
    let value = make_imported(ctx, ImportedResource { state })?;
    Ok((value, guard))
}

fn dispose_imported_resource<'js>(
    this: This<Class<'js, ImportedResource>>,
    ctx: Ctx<'js>,
) -> rquickjs::Result<()> {
    let resource = {
        let resource = this.0.try_borrow()?;
        match resource.state.ownership.get() {
            Ownership::Borrowed(_) => Err("cannot dispose a borrowed resource"),
            Ownership::Transferring => Err("cannot dispose a resource during ownership transfer"),
            Ownership::Owned(_) if resource.state.loans.get() != 0 => {
                Err("cannot dispose a resource while it is borrowed")
            }
            _ => Ok(resource.state.take_owned()),
        }
    }
    .map_err(|message| Exception::throw_type(&ctx, message))?;

    // The handle is invalidated and the class borrow is released before re-entry.
    if let Some((ty, handle)) = resource {
        unsafe { ty.drop()(handle) };
    }
    Ok(())
}

/// Run deferred owned-resource destructors at a safe point outside JS execution.
///
/// Call after evaluations/jobs, loan completion and explicit GC, while the runtime
/// and its WIT metadata are still alive. Never call from canonical post-return,
/// where imports and intrinsics are forbidden. Teardown must drain before
/// destroying runtime state; neither finalizers nor dropping the queue call hosts.
pub(crate) fn drain_resource_drops(ctx: &Ctx<'_>) {
    let drops = Rc::clone(&ctx.resource_classes().drops);
    if drops.draining.replace(true) {
        return;
    }

    struct DrainGuard<'a>(&'a Cell<bool>);

    impl Drop for DrainGuard<'_> {
        fn drop(&mut self) {
            self.0.set(false);
        }
    }

    let _guard = DrainGuard(&drops.draining);
    loop {
        let resource = drops.pending.borrow_mut().pop_front();
        let Some((ty, handle)) = resource else {
            break;
        };
        // No userdata or RefCell guard may survive this potentially re-entrant call.
        unsafe { ty.drop()(handle) };
    }
}

/// Table mapping `rep` indices to JS objects for exported resources.
#[derive(Default, JsLifetime)]
pub(crate) struct ResourceTable {
    inner: RefCell<Inner>,
}

#[derive(Default)]
struct Inner {
    map: DetHashMap<usize, Persistent<Value<'static>>>,
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

/// Per-resource JS "class" (constructor + prototype) for imported resources.
#[derive(Default, JsLifetime)]
pub(crate) struct ResourceClasses {
    inner: RefCell<ClassInner>,
    drops: Rc<ResourceDropQueue>,
}

#[derive(Default)]
struct ClassInner {
    map: DetHashMap<usize, ResourceClass>,
}

struct ResourceClass {
    class: Persistent<Constructor<'static>>,
    prototype: Persistent<Object<'static>>,
}

impl ResourceClasses {
    /// Register a resource's class and prototype by `Resource::index()`.
    pub(crate) fn insert(
        &self,
        index: usize,
        class: Persistent<Constructor<'static>>,
        prototype: Persistent<Object<'static>>,
    ) {
        self.inner
            .borrow_mut()
            .map
            .insert(index, ResourceClass { class, prototype });
    }

    /// Get the class (constructor) for a resource, if any.
    pub(crate) fn class(&self, index: usize) -> Option<Persistent<Constructor<'static>>> {
        self.inner.borrow().map.get(&index).map(|c| c.class.clone())
    }

    /// Get the prototype object for a resource, if any.
    pub(crate) fn prototype(&self, index: usize) -> Option<Persistent<Object<'static>>> {
        self.inner
            .borrow()
            .map
            .get(&index)
            .map(|c| c.prototype.clone())
    }
}

fn imported_resource_state(val: &Value<'_>) -> rquickjs::Result<Rc<ImportedResourceState>> {
    let resource = Class::<ImportedResource>::from_value(val)
        .map_err(|_| Exception::throw_type(val.ctx(), "expected an imported resource wrapper"))?;
    let state = Rc::clone(&resource.try_borrow()?.state);
    Ok(state)
}

/// Check type, lifetime and ownership without consuming or borrowing the handle.
///
/// Preflight each argument before entering the infallible `Call` ABI. This does
/// not reserve ownership or detect duplicate/mixed uses among several arguments;
/// the caller must check such aliasing before starting any transfers.
pub(crate) fn validate_imported_resource(
    ty: Resource,
    val: &Value<'_>,
    owned: bool,
) -> rquickjs::Result<u32> {
    imported_resource_state(val)?
        .validate(ty, owned)
        .map_err(|message| Exception::throw_type(val.ctx(), message))
}

/// Borrow a handle and retain its native state until the returned loan is dropped.
///
/// No JS root is needed to keep an owned handle alive. If its wrapper is collected,
/// releasing the final loan queues the owned handle for a safe-point drop.
pub(crate) fn borrow_imported_resource(
    ty: Resource,
    val: &Value<'_>,
) -> rquickjs::Result<(u32, ImportedResourceLoan)> {
    let state = imported_resource_state(val)?;
    let handle = state
        .validate(ty, false)
        .map_err(|message| Exception::throw_type(val.ctx(), message))?;
    let loans = state.loans.get().checked_add(1).ok_or_else(|| {
        Exception::throw_type(val.ctx(), "too many outstanding imported resource borrows")
    })?;
    state.loans.set(loans);
    Ok((handle, ImportedResourceLoan { state }))
}

/// Reserve ownership until the canonical ABI reports whether it consumed a value.
pub(crate) fn transfer_imported_resource(
    ty: Resource,
    val: &Value<'_>,
) -> rquickjs::Result<(u32, ImportedResourceTransfer)> {
    let state = imported_resource_state(val)?;
    let handle = state
        .validate(ty, true)
        .map_err(|message| Exception::throw_type(val.ctx(), message))?;
    state.ownership.set(Ownership::Transferring);
    Ok((handle, ImportedResourceTransfer { state, handle }))
}

/// Convert a js object to a canonical handle for an exported resource.
pub(crate) fn exported_resource_to_handle<'js>(
    ctx: &rquickjs::Ctx<'js>,
    ty: Resource,
    val: &Value<'js>,
) -> u32 {
    let obj = val.as_object().expect("expected resource object");
    if let Ok(handle) = obj.get::<_, u32>("__cqjs_handle") {
        return handle;
    }

    let rep = ctx.resources().insert(Persistent::save(ctx, val.clone()));
    let new_fn = ty.new().expect("exported resource must have new()");
    let handle = unsafe { new_fn(rep) };

    obj.set("__cqjs_handle", handle).unwrap();
    handle
}
