// SPDX-License-Identifier: CC0-1.0

//! `UpdateEnabledControl`, end to end over HTTP, with `GetEnabledControl` and
//! `ListControlOperations` reading back what it wrote.
//!
//! Driven through the same [`MultiServiceRouter`] the production emulator dispatches with,
//! so what these tests exercise is the wire behaviour a client sees — status codes, error
//! codes and JSON bodies — not the handlers' Rust signatures.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * `UpdateEnabledControl` starts an asynchronous `UPDATE_ENABLED_CONTROL` operation
//!   against the enabled control its identifier names, and once the clock has made that
//!   operation due the control's configured `parameters` are the requested ones — for any
//!   configuration, whatever its entry count, keys or `Document` kinds;
//! * while that operation is in flight the control reads as `UNDER_CHANGE` with the new
//!   operation as its `lastOperationIdentifier` and the configuration it had still
//!   applied, so the update really is asynchronous;
//! * a control whose enablement `SUCCEEDED` must be given a configuration that DIFFERS
//!   from the configured one, where a configuration is the key/value mapping a
//!   `parameters` list denotes and the order the entries were sent in does not distinguish
//!   two of them;
//! * the bound on a control's parameter list is the one rule `EnableControl` applies, so
//!   a list at either cap is accepted from the update too and one past it, or one
//!   repeating a key, is refused with `ValidationException` (400) and writes nothing;
//! * an update that overlaps an operation still changing the control is
//!   `ConflictException` (409), and an identifier no enabled control matches is
//!   `ResourceNotFoundException` (404) — from the update and from `GetEnabledControl`
//!   alike.
//!
//! The enum values written out by hand come from
//! `smithy_query(query="shape", shape="ControlOperationType,ControlOperationStatus,\
//! EnablementStatus,DriftStatus")` on this crate's `smithy-model/`: `ENABLE_CONTROL` |
//! `DISABLE_CONTROL` | `UPDATE_ENABLED_CONTROL` | `RESET_ENABLED_CONTROL`, `SUCCEEDED` |
//! `FAILED` | `IN_PROGRESS`, `SUCCEEDED` | `FAILED` | `UNDER_CHANGE` and `DRIFTED` |
//! `IN_SYNC` | `NOT_CHECKING` | `UNKNOWN`. `enabledControlIdentifier` is always the full
//! ARN `EnableControl` minted: the model constrains it to `Arn` (pattern
//! `^arn:aws[0-9a-zA-Z_\-:\/]+$`, length 20..2048), so no other identifier form reaches
//! the handler.

use smithy_server_runtime::MultiServiceRouter;

use crate::enabled_control_lifecycle::{
    configuration, configured, control, control_operation, details, drift, enable_with, entries,
    last_operation, member, ou, status_summary_status as status,
};
use crate::landing_zone_lifecycle::{
    call, is_operation_identifier, past_the_settle_span, set_up_emulator, Answer,
};

/// POST `/update-enabled-control`. `parameters` is a required member, so every call
/// carries a (possibly empty) list.
async fn update(
    router: &MultiServiceRouter,
    identifier: &str,
    parameters: serde_json::Value,
) -> Answer {
    call(
        router,
        "/update-enabled-control",
        serde_json::json!({
            "enabledControlIdentifier": identifier,
            "parameters": parameters,
        }),
    )
    .await
}

/// One row of the update table.
struct Row {
    case: &'static str,
    control_identifier: String,
    target_identifier: String,
    /// The `parameters` member `EnableControl` is given; `None` omits it, which configures
    /// the control with nothing.
    initial: Option<serde_json::Value>,
    /// The configuration the update asks for.
    requested: serde_json::Value,
}

