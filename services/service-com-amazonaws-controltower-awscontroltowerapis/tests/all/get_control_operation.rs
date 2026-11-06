// SPDX-License-Identifier: CC0-1.0

//! `GetControlOperation`, end to end over HTTP, over the records every call that acts on an
//! `EnabledControl` appends to the control operation log.
//!
//! Driven through the same [`MultiServiceRouter`] the production emulator dispatches with,
//! so what these tests exercise is the wire behaviour a client sees — status codes, error
//! codes and JSON bodies — not the handlers' Rust signatures.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * an `operationIdentifier` a control call returned resolves to the record that call
//!   wrote — the call's own `ControlOperationType`, the control, target and enabled control
//!   it acted on, `IN_PROGRESS` with no `endTime` while it is in flight and `SUCCEEDED` with
//!   a stable `endTime` once the clock has made it due — whichever of the four calls started
//!   it, whichever catalogued control and organizational-unit target it acts on, and in
//!   either of the control's two published ARN forms;
//! * the record this operation reports and the summary `ListControlOperations` reports for
//!   the same identifier are ONE record, so every member they share agrees;
//! * reading a record never changes it: repeated reads report the same record, and the log
//!   still holds exactly the records the calls wrote;
//! * an identifier no control call returned is `ResourceNotFoundException` (404), whether or
//!   not the account holds control operations, and including an identifier this service's
//!   LANDING ZONE or BASELINE log minted: the three logs are separate stores.
//!
//! The controls a row enables, the implementation each carries and the parameter keys it
//! declares are read off `state::CONTROL_CATALOG` — the catalog `EnableControl` resolves a
//! `controlIdentifier` against — rather than written out by hand, so no row can drift from
//! what this emulator publishes. The enum values written out here come from
//! `smithy_query(query="shape", shape="ControlOperationType,ControlOperationStatus")` on
//! this crate's `smithy-model/`: `ENABLE_CONTROL` | `DISABLE_CONTROL` |
//! `UPDATE_ENABLED_CONTROL` | `RESET_ENABLED_CONTROL` and `SUCCEEDED` | `FAILED` |
//! `IN_PROGRESS`; the input's `operationIdentifier` is 36 characters of the UUID pattern and
//! `ResourceNotFoundException` (404) is among the operation's modelled errors
//! (`smithy_query(query="operation", operation="GetControlOperation")`).

use service_com_amazonaws_controltower_awscontroltowerapis::state::{
    CatalogControl, CONTROL_CATALOG,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::enabled_control_lifecycle::{
    catalog_arn, control, control_operation, control_operations, enable_with, member, ou,
};
use crate::landing_zone_lifecycle::{
    call, emulator, is_operation_identifier, past_the_settle_span, set_up_emulator,
    simple_manifest, Answer,
};

/// POST `/get-control-operation`, handing back the answer plus its `controlOperation`
/// member, which is what every assertion below reads.
async fn get_operation(
    router: &MultiServiceRouter,
    operation_identifier: &str,
) -> (Answer, serde_json::Value) {
    let answer = call(
        router,
        "/get-control-operation",
        serde_json::json!({ "operationIdentifier": operation_identifier }),
    )
    .await;
    let operation = answer.get("controlOperation").clone();
    (answer, operation)
}

/// As [`get_operation`], asserting the identifier resolved.
async fn resolved(
    router: &MultiServiceRouter,
    case: &str,
    operation_identifier: &str,
) -> serde_json::Value {
    let (answer, operation) = get_operation(router, operation_identifier).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected {operation_identifier:?} to resolve, got {} body {:?}",
        answer.status, answer.body
    );
    operation
}

/// The catalog controls a caller may enable that declare NO enablement parameters.
fn plain_controls() -> Vec<&'static CatalogControl> {
    CONTROL_CATALOG
        .iter()
        .filter(|entry| entry.enablable && entry.parameters.is_empty())
        .collect()
}

/// The `index`th enablable control declaring no parameters, wrapping round the catalog so a
/// row never depends on the catalog's length.
fn plain_control(index: usize) -> &'static CatalogControl {
    let plain = plain_controls();
    assert!(
        !plain.is_empty(),
        "the catalog must publish a control a caller may enable that declares no parameters"
    );
    plain[index % plain.len()]
}

