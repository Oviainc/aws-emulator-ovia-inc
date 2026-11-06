// SPDX-License-Identifier: CC0-1.0

//! `ResetEnabledControl`, end to end over HTTP, with `GetEnabledControl` and
//! `ListControlOperations` reading back what it wrote.
//!
//! Driven through the same [`MultiServiceRouter`] the production emulator dispatches with,
//! so what these tests exercise is the wire behaviour a client sees — status codes, error
//! codes and JSON bodies — not the handlers' Rust signatures.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * a reset of an enablement of a control AWS Control Tower implements with a service
//!   control policy is refused with `ValidationException` (400) and starts nothing, because
//!   the operation "does not work for controls implemented with SCPs" — over every such
//!   control the catalog publishes, in either ARN form, configured or not, and whatever the
//!   enablement's current state is;
//! * for a control implemented otherwise, `ResetEnabledControl` starts an asynchronous
//!   `RESET_ENABLED_CONTROL` operation that re-deploys the configuration the enabled
//!   control ALREADY carries, so the configured `parameters` read the same before, during
//!   and after it — whichever such catalog control is enabled and whichever ARN form named
//!   it;
//! * while that operation is in flight the control reads as `UNDER_CHANGE` with the reset
//!   as its `lastOperationIdentifier` and `UNKNOWN` drift, because a control cannot be
//!   assessed against a configuration still being deployed; once the clock has made the
//!   operation due the control is `SUCCEEDED` and `IN_SYNC` in all three drift dimensions;
//! * every reset starts its OWN operation, so a repeated reset mints a fresh
//!   `operationIdentifier` and the log holds one `RESET_ENABLED_CONTROL` record per call;
//! * a reset that overlaps an operation still changing the control is `ConflictException`
//!   (409) and mints nothing, whichever call started that operation, and an identifier no
//!   enabled control matches is `ResourceNotFoundException` (404).
//!
//! The controls a row enables, the implementation each carries and the parameter keys it
//! configures them with are read off `state::CONTROL_CATALOG` — the catalog `EnableControl`
//! resolves a `controlIdentifier` against — rather than written out by hand, so no row can
//! drift from what this emulator publishes. Every control that catalog publishes WITH
//! parameters is a preventive one, so the configurations a started reset re-deploys are the
//! ones a parameterless control can carry; the configured rows are the refusal table's. The enum values written out here come from
//! `smithy_query(query="shape", shape="ControlOperationType,ControlOperationStatus,\
//! EnablementStatus,DriftStatus")` on this crate's `smithy-model/`: `ENABLE_CONTROL` |
//! `DISABLE_CONTROL` | `UPDATE_ENABLED_CONTROL` | `RESET_ENABLED_CONTROL`, `SUCCEEDED` |
//! `FAILED` | `IN_PROGRESS`, `SUCCEEDED` | `FAILED` | `UNDER_CHANGE` and `DRIFTED` |
//! `IN_SYNC` | `NOT_CHECKING` | `UNKNOWN`. `enabledControlIdentifier` is always the full
//! ARN `EnableControl` minted: the model constrains it to `Arn` (pattern
//! `^arn:aws[0-9a-zA-Z_\-:\/]+$`, length 20..2048), so no other identifier form reaches
//! the handler.