/// The update table: the rows differ along what the rule generalises over — the entry
/// count on either side of the update (none to one, one to many, many to one, one to
/// none), the `Document` kind a value carries ("a key/value pair, where Key is of type
/// String and Value is of type Document"), and whether a key is added, dropped, kept or
/// re-valued. A configuration names only keys its control declares, so the rows that carry
/// one name the control this emulator's catalog publishes with parameters; every row names
/// its own control/target pair, because a control is enabled at most once per target.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a control configured with nothing is given one parameter",
            control_identifier: control("AWS-GR_REGION_DENY"),
            target_identifier: ou("up01"),
            initial: None,
            requested: serde_json::json!([{ "key": "AllowedRegions", "value": "us-west-2" }]),
        },
        Row {
            case: "one parameter grows into a configuration of every declared key",
            control_identifier: control("AWS-GR_REGION_DENY"),
            target_identifier: ou("up02"),
            initial: Some(
                serde_json::json!([{ "key": "AllowedRegions", "value": "before" }]),
            ),
            requested: serde_json::json!([
                { "key": "AllowedRegions", "value": ["us-west-2", "eu-west-1"] },
                { "key": "ExemptedPrincipalArns", "value": "arn:aws:iam::000000000000:role/r" },
                { "key": "ExemptedActions", "value": { "inner": { "deep": [1, 2, 3] } } }
            ]),
        },
        Row {
            case: "a configuration of several keys is narrowed to one",
            control_identifier: control("AWS-GR_REGION_DENY"),
            target_identifier: ou("up06"),
            initial: Some(serde_json::json!([
                { "key": "AllowedRegions", "value": 1 },
                { "key": "ExemptedPrincipalArns", "value": "two" },
                { "key": "ExemptedActions", "value": [true, false] }
            ])),
            requested: serde_json::json!([
                { "key": "ExemptedPrincipalArns", "value": "two and a half" }
            ]),
        },
        Row {
            case: "an empty configuration clears the one applied",
            control_identifier: control("AWS-GR_REGION_DENY"),
            target_identifier: ou("up03"),
            initial: Some(serde_json::json!([{ "key": "ExemptedActions", "value": true }])),
            requested: serde_json::json!([]),
        },
        Row {
            case: "a value is replaced by one of another document kind",
            control_identifier: control("AWS-GR_REGION_DENY"),
            target_identifier: ou("up04"),
            initial: Some(serde_json::json!([{ "key": "ExemptedActions", "value": true }])),
            requested: serde_json::json!([
                {
                    "key": "ExemptedActions",
                    "value": { "enabled": false, "regions": ["us-west-2"] }
                }
            ]),
        },
        Row {
            case: "a control enabled with an empty list is given a configuration",
            control_identifier: control("AWS-GR_REGION_DENY"),
            target_identifier: ou("up05"),
            initial: Some(serde_json::json!([])),
            requested: serde_json::json!([
                {
                    "key": "ExemptedPrincipalArns",
                    "value": ["arn:aws:iam::123456789012:role/r"]
                }
            ]),
        },
    ]
}

