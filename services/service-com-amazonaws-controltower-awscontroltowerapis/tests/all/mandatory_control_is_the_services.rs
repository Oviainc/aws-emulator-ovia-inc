// SPDX-License-Identifier: CC0-1.0

//! The calls a caller makes against a control AWS Control Tower applied ITSELF, end to end
//! over HTTP.
//!
//! Driven through the same [`MultiServiceRouter`] the production emulator dispatches with,
//! so what these cases exercise is the wire behaviour a client sees — status codes, error
//! codes and JSON bodies — not the handlers' Rust signatures.
//!
//! The rule under test, asserted over a TABLE of inputs rather than one case: an enablement
//! of a MANDATORY catalog control is the SERVICE's, written when a target came under its
//! governance and removed when that governance ends, so no caller call that would create,
//! change, re-deploy or tear one down is accepted — every such request is refused with 400
//! `ValidationException`, naming the control and what the call would have done, and leaves
//! the enablement and the control operation log exactly as they were. Its counterpart is
//! asserted too: the same four calls are accepted on the enablements a CALLER made on the
//! same governed target, so the refusal is keyed on whose enablement it is rather than on
//! which call was made.
//!
//! Nothing here writes a control, a baseline or a parameter key out by hand: the mandatory
//! controls are read off `state::CONTROL_CATALOG` (the entries
//! [`CatalogControl::enablable`] is false for, the set `EnableControl` refuses) and so are
//! the enablable ones and the only declared parameter key a row configures — so the rows
//! span entries beyond the ones an issue happened to cite, on both sides of
//! `CatalogControl::implementation`, and no row can drift from the catalog it is about. The
//! errors come from `smithy_query(query="operation",
//! operation="EnableControl,DisableControl,UpdateEnabledControl,ResetEnabledControl")` on
//! this crate's `smithy-model/` (`ValidationException` is HTTP 400), and the status and
//! drift values from `smithy_query(query="shape", shape="EnablementStatus,DriftStatus")`.

use service_com_amazonaws_controltower_awscontroltowerapis::state::{
    CatalogControl, CONTROL_CATALOG,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::enabled_control_lifecycle::{
    catalog_arn, control, control_operations, details, drift_dimensions, enable_with,
    last_operation, member, ou, status_summary_status as status,
};
use crate::governed_target_mandatory_controls::{
    baseline, enable_baseline, governance_baseline, list, listed_arn_of, mandatory,
};
use crate::landing_zone_lifecycle::{call, past_the_settle_span, set_up_emulator, Answer};

/// One request of the table: a call a caller makes against an enabled control, and the word
/// the refusal must read back for what that call would have done.
struct Request {
    case: String,
    path: &'static str,
    body: serde_json::Value,
    action: &'static str,
}

/// The catalog controls a CALLER may enable, in catalog order.
fn enablable() -> Vec<&'static CatalogControl> {
    let enablable: Vec<&'static CatalogControl> = CONTROL_CATALOG
        .iter()
        .filter(|control| control.enablable)
        .collect();
    assert!(
        !enablable.is_empty(),
        "the catalog must publish at least one control a caller may enable"
    );
    enablable
}

/// A control a caller may enable that a reset works for: one AWS Control Tower implements
/// with something other than a service control policy, read off the catalog.
fn resettable_control() -> &'static CatalogControl {
    enablable()
        .into_iter()
        .find(|control| !control.implemented_with_service_control_policy())
        .expect("the catalog must publish an enablable control a reset works for")
}

/// A control a caller may enable that DECLARES enablement parameters, so an update row can
/// ask for a configuration the control accepts.
fn parameterized_control() -> &'static CatalogControl {
    enablable()
        .into_iter()
        .find(|control| !control.parameters.is_empty())
        .expect("the catalog must publish an enablable control that declares parameters")
}

/// An `EnabledControlParameter` key the catalog DECLARES somewhere, so an update row carries
/// a key that is a real parameter rather than an invented one.
fn declared_parameter_key() -> &'static str {
    parameterized_control().parameters[0]
}

/// A `parameters` list naming `key` once.
fn one_parameter(key: &str) -> serde_json::Value {
    serde_json::json!([{ "key": key, "value": "us-west-2" }])
}