use service_com_amazonaws_controltower_awscontroltowerapis::state::{
    CatalogControl, CONTROL_CATALOG,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::enabled_control_lifecycle::{
    catalog_arn, configuration, configured, control, control_operation, control_operations,
    details, drift_dimensions, enable_with, last_operation, member, ou,
    status_summary_status as status,
};
use crate::landing_zone_lifecycle::{
    call, is_operation_identifier, past_the_settle_span, set_up_emulator, Answer,
};

/// POST `/reset-enabled-control`. `enabledControlIdentifier` is the only member the model
/// gives this operation: a reset carries no configuration of its own.
async fn reset(router: &MultiServiceRouter, identifier: &str) -> Answer {
    call(
        router,
        "/reset-enabled-control",
        serde_json::json!({ "enabledControlIdentifier": identifier }),
    )
    .await
}

/// POST `/update-enabled-control`, for the rows whose configuration a call other than the
/// enablement wrote.
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

/// The filter narrowing the control operation log to the resets.
fn resets_only() -> serde_json::Value {
    serde_json::json!({ "controlOperationTypes": ["RESET_ENABLED_CONTROL"] })
}

/// The `operationIdentifier` of every record in `records`, sorted, so a case compares SETS
/// rather than the log's order.
fn identifiers(records: &[serde_json::Value]) -> Vec<String> {
    let mut sorted: Vec<String> = records
        .iter()
        .map(|record| {
            member(record, "operationIdentifier")
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    sorted.sort();
    sorted
}

/// The catalog controls a caller may enable whose reset starts an operation: the ones AWS
/// Control Tower implements with something other than a service control policy, in catalog
/// order.
fn resettable_controls() -> Vec<&'static CatalogControl> {
    CONTROL_CATALOG
        .iter()
        .filter(|control| control.enablable && !control.implemented_with_service_control_policy())
        .collect()
}

/// The catalog controls a caller may enable that AWS Control Tower implements WITH a service
/// control policy, which are the ones a reset does not work for, in catalog order.
fn service_control_policy_controls() -> Vec<&'static CatalogControl> {
    CONTROL_CATALOG
        .iter()
        .filter(|control| control.enablable && control.implemented_with_service_control_policy())
        .collect()
}

/// The catalog controls a caller may enable that DO declare enablement parameters.
fn enablable_with_parameters() -> Vec<&'static CatalogControl> {
    CONTROL_CATALOG
        .iter()
        .filter(|control| control.enablable && !control.parameters.is_empty())
        .collect()
}

/// The `index`th enablable control a reset works for, wrapping round the catalog so a row
/// never depends on the catalog's length.
fn resettable_control(index: usize) -> &'static CatalogControl {
    let resettable = resettable_controls();
    assert!(
        !resettable.is_empty(),
        "the catalog must publish a control a caller may enable that a reset works for"
    );
    resettable[index % resettable.len()]
}

/// A control the catalog publishes WITH parameters, which is what a configured row enables.
fn parameterized_control() -> &'static CatalogControl {
    let parameterized = enablable_with_parameters();
    assert!(
        !parameterized.is_empty(),
        "the catalog must publish an enablable control that declares parameters"
    );
    parameterized[0]
}

/// A `parameters` list over the first `count` keys the control declares, each carrying the
/// `Document` value at the same position of `values` (cycled), so a case configures a
/// control only with keys it declares while still varying the value's document kind.
fn parameters_of(
    control: &CatalogControl,
    count: usize,
    values: &[serde_json::Value],
) -> serde_json::Value {
    serde_json::Value::Array(
        control
            .parameters
            .iter()
            .take(count)
            .enumerate()
            .map(|(index, key)| {
                serde_json::json!({ "key": key, "value": values[index % values.len()] })
            })
            .collect(),
    )
}

/// One row of the reset table.
struct Row {
    case: String,
    control_identifier: String,
    target_identifier: String,
    /// The `parameters` member `EnableControl` is given; `None` omits it, which configures
    /// the control with nothing.
    initial: Option<serde_json::Value>,
}

