# Runtime Intrinsics Reference

This document describes the bridge identifiers and module-state conventions
that componentize-qjs uses to connect WIT with quickjs. These are internal
implementation details; user code should be written as an ES module and should
prefer explicit `import`/`export` syntax plus the public `wit.*` API where
possible.

## Naming Conventions

| Category | Convention | Examples |
|---|---|---|
| Internal namespace | `__cqjs` prefix | `__cqjs.makeStream()` |
| Hidden properties | `__cqjs_` prefix | `__cqjs_handle` |
| Public API | `wit.*` namespace | `wit.Stream`, `wit.Future` |
| JS classes | PascalCase | `StreamReadable`, `FutureWritable` |
| Prototype methods | camelCase | `read`, `cancelRead`, `writeAll` |
| WIT functions → JS | lowerCamelCase | `myFunction` from `my-function` |
| WIT resources → JS | UpperCamelCase class | `InputStream` from `input-stream` |
| Type constants | UPPER_SNAKE_CASE | `wit.Stream.U8`, `wit.Future.RESULT_STRING_U32` |

---

## `globalThis.__cqjs`: Internal Namespace

A frozen object containing all internal bridge functions. Installed during
WIT binding registration and not intended for direct use by application code.

### `__cqjs.makeStream(typeIndex)`

Create a new stream pair. Returns `{ readable, writable }` where `readable`
is a `StreamReadable` instance and `writable` is a `StreamWritable` instance.

- typeIndex (`number`) : Index into the WIT stream type table.
- Returns : `{ readable: StreamReadable, writable: StreamWritable }`

### `__cqjs.makeFuture(typeIndex)`

Create a new future pair. Returns `{ readable, writable }` where `readable`
is a `FutureReadable` instance and `writable` is a `FutureWritable` instance.

- typeIndex (`number`) : Index into the WIT future type table.
- Returns : `{ readable: FutureReadable, writable: FutureWritable }`

### `__cqjs.getMemoryUsage()`

Return quickjs engine memory statistics.

- Returns : Object with the following fields:
  - `mallocSize` : Total bytes allocated via malloc
  - `mallocCount` : Number of active malloc allocations
  - `memoryUsedSize` : Total memory used by the JS engine
  - `objCount` : Number of live JS objects
  - `strCount` : Number of live JS strings
  - `atomCount` : Number of live atoms (interned strings)
  - `atomSize` : Total bytes used by atoms
  - `propCount` : Number of live properties
  - `shapeCount` : Number of live shapes (hidden classes)
  - `arrayCount` : Number of live arrays

### `__cqjs.runGc()`

Trigger a quickjs garbage collection cycle.

- Returns : `undefined`

### `__cqjs.asyncExports`

An object containing wrapper functions for async WIT exports. Each wrapper
calls the user's export function and chains `.then()` to signal `task_return`
back to the component model host.

Structure mirrors the WIT export layout:

```js
__cqjs.asyncExports = {
  myFunc: Function,          // root-scope async export
  myInterface: {             // interface-scoped exports
    anotherFunc: Function,
  },
};
```

---

## `globalThis.wit` : Public Stream/Future API

The user-facing API for creating streams and futures from JavaScript.
Installed by the generated JS shim (see `src/codegen.rs`).

### `wit.Stream(type)`

Create a new stream pair for the given type constant.

```js
const { readable, writable } = wit.Stream(wit.Stream.U8);
```

If only one stream type exists in the WIT world, `type` may be omitted.

### `wit.Stream.from(iterable, type)`

Create a stream backed by a synchronous or asynchronous iterable. Returns
`{ readable, completion }`, where `completion` settles when the iterable has
finished or its producer fails. The `type` may be omitted if only one stream
type exists.

WIT function boundaries use this adapter automatically. When a JavaScript
iterable is passed to or returned from a WIT function, the expected stream type
comes from that function's signature. Iterable adaptation must happen while an
async component call is active.

### `wit.Future(type)`

Create a new future pair for the given type constant.

```js
const { readable, writable } = wit.Future(wit.Future.STRING);
```

If only one future type exists in the WIT world, `type` may be omitted.

### `wit.Future.from(value, type)`

Adapt a value or Promise to a WIT future. Returns `{ readable, completion }`.
Use the readable handle when returning a future from an async export; returning
the payload Promise directly would cause JavaScript to await it before the WIT
boundary is reached.

### Type Constants

Type constants are generated for each stream/future element type found in
the WIT world. They are available as static properties on `wit.Stream` and
`wit.Future`, and also via the `.types` map for runtime discovery. The numeric
value is an internal index into the runtime WIT stream/future table; user code
should pass the generated constant rather than hard-coding the index.

Named aliases for stream and future types are emitted as additional constants
for the same internal index. For example, `type prompt-stream =
stream<message>` adds `wit.Stream.PROMPT_STREAM`.

