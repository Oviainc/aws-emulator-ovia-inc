// SPDX-License-Identifier: CC0-1.0

//! Control enablement, end to end over HTTP: `EnableControl`, `ListEnabledControls` and
//! `GetEnabledControl`.
//!
//! Driven through the same [`MultiServiceRouter`] the production emulator dispatches
//! with, so what these tests exercise is the wire behaviour a client sees — status
//! codes, error codes and JSON bodies — not the handlers' Rust signatures.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * what `EnableControl` persists is what `ListEnabledControls` and
//!   `GetEnabledControl` report, for any catalogued control, any organizational-unit
//!   target, any parameter list that control declares and any tag map — and the two
//!   reporting operations agree, because they read one record;
//! * an enablement is `UNDER_CHANGE` with `UNKNOWN` drift until the clock makes it due,
//!   then `SUCCEEDED` and `IN_SYNC`, and reading it never changes it;
//! * the listing is the stored set narrowed by `targetIdentifier` and by EVERY member of
//!   `filter`, and paginated by `maxResults`/`nextToken` so a walk returns each control
//!   exactly once;
//! * a control is enabled at most once per target;
//! * a control is deployed into the landing zone's governed regions, whatever that list
//!   is, and into the request's own region when no landing zone is deployed.
//!
//! Every value set written out by hand here names its source: the `EnablementStatus` and
//! `DriftStatus` value sets come from
//! `smithy_query(query="shape", shape="EnablementStatus,DriftStatus")`, and the
//! `maxResults` bounds from `smithy_query(query="operation", operation=
//! "ListEnabledControls")` (`MaxResults`, range 1..200) on this crate's `smithy-model/`.

use smithy_server_runtime::MultiServiceRouter;

use crate::landing_zone_lifecycle::{
    call, emulator, is_operation_identifier, manifest_governing, past_the_settle_span,
    set_up_emulator, set_up_then_decommission, Answer,
};

/// The `ControlIdentifier` of a control, as AWS Control Tower names its controls: the
/// form `arn:aws:controltower:<region>::control/<CONTROL_NAME>`, inside the modelled
/// pattern `^arn:aws[0-9a-zA-Z_\-:\/]+$`.
///
/// `name` must be a control this emulator's catalog publishes
/// (`service_..._awscontroltowerapis::state::CONTROL_CATALOG`), because `EnableControl`
/// resolves the identifier against it; `tests/all/enable_control_validation.rs` is where
/// the identifiers it refuses are asserted.
pub(crate) fn control(name: &str) -> String {
    format!("arn:aws:controltower:us-west-2::control/{name}")
}

/// The AWS Control Catalog ARN of a control, whose resource segment is its opaque
/// `catalog_id`; the AWS Control Tower form, whose resource segment is the control's name,
/// is [`control`]. That catalog is global, so the ARN names neither a region nor an
/// account. The one definition every suite that names a control in this form uses.
pub(crate) fn catalog_arn(catalog_id: &str) -> String {
    format!("arn:aws:controlcatalog:::control/{catalog_id}")
}

/// An organizational-unit `TargetIdentifier`.
pub(crate) fn ou(id: &str) -> String {
    format!("arn:aws:organizations::000000000000:ou/o-emulatororg/ou-abcd-{id}")
}

/// The ARN prefix every enabled control this emulator mints carries: this service's ARN
/// namespace, the request's region and the emulator's canonical account id.
pub(crate) fn enabled_control_prefix() -> String {
    format!(
        "arn:aws:controltower:us-west-2:{}:enabledcontrol/",
        engine::emulator_account_id()
    )
}

/// `EnableControl` with `body`, asserting it succeeded and handing back
/// `(arn, operationIdentifier)`.
pub(crate) async fn enable(
    router: &MultiServiceRouter,
    body: serde_json::Value,
) -> (String, String) {
    let enabled = call(router, "/enable-control", body.clone()).await;
    assert_eq!(
        enabled.status, 200,
        "enable-control {body}: expected 200, got {} body {:?}",
        enabled.status, enabled.body
    );
    (enabled.string("arn"), enabled.string("operationIdentifier"))
}

/// A JSON member as a list, treating an absent member as the empty list: restJson1 may
/// omit an empty list rather than emit `[]`, and both say "no entries".
pub(crate) fn entries(value: &serde_json::Value) -> Vec<serde_json::Value> {
    value.as_array().cloned().unwrap_or_default()
}