/// The reset table: EVERY enablable control a reset works for, each named by one of the two
/// ARN forms AWS publishes for it and enabled either with no `parameters` member or with the
/// empty list — the configurations a control that declares no parameter can carry, which is
/// every control this catalog publishes a reset works for. Every row names its own
/// control/target pair, because a control is enabled at most once per target.
fn rows() -> Vec<Row> {
    resettable_controls()
        .into_iter()
        .enumerate()
        .map(|(index, entry)| {
            let by_catalog_arn = index % 2 == 1;
            let empty_list = (index / 2) % 2 == 1;
            Row {
                case: format!(
                    "{}: named by its {}, enabled with {}",
                    entry.name,
                    if by_catalog_arn {
                        "control catalog ARN"
                    } else {
                        "control ARN"
                    },
                    if empty_list {
                        "an empty parameters list"
                    } else {
                        "no parameters member"
                    },
                ),
                control_identifier: if by_catalog_arn {
                    catalog_arn(entry.catalog_id)
                } else {
                    control(entry.name)
                },
                target_identifier: ou(&format!("rs{index:02}")),
                initial: empty_list.then(|| serde_json::json!([])),
            }
        })
        .collect()
}

/// RULE: `ResetEnabledControl` starts an asynchronous `RESET_ENABLED_CONTROL` operation
/// against the enabled control its identifier names, re-deploying the configuration that
/// control already carries: while the operation is in flight the control is `UNDER_CHANGE`
/// with `UNKNOWN` drift, once the clock has made it due the control is `SUCCEEDED` and
/// `IN_SYNC`, and the configured parameters, the target and the regions never move.
#[tokio::test]
async fn a_reset_redeploys_the_configuration_the_control_already_carries() {
    let router = set_up_emulator().await;

    // Every row is its own enablement on its own target, so the rows do not interact.
    let mut enabled: Vec<(Row, String)> = Vec::new();
    for row in rows() {
        let (arn, _) = enable_with(
            &router,
            &row.control_identifier,
            &row.target_identifier,
            row.initial.clone(),
        )
        .await;
        enabled.push((row, arn));
    }

    // The enables are asynchronous too, so wait for them: a control still being deployed
    // refuses a reset as a conflict (asserted separately below).
    tokio::time::sleep(past_the_settle_span()).await;

    let mut started: Vec<(Row, String, String, Vec<serde_json::Value>)> = Vec::new();
    for (row, arn) in enabled {
        let before = details(&router, &arn).await;
        let configured_before =
            configuration(before.get("parameters").unwrap_or(&serde_json::Value::Null));
        let regions_before = before.get("targetRegions").cloned();
        let operation_before = last_operation(&before).unwrap_or_default().to_string();

        let answer = reset(&router, &arn).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: reset expected 200, got {} body {:?}",
            row.case, answer.status, answer.body
        );
        let operation_identifier = answer.string("operationIdentifier");
        assert!(
            is_operation_identifier(&operation_identifier),
            "case {:?}: expected a 36-char lowercase-hex operationIdentifier, got \
             {operation_identifier:?}",
            row.case
        );
        assert_ne!(
            operation_identifier, operation_before,
            "case {:?}: the reset must start its own operation, not report the previous one",
            row.case
        );

        // The reset starts an asynchronous operation, so a caller acting straight after the
        // call sees the transient state: the operation in flight, the control changing, its
        // drift not assessable, and the SAME configuration still applied.
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
                Some("RESET_ENABLED_CONTROL"),
                Some("IN_PROGRESS"),
                Some(row.control_identifier.as_str()),
                Some(row.target_identifier.as_str()),
                Some(arn.as_str()),
            ),
            "case {:?}: expected a RESET_ENABLED_CONTROL record in flight against the \
             control the call named, got {operation:?}",
            row.case
        );

        let in_flight = details(&router, &arn).await;
        assert_eq!(
            (
                status(&in_flight),
                last_operation(&in_flight),
                drift_dimensions(&in_flight),
            ),
            (
                Some("UNDER_CHANGE"),
                Some(operation_identifier.as_str()),
                (Some("UNKNOWN"), Some("UNKNOWN"), Some("UNKNOWN")),
            ),
            "case {:?}: expected UNDER_CHANGE with UNKNOWN drift and the reset as the \
             lastOperationIdentifier, got {in_flight:?}",
            row.case
        );
        assert_eq!(
            configuration(in_flight.get("parameters").unwrap_or(&serde_json::Value::Null)),
            configured_before,
            "case {:?}: a reset re-deploys the configuration in place, so it must not \
             change while the reset is in flight, got {in_flight:?}",
            row.case
        );
        assert_eq!(
            in_flight.get("targetRegions").cloned(),
            regions_before,
            "case {:?}: a reset must not move the regions the control is deployed into, \
             got {in_flight:?}",
            row.case
        );

        started.push((row, arn, operation_identifier, configured_before));
    }

    // Nothing sleeps server-side; the wait is the test's.
    tokio::time::sleep(past_the_settle_span()).await;

    for (row, arn, operation_identifier, configured_before) in &started {
        let operation = control_operation(&router, operation_identifier).await;
        assert_eq!(
            (
                member(&operation, "operationType"),
                member(&operation, "status"),
            ),
            (Some("RESET_ENABLED_CONTROL"), Some("SUCCEEDED")),
            "case {:?}: expected RESET_ENABLED_CONTROL SUCCEEDED once due, got {operation:?}",
            row.case
        );
        assert!(
            operation.get("endTime").is_some(),
            "case {:?}: expected an endTime once the reset is due, got {operation:?}",
            row.case
        );

        let applied = details(&router, arn).await;
        assert_eq!(
            (
                status(&applied),
                last_operation(&applied),
                drift_dimensions(&applied),
                member(&applied, "arn"),
                member(&applied, "controlIdentifier"),
                member(&applied, "targetIdentifier"),
            ),
            (
                Some("SUCCEEDED"),
                Some(operation_identifier.as_str()),
                (Some("IN_SYNC"), Some("IN_SYNC"), Some("IN_SYNC")),
                Some(arn.as_str()),
                Some(row.control_identifier.as_str()),
                Some(row.target_identifier.as_str()),
            ),
            "case {:?}: a completed reset leaves the control deployed and in sync with the \
             configuration it re-deployed, got {applied:?}",
            row.case
        );
        assert_eq!(
            &configuration(applied.get("parameters").unwrap_or(&serde_json::Value::Null)),
            configured_before,
            "case {:?}: a reset must re-deploy the configuration in place rather than \
             change it, got {applied:?}",
            row.case
        );
    }

    // The log tells the calls apart: the resets are the RESET_ENABLED_CONTROL records and
    // the enables and updates are not.
    let listed = identifiers(&control_operations(&router, resets_only()).await);
    let mut expected: Vec<String> = started
        .iter()
        .map(|(_, _, operation, _)| operation.clone())
        .collect();
    expected.sort();
    assert_eq!(
        listed, expected,
        "expected every reset's record and nothing else under RESET_ENABLED_CONTROL, got \
         {listed:?}"
    );
}

