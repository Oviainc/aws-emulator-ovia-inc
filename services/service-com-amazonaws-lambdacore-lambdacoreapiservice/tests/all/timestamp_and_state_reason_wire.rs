// SPDX-License-Identifier: CC0-1.0

//! The wire form of this service's `@timestampFormat("date-time")` members, and the
//! `StateReason` that accompanies every reported `State`.
//!
//! Two rules, each asserted over a SET of inputs rather than over one recorded response:
//!
//! 1. A member whose target carries `@timestampFormat("date-time")` goes onto the
//!    restJson1 wire as an RFC 3339 / ISO-8601 date-time STRING, never as the epoch NUMBER
//!    the protocol default would produce. Every timestamp this service models targets
//!    `com.amazonaws.lambdacore#CoreTimestamp`
//!    (`smithy_query(query="shape",shape="CoreTimestamp")` reports
//!    `"timestampFormat": "date-time"`), and the four response shapes that carry one are
//!    `GetNetworkConnectorResponse`, `UpdateNetworkConnectorResponse`,
//!    `NetworkConnectorSummary` and `NetworkConnectorVersionSummary` — so the rows below
//!    span every operation that reports an instant, each lookup form the identifier
//!    accepts, and both list projections.
//!
//! 2. A reported `State` comes with the human-readable `StateReason` for it: the reason
//!    names the event that set that state, changes when another event sets a new one, and
//!    is reported identically whichever identifier form named the resource. The rows span
//!    the states this control plane reaches — `PENDING` while provisioning, `ACTIVE` once
//!    the clock settles it, `DELETING` once a delete starts — and both resources that
//!    model the member (the connector and one of its versions).
//!
//! The harness (router, request helpers, the clock wait) is the sibling suite's; it is
//! `pub(crate)` there so both suites drive the emulator the same way.

use aws_smithy_schema::traits::TimestampFormat;
use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_lambdacore_lambdacoreapiservice::ops::types::{
    GetNetworkConnectorOutput, NetworkConnectorSummary, NetworkConnectorVersionSummary,
    UpdateNetworkConnectorOutput,
};

use super::network_connector_lifecycle::{
    at, create, delete, emulator, expected_arn, get, list, list_versions, minimal_create, text,
    update, vpc_egress, wait_for_the_clock, Answer,
};

/// Assert that the JSON value at `pointer` is the wire form a `date-time` member requires:
/// a STRING that parses as an RFC 3339 instant and renders back to the very same text.
///
/// A bare number is exactly the defect this pins — the restJson1 default for a timestamp
/// whose member schema reports no format — so the string check comes first and names what
/// it saw.
fn assert_iso8601(case: &str, body: &Value, pointer: &str) {
    let value = at(body, pointer);
    let Some(rendered) = value.as_str() else {
        panic!(
            "case {case:?}: {pointer} is modelled @timestampFormat(\"date-time\"), so it must \
             be an ISO-8601 STRING, got {value:?}"
        );
    };
    let parsed = DateTime::from_str(rendered, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: {pointer} must parse as an RFC 3339 date-time ({error}), got {rendered:?}")
    });
    let canonical = parsed
        .fmt(Format::DateTime)
        .unwrap_or_else(|error| panic!("case {case:?}: {pointer} must re-render ({error})"));
    assert_eq!(
        canonical, rendered,
        "case {case:?}: {pointer} must already be in the canonical date-time form, got \
         {rendered:?} which renders as {canonical:?}"
    );
    assert!(
        rendered.contains('T') && rendered.ends_with('Z'),
        "case {case:?}: {pointer} must be a UTC ISO-8601 date-time (T separator, Z offset), \
         got {rendered:?}"
    );
}

/// The `StateReason` an answer reports for the `State` at the same level.
fn state_and_reason(case: &str, body: &Value, prefix: &str) -> (String, String) {
    let state = text(body, &format!("{prefix}/State"));
    assert!(
        !state.is_empty(),
        "case {case:?}: {prefix}/State must be reported, got {:?}",
        at(body, &format!("{prefix}/State"))
    );
    let reason = at(body, &format!("{prefix}/StateReason"));
    let Some(reason) = reason.as_str() else {
        panic!(
            "case {case:?}: {prefix}/State is {state:?}, so {prefix}/StateReason must explain \
             it, got {reason:?}"
        );
    };
    assert!(
        !reason.trim().is_empty(),
        "case {case:?}: {prefix}/StateReason must be human-readable text, got {reason:?}"
    );
    // Every `NetworkConnectorStateReasonCode` value names a provisioning FAILURE
    // (`InvalidSubnet`, `InternalError`, …), which this emulator never produces, so no code
    // describes a state it does reach and the member must stay absent rather than be
    // invented.
    assert_eq!(
        at(body, &format!("{prefix}/StateReasonCode")),
        Value::Null,
        "case {case:?}: {prefix}/StateReasonCode names a failure this emulator never \
         produces, so it must be absent, got {:?}",
        at(body, &format!("{prefix}/StateReasonCode"))
    );
    (state, reason.to_string())
}