/// The `arn` of every entry of a `ListEnabledControls` answer, sorted, so a case can
/// compare SETS without depending on the minted-ARN order.
pub(crate) fn listed_arns(listed: &Answer) -> Vec<String> {
    let mut arns: Vec<String> = entries(listed.get("enabledControls"))
        .iter()
        .map(|entry| {
            entry
                .get("arn")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    arns.sort();
    arns
}

/// The one listed entry carrying `arn`, or `Value::Null`.
pub(crate) fn listed_entry(listed: &Answer, arn: &str) -> serde_json::Value {
    entries(listed.get("enabledControls"))
        .into_iter()
        .find(|entry| entry.get("arn").and_then(|value| value.as_str()) == Some(arn))
        .unwrap_or(serde_json::Value::Null)
}

/// `GetEnabledControl`'s `enabledControlDetails` for `identifier`, asserting it resolved.
pub(crate) async fn details(router: &MultiServiceRouter, identifier: &str) -> serde_json::Value {
    let got = call(
        router,
        "/get-enabled-control",
        serde_json::json!({ "enabledControlIdentifier": identifier }),
    )
    .await;
    assert_eq!(
        got.status, 200,
        "get-enabled-control {identifier}: expected 200, got {} body {:?}",
        got.status, got.body
    );
    got.get("enabledControlDetails").clone()
}

/// `EnableControl` with `parameters` (omitted when `None`), handing back the minted ARN
/// and the enabling operation's identifier.
pub(crate) async fn enable_with(
    router: &MultiServiceRouter,
    control_identifier: &str,
    target_identifier: &str,
    parameters: Option<serde_json::Value>,
) -> (String, String) {
    let mut body = serde_json::json!({
        "controlIdentifier": control_identifier,
        "targetIdentifier": target_identifier,
    });
    if let Some(parameters) = parameters {
        body["parameters"] = parameters;
    }
    enable(router, body).await
}

/// A member of a JSON object, as a `&str`.
pub(crate) fn member<'a>(value: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    value.get(name).and_then(serde_json::Value::as_str)
}

/// `statusSummary.status` of an `EnabledControlDetails`.
pub(crate) fn status_summary_status(details: &serde_json::Value) -> Option<&str> {
    details
        .get("statusSummary")
        .and_then(|summary| member(summary, "status"))
}

/// `statusSummary.lastOperationIdentifier` of an `EnabledControlDetails`.
pub(crate) fn last_operation(details: &serde_json::Value) -> Option<&str> {
    details
        .get("statusSummary")
        .and_then(|summary| member(summary, "lastOperationIdentifier"))
}

/// `driftStatusSummary.driftStatus` of an `EnabledControlDetails`.
pub(crate) fn drift(details: &serde_json::Value) -> Option<&str> {
    details
        .get("driftStatusSummary")
        .and_then(|summary| member(summary, "driftStatus"))
}

/// All THREE drift dimensions a control reports: the summary's own `driftStatus` and the
/// `inheritance`/`resource` members of its `types`.
pub(crate) fn drift_dimensions(
    details: &serde_json::Value,
) -> (Option<&str>, Option<&str>, Option<&str>) {
    let types = details
        .get("driftStatusSummary")
        .and_then(|summary| summary.get("types"));
    (
        drift(details),
        types
            .and_then(|types| types.get("inheritance"))
            .and_then(|inheritance| member(inheritance, "status")),
        types
            .and_then(|types| types.get("resource"))
            .and_then(|resource| member(resource, "status")),
    )
}

/// The configuration a control carries, as the key/value mapping a `parameters` list
/// denotes: the entries sorted by key, so a case compares configurations rather than the
/// order a list happened to be sent in. An absent member reads as the empty
/// configuration.
pub(crate) fn configuration(parameters: &serde_json::Value) -> Vec<serde_json::Value> {
    let mut sorted = entries(parameters);
    sorted.sort_by_key(|entry| {
        entry
            .get("key")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    });
    sorted
}

/// The configuration `GetEnabledControl` reports for `identifier`.
pub(crate) async fn configured(
    router: &MultiServiceRouter,
    identifier: &str,
) -> Vec<serde_json::Value> {
    configuration(
        details(router, identifier)
            .await
            .get("parameters")
            .unwrap_or(&serde_json::Value::Null),
    )
}

/// The control operation log, as `ListControlOperations` reports it, narrowed by `filter`
/// (`Value::Null` asks for the whole log). `GetControlOperation` resolves the same records
/// one at a time; `tests/all/get_control_operation.rs` is where the two readers are
/// asserted to agree.
pub(crate) async fn control_operations(
    router: &MultiServiceRouter,
    filter: serde_json::Value,
) -> Vec<serde_json::Value> {
    let mut body = serde_json::json!({ "maxResults": 100 });
    if !filter.is_null() {
        body["filter"] = filter;
    }
    let listed = call(router, "/list-control-operations", body).await;
    assert_eq!(
        listed.status, 200,
        "list-control-operations: expected 200, got {} body {:?}",
        listed.status, listed.body
    );
    entries(listed.get("controlOperations"))
}

/// The one logged record carrying `operation_identifier`, or `Value::Null`.
pub(crate) async fn control_operation(
    router: &MultiServiceRouter,
    operation_identifier: &str,
) -> serde_json::Value {
    control_operations(router, serde_json::Value::Null)
        .await
        .into_iter()
        .find(|record| member(record, "operationIdentifier") == Some(operation_identifier))
        .unwrap_or(serde_json::Value::Null)
}

/// The `statusSummary`/`driftStatusSummary` pair a control carries while its enablement
/// is at `status` and its three drift dimensions all read `drift`.
fn summaries(
    status: &str,
    drift: &str,
    operation: &str,
) -> (serde_json::Value, serde_json::Value) {
    (
        serde_json::json!({ "status": status, "lastOperationIdentifier": operation }),
        serde_json::json!({
            "driftStatus": drift,
            "types": {
                "inheritance": { "status": drift },
                "resource": { "status": drift }
            }
        }),
    )
}

/// RULE: what `EnableControl` persists is what the two reporting operations return — the
/// control, the target, the parameters in the order they were given, and the status of
/// the enablement it started — whatever the control, target, parameter list or tag map.
#[tokio::test]
async fn an_enabled_control_round_trips_through_list_and_get() {
    let cases: Vec<(&str, String, String, serde_json::Value, serde_json::Value)> = vec![
        (
            "no parameters and no tags, on an OU target",
            control("AWS-GR_ENCRYPTED_VOLUMES"),
            ou("1111"),
            serde_json::Value::Null,
            serde_json::Value::Null,
        ),
        (
            "one declared parameter and tags, on another OU",
            control("AWS-GR_REGION_DENY"),
            ou("1112"),
            serde_json::json!([{ "key": "AllowedRegions", "value": "us-west-2" }]),
            serde_json::json!({ "owner": "platform", "env": "test" }),
        ),
        (
            "every parameter the control declares, spanning the document types",
            control("AWS-GR_REGION_DENY"),
            ou("2222"),
            serde_json::json!([
                { "key": "AllowedRegions", "value": ["us-west-2", "eu-west-1"] },
                { "key": "ExemptedPrincipalArns", "value": "arn:aws:iam::000000000000:role/r" },
                { "key": "ExemptedActions", "value": { "inner": { "deep": [1, 2, 3] } } }
            ]),
            serde_json::Value::Null,
        ),
        (
            "an empty parameter list and an empty tag map",
            control("AWS-GR_MFA_ENABLED_FOR_IAM_CONSOLE_ACCESS"),
            ou("3333"),
            serde_json::json!([]),
            serde_json::json!({}),
        ),
    ];

    for (case, control_identifier, target_identifier, parameters, tags) in cases {
        let router = set_up_emulator().await;

        let mut body = serde_json::json!({
            "controlIdentifier": control_identifier,
            "targetIdentifier": target_identifier,
        });
        if !parameters.is_null() {
            body["parameters"] = parameters.clone();
        }
        if !tags.is_null() {
            body["tags"] = tags.clone();
        }
        let (arn, operation) = enable(&router, body).await;

        assert!(
            arn.starts_with(&enabled_control_prefix()) && arn.len() >= 20,
            "case {case:?}: expected an enabledcontrol ARN under {:?}, got {arn:?}",
            enabled_control_prefix()
        );
        assert!(
            is_operation_identifier(&operation),
            "case {case:?}: expected a 36-char lowercase-hex operationIdentifier, got \
             {operation:?}"
        );

        // The enablement has only just started, so it is still deploying and its drift
        // is not assessable yet.
        let (expected_status, expected_drift) = summaries("UNDER_CHANGE", "UNKNOWN", &operation);

        let listed = call(&router, "/list-enabled-controls", serde_json::json!({})).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: list expected 200, got {} body {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            listed_arns(&listed),
            vec![arn.clone()],
            "case {case:?}: expected exactly the enabled control, got {:?}",
            listed.body
        );

        let summary = listed_entry(&listed, &arn);
        for (member, expected) in [
            ("controlIdentifier", serde_json::json!(control_identifier)),
            ("targetIdentifier", serde_json::json!(target_identifier)),
            ("statusSummary", expected_status.clone()),
            ("driftStatusSummary", expected_drift.clone()),
        ] {
            assert_eq!(
                summary.get(member).cloned().unwrap_or_default(),
                expected,
                "case {case:?}: list entry {member} expected {expected:?}, got {:?}",
                summary.get(member)
            );
        }
        assert!(
            summary.get("parentIdentifier").is_none(),
            "case {case:?}: a control enabled directly on its target inherits from \
             nothing, so it carries no parentIdentifier, got {summary:?}"
        );

        let detail = details(&router, &arn).await;
        for (member, expected) in [
            ("arn", serde_json::json!(arn)),
            ("controlIdentifier", serde_json::json!(control_identifier)),
            ("targetIdentifier", serde_json::json!(target_identifier)),
            ("statusSummary", expected_status),
            ("driftStatusSummary", expected_drift),
        ] {
            assert_eq!(
                detail.get(member).cloned().unwrap_or_default(),
                expected,
                "case {case:?}: get detail {member} expected {expected:?}, got {:?}",
                detail.get(member)
            );
        }
        assert_eq!(
            entries(detail.get("parameters").unwrap_or(&serde_json::Value::Null)),
            entries(&parameters),
            "case {case:?}: expected the parameters EnableControl was given, in order, \
             got {:?}",
            detail.get("parameters")
        );
        // No landing zone is deployed in this case, so the only region the request names
        // is the one it was signed for.
        assert_eq!(
            detail.get("targetRegions").cloned().unwrap_or_default(),
            serde_json::json!([{ "name": "us-west-2" }]),
            "case {case:?}: expected the signed region as the only target region, got {:?}",
            detail.get("targetRegions")
        );
    }
}

/// RULE: an enablement is reported non-terminal (`UNDER_CHANGE`, drift `UNKNOWN`) until
/// the clock makes it due and `SUCCEEDED`/`IN_SYNC` after that — and a read never moves
/// it, so two reads of the same instant agree.
#[tokio::test]
async fn an_enablement_settles_from_under_change_to_succeeded() {
    let router = set_up_emulator().await;
    let (arn, operation) = enable(
        &router,
        serde_json::json!({
            "controlIdentifier": control("AWS-GR_ENCRYPTED_VOLUMES"),
            "targetIdentifier": ou("4444"),
        }),
    )
    .await;

    let (in_flight_status, in_flight_drift) = summaries("UNDER_CHANGE", "UNKNOWN", &operation);
    let (settled_status, settled_drift) = summaries("SUCCEEDED", "IN_SYNC", &operation);

    // Two reads before the span elapses: the second must see what the first did, so
    // reading has not consumed the transient state.
    for read in 0..2 {
        let detail = details(&router, &arn).await;
        assert_eq!(
            detail.get("statusSummary").cloned().unwrap_or_default(),
            in_flight_status,
            "read {read} before the settle span: expected {in_flight_status:?}, got {:?}",
            detail.get("statusSummary")
        );
        assert_eq!(
            detail.get("driftStatusSummary").cloned().unwrap_or_default(),
            in_flight_drift,
            "read {read} before the settle span: expected {in_flight_drift:?}, got {:?}",
            detail.get("driftStatusSummary")
        );
    }

    // The clock makes the enablement due; nothing sleeps server-side, the wait is the
    // caller's.
    tokio::time::sleep(past_the_settle_span()).await;

    for read in 0..2 {
        let detail = details(&router, &arn).await;
        assert_eq!(
            detail.get("statusSummary").cloned().unwrap_or_default(),
            settled_status,
            "read {read} after the settle span: expected {settled_status:?}, got {:?}",
            detail.get("statusSummary")
        );
        assert_eq!(
            detail.get("driftStatusSummary").cloned().unwrap_or_default(),
            settled_drift,
            "read {read} after the settle span: expected {settled_drift:?}, got {:?}",
            detail.get("driftStatusSummary")
        );
    }

    // And the listing reports the same settled record as the single-control read.
    let listed = call(&router, "/list-enabled-controls", serde_json::json!({})).await;
    assert_eq!(
        listed_entry(&listed, &arn)
            .get("statusSummary")
            .cloned()
            .unwrap_or_default(),
        settled_status,
        "the listing must report the same settled status as GetEnabledControl, got {:?}",
        listed.body
    );
}

/// RULE: the listing is the stored set narrowed by `targetIdentifier` and by every
/// member of `filter` — a member the caller omits narrows nothing, one they send keeps
/// the controls whose own value it holds.
///
/// The rows span every filter member, each on a value that selects and a value from the
/// same modelled value set that selects nothing, plus the two `includeChildren`
/// settings: this service stores no organizational hierarchy, so no stored control is a
/// child of another and the flag changes nothing.
#[tokio::test]
async fn the_listing_is_narrowed_by_target_and_by_every_filter_member() {
    let router = set_up_emulator().await;

    let encrypted = control("AWS-GR_ENCRYPTED_VOLUMES");
    let root_user = control("AWS-GR_RESTRICT_ROOT_USER");
    let first_ou = ou("5555");
    let second_target = ou("5556");
    let empty_ou = ou("6666");

    let (on_first_ou, _) = enable(
        &router,
        serde_json::json!({ "controlIdentifier": encrypted, "targetIdentifier": first_ou }),
    )
    .await;
    let (other_control_on_first_ou, _) = enable(
        &router,
        serde_json::json!({ "controlIdentifier": root_user, "targetIdentifier": first_ou }),
    )
    .await;
    let (on_second_target, _) = enable(
        &router,
        serde_json::json!({ "controlIdentifier": encrypted, "targetIdentifier": second_target }),
    )
    .await;

    // Past the settle span, so every control is `SUCCEEDED`/`IN_SYNC` and the status and
    // drift rows below do not race the clock.
    tokio::time::sleep(past_the_settle_span()).await;

    let sorted = |arns: &[&String]| {
        let mut arns: Vec<String> = arns.iter().map(|arn| (*arn).clone()).collect();
        arns.sort();
        arns
    };
    let all = sorted(&[&on_first_ou, &other_control_on_first_ou, &on_second_target]);
    let first_ou_controls = sorted(&[&on_first_ou, &other_control_on_first_ou]);
    let encrypted_controls = sorted(&[&on_first_ou, &on_second_target]);
    let none: Vec<String> = Vec::new();

    let cases: Vec<(&str, serde_json::Value, Vec<String>)> = vec![
        ("no target and no filter lists every control", serde_json::json!({}), all.clone()),
        (
            "targetIdentifier keeps the controls enabled on that target",
            serde_json::json!({ "targetIdentifier": first_ou }),
            first_ou_controls.clone(),
        ),
        (
            "targetIdentifier of the other target",
            serde_json::json!({ "targetIdentifier": second_target }),
            vec![on_second_target.clone()],
        ),
        (
            "targetIdentifier of a target nothing is enabled on",
            serde_json::json!({ "targetIdentifier": empty_ou }),
            none.clone(),
        ),
        (
            "includeChildren false narrows to the target itself",
            serde_json::json!({ "targetIdentifier": first_ou, "includeChildren": false }),
            first_ou_controls.clone(),
        ),
        (
            "includeChildren true adds nothing: no stored control is a child of another",
            serde_json::json!({ "targetIdentifier": first_ou, "includeChildren": true }),
            first_ou_controls.clone(),
        ),
        (
            "filter.controlIdentifiers keeps that control on every target",
            serde_json::json!({ "filter": { "controlIdentifiers": [encrypted] } }),
            encrypted_controls,
        ),
        (
            "filter.controlIdentifiers and targetIdentifier narrow together",
            serde_json::json!({
                "targetIdentifier": first_ou,
                "filter": { "controlIdentifiers": [encrypted] }
            }),
            vec![on_first_ou.clone()],
        ),
        (
            "filter.controlIdentifiers of a control nothing enabled",
            serde_json::json!({ "filter": { "controlIdentifiers": [control("AWS-GR_RDS_STORAGE_ENCRYPTED")] } }),
            none.clone(),
        ),
        // EnablementStatus value set: SUCCEEDED | FAILED | UNDER_CHANGE.
        (
            "filter.statuses SUCCEEDED, the status every settled enablement holds",
            serde_json::json!({ "filter": { "statuses": ["SUCCEEDED"] } }),
            all.clone(),
        ),
        (
            "filter.statuses UNDER_CHANGE, which no settled enablement holds",
            serde_json::json!({ "filter": { "statuses": ["UNDER_CHANGE"] } }),
            none.clone(),
        ),
        (
            "filter.statuses FAILED",
            serde_json::json!({ "filter": { "statuses": ["FAILED"] } }),
            none.clone(),
        ),
        // DriftStatus value set: DRIFTED | IN_SYNC | NOT_CHECKING | UNKNOWN.
        (
            "filter.driftStatuses IN_SYNC",
            serde_json::json!({ "filter": { "driftStatuses": ["IN_SYNC"] } }),
            all.clone(),
        ),
        (
            "filter.driftStatuses DRIFTED",
            serde_json::json!({ "filter": { "driftStatuses": ["DRIFTED"] } }),
            none.clone(),
        ),
        (
            "filter.driftStatuses NOT_CHECKING",
            serde_json::json!({ "filter": { "driftStatuses": ["NOT_CHECKING"] } }),
            none.clone(),
        ),
        (
            "filter.inheritanceDriftStatuses IN_SYNC",
            serde_json::json!({ "filter": { "inheritanceDriftStatuses": ["IN_SYNC"] } }),
            all.clone(),
        ),
        (
            "filter.inheritanceDriftStatuses UNKNOWN",
            serde_json::json!({ "filter": { "inheritanceDriftStatuses": ["UNKNOWN"] } }),
            none.clone(),
        ),
        (
            "filter.resourceDriftStatuses IN_SYNC",
            serde_json::json!({ "filter": { "resourceDriftStatuses": ["IN_SYNC"] } }),
            all.clone(),
        ),
        (
            "filter.resourceDriftStatuses DRIFTED",
            serde_json::json!({ "filter": { "resourceDriftStatuses": ["DRIFTED"] } }),
            none.clone(),
        ),
        (
            "filter.parentIdentifiers keeps only controls that inherit, and none do",
            serde_json::json!({ "filter": { "parentIdentifiers": [on_first_ou] } }),
            none.clone(),
        ),
        (
            "every filter member at once, all selecting",
            serde_json::json!({
                "targetIdentifier": first_ou,
                "filter": {
                    "controlIdentifiers": [encrypted],
                    "statuses": ["SUCCEEDED"],
                    "driftStatuses": ["IN_SYNC"],
                    "inheritanceDriftStatuses": ["IN_SYNC"],
                    "resourceDriftStatuses": ["IN_SYNC"]
                }
            }),
            vec![on_first_ou.clone()],
        ),
        (
            "every filter member at once, one of them selecting nothing",
            serde_json::json!({
                "targetIdentifier": first_ou,
                "filter": {
                    "controlIdentifiers": [encrypted],
                    "statuses": ["SUCCEEDED"],
                    "driftStatuses": ["DRIFTED"]
                }
            }),
            none.clone(),
        ),
    ];

    for (case, body, expected) in cases {
        let listed = call(&router, "/list-enabled-controls", body).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected 200, got {} body {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            listed_arns(&listed),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            listed_arns(&listed)
        );
    }
}

/// RULE: a paginated walk returns every control the narrowing selected exactly once, in
/// pages of at most `maxResults`, and the walk ends when the answer carries no
/// `nextToken`.
///
/// The rows are the page sizes: both published bounds of `MaxResults` (range 1..200),
/// a size that divides the set evenly, one that does not, and one equal to the set.
#[tokio::test]
async fn a_paginated_walk_returns_every_control_exactly_once() {
    let router = set_up_emulator().await;
    let target = ou("7777");
    // Six catalogued controls on one target: one enablement per control/target pair.
    let controls = [
        "AWS-GR_ENCRYPTED_VOLUMES",
        "AWS-GR_RESTRICT_ROOT_USER",
        "AWS-GR_RESTRICTED_COMMON_PORTS",
        "AWS-GR_RDS_STORAGE_ENCRYPTED",
        "AWS-GR_EC2_VOLUME_INUSE_CHECK",
        "AWS-GR_DISALLOW_VPN_CONNECTIONS",
    ];
    let mut expected: Vec<String> = Vec::new();
    for name in controls {
        let (arn, _) = enable(
            &router,
            serde_json::json!({
                "controlIdentifier": control(name),
                "targetIdentifier": target,
            }),
        )
        .await;
        expected.push(arn);
    }
    expected.sort();

    for page_size in [1usize, 2, 4, 6, 200] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut body = serde_json::json!({ "maxResults": page_size });
            if let Some(token) = &token {
                body["nextToken"] = serde_json::json!(token);
            }
            let page = call(&router, "/list-enabled-controls", body).await;
            assert_eq!(
                page.status, 200,
                "page size {page_size}, page {pages}: expected 200, got {} body {:?}",
                page.status, page.body
            );
            let arns = listed_arns(&page);
            assert!(
                arns.len() <= page_size && (!arns.is_empty() || expected.is_empty()),
                "page size {page_size}, page {pages}: expected 1..={page_size} entries, \
                 got {arns:?}"
            );
            walked.extend(arns);
            token = page
                .get("nextToken")
                .as_str()
                .map(|value| value.to_string());
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "page size {page_size}: the walk did not terminate, walked {walked:?}"
            );
            if token.is_none() {
                break;
            }
        }
        walked.sort();
        assert_eq!(
            walked, expected,
            "page size {page_size}: expected every control exactly once, got {walked:?}"
        );
        let full_pages = expected.len().div_ceil(page_size);
        assert_eq!(
            pages, full_pages,
            "page size {page_size}: expected {full_pages} pages for {} controls, got {pages}",
            expected.len()
        );
    }

    // A token this operation never handed out is refused, not silently read as the first
    // page — the same rule `ListLandingZones` applies to its own token.
    let refused = call(
        &router,
        "/list-enabled-controls",
        serde_json::json!({ "nextToken": "not-a-token" }),
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_code.as_deref()),
        (400, Some("ValidationException")),
        "a token the service never handed out: expected 400 ValidationException, got {:?} \
         body {:?}",
        (refused.status, refused.error_code.as_deref()),
        refused.body
    );

    // And an empty token is the first page.
    let first = call(
        &router,
        "/list-enabled-controls",
        serde_json::json!({ "nextToken": "", "maxResults": 1 }),
    )
    .await;
    assert_eq!(
        listed_arns(&first),
        vec![expected[0].clone()],
        "an empty nextToken: expected the first page, got {:?}",
        first.body
    );
}

