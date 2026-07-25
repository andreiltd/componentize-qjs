//! Shared decoding for tagged JavaScript representations of WIT values.

use rquickjs::{Ctx, Exception, Value};

/// Decode a `{ tag, val? }` JavaScript representation against exact WIT cases.
///
/// Returns the matched case's zero-based discriminant and reads `val` only
/// when that case carries a payload. Unknown tags produce a JavaScript
/// `TypeError` rather than being coerced to another case.
pub(crate) fn decode_tagged<'js, 'case>(
    ctx: &Ctx<'js>,
    value: Value<'js>,
    kind: &'static str,
    cases: impl IntoIterator<Item = (&'case str, bool)>,
) -> rquickjs::Result<(usize, Option<Value<'js>>)> {
    let object = value
        .as_object()
        .ok_or_else(|| rquickjs::Error::new_from_js(value.type_of().as_str(), kind))?;

    let tag: String = object.get("tag")?;

    let Some((discriminant, has_payload)) =
        cases
            .into_iter()
            .enumerate()
            .find_map(|(discriminant, (name, has_payload))| {
                (tag == name).then_some((discriminant, has_payload))
            })
    else {
        return Err(Exception::throw_type(
            ctx,
            &format!("unknown {kind} tag: {tag}"),
        ));
    };

    let payload = if has_payload {
        Some(object.get("val")?)
    } else {
        None
    };

    Ok((discriminant, payload))
}
