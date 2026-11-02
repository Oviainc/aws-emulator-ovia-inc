// SPDX-License-Identifier: CC0-1.0

//! The body of the modelled `ValidationException` wherever this service refuses a
//! request because of the request's OWN parameters.
//!
//! RULE under test: such a refusal's `Message` is the fixed opening "The following
//! parameter(s) are not valid: " followed by one clause per fault, comma-separated —
//! `"<parameter path>" is required` for a parameter the request has to carry and
//! omits, and the fixed clause "Unsupported request parameter(s) were found." when the
//! request carries a parameter it may not carry here (which names none of them). The
//! only variable part is the quoted parameter path, which is the path from the
//! operation's own input, and nothing else of the request — no value, no enum member,
//! no count — appears in the sentence.
//!
//! The rows span what the rule generalises over: every operation that cross-checks a
//! nested member set against the request's own discriminator (`CreateJob`'s
//! `Type`/`Details`, `SendDataSetNotification`'s `Type`/`Details`, an event action's
//! `Event` and `Action`), every value of the model's `Type` enum, and each
//! combination of faults a request can carry — a required parameter missing, an
//! unsupported one present, and both at once. One row of another clause kind (a value
//! outside a parameter's modelled value set) shows the opening is the same sentence
//! for every parameter fault.
//!
//! Every test drives the service through `MultiServiceRouter::dispatch` with a real
//! restJson1 request, so the route, the status, the error code and the serialized body
//! are all exercised.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::data_sets::{call, create_data_set, harness};
use crate::event_actions::entitle_data_set;
use crate::jobs::{create_job, data_set_with_revision, job_cases, LAKE_FORMATION_TYPE};

/// What AWS opens a refusal about the request's own parameters with. Stated here
/// independently of the constant the crate composes it from, so a change to that
/// constant is a failure rather than a tautology.
const PREFIX: &str = "The following parameter(s) are not valid: ";

/// The whole clause AWS states a request's unsupported parameters with: it names none
/// of them.
const UNSUPPORTED: &str = "Unsupported request parameter(s) were found.";

/// The clause for a parameter the request has to carry and does not.
fn required(path: &str) -> String {
    format!("{path:?} is required")
}

/// The whole `Message` the rule requires for a request whose faults are `clauses`.
fn expected_message(clauses: &[String]) -> String {
    format!("{PREFIX}{}", clauses.join(", "))
}

/// One request at fault in its own parameters, and the `Message` its refusal must
/// carry.
struct Case {
    label: String,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    expected: String,
}

/// An instant for the `date-time` member a notification detail carries.
const AN_INSTANT: &str = "2024-01-01T00:00:00Z";

/// A well-formed `Action` for an event action, so a row about `Event` is refused for
/// its event rather than for its action.
fn an_action() -> Value {
    json!({ "ExportRevisionToS3": { "RevisionDestination": { "Bucket": "export-bucket" } } })
}

/// Assert one row: the status, the error code and the whole `Message`.
async fn assert_refused(router: &MultiServiceRouter, case: &Case) {
    let label = &case.label;
    let answer = call(router, case.method, &case.uri, case.body.clone()).await;
    assert_eq!(
        answer.status, 400,
        "case {label}: expected 400, got {} body {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error.as_deref(),
        Some("ValidationException"),
        "case {label}: expected ValidationException, got {:?}",
        answer.error
    );
    let message = answer.string("Message");
    assert_eq!(
        message, case.expected,
        "case {label}: expected {:?}, got {message:?}",
        case.expected
    );
}