/// A control the catalog publishes WITH parameters, which is what the rows that configure a
/// control enable.
fn parameterized_control() -> &'static CatalogControl {
    let parameterized: Vec<&'static CatalogControl> = CONTROL_CATALOG
        .iter()
        .filter(|entry| entry.enablable && !entry.parameters.is_empty())
        .collect();
    assert!(
        !parameterized.is_empty(),
        "the catalog must publish an enablable control that declares parameters"
    );
    parameterized[0]
}

/// A control a reset works for: one AWS Control Tower implements with something other than a
/// service control policy, because `ResetEnabledControl` "does not work for controls
/// implemented with SCPs".
fn resettable_control() -> &'static CatalogControl {
    let resettable: Vec<&'static CatalogControl> = CONTROL_CATALOG
        .iter()
        .filter(|entry| entry.enablable && !entry.implemented_with_service_control_policy())
        .collect();
    assert!(
        !resettable.is_empty(),
        "the catalog must publish a control a caller may enable that a reset works for"
    );
    resettable[0]
}

/// A `parameters` list over the first key the control declares, so a row configures a
/// control only with a key it declares.
fn one_parameter(entry: &CatalogControl, value: serde_json::Value) -> serde_json::Value {
    let key = entry
        .parameters
        .first()
        .expect("the row's control declares a parameter");
    serde_json::json!([{ "key": key, "value": value }])
}

/// The call that starts the operation a row is about. `Enable` is the enabling call itself;
/// the other three act on the enablement that call created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Start {
    Enable,
    /// `DisableControl`, naming the enablement by the ARN `EnableControl` minted.
    DisableByArn,
    /// `DisableControl`, naming it by the `controlIdentifier`/`targetIdentifier` pair.
    DisableByPair,
    /// `UpdateEnabledControl`, with the configuration [`updated_configuration`] names.
    Update,
    Reset,
}

impl Start {
    /// The `ControlOperationType` the record this call appends carries.
    fn operation_type(self) -> &'static str {
        match self {
            Self::Enable => "ENABLE_CONTROL",
            Self::DisableByArn | Self::DisableByPair => "DISABLE_CONTROL",
            Self::Update => "UPDATE_ENABLED_CONTROL",
            Self::Reset => "RESET_ENABLED_CONTROL",
        }
    }
}

/// One row of the table: the enablement the row sets up and the call whose record is the
/// row's subject.
struct Row {
    case: String,
    /// The `controlIdentifier` `EnableControl` is given — one of the control's two published
    /// ARN forms.
    control_identifier: String,
    target_identifier: String,
    /// The `parameters` member `EnableControl` is given; `None` omits it.
    initial: Option<serde_json::Value>,
    start: Start,
}

/// The configuration the update row asks for: one key the parameterized control declares,
/// different from the one its enablement carries.
fn updated_configuration() -> serde_json::Value {
    one_parameter(parameterized_control(), serde_json::json!(["us-west-2"]))
}

/// The table: the rows differ along what the rule generalises over — the CALL that started
/// the operation (all four `ControlOperationType` values), the ARN form the enablement named
/// its control by, the control itself, the target, the configuration the control carries
/// (none, an empty list, one declared key) and, for a teardown, which of the two ways the
/// call named the enablement. Every row names its own control/target pair, because a control
/// is enabled at most once per target.
fn rows() -> Vec<Row> {
    let parameterized = parameterized_control();
    let resettable = resettable_control();
    vec![
        Row {
            case: format!(
                "{}: an enablement named by its control ARN, with no parameters member",
                plain_control(0).name
            ),
            control_identifier: control(plain_control(0).name),
            target_identifier: ou("go01"),
            initial: None,
            start: Start::Enable,
        },
        Row {
            case: format!(
                "{}: an enablement named by its control catalog ARN, configured with one \
                 declared key",
                parameterized.name
            ),
            control_identifier: catalog_arn(parameterized.catalog_id),
            target_identifier: ou("go02"),
            initial: Some(one_parameter(parameterized, serde_json::json!("eu-west-1"))),
            start: Start::Enable,
        },
        Row {
            case: format!(
                "{}: a teardown named by the ARN the enablement minted",
                plain_control(1).name
            ),
            control_identifier: control(plain_control(1).name),
            target_identifier: ou("go03"),
            initial: Some(serde_json::json!([])),
            start: Start::DisableByArn,
        },
        Row {
            case: format!(
                "{}: a teardown named by the control and target pair, on a catalog-ARN \
                 enablement",
                plain_control(2).name
            ),
            control_identifier: catalog_arn(plain_control(2).catalog_id),
            target_identifier: ou("go04"),
            initial: None,
            start: Start::DisableByPair,
        },
        Row {
            case: format!("{}: a configuration change", parameterized.name),
            control_identifier: control(parameterized.name),
            target_identifier: ou("go05"),
            initial: None,
            start: Start::Update,
        },
        Row {
            case: format!("{}: a re-deployment of the configuration", resettable.name),
            control_identifier: control(resettable.name),
            target_identifier: ou("go06"),
            initial: None,
            start: Start::Reset,
        },
    ]
}