/// RULE: `UpdateEnabledControl` starts an asynchronous `UPDATE_ENABLED_CONTROL` operation
/// against the enabled control its identifier names; while that operation is in flight the
/// control is `UNDER_CHANGE` with the configuration it had still applied, and once the
/// clock has made the operation due the configured parameters are the requested ones and
/// nothing else about the control has moved.
#[tokio::test]
async fn an_update_applies_the_requested_configuration_once_it_is_due() {
    let router = set_up_emulator().await;

    // Every row is its own enablement on its own target, so the rows do not interact.
    let mut enabled: Vec<(Row, String, String)> = Vec::new();
    for row in rows() {
        let (arn, enable_operation) = enable_with(
            &router,
            &row.control_identifier,
            &row.target_identifier,
            row.initial.clone(),
        )
        .await;
        enabled.push((row, arn, enable_operation));
    }

    // The enables are asynchronous too, so wait for them before updating: a control still
    // being deployed refuses an update as a conflict (asserted separately below).
    tokio::time::sleep(past_the_settle_span()).await;

    let mut started: Vec<(Row, String, String)> = Vec::new();
    for (row, arn, enable_operation) in enabled {
        let configured_before = configured(&router, &arn).await;

        let updated = update(&router, &arn, row.requested.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: update expected 200, got {} body {:?}",
            row.case, updated.status, updated.body
        );
        let operation_identifier = updated.string("operationIdentifier");
        assert!(
            is_operation_identifier(&operation_identifier),
            "case {:?}: expected a 36-char lowercase-hex operationIdentifier, got \
             {operation_identifier:?}",
            row.case
        );
        assert_ne!(
            operation_identifier, enable_operation,
            "case {:?}: the update must start its own operation, not report the enable's",
            row.case
        );

        // The update "starts an asynchronous operation", so a caller acting straight after
        // the call sees the transient state: the operation in flight, the control
        // changing, and the OLD configuration still applied.
        let operation = control_operation(&router, &operation_identifier).await;
        assert_eq!(
            (
                member(&operation, "operationType"),
                member(&operation, "status"),
                member(&operation, "controlIdentifier"),
                member(&operation, "targetIdentifier"),
                member(&operation, "enabledControlIdentifier"),
            ),
            (
                Some("UPDATE_ENABLED_CONTROL"),
                Some("IN_PROGRESS"),
                Some(row.control_identifier.as_str()),
                Some(row.target_identifier.as_str()),
                Some(arn.as_str()),
            ),
            "case {:?}: expected an UPDATE_ENABLED_CONTROL record in flight against the \
             control the call named, got {operation:?}",
            row.case
        );

        let in_flight = details(&router, &arn).await;
        assert_eq!(
            (status(&in_flight), last_operation(&in_flight)),
            (Some("UNDER_CHANGE"), Some(operation_identifier.as_str())),
            "case {:?}: expected UNDER_CHANGE with the update as the \
             lastOperationIdentifier, got {in_flight:?}",
            row.case
        );
        assert_eq!(
            configuration(in_flight.get("parameters").unwrap_or(&serde_json::Value::Null)),
            configured_before,
            "case {:?}: expected the configuration still applied while the update is in \
             flight, got {in_flight:?}",
            row.case
        );

        started.push((row, arn, operation_identifier));
    }

    // Nothing sleeps server-side; the wait is the test's.
    tokio::time::sleep(past_the_settle_span()).await;

    for (row, arn, operation_identifier) in &started {
        let operation = control_operation(&router, operation_identifier).await;
        assert_eq!(
            (
                member(&operation, "operationType"),
                member(&operation, "status")
            ),
            (Some("UPDATE_ENABLED_CONTROL"), Some("SUCCEEDED")),
            "case {:?}: expected UPDATE_ENABLED_CONTROL SUCCEEDED once due, got \
             {operation:?}",
            row.case
        );
        assert!(
            operation.get("endTime").is_some(),
            "case {:?}: expected an endTime once the update is due, got {operation:?}",
            row.case
        );

        let applied = details(&router, arn).await;
        assert_eq!(
            configuration(applied.get("parameters").unwrap_or(&serde_json::Value::Null)),
            configuration(&row.requested),
            "case {:?}: expected the requested configuration applied once the update is \
             due, got {applied:?}",
            row.case
        );
        assert_eq!(
            (
                status(&applied),
                last_operation(&applied),
                drift(&applied),
                member(&applied, "arn"),
                member(&applied, "controlIdentifier"),
                member(&applied, "targetIdentifier"),
            ),
            (
                Some("SUCCEEDED"),
                Some(operation_identifier.as_str()),
                Some("IN_SYNC"),
                Some(arn.as_str()),
                Some(row.control_identifier.as_str()),
                Some(row.target_identifier.as_str()),
            ),
            "case {:?}: an update changes the configuration and nothing else, got \
             {applied:?}",
            row.case
        );
    }

    // The log tells the two calls apart: the updates are the UPDATE_ENABLED_CONTROL
    // records and the enables are not.
    let listed = call(
        &router,
        "/list-control-operations",
        serde_json::json!({
            "filter": { "controlOperationTypes": ["UPDATE_ENABLED_CONTROL"] },
            "maxResults": 100,
        }),
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "list-control-operations: expected 200, got {} body {:?}",
        listed.status, listed.body
    );
    let mut listed_updates: Vec<String> = entries(listed.get("controlOperations"))
        .iter()
        .map(|record| {
            member(record, "operationIdentifier")
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    listed_updates.sort();
    let mut expected: Vec<String> = started
        .iter()
        .map(|(_, _, operation)| operation.clone())
        .collect();
    expected.sort();
    assert_eq!(
        listed_updates, expected,
        "expected every update's record and nothing else under \
         UPDATE_ENABLED_CONTROL, got {:?}",
        listed.body
    );
}

/// RULE: a control whose enablement `SUCCEEDED` must be given a configuration that DIFFERS
/// from the configured one, where a configuration is the key/value mapping a `parameters`
/// list denotes — so a list repeating the configured mapping, in any order, is refused with
/// `ValidationException` (400) and writes nothing, while a list that changes any value, adds
/// a key or drops one is accepted.
#[tokio::test]
async fn an_update_to_the_configuration_already_carried_is_refused() {
    // `case`, the configuration the enable applies, a list denoting the SAME
    // configuration, and one denoting a different one.
    let cases: Vec<(&str, Option<serde_json::Value>, serde_json::Value, serde_json::Value)> = vec![
        (
            "a control configured with nothing, asked for nothing",
            None,
            serde_json::json!([]),
            serde_json::json!([{ "key": "AllowedRegions", "value": "us-west-2" }]),
        ),
        (
            "the one configured parameter, sent again",
            Some(serde_json::json!([{ "key": "AllowedRegions", "value": "us-west-2" }])),
            serde_json::json!([{ "key": "AllowedRegions", "value": "us-west-2" }]),
            serde_json::json!([{ "key": "AllowedRegions", "value": "eu-west-1" }]),
        ),
        (
            "the configured parameters in another order, which is the same configuration",
            Some(serde_json::json!([
                { "key": "AllowedRegions", "value": 1 },
                { "key": "ExemptedPrincipalArns", "value": "two" },
                { "key": "ExemptedActions", "value": true }
            ])),
            serde_json::json!([
                { "key": "ExemptedActions", "value": true },
                { "key": "AllowedRegions", "value": 1 },
                { "key": "ExemptedPrincipalArns", "value": "two" }
            ]),
            serde_json::json!([
                { "key": "ExemptedActions", "value": true },
                { "key": "AllowedRegions", "value": 1 }
            ]),
        ),
        (
            "a nested document repeated exactly",
            Some(serde_json::json!([
                { "key": "ExemptedActions", "value": { "inner": { "deep": [1, 2, 3] } } }
            ])),
            serde_json::json!([
                { "key": "ExemptedActions", "value": { "inner": { "deep": [1, 2, 3] } } }
            ]),
            serde_json::json!([
                { "key": "ExemptedActions", "value": { "inner": { "deep": [1, 2, 3, 4] } } }
            ]),
        ),
    ];

    for (index, (case, initial, same, different)) in cases.into_iter().enumerate() {
        let router = set_up_emulator().await;
        let (arn, enable_operation) = enable_with(
            &router,
            &control("AWS-GR_REGION_DENY"),
            &ou(&format!("sa{index:02}")),
            initial.clone(),
        )
        .await;
        tokio::time::sleep(past_the_settle_span()).await;

        let refused = update(&router, &arn, same.clone()).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected 400 ValidationException, got {:?} body {:?}",
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );
        assert!(
            refused.body.get("operationIdentifier").is_none(),
            "case {case:?}: a refused update must mint no operation, got {:?}",
            refused.body
        );

        let untouched = details(&router, &arn).await;
        assert_eq!(
            (
                configuration(untouched.get("parameters").unwrap_or(&serde_json::Value::Null)),
                status(&untouched),
                last_operation(&untouched),
            ),
            (
                configuration(&initial.clone().unwrap_or_else(|| serde_json::json!([]))),
                Some("SUCCEEDED"),
                Some(enable_operation.as_str()),
            ),
            "case {case:?}: a refused update must write nothing, got {untouched:?}"
        );

        // The same control takes a configuration that really differs.
        let accepted = update(&router, &arn, different.clone()).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: a different configuration expected 200, got {} body {:?}",
            accepted.status, accepted.body
        );
        tokio::time::sleep(past_the_settle_span()).await;
        assert_eq!(
            configured(&router, &arn).await,
            configuration(&different),
            "case {case:?}: expected the differing configuration applied"
        );
    }
}

