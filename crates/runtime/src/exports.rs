//! Shared export lookup, argument binding, invocation, and async completion.

use heck::ToUpperCamelCase;
use rquickjs::function::{Args, Constructor, Rest};
use rquickjs::{Ctx, Exception, Function, Object, Persistent, Value};
use wit_dylib_ffi::ExportFunction;

use crate::result::{JsCompletion, ResultBoundary};
use crate::trivia::{fn_lookup, iface_lookup};
use crate::wit_imports::{FuncKind, classify};
use crate::{CtxExt, QjsCallContext, coerce_fn};

/// A resolved export with its arguments and JavaScript receiver already bound.
pub(crate) enum ExportCall<'js> {
    /// A freestanding function, resource method, or static method.
    Function(Function<'js>, Args<'js>),
    /// A resource constructor, invoked with JavaScript construction semantics.
    Constructor(Constructor<'js>, Args<'js>),
}

impl<'js> ExportCall<'js> {
    /// Invoke the resolved function or resource constructor.
    pub(crate) fn invoke(self) -> rquickjs::Result<Value<'js>> {
        match self {
            Self::Function(function, args) => function.call_arg(args),
            Self::Constructor(constructor, args) => constructor.construct_args(args),
        }
    }

    /// Invoke an async export and attach its canonical completion callbacks.
    ///
    /// Keep thenable semantics: successful values and rejection reasons pass
    /// through the WIT result boundary when the returned promise settles.
    pub(crate) fn invoke_async(
        self,
        ctx: &Ctx<'js>,
        func: ExportFunction,
    ) -> rquickjs::Result<Value<'js>> {
        let result = self.invoke()?;
        let promise = result
            .as_object()
            .ok_or_else(|| rquickjs::Error::new_from_js("value", "promise"))?;

        let then: Function = promise.get("then")?;

        let resolve = Function::new(
            ctx.clone(),
            coerce_fn(move |ctx: Ctx<'_>, args: Rest<Value<'_>>| {
                let value = args
                    .0
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| Value::new_undefined(ctx.clone()));

                finish_async_export(ctx, func, JsCompletion::Return(value))
            }),
        )?;

        let reject = Function::new(
            ctx.clone(),
            coerce_fn(move |ctx: Ctx<'_>, args: Rest<Value<'_>>| {
                let reason = args
                    .0
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| Value::new_undefined(ctx.clone()));

                finish_async_export(ctx, func, JsCompletion::Throw(reason))
            }),
        )?;

        let mut args = Args::new(ctx.clone(), 2);
        args.this(result)?;
        args.push_arg(resolve)?;
        args.push_arg(reject)?;
        then.call_arg(args)
    }
}

/// Lower one promise settlement, release argument guards, and return to the host.
///
/// Cancellation owns task completion once requested. Otherwise, keep the result
/// conversion context alive until `task.return` has consumed its transfers.
fn finish_async_export<'js>(
    ctx: Ctx<'js>,
    func: ExportFunction,
    compl: JsCompletion<'js>,
) -> rquickjs::Result<Value<'js>> {
    if ctx.task().is_cancelling() {
        return Ok(Value::new_undefined(ctx));
    }

    let boundary = ResultBoundary::new(func.result());
    let maybe_val = match compl {
        JsCompletion::Return(value) => boundary.lower_value(&ctx, value),
        JsCompletion::Throw(reason) => boundary.lower_throw(&ctx, reason),
    };

    let val = maybe_val.expect("Failed to complete");
    let mut call = QjsCallContext::default();

    if let Some(val) = val {
        call.push_value(&ctx, val);
    }

    ctx.task().finish_export();
    func.call_task_return(&mut call);
    call.complete_transfers(1);
    Ok(Value::new_undefined(ctx))
}

/// Resolve an export's namespace/member and bind its receiver and arguments.
///
/// Resource methods consume the first canonical argument as `this`; static
/// methods bind their class instead. Other exports preserve every argument.
pub(crate) fn prepare_export<'js>(
    ctx: &Ctx<'js>,
    func: ExportFunction,
    mut values: impl ExactSizeIterator<Item = Persistent<Value<'static>>>,
) -> rquickjs::Result<ExportCall<'js>> {
    let kind = classify(func.name());

    let receiver = if matches!(kind, FuncKind::Method { .. }) {
        let recv = values
            .next()
            .ok_or_else(|| Exception::throw_type(ctx, "no resource method receiver"))?
            .restore(ctx)?;
        Some(recv)
    } else {
        None
    };

    let mut args = Args::new(ctx.clone(), values.len());

    for val in values {
        args.push_arg(val.restore(ctx)?)?;
    }

    match kind {
        FuncKind::Constructor { resource } => {
            let scope = export_scope(ctx, func.interface())?;
            let ctor = scope.get(resource.to_upper_camel_case())?;
            Ok(ExportCall::Constructor(ctor, args))
        }
        FuncKind::Method { method, .. } => {
            let recv = receiver.expect("method receiver was consumed above");
            let obj = recv.as_object().ok_or_else(|| {
                Exception::throw_type(ctx, "resource method receiver is not an object")
            })?;
            let func = obj.get(fn_lookup(ctx, method))?;
            args.this(recv)?;
            Ok(ExportCall::Function(func, args))
        }
        FuncKind::Static { resource, method } => {
            let scope = export_scope(ctx, func.interface())?;
            let class: Object = scope.get(resource.to_upper_camel_case())?;
            let function = class.get(fn_lookup(ctx, method))?;
            args.this(class)?;
            Ok(ExportCall::Function(function, args))
        }
        FuncKind::Freestanding => {
            let scope = export_scope(ctx, func.interface())?;
            let function = scope.get(fn_lookup(ctx, func.name()))?;
            Ok(ExportCall::Function(function, args))
        }
    }
}

/// Select the root or interface-scoped user module namespace.
fn export_scope<'js>(ctx: &Ctx<'js>, ifc: Option<&'static str>) -> rquickjs::Result<Object<'js>> {
    let exports = ctx.user_module().exports(ctx)?;

    match ifc {
        Some(ifc) => exports.get(iface_lookup(ctx, ifc)),
        None => Ok(exports),
    }
}
