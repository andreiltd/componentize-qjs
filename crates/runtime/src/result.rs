//! Top-level WIT `result` boundary semantics.
//!
//! Nested WIT `result` values use the normal tagged-object representation.
//! Function returns, however, follow the JCO/ComponentizeJS convention:
//! `ok` is returned/resolved and `err` is thrown/rejected.

use rquickjs::function::Args;
use rquickjs::object::Property;
use rquickjs::{
    CatchResultExt, CaughtError, CaughtResult, Constructor, Ctx, Function, Object, Persistent,
    Result, Value,
};
use wit_dylib_ffi::{Type, WitResult};

use crate::{reject_promise, resolve_promise};

#[derive(Clone, Copy)]
enum ReturnShape {
    None,
    Plain,
    Result(WitResult),
}

/// JavaScript control-flow produced by lifting a top-level WIT `result`.
pub(crate) enum JsCompletion<'js> {
    /// Return/resolve with this value.
    Return(Value<'js>),
    /// Throw/reject with this value.
    Throw(Value<'js>),
}

impl<'js> JsCompletion<'js> {
    /// Convert into a synchronous JS return or throw.
    pub(crate) fn into_result(self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        match self {
            Self::Return(value) => Ok(value),
            Self::Throw(reason) => Err(ctx.throw(reason)),
        }
    }

    /// Resolve or reject an in-context promise pair.
    pub(crate) fn settle(self, resolve: &Function<'js>, reject: &Function<'js>) -> Result<()> {
        match self {
            Self::Return(value) => {
                resolve.call::<_, Value>((value,))?;
            }
            Self::Throw(reason) => {
                reject.call::<_, Value>((reason,))?;
            }
        }
        Ok(())
    }

    /// Resolve or reject a promise pair saved across an async import boundary.
    pub(crate) fn settle_persistent(
        self,
        ctx: &Ctx<'js>,
        resolve: Persistent<Value<'static>>,
        reject: Persistent<Value<'static>>,
    ) {
        match self {
            JsCompletion::Return(result) => {
                resolve_promise(resolve, Some(Persistent::save(ctx, result)));
            }
            JsCompletion::Throw(reason) => {
                reject_promise(reject, Persistent::save(ctx, reason));
            }
        }
    }
}

/// Adapter between canonical tagged `result` values and JS return/throw.
#[derive(Clone, Copy)]
pub(crate) struct ResultBoundary {
    shape: ReturnShape,
}

impl ResultBoundary {
    /// Create a boundary for a function return type.
    pub(crate) fn new(result: Option<Type>) -> Self {
        let shape = match result.map(resolve_alias) {
            Some(Type::Result(result)) => ReturnShape::Result(result),
            Some(_) => ReturnShape::Plain,
            None => ReturnShape::None,
        };

        Self { shape }
    }

    /// Lift a canonical import return into JS return/throw control flow.
    pub(crate) fn lift<'js>(
        &self,
        ctx: &Ctx<'js>,
        value: Option<Value<'js>>,
    ) -> Result<JsCompletion<'js>> {
        let value = value.unwrap_or_else(|| Value::new_undefined(ctx.clone()));
        let ReturnShape::Result(result_ty) = self.shape else {
            return Ok(JsCompletion::Return(value));
        };

        let obj = value
            .as_object()
            .ok_or_else(|| rquickjs::Error::new_from_js(value.type_of().as_str(), "result"))?;

        let tag: String = obj.get("tag")?;
        let is_err = tag != "ok";

        let has_payload = if is_err {
            result_ty.err().is_some()
        } else {
            result_ty.ok().is_some()
        };

        let payload = if has_payload {
            obj.get("val")
                .unwrap_or_else(|_| Value::new_undefined(ctx.clone()))
        } else {
            Value::new_undefined(ctx.clone())
        };

        if is_err {
            Ok(JsCompletion::Throw(component_error_value(ctx, payload)?))
        } else {
            Ok(JsCompletion::Return(payload))
        }
    }

    /// Lower a synchronous JS export call into a canonical return value.
    pub(crate) fn lower_call<'js>(
        &self,
        ctx: &Ctx<'js>,
        result: Result<Value<'js>>,
    ) -> CaughtResult<'js, Option<Value<'js>>> {
        self.lower_caught(ctx, result.catch(ctx))
    }

    /// Lower a rejected async JS export into a canonical return value.
    pub(crate) fn lower_throw<'js>(
        &self,
        ctx: &Ctx<'js>,
        reason: Value<'js>,
    ) -> CaughtResult<'js, Option<Value<'js>>> {
        self.lower_caught(ctx, Err(CaughtError::Value(reason)))
    }

    /// Lower a fulfilled async JS export into a canonical return value.
    pub(crate) fn lower_value<'js>(
        &self,
        ctx: &Ctx<'js>,
        value: Value<'js>,
    ) -> CaughtResult<'js, Option<Value<'js>>> {
        match self.shape {
            ReturnShape::None => Ok(None),
            ReturnShape::Plain => Ok(Some(value)),
            ReturnShape::Result(result_ty) => Ok(Some(tagged_ok(ctx, result_ty, value)?)),
        }
    }

    fn lower_caught<'js>(
        &self,
        ctx: &Ctx<'js>,
        result: CaughtResult<'js, Value<'js>>,
    ) -> CaughtResult<'js, Option<Value<'js>>> {
        let ReturnShape::Result(result_ty) = self.shape else {
            return match (self.shape, result) {
                (ReturnShape::None, Ok(_)) => Ok(None),
                (ReturnShape::Plain, Ok(value)) => Ok(Some(value)),
                (ReturnShape::Result(_), _) => unreachable!(),
                (_, Err(err)) => Err(err),
            };
        };

        match result {
            Ok(value) => Ok(Some(tagged_ok(ctx, result_ty, value)?)),
            Err(err) => Ok(Some(tagged_err(ctx, result_ty, err)?)),
        }
    }
}