/// RULE: every reset starts its OWN asynchronous operation, so repeating the same request
/// once the previous one is due mints a fresh `operationIdentifier`, appends another
/// `RESET_ENABLED_CONTROL` record and leaves the configuration where it was.
#[tokio::test]
async fn each_reset_starts_its_own_operation() {
    let router = set_up_emulator().await;
    // A control a reset works for. Every control the catalog publishes WITH parameters is
    // implemented with a service control policy, so the configuration a repeat re-deploys
    // here is the empty one this control carries.
    let resettable = resettable_control(0);
    let configured_at_enable = serde_json::json!([]);
    let (arn, enable_operation) = enable_with(
        &router,
        &control(resettable.name),
        &ou("rp01"),
        Some(configured_at_enable.clone()),
    )
    .await;
    tokio::time::sleep(past_the_settle_span()).await;

    // Three resets in a row: the count is what this case varies, so the rule holds for a
    // repeat and not only for the first call.
    let mut operations: Vec<String> = Vec::new();
    for round in 1..=3u8 {
        let answer = reset(&router, &arn).await;
        assert_eq!(
            answer.status, 200,
            "round {round}: reset expected 200, got {} body {:?}",
            answer.status, answer.body
        );
        let operation_identifier = answer.string("operationIdentifier");
        assert!(
            !operations.contains(&operation_identifier) && operation_identifier != enable_operation,
            "round {round}: expected an operationIdentifier of its own, got \
             {operation_identifier:?} among {operations:?}"
        );
        operations.push(operation_identifier);
        tokio::time::sleep(past_the_settle_span()).await;
    }

    let listed = identifiers(&control_operations(&router, resets_only()).await);
    let mut expected = operations.clone();
    expected.sort();
    assert_eq!(
        listed, expected,
        "expected one RESET_ENABLED_CONTROL record per reset, got {listed:?}"
    );

    let settled = details(&router, &arn).await;
    assert_eq!(
        (
            status(&settled),
            last_operation(&settled),
            drift_dimensions(&settled),
            configuration(settled.get("parameters").unwrap_or(&serde_json::Value::Null)),
        ),
        (
            Some("SUCCEEDED"),
            operations.last().map(String::as_str),
            (Some("IN_SYNC"), Some("IN_SYNC"), Some("IN_SYNC")),
            configuration(&configured_at_enable),
        ),
        "expected the newest reset as the lastOperationIdentifier and the configuration \
         untouched, got {settled:?}"
    );
}