| Constant Pattern | Example | WIT Type |
|---|---|---|
| Primitives | `U8`, `STRING`, `BOOL` | `u8`, `string`, `bool` |
| Named types | `MY_TYPE` | `my-type` (user-defined) |
| Options | `OPTION_U32` | `option<u32>` |
| Results | `RESULT_STRING_U32` | `result<string, u32>` |
| Tuples | `TUPLE_U32_STRING` | `tuple<u32, string>` |
| Lists | `LIST_U8` | `list<u8>` |
| Maps | `MAP_STRING_U32` | `map<string, u32>` |
| Nested streams/futures | `STREAM_U8`, `FUTURE_STRING` | `stream<u8>`, `future<string>` |
| Unit | `UNIT` | (no payload) |

Constants for anonymous composite payloads are named recursively. For example,
`stream<result<string, u32>>` produces `wit.Stream.RESULT_STRING_U32`, and
`stream<stream<u8>>` produces `wit.Stream.STREAM_U8`. True self-recursive WIT
type cycles are rejected by `wit-parser`, so the runtime only needs to support
finite nested type graphs here.

If two payload types would produce the same local constant name, the generated
shim qualifies the duplicate names with their owner. For example, if
`test:dupe/left.point` and `test:dupe/right.point` are both used as stream
payloads, the constants are emitted as `TEST_DUPE_LEFT_POINT` and
`TEST_DUPE_RIGHT_POINT` rather than overwriting `POINT`.

---

## JS Classes

Native quickjs classes registered on `globalThis` via `Class::define`.
These are not user-constructible : instances are created internally by
`__cqjs.makeStream()`, `__cqjs.makeFuture()`, and WIT type lifting.

### `StreamReadable`

Readable endpoint of a component-model stream.

| Method | Description |
|---|---|
| `read(count?)` | Read up to `count` items (default 1). Returns a Promise resolving to an Array (or Uint8Array for `stream<u8>`). |
| `next()` | Read the next async-iterator value. `stream<u8>` yields bounded `Uint8Array` chunks. |
| `return()` | End iteration and release the readable handle. |
| `cancelRead()` | Cancel an in-progress async read. Returns `{ progress, result }` or `undefined` if the cancel itself blocks. |
| `drop()` | Drop the readable end, releasing the underlying handle. |
| `[Symbol.asyncIterator]()` | Return this readable as a one-shot async iterator. |
| `[Symbol.dispose]()` | Alias for `drop()`. |

### `StreamWritable`

Writable endpoint of a component-model stream.

| Method | Description |
|---|---|
| `write(data)` | Write a single item or array of items. Returns a Promise resolving to the number of items written. |
| `writeOne(value)` | Write exactly one item, including list and tuple values represented by JavaScript arrays. |
| `writeAll(buffer)` | Write all items from buffer, calling `write` repeatedly. Returns a Promise resolving to the total count written. |
| `writeIterableItem(value)` | Write one iterable yield. Matching numeric typed arrays are written as batches; all other values are written as one WIT item. Returns a Promise resolving to whether the complete value was written. |
| `cancelWrite()` | Cancel an in-progress async write. Returns `{ progress, result }` or `undefined` if the cancel itself blocks. |
| `drop()` | Drop the writable end, releasing the underlying handle. |
| `[Symbol.dispose]()` | Alias for `drop()`. |

### `FutureReadable`

Readable endpoint of a component-model future.
This object is not a thenable; `await readable.read()` obtains its payload.
The native class traces its cached read Promise so repeated reads share one
operation without hiding JavaScript references from the garbage collector.

| Method | Description |
|---|---|
| `read()` | Read the future value. Returns a Promise that resolves with the value or rejects if the writer was dropped/cancelled. |
| `cancelRead()` | Cancel an in-progress async read. Returns the `CopyResult` code or `undefined` if the cancel blocks. |
| `drop()` | Drop the readable end. |
| `[Symbol.dispose]()` | Alias for `drop()`. |

An immediately completed cancellation settles the original operation just like
a callback completion. A cancelled read clears the cached Promise, allowing a
new `read()` to retry. A completed future cannot be written a second time.

### `FutureWritable`

Writable endpoint of a component-model future.

| Method | Description |
|---|---|
| `write(value)` | Write a value to the future. Returns a Promise resolving to `true` on success, `false` otherwise. |
| `cancelWrite()` | Cancel an in-progress async write. Returns the `CopyResult` code or `undefined` if the cancel blocks. |
| `drop()` | Drop the writable end. |
| `[Symbol.dispose]()` | Alias for `drop()`. |

All endpoint disposal methods use the well-known `Symbol.dispose`, not
`Symbol.for("dispose")`. Disposal and ownership transfer reject endpoints with
an in-progress operation; cancellation must settle first.

### Imported Resources

Imported resource wrappers store their resource type, handle, and ownership
state in native class data, not JavaScript properties. Their WIT-specific
prototypes inherit native disposal methods. Owned handles are invalidated on
transfer or explicit disposal; borrowed wrappers expire at the end of their
component call.