/// Every call a caller can make against the enablement `arn` of the mandatory control
/// `entry` on `target`: the enable of that control (in both ARN forms AWS publishes for a
/// control), its disable (by the enablement's own ARN and by the control/target pair in
/// both forms), its reset, and an update of its configuration (with the empty list and with
/// a key the catalog declares) — the whole set of calls that would write such an enablement.
fn requests(entry: &CatalogControl, target: &str, arn: &str) -> Vec<Request> {
    let identifier = control(entry.name);
    let catalog = catalog_arn(entry.catalog_id);
    vec![
        Request {
            case: format!("{}: enable-control by its control ARN", entry.name),
            path: "/enable-control",
            body: serde_json::json!({
                "controlIdentifier": identifier,
                "targetIdentifier": target,
            }),
            action: "enabled",
        },
        Request {
            case: format!("{}: enable-control by its control catalog ARN", entry.name),
            path: "/enable-control",
            body: serde_json::json!({
                "controlIdentifier": catalog,
                "targetIdentifier": target,
            }),
            action: "enabled",
        },
        Request {
            case: format!("{}: disable-control by the enablement's ARN", entry.name),
            path: "/disable-control",
            body: serde_json::json!({ "enabledControlIdentifier": arn }),
            action: "disabled",
        },
        Request {
            case: format!("{}: disable-control by the control/target pair", entry.name),
            path: "/disable-control",
            body: serde_json::json!({
                "controlIdentifier": identifier,
                "targetIdentifier": target,
            }),
            action: "disabled",
        },
        Request {
            case: format!("{}: disable-control by the catalog ARN pair", entry.name),
            path: "/disable-control",
            body: serde_json::json!({
                "controlIdentifier": catalog,
                "targetIdentifier": target,
            }),
            action: "disabled",
        },
        Request {
            case: format!("{}: reset-enabled-control by the enablement's ARN", entry.name),
            path: "/reset-enabled-control",
            body: serde_json::json!({ "enabledControlIdentifier": arn }),
            action: "reset",
        },
        Request {
            case: format!("{}: update-enabled-control with an empty parameters list", entry.name),
            path: "/update-enabled-control",
            body: serde_json::json!({
                "enabledControlIdentifier": arn,
                "parameters": [],
            }),
            action: "updated",
        },
        Request {
            case: format!(
                "{}: update-enabled-control with the declared key {}",
                entry.name,
                declared_parameter_key()
            ),
            path: "/update-enabled-control",
            body: serde_json::json!({
                "enabledControlIdentifier": arn,
                "parameters": one_parameter(declared_parameter_key()),
            }),
            action: "updated",
        },
    ]
}

/// Assert `refused` is the modelled 400 `ValidationException` that names `entry`, says what
/// the call would have done and starts nothing.
fn assert_refused(request: &Request, entry: &CatalogControl, refused: &Answer) {
    let case = &request.case;
    assert_eq!(
        (refused.status, refused.error_code.as_deref()),
        (400, Some("ValidationException")),
        "case {case:?} ({}): expected 400 ValidationException, got {:?} body {:?}",
        request.body,
        (refused.status, refused.error_code.as_deref()),
        refused.body
    );
    let message = refused.string("message");
    assert!(
        message.contains(entry.name),
        "case {case:?}: the refusal must name the control, got message {message:?}"
    );
    assert!(
        message.contains("mandatory"),
        "case {case:?}: the refusal must say the control is a mandatory one, got message \
         {message:?}"
    );
    let reads_back = format!("may be {}", request.action);
    assert!(
        message.contains(&reads_back),
        "case {case:?}: the refusal must read back {reads_back:?}, got message {message:?}"
    );
    assert!(
        refused.body.get("operationIdentifier").is_none(),
        "case {case:?}: a refusal must start no operation, got {:?}",
        refused.body
    );
}

/// The operation log narrowed to one `ControlOperationType`.
async fn logged(router: &MultiServiceRouter, operation_type: &str) -> Vec<serde_json::Value> {
    control_operations(
        router,
        serde_json::json!({ "controlOperationTypes": [operation_type] }),
    )
    .await
}

