// SPDX-License-Identifier: CC0-1.0

//! An `@httpQuery`-bound TIMESTAMP read in the `epoch-seconds` form, as a wrapper any
//! service can serve its operations behind.
//!
//! # The gap this closes
//!
//! The framework's query-value deserializer picks a timestamp's wire format off the
//! member's schema: `@timestampFormat("epoch-seconds")` reads the number of seconds since
//! the epoch, and a member that declares no format is read in the protocol's default for a
//! URI-bound timestamp, RFC-3339 `date-time`
//! (`vendor/smithy-schema-server/src/http_binding.rs`'s `read_timestamp`). But the schema
//! built from the model carries no member-level `@timestampFormat` at all
//! (`grep -r timestampFormat vendor/smithy-macros/src` finds nothing), so EVERY
//! `@httpQuery` timestamp is read as `date-time` — including the ones whose model declares
//! `epoch-seconds`, and the ones whose own documentation states the epoch-seconds form
//! ("When used in a raw HTTP Query API, it is formatted as epoch time in seconds"). A
//! client sending the form the model describes is then refused, with HTTP 400, before the
//! operation's handler runs.
//!
//! [`EpochSecondsQueryTimestamps`] closes that for the operations of a service that bind a
//! timestamp to the query string: a query value in the epoch-seconds form is rewritten
//! into the RFC-3339 rendering of the SAME instant before the framework's walk reads the
//! request, so the walk binds the instant the client sent and everything downstream —
//! constraint validation, the handler, the response — sees it. A value that is not a
//! number is left exactly as sent, so a client using the `date-time` form still binds, and
//! a value that is neither is refused by the framework's own deserializer as before.
//!
//! The structural fix is in the vendored codegen — emit the member trait into the schema
//! and the deserializer already does the right thing — which cannot be made from a service
//! crate. Until it is, a service whose model binds a timestamp to the query string serves
//! those operations through this wrapper (from its `override_operation` seam) rather than
//! inlining the rewrite again.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::{Schema, ShapeType};
use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, Operation, OperationContext, Service,
};

/// One modelled operation, served with each of its `@httpQuery`-bound timestamps read in
/// the `epoch-seconds` form as well as in the RFC-3339 `date-time` form the framework
/// defaults to.
///
/// Nothing else is re-implemented: the request is handed to the generated
/// [`ErasedOperation::run`], which owns deserialize -> validate -> handle -> serialize,
/// telemetry and every modelled error.
pub struct EpochSecondsQueryTimestamps<O> {
    operation: O,
    /// The `@httpQuery` names of this operation's timestamp members that the generated
    /// schema reads in the default `date-time` format, read off its input schema ONCE at
    /// construction: the schema is a `&'static Schema` baked at codegen, so the answer is
    /// fixed per operation and nothing about it is per-request.
    query_names: Vec<&'static str>,
}

impl<O: Operation> EpochSecondsQueryTimestamps<O> {
    /// Wrap one operation, already constructed from the shared engine.
    ///
    /// An operation that binds NO timestamp to the query string is wrapped harmlessly: it
    /// has no name to rewrite, so every request reaches it untouched.
    pub fn new(operation: O) -> Self {
        Self {
            operation,
            query_names: date_time_query_members(O::INPUT_SCHEMA),
        }
    }
}

impl<O> ErasedOperation for EpochSecondsQueryTimestamps<O>
where
    O: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            match as_date_time_query(req, &self.query_names) {
                // The rewritten request lives for the call it is read by, which is all the
                // framework's walk borrows it for; `ctx` still carries the request parts
                // the client sent, so a handler reading the URI sees the original.
                Some(rewritten) => self.operation.run(svc, &rewritten, ctx).await,
                None => self.operation.run(svc, req, ctx).await,
            }
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not
    /// change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// `req` with every epoch-seconds query value bound to one of the `names` rewritten into
/// its RFC-3339 rendering, or `None` when the request carries no such value and so needs
/// no rewriting at all.
fn as_date_time_query(req: &RtRequest, names: &[&str]) -> Option<RtRequest> {
    if names.is_empty() {
        return None;
    }
    let (path, query) = req.uri().split_once('?')?;

    let mut rewritten = String::with_capacity(query.len());
    let mut changed = false;
    for (index, pair) in query.split('&').enumerate() {
        if index > 0 {
            rewritten.push('&');
        }
        // A modelled `@httpQuery` name is a shape-name-like literal (`startTime`), so it
        // needs no percent-decoding to be recognised; a value that does carry
        // percent-escapes is not a bare number and so is left alone below.
        let rendered = pair
            .split_once('=')
            .filter(|(name, _)| names.iter().any(|bound| bound == name))
            .and_then(|(name, value)| Some((name, as_date_time(value)?)));
        match rendered {
            Some((name, value)) => {
                changed = true;
                rewritten.push_str(name);
                rewritten.push('=');
                rewritten.push_str(&value);
            }
            None => rewritten.push_str(pair),
        }
    }
    if !changed {
        return None;
    }

    let mut normalized = req.try_clone()?;
    normalized.set_uri(format!("{path}?{rewritten}")).ok()?;
    Some(normalized)
}

