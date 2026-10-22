// SPDX-License-Identifier: CC0-1.0

//! The template-group helpers the group suites share, layered on the crate's one
//! wire harness ([`crate::wire`]).
//!
//! [`timestamp`] and [`member`] read a response member for EVERY group suite, so
//! there is one reading of a wire timestamp in this crate rather than one per
//! suite. The request helpers here are the cloudwatch alarm template group's own
//! (the eventbridge suite keeps its own URI and calls); a third suite should lift
//! what it would otherwise copy into this module rather than add a third copy.
//!
//! No test lives here and no request is built here: the dispatch, the signing
//! and the error-code reading all stay in `wire`, so this module cannot drift
//! from how every other suite reaches the emulator. What it adds is the group
//! collection's URI, its two implemented operations as one call each, and the
//! readers the group assertions need.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;

use crate::wire::call_in_region;

/// The collection URI `CreateCloudWatchAlarmTemplateGroup` is bound to
/// (`smithy_query(query="operation",
/// operation="CreateCloudWatchAlarmTemplateGroup")`: POST
/// /prod/cloudwatch-alarm-template-groups).
pub const GROUPS: &str = "/prod/cloudwatch-alarm-template-groups";

/// `CreateCloudWatchAlarmTemplateGroup`, signed for `region`.
pub async fn create_in(
    router: &MultiServiceRouter,
    region: &str,
    body: Value,
) -> (u16, Value) {
    call_in_region(router, region, "POST", GROUPS, Some(body)).await
}

/// `GetCloudWatchAlarmTemplateGroup` on `identifier`, signed for `region`.
pub async fn get_in(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "GET",
        &format!("{GROUPS}/{}", percent_encoded(identifier)),
        None,
    )
    .await
}

/// `Identifier` is an `@httpLabel`, so an identifier that is not made of
/// URI-unreserved characters travels percent-encoded. Encoding every
/// non-alphanumeric byte is a superset of what the label needs, and the
/// `percent-encoding` crate owns the encoding itself.
fn percent_encoded(label: &str) -> String {
    percent_encoding::utf8_percent_encode(label, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// A timestamp member, read as an instant whichever rendering the wire used.
///
/// The model targets these members at `__timestampIso8601`, which carries
/// `@timestampFormat("date-time")`, but the vendored schema codegen never emits
/// `with_timestamp_format`, so the restJson1 JSON codec falls back to its
/// protocol default and writes epoch seconds. `vendor/` is outside what this fix
/// may change, so this accepts either rendering and asserts on the INSTANT.
pub fn timestamp(case: &str, member: &str, body: &Value) -> DateTime {
    match &body[member] {
        Value::String(text) => DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
            panic!("case {case:?}: {member} {text:?} is not an ISO8601 timestamp: {error}")
        }),
        Value::Number(seconds) => DateTime::from_secs_f64(seconds.as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: {member} {seconds} is not a number of seconds")
        })),
        other => panic!("case {case:?}: expected a timestamp for {member}, got {other:?}"),
    }
}

/// A member the request did not supply is absent from the response, which reads
/// the same as JSON null here.
pub fn member(body: &Value, name: &str) -> Value {
    body.get(name).cloned().unwrap_or(Value::Null)
}
