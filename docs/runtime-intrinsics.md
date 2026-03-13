# Runtime Intrinsics Reference

This document describes the bridge identifiers that componentize-qjs installs
in the JavaScript global scope to connect WIT with quickjs. These are internal
implementation details and user code should prefer the public `wit.*` API where 
possible.

## Naming Conventions

| Category | Convention | Examples |
|---|---|---|
| Internal namespace | `__cqjs` prefix | `__cqjs.makeStream()` |
| Hidden properties | `__cqjs_` prefix | `__cqjs_handle` |
| Transient globals | `__cqjs_` prefix | `__cqjs_ctor_args` |
| Public API | `wit.*` namespace | `wit.Stream`, `wit.Future` |
| JS classes | PascalCase | `StreamReadable`, `FutureWritable` |
| Prototype methods | camelCase | `read`, `cancelRead`, `writeAll` |
| WIT functions → JS | lowerCamelCase | `myFunction` from `my-function` |
| WIT types → JS | UpperCamelCase | `MyRecord` from `my-record` |
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

### `wit.Future(type)`

Create a new future pair for the given type constant.

```js
const { readable, writable } = wit.Future(wit.Future.STRING);
```

If only one future type exists in the WIT world, `type` may be omitted.

### Type Constants

Type constants are generated for each stream/future element type found in
the WIT world. They are available as static properties on `wit.Stream` and
`wit.Future`, and also via the `.types` map for runtime discovery.

| Constant Pattern | Example | WIT Type |
|---|---|---|
| Primitives | `U8`, `STRING`, `BOOL` | `u8`, `string`, `bool` |
| Named types | `MY_TYPE` | `my-type` (user-defined) |
| Options | `OPTION_U32` | `option<u32>` |
| Results | `RESULT_STRING_U32` | `result<string, u32>` |
| Tuples | `TUPLE_U32_STRING` | `tuple<u32, string>` |
| Lists | `LIST_U8` | `list<u8>` |
| Unit | `UNIT` | (no payload) |

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
| `cancelRead()` | Cancel an in-progress async read. Returns `{ progress, result }` or `undefined` if the cancel itself blocks. |
| `drop()` | Drop the readable end, releasing the underlying handle. |
| `[Symbol.dispose]()` | Alias for `drop()`. |

### `StreamWritable`

Writable endpoint of a component-model stream.

| Method | Description |
|---|---|
| `write(data)` | Write a single item or array of items. Returns a Promise resolving to the number of items written. |
| `writeAll(buffer)` | Write all items from buffer, calling `write` repeatedly. Returns a Promise resolving to the total count written. |
| `cancelWrite()` | Cancel an in-progress async write. Returns `{ progress, result }` or `undefined` if the cancel itself blocks. |
| `drop()` | Drop the writable end, releasing the underlying handle. |
| `[Symbol.dispose]()` | Alias for `drop()`. |

### `FutureReadable`

Readable endpoint of a component-model future.

| Method | Description |
|---|---|
| `read()` | Read the future value. Returns a Promise that resolves with the value or rejects if the writer was dropped/cancelled. |
| `cancelRead()` | Cancel an in-progress async read. Returns the `CopyResult` code or `undefined` if the cancel blocks. |
| `drop()` | Drop the readable end. |
| `[Symbol.dispose]()` | Alias for `drop()`. |

### `FutureWritable`

Writable endpoint of a component-model future.

| Method | Description |
|---|---|
| `write(value)` | Write a value to the future. Returns a Promise resolving to `true` on success, `false` otherwise. |
| `cancelWrite()` | Cancel an in-progress async write. Returns the `CopyResult` code or `undefined` if the cancel blocks. |
| `drop()` | Drop the writable end. |
| `[Symbol.dispose]()` | Alias for `drop()`. |

---

## Hidden Object Properties

### `__cqjs_handle`

A numeric property set on JS objects that wrap imported or exported WIT
resources. Stores the canonical component-model resource handle (`u32`).

- Set on: Resource wrapper objects during `push_borrow`, `push_own`, and
  `exported_resource_to_handle` calls.
- Read by: `imported_resource_to_handle` and `exported_resource_to_handle`
  to retrieve the canonical handle.
- Removed: When an owned resource is lifted back to JS via `push_own`, the
  property is removed since the handle is no longer valid.

---

## Transient Globals

### `__cqjs_ctor_args` / `__cqjs_ctor_fn`

Temporary globals used during resource constructor calls. These are set on
`globalThis` immediately before evaluating `new __cqjs_ctor_fn(...__cqjs_ctor_args)`
and removed immediately after. They are never visible to user code during
normal execution.

---

## WIT Import/Export Naming

### Import Interfaces

WIT import interfaces are registered as objects on `globalThis` using their
full WIT path:

```js
// Both are set (with and without version):
globalThis["wasi:random/random@0.2.6"]  // full path
globalThis["wasi:random/random"]        // versionless alias
```

### Import Functions

WIT function names are converted from kebab-case to lowerCamelCase:

| WIT Name | JS Name |
|---|---|
| `get-random-bytes` | `getRandomBytes` |
| `my-function` | `myFunction` |

### Import Types

| WIT Category | JS Convention | Example |
|---|---|---|
| Flags | UpperCamelCase object | `MyFlags.FlagA = 1`, `MyFlags.FlagB = 2` |
| Enums | UpperCamelCase object | `MyEnum.VariantA = 0`, `MyEnum[0] = "variant-a"` |
| Variants | UpperCamelCase object | `MyVariant.CaseA = 0`, `MyVariant[0] = "case-a"` |
| Records | camelCase fields | `{ fieldName: value }` |

### Export Functions

Async export functions are looked up from user code using the same
lowerCamelCase convention, optionally nested under a lowerCamelCase
interface name.

### Result / Variant Protocol

WIT `result` and `variant` values are represented as plain objects:

```js
// result<string, u32>
{ tag: "ok", val: "hello" }
{ tag: "err", val: 42 }

// variant (tag is numeric)
{ tag: 0, val: "payload" }
{ tag: 1 }  // no payload case
```
