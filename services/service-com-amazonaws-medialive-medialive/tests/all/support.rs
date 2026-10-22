// SPDX-License-Identifier: CC0-1.0

//! The template-group helpers the group suites share, layered on the crate's one
//! wire harness ([`crate::wire`]).
//!
//! [`timestamp`] and [`member`] read a response member for EVERY group suite, so
//! there is one reading of a wire timestamp in this crate rather than one per
//! suite, and [`reported_tags`] states once what a response's `Tags` member is.
//! The request helpers here are the cloudwatch alarm template group's own
//! (the eventbridge suite keeps its own URI and calls); a third suite should lift
//! what it would otherwise copy into this module rather than add a third copy.
//!
//! No test lives here and no request is built here: the dispatch, the signing
//! and the error-code reading all stay in `wire`, so this module cannot drift
//! from how every other suite reaches the emulator. What it adds is the group
//! collection's URI, its two implemented operations as one call each, the
//! label encoder every `@httpLabel` suite shares ([`percent_encoded`]), the
//! readers the group assertions need, the two clock readings a timestamp
//! assertion compares ([`epoch_seconds`], [`unix_seconds`]) and the one channel
//! a signal map's discovery is entered from ([`entry_point`]).

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;

use crate::wire::{call, call_in_region};

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

/// Percent-encode one URI path segment.
///
/// `Identifier` and `ResourceArn` are `@httpLabel`s, so a label that is not made
/// of URI-unreserved characters travels percent-encoded. Encoding every
/// non-alphanumeric byte is a superset of what a label needs (it also covers the
/// `:` and `/` of an ARN), and the `percent-encoding` crate owns the encoding
/// itself.
pub fn percent_encoded(label: &str) -> String {
    percent_encoding::utf8_percent_encode(label, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// A timestamp member, read as the instant its RFC 3339 / ISO-8601 string names.
///
/// The model targets every timestamp member of this service at
/// `__timestampIso8601`, which carries `@timestampFormat("date-time")`, so the only
/// wire form that is the modelled one is a date-time STRING: the restJson1 default
/// (a JSON number of epoch seconds) fails here rather than being accepted, which is
/// what makes every suite that reads a timestamp through this helper pin the form as
/// well as the instant.
pub fn timestamp(case: &str, member: &str, body: &Value) -> DateTime {
    match &body[member] {
        Value::String(text) => DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
            panic!("case {case:?}: {member} {text:?} is not an ISO-8601 date-time: {error}")
        }),
        other => panic!(
            "case {case:?}: expected {member} as an ISO-8601 date-time string, got {other:?}"
        ),
    }
}

/// The `tags` member a response reports for a resource created with `requested` tags.
///
/// `Tags` targets `TagMap` and is always reported as the map it is modelled as, so a
/// resource created with no tags at all reports the EMPTY map rather than omitting the
/// member: zero tags is an empty tag set, not an unknown one.
pub fn reported_tags(requested: Option<&Value>) -> Value {
    requested.cloned().unwrap_or_else(|| json!({}))
}

/// A member the request did not supply is absent from the response, which reads
/// the same as JSON null here.
pub fn member(body: &Value, name: &str) -> Value {
    body.get(name).cloned().unwrap_or(Value::Null)
}

/// A wire timestamp as seconds since the Unix epoch, so it can be compared with a
/// [`SystemTime`] the test took around the call.
pub fn epoch_seconds(stamp: DateTime) -> f64 {
    stamp.secs() as f64 + f64::from(stamp.subsec_nanos()) / 1e9
}

/// The same reading of a [`SystemTime`] the test took itself.
pub fn unix_seconds(instant: SystemTime) -> f64 {
    instant
        .duration_since(UNIX_EPOCH)
        .expect("the test clock is after the Unix epoch")
        .as_secs_f64()
}

/// One channel named `name`, answering its ARN — the top-level resource a signal
/// map's `DiscoveryEntryPointArn` names.
pub async fn entry_point(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/prod/channels",
        Some(json!({ "name": name })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: expected CreateChannel to answer 201, got {status}: {created}"
    );
    created["channel"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}