/// RULE: an enablement of a MANDATORY control is the service's own — written when its target
/// came under AWS Control Tower's governance — so none of the calls a caller makes against
/// an enabled control is accepted on one: each is refused with 400 `ValidationException`
/// naming the control and what the call would have done, the enablement reads back exactly
/// as it did, and no control operation is appended against it.
///
/// The rows span every mandatory catalog entry (both implementations the catalog publishes),
/// every call that writes an enablement, and every way each of those calls lets a request
/// name one.
#[tokio::test]
async fn a_control_the_service_applied_is_not_a_callers_to_change() {
    let governance = governance_baseline();
    let target = ou("mu01");

    let router = set_up_emulator().await;
    enable_baseline(
        &router,
        &baseline(governance.id),
        governance.versions[0],
        &target,
    )
    .await;
    // Past the settle span, so no refusal below can be the in-flight `ConflictException` a
    // control still being applied answers with.
    tokio::time::sleep(past_the_settle_span()).await;

    let listed = list(&router, serde_json::json!({ "targetIdentifier": target })).await;

    for entry in mandatory() {
        let arn = listed_arn_of(&listed, &control(entry.name));
        assert!(
            !arn.is_empty(),
            "control {}: the governed target must report the control the service applied, \
             got {:?}",
            entry.name,
            listed.body
        );

        let before = details(&router, &arn).await;
        assert_eq!(
            (
                status(&before),
                drift_dimensions(&before),
                last_operation(&before).is_some()
            ),
            (
                Some("SUCCEEDED"),
                (Some("IN_SYNC"), Some("IN_SYNC"), Some("IN_SYNC")),
                true
            ),
            "control {}: the applied control must be deployed before the rows run, got {:?}",
            entry.name,
            before
        );

        for request in requests(entry, &target, &arn) {
            let refused = call(&router, request.path, request.body.clone()).await;
            assert_refused(&request, entry, &refused);

            // Nothing the refusal left behind: the enablement the service applied reads back
            // exactly as it did, in every member `GetEnabledControl` reports.
            let after = details(&router, &arn).await;
            assert_eq!(
                after, before,
                "case {:?}: the applied control must be untouched, expected {before:?}, got \
                 {after:?}",
                request.case
            );
        }
    }

    // Only the deployments governance itself started are in the log: no refusal appended a
    // record against a control the caller may not act on.
    for operation_type in ["RESET_ENABLED_CONTROL", "UPDATE_ENABLED_CONTROL", "DISABLE_CONTROL"] {
        let records = logged(&router, operation_type).await;
        assert!(
            records.is_empty(),
            "no {operation_type} operation may be logged, got {records:?}"
        );
    }
    let enables = logged(&router, "ENABLE_CONTROL").await;
    assert_eq!(
        enables.len(),
        mandatory().len(),
        "the log must hold exactly the enables governance started, got {enables:?}"
    );
}

/// RULE (the counterpart): the very same calls are the caller's to make on the enablements a
/// CALLER made — on the same governed target, where the service's own enablements refuse
/// them — so what the refusal is keyed on is whose enablement the request names, not which
/// call it is.
///
/// The rows are one enablable catalog control per call the previous case refuses: a reset of
/// a control a reset works for, an update of the control the catalog publishes with
/// parameters, and a disable — each read off the catalog rather than named here.
#[tokio::test]
async fn the_same_calls_are_accepted_on_the_controls_a_caller_enabled() {
    let governance = governance_baseline();
    let target = ou("mu02");
    let resettable = resettable_control();
    let parameterized = parameterized_control();
    let disabled = enablable()
        .into_iter()
        .find(|entry| entry.name != resettable.name && entry.name != parameterized.name)
        .expect("the catalog must publish three controls a caller may enable");

    let router = set_up_emulator().await;
    enable_baseline(
        &router,
        &baseline(governance.id),
        governance.versions[0],
        &target,
    )
    .await;

    let mut own: Vec<(&'static CatalogControl, String)> = Vec::new();
    for entry in [resettable, parameterized, disabled] {
        let (arn, _) = enable_with(&router, &control(entry.name), &target, None).await;
        own.push((entry, arn));
    }
    tokio::time::sleep(past_the_settle_span()).await;

    let accepted: Vec<(String, &'static str, serde_json::Value)> = vec![
        (
            format!("{}: reset-enabled-control", resettable.name),
            "/reset-enabled-control",
            serde_json::json!({ "enabledControlIdentifier": own[0].1 }),
        ),
        (
            format!(
                "{}: update-enabled-control with the declared key {}",
                parameterized.name,
                declared_parameter_key()
            ),
            "/update-enabled-control",
            serde_json::json!({
                "enabledControlIdentifier": own[1].1,
                "parameters": one_parameter(declared_parameter_key()),
            }),
        ),
        (
            format!("{}: disable-control", disabled.name),
            "/disable-control",
            serde_json::json!({ "enabledControlIdentifier": own[2].1 }),
        ),
    ];

    for (case, path, body) in accepted {
        let answer = call(&router, path, body.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?} ({body}): expected 200, got {} body {:?}",
            answer.status, answer.body
        );
        assert!(
            member(&answer.body, "operationIdentifier").is_some(),
            "case {case:?}: an accepted call starts an operation, got {:?}",
            answer.body
        );
    }

    // The accepted reset is logged against the caller's enablement, while the governed
    // target's service-applied controls still have no reset of their own.
    let resets = logged(&router, "RESET_ENABLED_CONTROL").await;
    assert_eq!(
        resets.len(),
        1,
        "exactly the caller's reset must be logged, got {resets:?}"
    );
    assert_eq!(
        member(&resets[0], "enabledControlIdentifier"),
        Some(own[0].1.as_str()),
        "the logged reset must name the caller's enablement, got {:?}",
        resets[0]
    );
}