/// An enablement a row set up, and the operation whose record the row asserts on.
struct Subject {
    case: String,
    /// The `controlIdentifier` the enablement carries, which is what every record of an
    /// operation against it reports.
    control_identifier: String,
    target_identifier: String,
    arn: String,
    operation_identifier: String,
    operation_type: &'static str,
}

/// The members naming WHAT a record is about, each beside the member's name so a mismatch
/// says which member differs.
const IDENTIFYING_MEMBERS: [&str; 6] = [
    "operationIdentifier",
    "operationType",
    "status",
    "controlIdentifier",
    "targetIdentifier",
    "enabledControlIdentifier",
];

/// Those members as the record reports them.
fn reported(operation: &serde_json::Value) -> Vec<(&'static str, Option<&str>)> {
    IDENTIFYING_MEMBERS
        .into_iter()
        .map(|name| (name, member(operation, name)))
        .collect()
}

/// Those members as the call that started the operation implies them, with `status` the one
/// the phase expects.
fn expected<'a>(
    subject: &'a Subject,
    status: &'static str,
) -> Vec<(&'static str, Option<&'a str>)> {
    vec![
        (
            "operationIdentifier",
            Some(subject.operation_identifier.as_str()),
        ),
        ("operationType", Some(subject.operation_type)),
        ("status", Some(status)),
        (
            "controlIdentifier",
            Some(subject.control_identifier.as_str()),
        ),
        ("targetIdentifier", Some(subject.target_identifier.as_str())),
        ("enabledControlIdentifier", Some(subject.arn.as_str())),
    ]
}

/// Assert the record in flight: the call's own identifiers and type, `IN_PROGRESS`, a
/// `startTime`, and neither an `endTime` nor a `statusMessage`.
fn assert_in_flight(subject: &Subject, operation: &serde_json::Value) {
    assert_eq!(
        reported(operation),
        expected(subject, "IN_PROGRESS"),
        "case {:?} while the operation is in flight: expected the record the call wrote, \
         got {operation:?}",
        subject.case
    );
    assert!(
        operation.get("startTime").is_some(),
        "case {:?}: expected a startTime, got {operation:?}",
        subject.case
    );
    assert!(
        operation.get("endTime").is_none(),
        "case {:?}: expected no endTime while the operation is in flight, got {operation:?}",
        subject.case
    );
    assert!(
        operation.get("statusMessage").is_none(),
        "case {:?}: a statusMessage is reported in case of failure only, got {operation:?}",
        subject.case
    );
}

/// Assert the record agrees with the summary `ListControlOperations` reports for the same
/// identifier: one record, two readers.
async fn assert_agrees_with_listing(
    router: &MultiServiceRouter,
    subject: &Subject,
    operation: &serde_json::Value,
) {
    let listed = control_operation(router, &subject.operation_identifier).await;
    for name in IDENTIFYING_MEMBERS
        .into_iter()
        .chain(["startTime", "endTime", "statusMessage"])
    {
        assert_eq!(
            operation.get(name),
            listed.get(name),
            "case {:?}: the record and the listing's summary must report the same {name}, \
             got {:?} and {:?}",
            subject.case,
            operation.get(name),
            listed.get(name)
        );
    }
}