/// RULE: the bound on an enabled control's configuration is one rule, applied wherever a
/// configuration is supplied: a list inside the published weight cap, naming only keys the
/// control declares, is accepted, and one past the cap, naming an undeclared key or
/// repeating a key is refused with `ValidationException` (400) and writes nothing.
///
/// The rows sit inside the cap and past it, vary the shape the weight is carried in (one
/// long value, and several declared keys), name a key the control does not declare and
/// repeat a key — the cases the cap itself does not cover. The cap comes from
/// `service_..._awscontroltowerapis::state`, which is where this emulator publishes it.
#[tokio::test]
async fn the_requested_configuration_is_bounded_the_same_way_an_enablement_is() {
    use service_com_amazonaws_controltower_awscontroltowerapis::state::MAX_ENABLED_CONTROL_PARAMETER_BYTES;

    /// A configuration carrying the first `count` parameters the Region deny control
    /// declares, each with a string value of `value_len` bytes.
    fn parameters(count: usize, value_len: usize) -> serde_json::Value {
        // The keys that control declares, from
        // `service_..._awscontroltowerapis::state::CONTROL_CATALOG`.
        let declared = ["AllowedRegions", "ExemptedPrincipalArns", "ExemptedActions"];
        serde_json::Value::Array(
            declared
                .iter()
                .take(count)
                .map(|key| {
                    serde_json::json!({
                        "key": key,
                        "value": "x".repeat(value_len)
                    })
                })
                .collect(),
        )
    }

    let cases: Vec<(&str, serde_json::Value, bool)> = vec![
        (
            "one parameter whose value sits inside the weight cap",
            parameters(1, MAX_ENABLED_CONTROL_PARAMETER_BYTES - 1024),
            true,
        ),
        (
            "every declared parameter, together inside the weight cap",
            parameters(3, MAX_ENABLED_CONTROL_PARAMETER_BYTES / 4),
            true,
        ),
        (
            "one parameter whose value is past the weight cap",
            parameters(1, MAX_ENABLED_CONTROL_PARAMETER_BYTES + 1),
            false,
        ),
        (
            "declared parameters whose values together pass the weight cap",
            parameters(3, MAX_ENABLED_CONTROL_PARAMETER_BYTES / 2),
            false,
        ),
        (
            "a key the control does not declare",
            serde_json::json!([{ "key": "NoSuchParameter", "value": true }]),
            false,
        ),
        (
            "a list repeating a key",
            serde_json::json!([
                { "key": "AllowedRegions", "value": "us-west-2" },
                { "key": "AllowedRegions", "value": "eu-west-1" }
            ]),
            false,
        ),
    ];

    for (index, (case, requested, accepted)) in cases.into_iter().enumerate() {
        let router = set_up_emulator().await;
        // The control is enabled with one parameter, so no row's list is refused merely
        // for repeating the configured one.
        let (arn, enable_operation) = enable_with(
            &router,
            &control("AWS-GR_REGION_DENY"),
            &ou(&format!("bd{index:02}")),
            Some(serde_json::json!([{ "key": "ExemptedActions", "value": "configured" }])),
        )
        .await;
        tokio::time::sleep(past_the_settle_span()).await;

        let answer = update(&router, &arn, requested.clone()).await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} body {:?}",
                answer.status, answer.body
            );
            tokio::time::sleep(past_the_settle_span()).await;
            assert_eq!(
                configured(&router, &arn).await,
                configuration(&requested),
                "case {case:?}: an accepted update must apply the configuration it carried"
            );
        } else {
            assert_eq!(
                (answer.status, answer.error_code.as_deref()),
                (400, Some("ValidationException")),
                "case {case:?}: expected 400 ValidationException, got {:?} body {:?}",
                (answer.status, answer.error_code.as_deref()),
                answer.body
            );
            let untouched = details(&router, &arn).await;
            assert_eq!(
                (
                    configuration(untouched.get("parameters").unwrap_or(&serde_json::Value::Null)),
                    last_operation(&untouched),
                ),
                (
                    configuration(&serde_json::json!([
                        { "key": "ExemptedActions", "value": "configured" }
                    ])),
                    Some(enable_operation.as_str()),
                ),
                "case {case:?}: a refused update must write nothing, got {untouched:?}"
            );
        }
    }
}