/// A configuration that differs from [`minimal_configuration`] in every member, so an
/// update built from it really is a configuration change and appends a version.
fn changed_configuration() -> Value {
    vpc_egress(
        &["subnet-0123456789abcdef0", "subnet-0fedcba9876543210"],
        &["sg-0123456789abcdef0"],
        Some("DualStack"),
        Some("MicroVm"),
    )
}

/// RULE 1 on the wire: every response of this service that reports an instant reports it
/// as an ISO-8601 date-time string.
///
/// The rows differ along what the rule generalises over: the operation (Get, Update, the
/// two Lists), the identifier form (name, id, unqualified ARN, version-qualified ARN) and
/// the shape the member sits in (two response shapes and two summary projections).
#[tokio::test]
async fn every_date_time_member_is_an_iso8601_string_on_the_wire() {
    let router = emulator();
    let name = "wire-shaped";
    let created = create(&router, minimal_create(name)).await;
    assert_eq!(
        created.status, 202,
        "expected HTTP 202 from CreateNetworkConnector, got {} with {:?}",
        created.status, created.body
    );
    let id = text(&created.body, "/Id");
    let arn = expected_arn(name);

    // An update is accepted once provisioning has settled the connector to ACTIVE, and it
    // is what gives the versions list a second row.
    wait_for_the_clock();
    let updated = update(&router, name, json!({ "Configuration": changed_configuration() })).await;
    assert_eq!(
        updated.status, 202,
        "expected HTTP 202 from UpdateNetworkConnector, got {} with {:?}",
        updated.status, updated.body
    );
    wait_for_the_clock();

    let by_name = get(&router, name).await;
    let by_id = get(&router, &id).await;
    let by_arn = get(&router, &arn).await;
    let by_version_arn = get(&router, &format!("{arn}:1")).await;
    let listed = list(&router, "").await;
    let versions = list_versions(&router, name, "").await;

    let rows: Vec<(&str, &Answer, &str)> = vec![
        ("GetNetworkConnector by name", &by_name, "/LastModified"),
        ("GetNetworkConnector by id", &by_id, "/LastModified"),
        (
            "GetNetworkConnector by unqualified ARN",
            &by_arn,
            "/LastModified",
        ),
        (
            "GetNetworkConnector by version-qualified ARN",
            &by_version_arn,
            "/LastModified",
        ),
        ("UpdateNetworkConnector", &updated, "/LastModified"),
        (
            "ListNetworkConnectors summary",
            &listed,
            "/NetworkConnectors/0/LastModified",
        ),
        (
            "ListNetworkConnectorVersions, version 1",
            &versions,
            "/NetworkConnectorVersions/0/LastModified",
        ),
        (
            "ListNetworkConnectorVersions, version 2",
            &versions,
            "/NetworkConnectorVersions/1/LastModified",
        ),
    ];

    for (case, answer, pointer) in rows {
        assert_eq!(
            answer.status / 100,
            2,
            "case {case:?}: expected a 2xx answer, got {} with {:?}",
            answer.status,
            answer.body
        );
        assert_iso8601(case, &answer.body, pointer);
    }

    // The three equivalent lookup forms describe one record, so they report one instant.
    for (case, answer) in [("by id", &by_id), ("by unqualified ARN", &by_arn)] {
        assert_eq!(
            at(&answer.body, "/LastModified"),
            at(&by_name.body, "/LastModified"),
            "case {case:?}: an equivalent lookup reports the same LastModified as the \
             lookup by name, got {:?}",
            at(&answer.body, "/LastModified")
        );
    }
}

/// RULE 1 on the schema: every member the model marks `@timestampFormat("date-time")`
/// carries that format on its member schema, which is what the JSON codec reads to pick
/// the wire form — and a member the model does not mark carries none.
///
/// The marked members are every `CoreTimestamp` member of this service's model
/// (`smithy_query(query="shape",shape="CoreTimestamp")` plus `query="shape"` on the four
/// shapes below); `Arn` and `Version` stand for the members that must stay unmarked.
#[test]
fn every_date_time_member_schema_reports_the_modelled_format() {
    let marked = [
        (
            "GetNetworkConnectorResponse$LastModified",
            &GetNetworkConnectorOutput::M_LAST_MODIFIED,
        ),
        (
            "UpdateNetworkConnectorResponse$LastModified",
            &UpdateNetworkConnectorOutput::M_LAST_MODIFIED,
        ),
        (
            "NetworkConnectorSummary$LastModified",
            &NetworkConnectorSummary::M_LAST_MODIFIED,
        ),
        (
            "NetworkConnectorVersionSummary$LastModified",
            &NetworkConnectorVersionSummary::M_LAST_MODIFIED,
        ),
    ];
    for (case, schema) in marked {
        let format = schema.timestamp_format().map(|trait_| trait_.format());
        assert!(
            matches!(format, Some(TimestampFormat::DateTime)),
            "case {case:?}: the member schema must carry @timestampFormat(date-time), got \
             {format:?}"
        );
    }

    let unmarked = [
        ("GetNetworkConnectorResponse$Arn", &GetNetworkConnectorOutput::M_ARN),
        (
            "NetworkConnectorVersionSummary$Version",
            &NetworkConnectorVersionSummary::M_VERSION,
        ),
    ];
    for (case, schema) in unmarked {
        let format = schema.timestamp_format().map(|trait_| trait_.format());
        assert!(
            format.is_none(),
            "case {case:?}: a member the model does not mark carries no timestamp format, \
             got {format:?}"
        );
    }
}