/// Assert the record once the clock has made it due: `SUCCEEDED` with an `endTime` the next
/// read reports unchanged, and every identifier it carried still the same.
async fn assert_settled(router: &MultiServiceRouter, subject: &Subject) {
    let operation = resolved(router, &subject.case, &subject.operation_identifier).await;
    assert_eq!(
        reported(&operation),
        expected(subject, "SUCCEEDED"),
        "case {:?} once the operation is due: expected SUCCEEDED and the recorded \
         identifiers unchanged, got {operation:?}",
        subject.case
    );
    let end_time = operation.get("endTime").cloned();
    assert!(
        end_time.is_some(),
        "case {:?}: expected an endTime once the operation is due, got {operation:?}",
        subject.case
    );

    // RULE (the read side): reading a record changes nothing — a terminal record is stable,
    // so a second read reports the same endTime rather than re-stamping it.
    let reread = resolved(router, &subject.case, &subject.operation_identifier).await;
    assert_eq!(
        reread, operation,
        "case {:?}: re-reading a terminal operation must report the same record, got \
         {reread:?}",
        subject.case
    );
    assert_agrees_with_listing(router, subject, &operation).await;
}

/// RULE: the `operationIdentifier` a call acting on an `EnabledControl` returns resolves
/// through `GetControlOperation` to that call's own record — its `ControlOperationType`, the
/// control, target and enabled control it acted on, `IN_PROGRESS` while the operation is in
/// flight and `SUCCEEDED` with a stable `endTime` once the clock has made it due — whichever
/// of the four calls started it.
#[tokio::test]
async fn an_identifier_a_control_call_returned_reports_that_call() {
    let router = set_up_emulator().await;

    // Phase 1: every row's enablement, with the `ENABLE_CONTROL` rows' records asserted in
    // flight straight after their own call — the settle span is short on purpose, so a case
    // that enabled every row first could race the clock.
    let mut enablements: Vec<(Row, String)> = Vec::new();
    let mut subjects: Vec<Subject> = Vec::new();
    for row in rows() {
        let (arn, operation_identifier) = enable_with(
            &router,
            &row.control_identifier,
            &row.target_identifier,
            row.initial.clone(),
        )
        .await;
        assert!(
            is_operation_identifier(&operation_identifier),
            "case {:?}: expected a 36-char lowercase-hex operationIdentifier, got \
             {operation_identifier:?}",
            row.case
        );

        if row.start == Start::Enable {
            let subject = Subject {
                case: row.case.clone(),
                control_identifier: row.control_identifier.clone(),
                target_identifier: row.target_identifier.clone(),
                arn: arn.clone(),
                operation_identifier: operation_identifier.clone(),
                operation_type: row.start.operation_type(),
            };
            let operation = resolved(&router, &subject.case, &subject.operation_identifier).await;
            assert_in_flight(&subject, &operation);
            assert_agrees_with_listing(&router, &subject, &operation).await;
            subjects.push(subject);
        }
        enablements.push((row, arn));
    }

    // The clock makes every enablement due; nothing sleeps server-side, the wait is the
    // test's.
    tokio::time::sleep(past_the_settle_span()).await;

    // Phase 2: the calls that act on an enablement already in place, each record asserted in
    // flight straight after its own call.
    for (row, arn) in &enablements {
        let answer = match row.start {
            Start::Enable => continue,
            Start::DisableByArn => {
                call(
                    &router,
                    "/disable-control",
                    serde_json::json!({ "enabledControlIdentifier": arn }),
                )
                .await
            }
            Start::DisableByPair => {
                call(
                    &router,
                    "/disable-control",
                    serde_json::json!({
                        "controlIdentifier": row.control_identifier,
                        "targetIdentifier": row.target_identifier,
                    }),
                )
                .await
            }
            Start::Update => {
                call(
                    &router,
                    "/update-enabled-control",
                    serde_json::json!({
                        "enabledControlIdentifier": arn,
                        "parameters": updated_configuration(),
                    }),
                )
                .await
            }
            Start::Reset => {
                call(
                    &router,
                    "/reset-enabled-control",
                    serde_json::json!({ "enabledControlIdentifier": arn }),
                )
                .await
            }
        };
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected the call that starts the operation to succeed, got {} body \
             {:?}",
            row.case, answer.status, answer.body
        );
        let operation_identifier = answer.string("operationIdentifier");
        assert!(
            is_operation_identifier(&operation_identifier),
            "case {:?}: expected a 36-char lowercase-hex operationIdentifier, got \
             {operation_identifier:?}",
            row.case
        );

        let subject = Subject {
            case: row.case.clone(),
            control_identifier: row.control_identifier.clone(),
            target_identifier: row.target_identifier.clone(),
            arn: arn.clone(),
            operation_identifier,
            operation_type: row.start.operation_type(),
        };
        let operation = resolved(&router, &subject.case, &subject.operation_identifier).await;
        assert_in_flight(&subject, &operation);
        assert_agrees_with_listing(&router, &subject, &operation).await;
        subjects.push(subject);
    }

    tokio::time::sleep(past_the_settle_span()).await;

    for subject in &subjects {
        assert_settled(&router, subject).await;
    }

    // RULE (the read side, over the whole log): the reads above wrote nothing — the log
    // still holds exactly the records the calls wrote, one per call, and no read added or
    // removed one. Two records per non-`Enable` row (its enablement and its own call), one
    // per `Enable` row.
    let mut logged: Vec<String> = control_operations(&router, serde_json::Value::Null)
        .await
        .iter()
        .map(|record| {
            member(record, "operationIdentifier")
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    logged.sort();
    let expected_count = rows().len()
        + rows()
            .iter()
            .filter(|row| row.start != Start::Enable)
            .count();
    assert_eq!(
        logged.len(),
        expected_count,
        "expected one record per call and nothing else, got {logged:?}"
    );
    for subject in &subjects {
        assert!(
            logged.contains(&subject.operation_identifier),
            "case {:?}: expected the log to still hold {:?}, got {logged:?}",
            subject.case,
            subject.operation_identifier
        );
    }
}

/// RULE: an identifier no control call returned does not resolve — it is 404
/// `ResourceNotFoundException`, whether or not the account holds control operations, and
/// including an identifier this service's landing zone or baseline log minted: the three
/// logs are separate stores.
#[tokio::test]
async fn an_identifier_no_control_call_returned_is_not_found() {
    for populated in [false, true] {
        let router = emulator();

        // A landing zone operation identifier: minted by this service, logged elsewhere.
        let created = call(
            &router,
            "/create-landingzone",
            serde_json::json!({ "version": "3.3", "manifest": simple_manifest() }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "create-landingzone: expected 200, got {} body {:?}",
            created.status, created.body
        );
        let landing_zone_operation = created.string("operationIdentifier");

        // A baseline operation identifier: the third log, which is not this one either.
        let enabled_baseline = call(
            &router,
            "/enable-baseline",
            serde_json::json!({
                "baselineIdentifier": "arn:aws:controltower:us-west-2::baseline/17BSJV3IGJ2QSGA2",
                "baselineVersion": "4.0",
                "targetIdentifier": ou("gn01"),
            }),
        )
        .await;
        assert_eq!(
            enabled_baseline.status, 200,
            "enable-baseline: expected 200, got {} body {:?}",
            enabled_baseline.status, enabled_baseline.body
        );
        let baseline_operation = enabled_baseline.string("operationIdentifier");

        if populated {
            let entry = plain_control(0);
            let (_, started) =
                enable_with(&router, &control(entry.name), &ou("gn02"), None).await;
            assert!(
                is_operation_identifier(&started),
                "populating enable: expected a 36-char lowercase-hex operationIdentifier, \
                 got {started:?}"
            );
        }

        // Every value satisfies `OperationIdentifier`'s length and pattern, so the front
        // door passes them and the handler is what refuses them.
        let cases: Vec<(&str, String)> = vec![
            (
                "an all-zero identifier",
                "00000000-0000-0000-0000-000000000000".to_string(),
            ),
            (
                "an identifier no call minted",
                "deadbeef-dead-beef-dead-beefdeadbeef".to_string(),
            ),
            (
                "an identifier the landing zone log minted",
                landing_zone_operation.clone(),
            ),
            (
                "an identifier the baseline log minted",
                baseline_operation.clone(),
            ),
        ];

        for (case, identifier) in cases {
            let (answer, _) = get_operation(&router, &identifier).await;
            assert_eq!(
                (answer.status, answer.error_code.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?} (log populated: {populated}): expected 404 \
                 ResourceNotFoundException, got {:?} body {:?}",
                (answer.status, answer.error_code.as_deref()),
                answer.body
            );
        }
    }
}