/// The `@httpQuery` names of `schema`'s timestamp members that are read in the protocol's
/// default `date-time` format — the members an epoch-seconds value has to be rewritten
/// for. A member whose generated schema DOES carry its own `@timestampFormat` is read in
/// THAT format and is left to the framework.
fn date_time_query_members(schema: &'static Schema) -> Vec<&'static str> {
    schema
        .members()
        .iter()
        .filter(|member| matches!(member.shape_type(), ShapeType::Timestamp))
        .filter(|member| member.timestamp_format().is_none())
        .filter_map(|member| member.http_query().map(|query| query.value()))
        .collect()
}

/// The RFC-3339 `date-time` rendering, percent-encoded for a query string, of a query
/// value in the `epoch-seconds` form. `None` for anything else — a value already in
/// `date-time` form, an empty value, a number in a form `epoch-seconds` does not describe,
/// or one that names no instant that can be rendered — each of which is left for the
/// framework's own deserializer to read or refuse.
///
/// The recognised form is exactly the one `epoch-seconds` describes: an optionally negative
/// decimal number, with at most one fractional part (`1698778057`, `1698778057.5`,
/// `-86400`). Nothing wider is read as an instant, so a value in an encoding neither the
/// member's format nor the protocol describes (`1e9`, `+1`, `inf`) is refused rather than
/// quietly accepted as a new wire form.
fn as_date_time(value: &str) -> Option<String> {
    if !is_decimal_number(value) {
        return None;
    }
    let seconds: f64 = value.parse().ok()?;
    if !seconds.is_finite() {
        return None;
    }
    let rendered = DateTime::from_secs_f64(seconds).fmt(Format::DateTime).ok()?;
    // `:` is the only character the rendering carries that a query string reserves.
    Some(rendered.replace(':', "%3A"))
}

/// Whether `value` is a plain decimal number: an optional `-`, at least one digit, and at
/// most one `.` followed by at least one digit.
fn is_decimal_number(value: &str) -> bool {
    let digits = value.strip_prefix('-').unwrap_or(value);
    let (whole, fraction) = match digits.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (digits, None),
    };
    !whole.is_empty()
        && whole.bytes().all(|byte| byte.is_ascii_digit())
        && fraction.is_none_or(|fraction| {
            !fraction.is_empty() && fraction.bytes().all(|byte| byte.is_ascii_digit())
        })
}

#[cfg(test)]
mod tests {
    use super::{as_date_time, is_decimal_number};

    /// The forms `epoch-seconds` describes are rendered as the same instant in RFC-3339;
    /// every other form is left for the framework's deserializer.
    #[test]
    fn only_the_epoch_seconds_form_is_rewritten() {
        // Rows differ along the form of the value: whole seconds, a fractional part, the
        // epoch itself, an instant before it, and six forms that are not the documented
        // one (an RFC-3339 string, scientific notation, a signed number, a bare sign, a
        // word, and nothing at all).
        let rows: [(&str, Option<&str>); 11] = [
            ("1698778057", Some("2023-10-31T18%3A47%3A37Z")),
            ("1698778057.5", Some("2023-10-31T18%3A47%3A37.5Z")),
            ("0", Some("1970-01-01T00%3A00%3A00Z")),
            ("-86400", Some("1969-12-31T00%3A00%3A00Z")),
            ("2023-10-31T19:27:37Z", None),
            ("1.6e9", None),
            ("+1", None),
            ("-", None),
            ("noon", None),
            ("", None),
            ("1698778057.", None),
        ];
        for (value, expected) in rows {
            assert_eq!(
                as_date_time(value).as_deref(),
                expected,
                "case {value:?}: expected {expected:?}, got {:?}",
                as_date_time(value)
            );
        }
    }

    #[test]
    fn a_decimal_number_is_digits_with_at_most_one_point() {
        for value in ["0", "7", "-7", "1698778057", "1698778057.5", "-0.25"] {
            assert!(
                is_decimal_number(value),
                "case {value:?}: expected a decimal number"
            );
        }
        for value in ["", "-", ".", "1.", ".5", "1.2.3", "+1", "1e9", " 1", "1 "] {
            assert!(
                !is_decimal_number(value),
                "case {value:?}: expected NOT a decimal number"
            );
        }
    }
}