/// RULE: an update that overlaps an operation still changing the control is refused with
/// the modelled `ConflictException` (409) and writes nothing, whichever call started that
/// operation; once the clock has made it due the same request is accepted.
#[tokio::test]
async fn an_update_overlapping_an_operation_in_flight_is_a_conflict() {
    for (case, start_a_second_operation) in [
        ("while the enable is in flight", false),
        ("while another update is in flight", true),
    ] {
        let router = set_up_emulator().await;
        let (arn, _) = enable_with(
            &router,
            &control("AWS-GR_REGION_DENY"),
            &ou(&format!("cf{}", u8::from(start_a_second_operation))),
            Some(serde_json::json!([{ "key": "AllowedRegions", "value": "enabled" }])),
        )
        .await;

        if start_a_second_operation {
            tokio::time::sleep(past_the_settle_span()).await;
            let first = update(
                &router,
                &arn,
                serde_json::json!([{ "key": "AllowedRegions", "value": "first update" }]),
            )
            .await;
            assert_eq!(
                first.status, 200,
                "case {case:?}: the first update expected 200, got {} body {:?}",
                first.status, first.body
            );
        }

        let overlapping = serde_json::json!([{ "key": "AllowedRegions", "value": "overlapping" }]);
        let refused = update(&router, &arn, overlapping.clone()).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (409, Some("ConflictException")),
            "case {case:?}: expected 409 ConflictException, got {:?} body {:?}",
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );
        assert!(
            refused.body.get("operationIdentifier").is_none(),
            "case {case:?}: a refused update must mint no operation, got {:?}",
            refused.body
        );

        // Whatever was in flight when the overlapping update arrived, it did not apply
        // the refused configuration.
        tokio::time::sleep(past_the_settle_span()).await;
        assert_ne!(
            configured(&router, &arn).await,
            configuration(&overlapping),
            "case {case:?}: a refused update must apply nothing"
        );

        // Once everything in flight is due, the SAME request is accepted.
        let accepted = update(&router, &arn, overlapping.clone()).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: the same update once nothing is in flight: expected 200, got {} \
             body {:?}",
            accepted.status, accepted.body
        );
        tokio::time::sleep(past_the_settle_span()).await;
        assert_eq!(
            configured(&router, &arn).await,
            configuration(&overlapping),
            "case {case:?}: expected the accepted update applied"
        );
    }
}

