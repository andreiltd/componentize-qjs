use crate::CtxExt;
use crate::with_ctx;

use heck::ToLowerCamelCase;
use rquickjs::{Function, Object, Persistent, Result, Value, function::Rest};

/// Coerce closure lifetimes so the returned `Value<'js>` gets the same
/// lifetime as the `Ctx<'js>` argument.
///
/// See: <https://github.com/rust-lang/rust/issues/97362>
pub(crate) fn coerce_fn<F>(f: F) -> F
where
    F: for<'js> Fn(rquickjs::Ctx<'js>, Rest<Value<'js>>) -> Result<Value<'js>>,
{
    f
}

/// Resolve a JS Promise by calling the stored resolve function with the given value.
pub(crate) fn resolve_promise(
    resolve: Persistent<Value<'static>>,
    result: Option<Persistent<Value<'static>>>,
) {
    with_ctx(|ctx| {
        let resolve_val = resolve.restore(ctx).unwrap();
        let resolve_fn = resolve_val.get::<Function>().unwrap();
        let result_val = result.map_or(Value::new_undefined(ctx.clone()), |res| {
            res.restore(ctx).unwrap()
        });

        resolve_fn
            .call::<_, Value>((result_val,))
            .expect("Failed to resolve promise");
    });
}

/// Get `Symbol.for("dispose")` via the rquickjs API.
pub(crate) fn symbol_dispose<'js>(ctx: &rquickjs::Ctx<'js>) -> Result<Value<'js>> {
    let symbol: Object = ctx.globals().get("Symbol")?;
    let for_fn: Function = symbol.get("for")?;
    for_fn.call(("dispose",))
}

/// Convert a WIT function name to lower camel case, caching the result.
pub(crate) fn fn_lookup(ctx: &rquickjs::Ctx<'_>, name: &'static str) -> &'static str {
    let cache = ctx.fns();
    let mut map = cache.0.borrow_mut();

    map.entry(name)
        .or_insert_with(|| Box::leak(name.to_lower_camel_case().into_boxed_str()))
}

/// Extract the short name from a WIT interface path and convert to lower camel case.
pub(crate) fn iface_lookup(ctx: &rquickjs::Ctx<'_>, full_name: &'static str) -> &'static str {
    let short = full_name
        .rsplit_once('/')
        .map_or(full_name, |(_, short)| short);

    let short = short.split('@').next().unwrap_or(short);
    let cache = ctx.fns();
    let mut map = cache.0.borrow_mut();

    map.entry(full_name)
        .or_insert_with(|| Box::leak(short.to_lower_camel_case().into_boxed_str()))
}