fn resolve_alias(mut ty: Type) -> Type {
    while let Type::Alias(alias) = ty {
        ty = alias.ty();
    }

    ty
}

fn tagged_ok<'js>(
    ctx: &Ctx<'js>,
    ty: WitResult,
    payload: Value<'js>,
) -> CaughtResult<'js, Value<'js>> {
    let obj = Object::new(ctx.clone()).map_err(|err| CaughtError::from_error(ctx, err))?;
    obj.set("tag", "ok")
        .map_err(|err| CaughtError::from_error(ctx, err))?;

    if ty.ok().is_some() {
        obj.set("val", payload)
            .map_err(|err| CaughtError::from_error(ctx, err))?;
    }

    Ok(obj.into_value())
}

fn tagged_err<'js>(
    ctx: &Ctx<'js>,
    ty: WitResult,
    err: CaughtError<'js>,
) -> CaughtResult<'js, Value<'js>> {
    let payload = result_error_payload(ctx, ty.err(), err)?;
    let obj = Object::new(ctx.clone()).map_err(|err| CaughtError::from_error(ctx, err))?;

    obj.set("tag", "err")
        .map_err(|err| CaughtError::from_error(ctx, err))?;

    if ty.err().is_some() {
        obj.set("val", payload)
            .map_err(|err| CaughtError::from_error(ctx, err))?;
    }

    Ok(obj.into_value())
}

fn result_error_payload<'js>(
    ctx: &Ctx<'js>,
    err_ty: Option<Type>,
    err: CaughtError<'js>,
) -> CaughtResult<'js, Value<'js>> {
    let reason = match err {
        CaughtError::Exception(exception) => exception.into_value(),
        CaughtError::Value(reason) => reason,
        CaughtError::Error(err) => return Err(CaughtError::Error(err)),
    };

    let Some(obj) = reason.as_object() else {
        return Ok(reason);
    };

    if has_own_property(ctx, obj, "payload").map_err(|err| CaughtError::from_error(ctx, err))? {
        return obj
            .get("payload")
            .map_err(|err| CaughtError::from_error(ctx, err));
    }

    if reason.is_error()
        && matches!(err_ty.map(resolve_alias), Some(Type::String))
        && let Ok(message) = obj.get::<_, rquickjs::String>("message")
    {
        return Ok(message.into_value());
    }

    if reason.is_error() {
        return Err(CaughtError::Value(reason));
    }

    Ok(reason)
}

fn has_own_property<'js>(ctx: &Ctx<'js>, obj: &Object<'js>, key: &str) -> Result<bool> {
    let ctor: Object = ctx.globals().get("Object")?;
    let proto: Object = ctor.get("prototype")?;
    let has_own_property: Function = proto.get("hasOwnProperty")?;

    let mut args = Args::new(ctx.clone(), 1);
    args.this(obj.clone())?;
    args.push_arg(key)?;

    has_own_property.call_arg(args)
}

fn component_error_value<'js>(ctx: &Ctx<'js>, payload: Value<'js>) -> Result<Value<'js>> {
    let is_string = payload.is_string();
    let message = if is_string {
        payload.get::<String>()?
    } else {
        let string_fn: Function = ctx.globals().get("String")?;
        let text: String = string_fn.call((payload.clone(),))?;
        format!("{text} (see error.payload)")
    };

    let ctor: Constructor = ctx.globals().get("Error")?;
    let error: Object = ctor.construct((message,))?;

    let payload_prop = if is_string {
        Property::from(payload)
    } else {
        Property::from(payload).enumerable()
    };

    error.prop("payload", payload_prop)?;
    Ok(error.into_value())
}