/// RULE: a reset that overlaps an operation still changing the control is refused with the
/// modelled `ConflictException` (409) and writes nothing, whichever call started that
/// operation; once the clock has made it due the same request is accepted.
#[tokio::test]
async fn a_reset_overlapping_an_operation_in_flight_is_a_conflict() {
    let resettable = resettable_control(0);
    let identifier = control(resettable.name);

    // `case` and which call is left in flight when the reset arrives. An update is not among
    // them: a control declaring no parameter has no configuration to supply that differs
    // from the one it carries, and every control the catalog publishes WITH parameters is
    // one a reset does not work for at all — where the refusal is the documented one
    // whatever is in flight, asserted in the service-control-policy case below.
    for (index, (case, in_flight)) in [
        ("while the enable is in flight", "enable"),
        ("while another reset is in flight", "reset"),
    ]
    .into_iter()
    .enumerate()
    {
        let router = set_up_emulator().await;
        let (arn, enable_operation) =
            enable_with(&router, &identifier, &ou(&format!("cf{index:02}")), None).await;

        let blocking = match in_flight {
            "enable" => enable_operation.clone(),
            _ => {
                tokio::time::sleep(past_the_settle_span()).await;
                let first = reset(&router, &arn).await;
                assert_eq!(
                    first.status, 200,
                    "case {case:?}: the first reset expected 200, got {} body {:?}",
                    first.status, first.body
                );
                first.string("operationIdentifier")
            }
        };

        let refused = reset(&router, &arn).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (409, Some("ConflictException")),
            "case {case:?}: expected 409 ConflictException, got {:?} body {:?}",
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );
        assert!(
            refused.body.get("operationIdentifier").is_none(),
            "case {case:?}: a refused reset must mint no operation, got {:?}",
            refused.body
        );

        // The refusal wrote nothing: the call that was in flight is still the control's
        // last operation.
        let untouched = details(&router, &arn).await;
        assert_eq!(
            last_operation(&untouched),
            Some(blocking.as_str()),
            "case {case:?}: a refused reset must leave the operation in flight as the \
             lastOperationIdentifier, got {untouched:?}"
        );

        // Once everything in flight is due, the SAME request is accepted.
        tokio::time::sleep(past_the_settle_span()).await;
        let accepted = reset(&router, &arn).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: the same reset once nothing is in flight: expected 200, got {} \
             body {:?}",
            accepted.status, accepted.body
        );
        tokio::time::sleep(past_the_settle_span()).await;
        assert_eq!(
            last_operation(&details(&router, &arn).await),
            Some(accepted.string("operationIdentifier").as_str()),
            "case {case:?}: expected the accepted reset as the lastOperationIdentifier"
        );
    }
}