/// Every way a `CreateJob` request's `Details` can disagree with its `Type`, for every
/// `Type` the model admits.
fn create_job_cases(data_set_id: &str, revision_id: &str) -> Vec<Case> {
    let cases = job_cases(data_set_id, revision_id);
    let mut rows: Vec<Case> = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        // The NEXT row's member, so every member of the catalog stands in as the
        // unsupported one for some type.
        let other = &cases[(index + 1) % cases.len()];
        let path = format!("Details.{}", case.member);
        rows.push(Case {
            label: format!("CreateJob {} with no Details member", case.job_type),
            method: "POST",
            uri: "/v1/jobs".to_string(),
            body: Some(json!({ "Type": case.job_type, "Details": {} })),
            expected: expected_message(&[required(&path)]),
        });
        rows.push(Case {
            label: format!(
                "CreateJob {} carrying {} instead",
                case.job_type, other.member
            ),
            method: "POST",
            uri: "/v1/jobs".to_string(),
            body: Some(json!({
                "Type": case.job_type,
                "Details": { other.member: other.detail.clone() },
            })),
            expected: expected_message(&[required(&path), UNSUPPORTED.to_string()]),
        });
        rows.push(Case {
            label: format!(
                "CreateJob {} carrying its own member and {}",
                case.job_type, other.member
            ),
            method: "POST",
            uri: "/v1/jobs".to_string(),
            body: Some(json!({
                "Type": case.job_type,
                "Details": {
                    case.member: case.detail.clone(),
                    other.member: other.detail.clone(),
                },
            })),
            expected: expected_message(&[UNSUPPORTED.to_string()]),
        });
    }

    // The one `Type` whose own details no request can carry (its `CatalogId` pattern
    // admits no value) is still cross-checked against its `Type`: the disagreement is
    // settled before any of that member's own constraints matter.
    let lake_formation = "Details.ImportAssetsFromLakeFormationTagPolicy";
    rows.push(Case {
        label: format!("CreateJob {LAKE_FORMATION_TYPE} with no Details member"),
        method: "POST",
        uri: "/v1/jobs".to_string(),
        body: Some(json!({ "Type": LAKE_FORMATION_TYPE, "Details": {} })),
        expected: expected_message(&[required(lake_formation)]),
    });
    rows.push(Case {
        label: format!(
            "CreateJob {LAKE_FORMATION_TYPE} carrying {} instead",
            cases[0].member
        ),
        method: "POST",
        uri: "/v1/jobs".to_string(),
        body: Some(json!({
            "Type": LAKE_FORMATION_TYPE,
            "Details": { cases[0].member: cases[0].detail.clone() },
        })),
        expected: expected_message(&[required(lake_formation), UNSUPPORTED.to_string()]),
    });
    rows
}

/// Every way a `SendDataSetNotification` request's `Details` can disagree with its
/// `Type`: the detail member a `DEPRECATION` or `SCHEMA_CHANGE` notification must
/// carry is absent, a member another type names is present, or both.
fn notification_cases(data_set_id: &str) -> Vec<Case> {
    let uri = format!("/v1/data-sets/{data_set_id}/notification");
    let data_update = json!({ "DataUpdate": { "DataUpdatedAt": AN_INSTANT } });
    let deprecation = json!({ "Deprecation": { "DeprecationAt": AN_INSTANT } });
    let schema_change = json!({ "SchemaChange": { "SchemaChangeAt": AN_INSTANT } });
    vec![
        Case {
            label: "SendDataSetNotification DEPRECATION with no Details at all".to_string(),
            method: "POST",
            uri: uri.clone(),
            body: Some(json!({ "Type": "DEPRECATION" })),
            expected: expected_message(&[required("Details.Deprecation")]),
        },
        Case {
            label: "SendDataSetNotification SCHEMA_CHANGE with an empty Details".to_string(),
            method: "POST",
            uri: uri.clone(),
            body: Some(json!({ "Type": "SCHEMA_CHANGE", "Details": {} })),
            expected: expected_message(&[required("Details.SchemaChange")]),
        },
        Case {
            label: "SendDataSetNotification DEPRECATION carrying DataUpdate".to_string(),
            method: "POST",
            uri: uri.clone(),
            body: Some(json!({ "Type": "DEPRECATION", "Details": data_update.clone() })),
            expected: expected_message(&[
                required("Details.Deprecation"),
                UNSUPPORTED.to_string(),
            ]),
        },
        Case {
            label: "SendDataSetNotification SCHEMA_CHANGE carrying Deprecation".to_string(),
            method: "POST",
            uri: uri.clone(),
            body: Some(json!({ "Type": "SCHEMA_CHANGE", "Details": deprecation.clone() })),
            expected: expected_message(&[
                required("Details.SchemaChange"),
                UNSUPPORTED.to_string(),
            ]),
        },
        Case {
            label: "SendDataSetNotification DATA_UPDATE carrying Deprecation".to_string(),
            method: "POST",
            uri: uri.clone(),
            body: Some(json!({ "Type": "DATA_UPDATE", "Details": deprecation })),
            expected: expected_message(&[UNSUPPORTED.to_string()]),
        },
        Case {
            label: "SendDataSetNotification DATA_DELAY carrying SchemaChange".to_string(),
            method: "POST",
            uri: uri.clone(),
            body: Some(json!({ "Type": "DATA_DELAY", "Details": schema_change })),
            expected: expected_message(&[UNSUPPORTED.to_string()]),
        },
        Case {
            label: "SendDataSetNotification DATA_UPDATE carrying DataUpdate and Deprecation"
                .to_string(),
            method: "POST",
            uri,
            body: Some(json!({
                "Type": "DATA_UPDATE",
                "Details": {
                    "DataUpdate": { "DataUpdatedAt": AN_INSTANT },
                    "Deprecation": { "DeprecationAt": AN_INSTANT },
                },
            })),
            expected: expected_message(&[UNSUPPORTED.to_string()]),
        },
    ]
}