/// RULE: a control is enabled at most once per target — a repeat while the first
/// enablement is still deploying is a 409 `ConflictException`, a repeat once it is
/// deployed a 400 `ValidationException` — and the rule is per control/target PAIR, so
/// the same control on another target and another control on the same target both
/// succeed.
#[tokio::test]
async fn a_control_is_enabled_at_most_once_per_target() {
    let router = set_up_emulator().await;
    let encrypted = control("AWS-GR_ENCRYPTED_VOLUMES");
    let target = ou("8888");
    let request = serde_json::json!({
        "controlIdentifier": encrypted,
        "targetIdentifier": target,
    });

    let (first, _) = enable(&router, request.clone()).await;

    let while_deploying = call(&router, "/enable-control", request.clone()).await;
    assert_eq!(
        (while_deploying.status, while_deploying.error_code.as_deref()),
        (409, Some("ConflictException")),
        "a repeat while the first enablement is still deploying: expected 409 \
         ConflictException, got {:?} body {:?}",
        (while_deploying.status, while_deploying.error_code.as_deref()),
        while_deploying.body
    );

    // Different pair, same call: both succeed, and each gets its own ARN.
    let (same_control_other_target, _) = enable(
        &router,
        serde_json::json!({
            "controlIdentifier": encrypted,
            "targetIdentifier": ou("8889"),
        }),
    )
    .await;
    let (other_control_same_target, _) = enable(
        &router,
        serde_json::json!({
            "controlIdentifier": control("AWS-GR_RESTRICT_ROOT_USER"),
            "targetIdentifier": target,
        }),
    )
    .await;
    assert!(
        same_control_other_target != first && other_control_same_target != first,
        "each control/target pair is its own enabled control, got {first:?}, \
         {same_control_other_target:?} and {other_control_same_target:?}"
    );

    tokio::time::sleep(past_the_settle_span()).await;

    let once_deployed = call(&router, "/enable-control", request).await;
    assert_eq!(
        (once_deployed.status, once_deployed.error_code.as_deref()),
        (400, Some("ValidationException")),
        "a repeat once the control is deployed: expected 400 ValidationException, got \
         {:?} body {:?}",
        (once_deployed.status, once_deployed.error_code.as_deref()),
        once_deployed.body
    );

    // The refusals changed nothing: the store still holds exactly the three enablements.
    let listed = call(&router, "/list-enabled-controls", serde_json::json!({})).await;
    assert_eq!(
        listed_arns(&listed).len(),
        3,
        "a refused enable must persist nothing, got {:?}",
        listed.body
    );
}