/// RULE: an identifier no enabled control matches is `ResourceNotFoundException` (404) from
/// `UpdateEnabledControl` and from `GetEnabledControl` alike — including a well-formed ARN
/// of another resource kind — and such a refusal leaves the control that does exist alone.
#[tokio::test]
async fn an_identifier_no_enabled_control_matches_is_not_found() {
    let router = set_up_emulator().await;
    let (arn, enable_operation) = enable_with(
        &router,
        &control("AWS-GR_REGION_DENY"),
        &ou("nf01"),
        Some(serde_json::json!([{ "key": "AllowedRegions", "value": "me" }])),
    )
    .await;
    tokio::time::sleep(past_the_settle_span()).await;

    // Every value satisfies `Arn`'s pattern and length, so the front door passes them and
    // the handlers are what refuse them.
    let cases: Vec<(&str, String)> = vec![
        (
            "an enabledcontrol ARN no call minted",
            format!(
                "arn:aws:controltower:us-west-2:{}:enabledcontrol/absent-enablement",
                engine::emulator_account_id()
            ),
        ),
        (
            "a well-formed ARN of another resource kind",
            format!(
                "arn:aws:controltower:us-west-2:{}:landingzone/absent",
                engine::emulator_account_id()
            ),
        ),
        (
            "the control ARN rather than the enablement's",
            control("AWS-GR_REGION_DENY"),
        ),
        ("the target ARN rather than the enablement's", ou("nf01")),
    ];

    for (case, identifier) in cases {
        let refused = update(
            &router,
            &identifier,
            serde_json::json!([{ "key": "AllowedRegions", "value": "us-west-2" }]),
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: update expected 404 ResourceNotFoundException, got {:?} body \
             {:?}",
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );

        let read = call(
            &router,
            "/get-enabled-control",
            serde_json::json!({ "enabledControlIdentifier": identifier }),
        )
        .await;
        assert_eq!(
            (read.status, read.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: get expected 404 ResourceNotFoundException, got {:?} body {:?}",
            (read.status, read.error_code.as_deref()),
            read.body
        );
    }

    // None of those refusals disturbed the control that does exist.
    let untouched = details(&router, &arn).await;
    assert_eq!(
        (
            configuration(untouched.get("parameters").unwrap_or(&serde_json::Value::Null)),
            status(&untouched),
            last_operation(&untouched),
        ),
        (
            configuration(&serde_json::json!([{ "key": "AllowedRegions", "value": "me" }])),
            Some("SUCCEEDED"),
            Some(enable_operation.as_str()),
        ),
        "the stored control must be untouched by refusals, got {untouched:?}"
    );
}