/// RULE: an identifier no enabled control matches is `ResourceNotFoundException` (404) from
/// `ResetEnabledControl` — including a well-formed ARN of another resource kind — and such a
/// refusal leaves the control that does exist alone.
#[tokio::test]
async fn an_identifier_no_enabled_control_matches_is_not_found() {
    let router = set_up_emulator().await;
    let parameterized = parameterized_control();
    let control_identifier = control(parameterized.name);
    let enabled_with = parameters_of(parameterized, 1, &[serde_json::json!(["us-west-2"])]);
    let (arn, enable_operation) = enable_with(
        &router,
        &control_identifier,
        &ou("nf02"),
        Some(enabled_with.clone()),
    )
    .await;
    tokio::time::sleep(past_the_settle_span()).await;

    // Every value satisfies `Arn`'s pattern and length, so the front door passes them and
    // the handler is what refuses them.
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
            control_identifier.clone(),
        ),
        ("the target ARN rather than the enablement's", ou("nf02")),
        (
            "an enabled BASELINE's ARN form rather than an enabled control's",
            format!(
                "arn:aws:controltower:us-west-2:{}:enabledbaseline/absent",
                engine::emulator_account_id()
            ),
        ),
    ];

    for (case, identifier) in cases {
        let refused = reset(&router, &identifier).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: reset expected 404 ResourceNotFoundException, got {:?} body {:?}",
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );
        assert!(
            refused.body.get("operationIdentifier").is_none(),
            "case {case:?}: a refused reset must mint no operation, got {:?}",
            refused.body
        );
    }

    // None of those refusals disturbed the control that does exist, and none of them
    // appended a record to the log.
    let untouched = details(&router, &arn).await;
    assert_eq!(
        (
            configured(&router, &arn).await,
            status(&untouched),
            last_operation(&untouched),
        ),
        (
            configuration(&enabled_with),
            Some("SUCCEEDED"),
            Some(enable_operation.as_str()),
        ),
        "the stored control must be untouched by refusals, got {untouched:?}"
    );
    assert!(
        control_operations(&router, resets_only()).await.is_empty(),
        "a refused reset must append no RESET_ENABLED_CONTROL record"
    );
}

/// The documented refusal, all of it: the modelled `ValidationException` (400), a message
/// naming the control the request could not be served for, and that the call started
/// nothing.
fn assert_documented_refusal(refused: &Answer, case: &str, control_name: &str) {
    assert_eq!(
        (refused.status, refused.error_code.as_deref()),
        (400, Some("ValidationException")),
        "case {case:?}: expected 400 ValidationException, got {:?} body {:?}",
        (refused.status, refused.error_code.as_deref()),
        refused.body
    );
    let message = refused.string("message");
    assert!(
        message.contains(control_name) && message.contains("SCP"),
        "case {case:?}: expected a message naming control {control_name} and the \
         implementation a reset does not work for, got {message:?}"
    );
    assert!(
        refused.body.get("operationIdentifier").is_none(),
        "case {case:?}: a refused reset must mint no operation, got {:?}",
        refused.body
    );
}