/// RULE: a request this handler cannot satisfy is refused with the operation's own
/// modelled error, naming the member at fault — a repeated parameter key is a 400
/// `ValidationException` and an identifier no enablement minted is a 404
/// `ResourceNotFoundException`.
///
/// The rows also span the refusals that belong to the schema front door rather than to
/// these handlers: a filter value outside a modelled enum's value set is a 400
/// `BadRequestException` before the handler runs (the rule
/// `tests/all/front_door_validation.rs` pins), which is why the handler does not
/// re-check those value sets.
#[tokio::test]
async fn a_request_the_state_cannot_satisfy_is_refused_with_its_modeled_error() {
    let router = set_up_emulator().await;
    let (arn, _) = enable(
        &router,
        serde_json::json!({
            "controlIdentifier": control("AWS-GR_ENCRYPTED_VOLUMES"),
            "targetIdentifier": ou("9999"),
        }),
    )
    .await;

    let cases: Vec<(&str, &str, serde_json::Value, u16, &str)> = vec![
        (
            "filter.statuses value outside the EnablementStatus value set",
            "/list-enabled-controls",
            serde_json::json!({ "filter": { "statuses": ["IN_PROGRESS"] } }),
            400,
            "BadRequestException",
        ),
        (
            "filter.driftStatuses value outside the DriftStatus value set",
            "/list-enabled-controls",
            serde_json::json!({ "filter": { "driftStatuses": ["SUCCEEDED"] } }),
            400,
            "BadRequestException",
        ),
        (
            "filter.inheritanceDriftStatuses value outside the DriftStatus value set",
            "/list-enabled-controls",
            serde_json::json!({ "filter": { "inheritanceDriftStatuses": ["in_sync"] } }),
            400,
            "BadRequestException",
        ),
        (
            "filter.resourceDriftStatuses value outside the DriftStatus value set",
            "/list-enabled-controls",
            serde_json::json!({ "filter": { "resourceDriftStatuses": [""] } }),
            400,
            "BadRequestException",
        ),
        (
            "parameters carrying the same key twice",
            "/enable-control",
            serde_json::json!({
                "controlIdentifier": control("AWS-GR_REGION_DENY"),
                "targetIdentifier": ou("9999"),
                "parameters": [
                    { "key": "AllowedRegions", "value": "us-west-2" },
                    { "key": "AllowedRegions", "value": "eu-west-1" }
                ]
            }),
            400,
            "ValidationException",
        ),
        (
            "an enabledControlIdentifier no enablement minted",
            "/get-enabled-control",
            serde_json::json!({
                "enabledControlIdentifier":
                    format!("{}00000000-0000-4000-8000-000000000000", enabled_control_prefix())
            }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an enabledControlIdentifier that is a well-formed ARN of another service",
            "/get-enabled-control",
            serde_json::json!({
                "enabledControlIdentifier": "arn:aws:organizations::000000000000:ou/o-x/ou-1"
            }),
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (case, path, body, status, code) in cases {
        let refused = call(&router, path, body).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (status, Some(code)),
            "case {case:?}: expected {status} {code}, got {:?} body {:?}",
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );
    }

    // None of those refusals disturbed the one enablement: it still resolves by the ARN
    // `EnableControl` minted, which is the only identifier form `enabledControlIdentifier`
    // (an `Arn`) admits.
    let detail = details(&router, &arn).await;
    assert_eq!(
        detail.get("arn").and_then(|value| value.as_str()),
        Some(arn.as_str()),
        "identifier {arn:?}: expected the stored control, got {detail:?}"
    );
}

/// RULE: an `EnabledControlParameter$value` is an unconstrained `Document`, so
/// `EnableControl` bounds the WEIGHT of the configuration a caller may send: a list inside
/// the cap is accepted and one past it is refused with the operation's own
/// `ValidationException`, naming the bound. The entry COUNT needs no cap of its own: a key
/// outside the control's declared set is refused anyway and each declared key may be given
/// once, which `tests/all/enable_control_validation.rs` asserts.
///
/// The rows sit inside the cap and past it, and vary the shape the weight is carried in
/// (one long value, and several declared keys) so the bound is on the payload rather than
/// on a particular document.
#[tokio::test]
async fn the_parameter_list_is_bounded_in_weight() {
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
    ];

    for (case, parameters, accepted) in cases {
        let router = set_up_emulator().await;
        let answer = call(
            &router,
            "/enable-control",
            serde_json::json!({
                "controlIdentifier": control("AWS-GR_REGION_DENY"),
                "targetIdentifier": ou("1212"),
                "parameters": parameters,
            }),
        )
        .await;

        let listed_after = call(&router, "/list-enabled-controls", serde_json::json!({})).await;
        let listed_after = listed_arns(&listed_after);

        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} body {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                listed_after,
                vec![answer.string("arn")],
                "case {case:?}: an accepted enablement must be listed"
            );
        } else {
            assert_eq!(
                (answer.status, answer.error_code.as_deref()),
                (400, Some("ValidationException")),
                "case {case:?}: expected 400 ValidationException, got {:?}",
                (answer.status, answer.error_code.as_deref())
            );
            assert_eq!(
                listed_after,
                Vec::<String>::new(),
                "case {case:?}: a refused enablement must persist nothing"
            );
        }
    }
}