/// `CreateEventAction`'s own member sets: the `Event` that starts the action and the
/// `Action` it then performs each carry exactly one member the request must name.
fn event_action_cases(data_set_id: &str) -> Vec<Case> {
    let published = json!({ "RevisionPublished": { "DataSetId": data_set_id } });
    vec![
        Case {
            label: "CreateEventAction whose Event names no member".to_string(),
            method: "POST",
            uri: "/v1/event-actions".to_string(),
            body: Some(json!({ "Event": {}, "Action": an_action() })),
            expected: expected_message(&[required("Event.RevisionPublished")]),
        },
        Case {
            label: "CreateEventAction whose Action names no member".to_string(),
            method: "POST",
            uri: "/v1/event-actions".to_string(),
            body: Some(json!({ "Event": published, "Action": {} })),
            expected: expected_message(&[required("Action.ExportRevisionToS3")]),
        },
        Case {
            label: "CreateEventAction whose Event and Action both name no member".to_string(),
            method: "POST",
            uri: "/v1/event-actions".to_string(),
            body: Some(json!({ "Event": {}, "Action": {} })),
            // Two faults in one request, so one clause each: the rule states every
            // fault of the request, not the first one found.
            expected: expected_message(&[
                required("Event.RevisionPublished"),
                required("Action.ExportRevisionToS3"),
            ]),
        },
    ]
}

/// RULE: every refusal about the request's own parameters carries the fixed opening
/// and one clause per fault — on `CreateJob` for every `Type` the model admits, on
/// `SendDataSetNotification`, and on an event action's `Event` and `Action`.
#[tokio::test]
async fn a_parameter_refusal_states_each_fault_after_the_fixed_opening() {
    let (router, _engine) = harness();
    let (data_set_id, revision_id) = data_set_with_revision(&router, "parameter-faults").await;

    let mut rows = create_job_cases(&data_set_id, &revision_id);
    rows.extend(notification_cases(&data_set_id));
    rows.extend(event_action_cases(&data_set_id));

    for case in &rows {
        assert_refused(&router, case).await;
    }

    // Each combination of faults the rule covers is actually exercised above: a
    // required parameter alone, an unsupported one alone, and both in one sentence.
    for (shape, carries_required, carries_unsupported) in [
        ("a required parameter alone", true, false),
        ("an unsupported parameter alone", false, true),
        ("a required parameter and an unsupported one", true, true),
    ] {
        let seen = rows
            .iter()
            .filter(|case| {
                case.expected.contains(" is required") == carries_required
                    && case.expected.contains(UNSUPPORTED) == carries_unsupported
            })
            .count();
        assert!(
            seen > 0,
            "case coverage of {shape}: expected at least one row among {}, got {seen}",
            rows.len()
        );
    }
}