/// RULE: `ResetEnabledControl` does not work for a control AWS Control Tower implements with
/// a service control policy, so a reset of an enablement of such a control is refused with
/// the modelled `ValidationException` (400): it starts no operation, appends no record to the
/// control operation log and leaves the enablement exactly as it was. Asserted over EVERY
/// enablable control the catalog publishes with that implementation, named by both ARN forms
/// AWS publishes for a control, configured with every parameter it declares as well as with
/// none, and read both while its enablement is still deploying and once it is deployed.
#[tokio::test]
async fn a_control_implemented_with_a_service_control_policy_cannot_be_reset() {
    let router = set_up_emulator().await;
    let policy_controls = service_control_policy_controls();
    assert!(
        !policy_controls.is_empty(),
        "the catalog must publish a control a caller may enable that AWS Control Tower \
         implements with a service control policy"
    );

    let values = [
        serde_json::json!(["us-west-2", "eu-west-1"]),
        serde_json::json!("arn:aws:iam::123456789012:role/exempted"),
        serde_json::json!({ "nested": { "deep": [1, 2, 3] }, "on": true }),
    ];

    // Two rows per such control: one naming it by its control ARN with no `parameters`
    // member, one naming it by its control catalog ARN carrying every parameter it declares
    // (the empty list for a control that declares none), so neither the identifier's form
    // nor the configuration in place moves the answer.
    let mut cases: Vec<(String, String, String, Option<serde_json::Value>)> = Vec::new();
    for entry in &policy_controls {
        cases.push((
            format!(
                "{}: named by its control ARN, configured with nothing",
                entry.name
            ),
            entry.name.to_string(),
            control(entry.name),
            None,
        ));
        cases.push((
            format!(
                "{}: named by its control catalog ARN, configured with every parameter it \
                 declares",
                entry.name
            ),
            entry.name.to_string(),
            catalog_arn(entry.catalog_id),
            Some(parameters_of(entry, entry.parameters.len(), &values)),
        ));
    }

    let mut enabled: Vec<(String, String, String, String)> = Vec::new();
    for (index, (case, control_name, control_identifier, initial)) in
        cases.into_iter().enumerate()
    {
        let (arn, enable_operation) = enable_with(
            &router,
            &control_identifier,
            &ou(&format!("sc{index:02}")),
            initial,
        )
        .await;

        // The restriction is a property of the control the enablement applies rather than of
        // the enablement's state, so it is already the answer while the enable is in flight
        // — where a control a reset works for earns the 409 instead.
        let refused = reset(&router, &arn).await;
        assert_documented_refusal(
            &refused,
            &format!("{case}, while the enable is in flight"),
            &control_name,
        );

        enabled.push((case, control_name, arn, enable_operation));
    }

    tokio::time::sleep(past_the_settle_span()).await;

    for (case, control_name, arn, enable_operation) in &enabled {
        let configured_before = configured(&router, arn).await;
        let refused = reset(&router, arn).await;
        assert_documented_refusal(
            &refused,
            &format!("{case}, once the control is deployed"),
            control_name,
        );

        let untouched = details(&router, arn).await;
        assert_eq!(
            (
                status(&untouched),
                last_operation(&untouched),
                drift_dimensions(&untouched),
                configured(&router, arn).await,
            ),
            (
                Some("SUCCEEDED"),
                Some(enable_operation.as_str()),
                (Some("IN_SYNC"), Some("IN_SYNC"), Some("IN_SYNC")),
                configured_before,
            ),
            "case {case:?}: a refused reset must leave the enablement exactly as the enable \
             left it, got {untouched:?}"
        );
    }

    // An operation of this service's OWN in flight against such a control does not change
    // the answer either: the configurable control the catalog publishes is one a reset does
    // not work for, so an `UpdateEnabledControl` can be mid-flight when the reset arrives.
    if let Some(configurable) = policy_controls
        .iter()
        .find(|entry| !entry.parameters.is_empty())
    {
        let case = format!("{}: while an update is in flight", configurable.name);
        let (arn, _) = enable_with(
            &router,
            &control(configurable.name),
            &ou("scup"),
            Some(parameters_of(configurable, 1, &values)),
        )
        .await;
        tokio::time::sleep(past_the_settle_span()).await;

        let started = update(
            &router,
            &arn,
            parameters_of(
                configurable,
                configurable.parameters.len(),
                &[serde_json::json!(["eu-west-1"]), serde_json::json!("changed")],
            ),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: the update expected 200, got {} body {:?}",
            started.status, started.body
        );

        let refused = reset(&router, &arn).await;
        assert_documented_refusal(&refused, &case, configurable.name);
    }

    assert!(
        control_operations(&router, resets_only()).await.is_empty(),
        "a reset a control implemented with a service control policy refuses must append no \
         RESET_ENABLED_CONTROL record"
    );
}