/// One row of [`target_regions_are_the_governed_regions_of_the_deployed_landing_zone`]:
/// the case name, the `governedRegions` of the landing zone to deploy first (or `None` to
/// deploy none, which is the account set up and decommissioned again), and the target
/// regions the enablement must then report.
type RegionCase<'a> = (&'a str, Option<Vec<&'a str>>, Vec<&'a str>);

/// RULE: a control is deployed into the regions the landing zone governs, whatever that
/// list is — and into the region the request was signed for when no landing zone is
/// deployed to read a governed-region list from.
#[tokio::test]
async fn target_regions_are_the_governed_regions_of_the_deployed_landing_zone() {
    let cases: Vec<RegionCase<'_>> = vec![
        (
            "no landing zone deployed: the request's own region",
            None,
            vec!["us-west-2"],
        ),
        (
            "a landing zone governing one region",
            Some(vec!["eu-central-1"]),
            vec!["eu-central-1"],
        ),
        (
            "a landing zone governing several regions, in its manifest's order",
            Some(vec!["us-west-2", "eu-west-1", "ap-south-1"]),
            vec!["us-west-2", "eu-west-1", "ap-south-1"],
        ),
        (
            "a manifest whose governedRegions list is empty: the request's own region",
            Some(vec![]),
            vec!["us-west-2"],
        ),
    ];

    for (case, governed, expected) in cases {
        let router = emulator();
        match &governed {
            Some(governed) => {
                let created = call(
                    &router,
                    "/create-landingzone",
                    serde_json::json!({
                        "version": "3.3",
                        "manifest": manifest_governing(governed)
                    }),
                )
                .await;
                assert_eq!(
                    created.status, 200,
                    "case {case:?}: create-landingzone expected 200, got {} body {:?}",
                    created.status, created.body
                );
            }
            // A control enables only for an account Control Tower has been set up in, so the
            // row that enables with NO landing zone deployed sets the account up and
            // decommissions it again: nothing is left to read a governed-region list from.
            None => set_up_then_decommission(&router, case).await,
        }

        let (arn, _) = enable(
            &router,
            serde_json::json!({
                "controlIdentifier": control("AWS-GR_ENCRYPTED_VOLUMES"),
                "targetIdentifier": ou("1010"),
            }),
        )
        .await;

        let detail = details(&router, &arn).await;
        let expected_regions: Vec<serde_json::Value> = expected
            .iter()
            .map(|name| serde_json::json!({ "name": name }))
            .collect();
        assert_eq!(
            entries(detail.get("targetRegions").unwrap_or(&serde_json::Value::Null)),
            expected_regions,
            "case {case:?}: expected target regions {expected_regions:?}, got {:?}",
            detail.get("targetRegions")
        );
    }
}