/// RULE: the same opening and the same clause state a fault of the request body of an
/// UPDATE, so the sentence is the operation's parameters' and not the create's.
#[tokio::test]
async fn an_update_states_its_own_parameter_fault_the_same_way() {
    let (router, engine) = harness();
    let data_set_id = create_data_set(&router, "updatable-action", "for an update", "S3_SNAPSHOT", None)
        .await
        .string("Id");
    entitle_data_set(&engine, &data_set_id);

    let created = call(
        &router,
        "POST",
        "/v1/event-actions",
        Some(json!({
            "Event": { "RevisionPublished": { "DataSetId": data_set_id } },
            "Action": an_action(),
        })),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case setup: expected 201 from CreateEventAction, got {} body {:?}",
        created.status, created.body
    );
    let event_action_id = created.string("Id");

    assert_refused(
        &router,
        &Case {
            label: "UpdateEventAction whose Action names no member".to_string(),
            method: "PATCH",
            uri: format!("/v1/event-actions/{event_action_id}"),
            body: Some(json!({ "Action": {} })),
            expected: expected_message(&[required("Action.ExportRevisionToS3")]),
        },
    )
    .await;

    // The stored action is untouched by the refusal, so the sentence is about the
    // request rather than about the record.
    let read = call(
        &router,
        "GET",
        &format!("/v1/event-actions/{event_action_id}"),
        None,
    )
    .await;
    assert_eq!(
        read.member("Action"),
        &an_action(),
        "case the stored action after a refused update: expected it unchanged, got {:?}",
        read.member("Action")
    );
}

/// RULE: the opening is the same for a parameter fault of another kind — a value
/// outside the set the model admits for the parameter — so it is the sentence every
/// parameter refusal of this service opens with, not one operation's wording.
#[tokio::test]
async fn a_value_outside_a_parameters_modelled_set_opens_the_same_way() {
    let (router, _engine) = harness();

    // Source: the model's `Origin` enum, in the model's own order
    // (`smithy_query(query="shape", shape="Origin")`).
    let answer = call(&router, "GET", "/v1/data-sets?origin=NOT_AN_ORIGIN", None).await;
    assert_eq!(
        answer.status, 400,
        "case origin outside the modelled set: expected 400, got {} body {:?}",
        answer.status, answer.body
    );
    let message = answer.string("Message");
    assert_eq!(
        message,
        expected_message(&["\"origin\" must be one of [OWNED, ENTITLED].".to_string()]),
        "case origin outside the modelled set: expected the fixed opening and the value-set \
         clause, got {message:?}"
    );
}

/// RULE (negative control): a request whose parameters are exactly the admitted set is
/// not refused for its parameters — whatever else the service then says about it — so
/// the sentence above belongs to the fault and not to the operation.
#[tokio::test]
async fn a_request_whose_parameters_are_admitted_is_not_refused_for_them() {
    let (router, _engine) = harness();
    let (data_set_id, revision_id) = data_set_with_revision(&router, "admitted-parameters").await;

    for case in job_cases(&data_set_id, &revision_id) {
        let created = create_job(
            &router,
            case.job_type,
            json!({ case.member: case.detail.clone() }),
            None,
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {} with exactly its own Details member: expected 201, got {} body {:?}",
            case.job_type, created.status, created.body
        );
    }

    // A notification whose details ARE the ones its type names reaches the service's
    // next answer about it, which says nothing about the request's parameters.
    let notified = call(
        &router,
        "POST",
        &format!("/v1/data-sets/{data_set_id}/notification"),
        Some(json!({
            "Type": "DEPRECATION",
            "Details": { "Deprecation": { "DeprecationAt": AN_INSTANT } },
        })),
    )
    .await;
    let message = notified
        .member("Message")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !message.starts_with(PREFIX),
        "case a notification carrying the details its type names: expected no parameter \
         refusal, got {message:?}"
    );
}