Outbound loans keep native owner state alive and block disposal or transfer until
the import completes. Own transfers remain reserved until consumption is known:
partial writes restore unconsumed resource wrappers, and imports cancelled before
starting restore their owned arguments. Incoming borrow guards reject scope exit
while a borrowing import remains outstanding.

The imported-resource registry and deferred-drop queue are initialized with the
JavaScript context, including for empty WIT worlds with no bindings to initialize.

Garbage-collection finalizers enqueue abandoned owned handles. Host destructors
run from safe runtime boundaries after QuickJS has left its finalizer, with no
Rust queue borrow held across a host call. Explicit disposal is preferred when
prompt cleanup is required.

### Task Cancellation

Cancellation unjoins each pending waitable before invoking its cancellation
intrinsic. An immediate completion uses the normal completion handler; a
blocked cancellation rejoins the waitable and retains its conversion context,
callbacks, and ABI buffer until completion.

Cancelled async imports reject their JavaScript Promises. During task
cancellation, new asynchronous operations are rejected and export completion
callbacks do not call `task.return`. The runtime acknowledges `task.cancel`
only after pending operations and their borrowed arguments have been released.

---

## rquickjs Integration

The runtime uses rquickjs 0.13. Numeric typed arrays are copied into aligned,
Rust-owned ABI buffers through `typed_array::TypedArrayExt`. The same module
contains native-view validation and shared type-specific casting macros.
`BufferGuard` only handles allocation and ownership; WIT type dispatch remains
in the call and stream conversion code.

The adapter uses `TypedArray::as_raw()`. The raw view supplies the
actual view offset and byte length; element counts do not read an overridable
JavaScript `length` property. No JavaScript executes between obtaining the raw
view and copying its bytes, and no JS-backed pointer survives the copy. Attached
empty views are valid; detached or out-of-bounds views report an error.

The custom job helper remains necessary: `Ctx::execute_pending_job()` still
loses exception status, while `Runtime::execute_pending_job()` would reacquire
the lock already held by `Context::with()`. `Promise::finish()` uses the same
lossy context method, so module evaluation retains its explicit job loop.
Likewise, rquickjs does not yet expose a typed `Symbol.dispose` constructor;
the runtime retrieves the well-known symbol from the global `Symbol` object.

## Hidden Object Properties

### `__cqjs_handle`

A numeric property used on exported, JS-backed WIT resources. Stores the
canonical component-model resource handle (`u32`). Imported resources instead
use native opaque state and never trust this property.

- Set/read by: `exported_resource_to_handle`.
- Removed: When an exported owned resource is lifted back to JS via
  `push_own`, since the handle is no longer valid.

## WIT Import/Export Naming

### Import Interfaces

User code imports WIT interfaces as ES modules using their full WIT path:

```js
import random from "wasi:random/random@0.2.6";

export function getRandomU64() {
  return random.getRandomU64();
}
```

The runtime resolves both the versioned specifier and the versionless alias
internally, for example `wasi:random/random@0.2.6` and `wasi:random/random`.
The resolved module is a native runtime module whose exports call the host WIT
imports directly.

### Import Functions

WIT function names are converted from kebab-case to lowerCamelCase:

| WIT Name | JS Name |
|---|---|
| `get-random-bytes` | `getRandomBytes` |
| `my-function` | `myFunction` |

### Import Types

Enums, variants, flags, and records have no standalone runtime object; they are
represented directly by their values. Resources are exposed as classes.

| WIT Category | JS Representation | Example |
|---|---|---|
| Enums | case-name string | `"variant-a"` |
| Variants | `{ tag, val }` (string tag) | `{ tag: "case-a", val }` |
| Flags | `{ name: boolean }` object | `{ flagA: true, flagB: false }` |
| Records | object, camelCase fields | `{ fieldName: value }` |
| Maps | `Map` | `new Map([["key", value]])` |
| Resources | class instance; methods on prototype | `input.blockingRead(n)` |

### Export Functions

Export functions are looked up from the evaluated user ES module namespace
using the same lowerCamelCase convention, optionally nested under the
lowerCamelCase exported interface name.

### Result / Variant / Enum / Flags / Option Protocol

Top-level function returns of WIT `result<T, E>` use JavaScript's exception
convention: return the `ok` payload directly, or throw the `err` payload. When a
WIT import returns `err`, JavaScript receives a thrown object with the payload on
`error.payload`.

Nested WIT `result` values and all `variant` values are represented as plain
objects with a string `tag`:

```js
// nested result<string, u32>
{ tag: "ok", val: "hello" }
{ tag: "err", val: 42 }

// variant (tag is the case name)
{ tag: "circle", val: "payload" }
{ tag: "none" }  // no payload case
```

`enum` values are case-name strings (`"variant-a"`), `flags` are
`{ camelCaseName: boolean }` objects, and `option<T>` is `T | null` — except a
nested `option<option<T>>`, which is wrapped as `{ tag: "some", val } |
{ tag: "none" }` so that `none` and `some(none)` stay distinct.