/// RULE 2: the `State` a read reports comes with the `StateReason` for it — in every state
/// this control plane reaches, through every identifier form, and on a version as well as
/// on the connector.
#[tokio::test]
async fn a_reported_state_always_carries_the_reason_for_it() {
    let router = emulator();
    let name = "state-reasoned";
    let created = create(&router, minimal_create(name)).await;
    assert_eq!(
        created.status, 202,
        "expected HTTP 202 from CreateNetworkConnector, got {} with {:?}",
        created.status, created.body
    );
    let id = text(&created.body, "/Id");
    let arn = expected_arn(name);

    // While provisioning is still owed, the connector is in its transient state — and the
    // reason for it is reported, which is the case the issue's recording covers.
    let pending_forms = [
        ("PENDING, looked up by name", name.to_string()),
        ("PENDING, looked up by id", id.clone()),
        ("PENDING, looked up by unqualified ARN", arn.clone()),
        (
            "PENDING, looked up by version-qualified ARN",
            format!("{arn}:1"),
        ),
    ];
    let mut pending_reason = String::new();
    for (case, identifier) in &pending_forms {
        let answer = get(&router, identifier).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        let (state, reason) = state_and_reason(case, &answer.body, "");
        assert_eq!(
            state, "PENDING",
            "case {case:?}: a connector still provisioning is PENDING, got {state:?}"
        );
        if pending_reason.is_empty() {
            pending_reason = reason;
        } else {
            assert_eq!(
                reason, pending_reason,
                "case {case:?}: every equivalent lookup form reports one record, so one \
                 StateReason, got {reason:?}"
            );
        }
    }

    // Once the clock settles provisioning the state moves, and the reason the connector
    // reached it is still reported with it.
    wait_for_the_clock();
    let settled = get(&router, name).await;
    let (state, active_reason) = state_and_reason("ACTIVE after provisioning", &settled.body, "");
    assert_eq!(
        state, "ACTIVE",
        "provisioning settles a connector to ACTIVE, got {state:?}"
    );

    // An update appends a version. Each version reports its own state with its own reason,
    // and the two were put in that state by different events.
    let updated = update(&router, name, json!({ "Configuration": changed_configuration() })).await;
    assert_eq!(
        updated.status, 202,
        "expected HTTP 202 from UpdateNetworkConnector, got {} with {:?}",
        updated.status, updated.body
    );
    let versions = list_versions(&router, name, "").await;
    let (first_state, first_reason) =
        state_and_reason("version 1", &versions.body, "/NetworkConnectorVersions/0");
    let (second_state, second_reason) =
        state_and_reason("version 2", &versions.body, "/NetworkConnectorVersions/1");
    assert_eq!(
        first_state, "ACTIVE",
        "version 1 finished provisioning before the update, got {first_state:?}"
    );
    assert_eq!(
        second_state, "PENDING",
        "the version an update appends provisions in turn, got {second_state:?}"
    );
    assert_eq!(
        first_reason, active_reason,
        "version 1 and the connector were put in their state by the same creation, so they \
         report the same reason, got {first_reason:?} and {active_reason:?}"
    );
    assert_ne!(
        second_reason, first_reason,
        "a version appended by an update was put in its state by that update, not by the \
         creation, so its reason differs, got {second_reason:?}"
    );

    // A delete moves the connector to its own state, with the reason for THAT state.
    let deleted = delete(&router, name).await;
    assert_eq!(
        deleted.status, 202,
        "expected HTTP 202 from DeleteNetworkConnector, got {} with {:?}",
        deleted.status, deleted.body
    );
    let deleting = get(&router, name).await;
    let (state, deleting_reason) = state_and_reason("DELETING after a delete", &deleting.body, "");
    assert_eq!(
        state, "DELETING",
        "a delete moves the connector to DELETING while its resources are cleaned up, got \
         {state:?}"
    );
    assert_ne!(
        deleting_reason, active_reason,
        "the state changed, so the reason naming the event that set it changed too, got \
         {deleting_reason:?}"
    );
}
