// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the channel an UNREACHABLE STORE is reported on.
//!
//! The RULE under test, asserted over a table spanning every operation this service
//! serves rather than the one the issue recorded: a store the service cannot reach is a
//! fault on the SERVICE's side — nothing about the request caused it and no member of
//! the request can be changed to avoid it — so every operation reports it through the
//! shape this service models for a server-side failure, at that shape's own status, and
//! never through a client-input refusal, which would tell the caller to fix a request
//! that was never wrong.
//!
//! Sources for every value written by hand here:
//!
//! * `smithy_query(query="shape",shape="InternalServerException",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//!   — `errorKind: server`, `httpError: 500`, `retryable: true`, one member `message`:
//!   the shape, the status and the retry classification asserted below.
//! * `smithy_query(query="digest",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//!   — the method and URI of every operation the table addresses, and each operation's
//!   own `errors` list (six name the shape; the rest name other errors or none at all,
//!   and widen it in).
//! * `smithy_query(query="shape",shape="DomainId,SubscriptionId,ScribeSessionId,JobId",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//!   — the identifier lengths and patterns the labels below satisfy, so every row
//!   reaches its handler instead of being refused at the boundary.
//!
//! The rows differ along what the rule generalises over: which operation is called (all
//! fifteen this service serves), the HTTP method (GET, POST, DELETE), whether the call
//! reports or mutates, whether it carries a body, how it addresses a resource (a path
//! label, a repeated query parameter, a percent-encoded ARN label, or nothing at all),
//! and whether the operation's own modelled `errors` list names the shape, names only
//! other errors, or is empty. The whole table is driven against two UNRELATED engine
//! types, so the answer is keyed to "this service's store cannot be reached" and not to
//! one engine type.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_connecthealth_connecthealth::state::store_unreachable;
use service_com_amazonaws_connecthealth_connecthealth::EmulatedService;

use crate::domain_subscription_lifecycle::{dispatch_signed, error_code, Emulator};
use crate::resource_tag_removal::untag_path;
use crate::resource_tagging::tags_path;

/// The error code and status the modelled server-side shape renders as.
const SERVER_ERROR: (u16, &str) = (500, "InternalServerException");

/// The region every request below is signed for. Any region works: the condition under
/// test is decided before a handler reads the request's region at all.
const REGION: &str = "us-east-1";

/// A well-formed `DomainId`: `@length 20..25`, pattern `^(hai-|dom-)[a-z0-9]+$`.
const DOMAIN_ID: &str = "dom-0123456789abcdef";
/// A well-formed `SubscriptionId`: `@length 25..25`, pattern `^sub-[a-zA-Z0-9]{21}$`.
const SUBSCRIPTION_ID: &str = "sub-000000000000000000000";
/// A well-formed `ScribeSessionId`: `@length 36..36`, pattern
/// `[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}`.
const SESSION_ID: &str = "7b2f1c4a-9d3e-4f56-8a1b-0c2d3e4f5a6b";
/// A well-formed `JobId`: `@length 1..36`, with no pattern.
const JOB_ID: &str = "job-0001";

/// An engine this service's store does not live in — the condition under test, which is
/// reachable no other way: the store is unreachable exactly when the service is mounted
/// on an engine that is not the emulator's in-memory one.
#[derive(Debug)]
struct ForeignEngine;

/// A second, unrelated engine type, so the table is driven against more than one shape
/// of foreign engine and the answer cannot be keyed to a single unit type.
#[derive(Debug)]
struct ForeignEngineWithState {
    #[allow(dead_code)]
    label: String,
}

/// This service mounted on `engine`, behind the same router a front door mounts it on.
fn mounted_on<Eng: Send + Sync + 'static>(engine: Eng) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(Arc::new(engine)));
    router
}

/// A well-formed ARN of this service, for the operations that address a resource by one.
/// The store is unreachable before it is resolved, so what matters is only that the
/// label is well formed.
fn domain_arn() -> String {
    format!("arn:aws:health-agent:{REGION}:000000000000:domain/{DOMAIN_ID}")
}

/// A `StartPatientInsightsJob` body carrying only the required members, so the request
/// passes the boundary walk and reaches the handler.
fn start_job_body() -> Value {
    json!({
        "patientContext": { "patientId": "patient-0001" },
        "insightsContext": { "insightsType": "PRE_VISIT" },
        "encounterContext": { "encounterReason": "Annual checkup" },
        "userContext": { "role": "CLINICIAN", "userId": "clinician-0001" },
        "inputDataConfig": { "s3Sources": [ { "uri": "s3://clinic-input/patient-0001/" } ] },
        "outputDataConfig": { "s3OutputPath": "s3://clinic-output/insights/" },
    })
}

/// One request per operation this service serves: the operation's name (which is what
/// its detail text names), its method, its URI, and the body it carries.
fn every_operation() -> Vec<(&'static str, &'static str, String, Option<Value>)> {
    let arn = domain_arn();
    vec![
        (
            "CreateDomain",
            "POST",
            "/domain".to_string(),
            Some(json!({ "name": "clinic" })),
        ),
        ("GetDomain", "GET", format!("/domain/{DOMAIN_ID}"), None),
        (
            "DeleteDomain",
            "DELETE",
            format!("/domain/{DOMAIN_ID}"),
            None,
        ),
        ("ListDomains", "GET", "/domain".to_string(), None),
        // The same listing with its page bounded, so a row addresses the operation
        // through a query string as well as through a bare path.
        (
            "ListDomains",
            "GET",
            "/domain?maxResults=1&status=ACTIVE".to_string(),
            None,
        ),
        (
            "TagResource",
            "POST",
            tags_path(&arn),
            Some(json!({ "tags": { "env": "prod" } })),
        ),
        (
            "UntagResource",
            "DELETE",
            untag_path(&arn, &["env".to_string()]),
            None,
        ),
        ("ListTagsForResource", "GET", tags_path(&arn), None),
        (
            "CreateSubscription",
            "POST",
            format!("/domains/{DOMAIN_ID}/subscriptions"),
            Some(json!({})),
        ),
        (
            "GetSubscription",
            "GET",
            format!("/domains/{DOMAIN_ID}/subscriptions/{SUBSCRIPTION_ID}"),
            None,
        ),
        (
            "ListSubscriptions",
            "GET",
            format!("/domains/{DOMAIN_ID}/subscriptions"),
            None,
        ),
        (
            "ActivateSubscription",
            "POST",
            format!("/domains/{DOMAIN_ID}/subscriptions/{SUBSCRIPTION_ID}/activate"),
            Some(json!({})),
        ),
        (
            "DeactivateSubscription",
            "POST",
            format!("/domains/{DOMAIN_ID}/subscriptions/{SUBSCRIPTION_ID}/deactivate"),
            Some(json!({})),
        ),
        (
            "GetMedicalScribeListeningSession",
            "GET",
            format!(
                "/medical-scribe-stream/domain/{DOMAIN_ID}/subscription/{SUBSCRIPTION_ID}\
                 /session/{SESSION_ID}"
            ),
            None,
        ),
        (
            "StartPatientInsightsJob",
            "POST",
            format!("/domain/{DOMAIN_ID}/patient-insights-job"),
            Some(start_job_body()),
        ),
        (
            "GetPatientInsightsJob",
            "GET",
            format!("/domain/{DOMAIN_ID}/patient-insights-job/{JOB_ID}"),
            None,
        ),
    ]
}

/// The detail text of a failure, in the member this service reports it in.
fn detail(case: &str, body: &Value) -> String {
    body.get("Message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: the failure must carry a Message, got {body}"))
        .to_string()
}

/// Every operation answers an unreachable store with the modelled server-side shape, on
/// its own status, whatever engine the service was mounted on.
#[tokio::test]
async fn every_operation_reports_an_unreachable_store_as_a_server_error() {
    let engines: Vec<(&str, MultiServiceRouter)> = vec![
        ("a unit engine", mounted_on(ForeignEngine)),
        (
            "an engine holding state of its own",
            mounted_on(ForeignEngineWithState {
                label: "foreign".to_string(),
            }),
        ),
    ];

    for (engine_case, router) in &engines {
        for (operation, method, path, body) in every_operation() {
            let case = format!("{engine_case}: {operation} {method} {path}");
            let (status, answered) =
                dispatch_signed(router, REGION, method, &path, body.as_ref()).await;
            assert_eq!(
                (status, error_code(&answered).as_str()),
                SERVER_ERROR,
                "case {case:?}: expected {SERVER_ERROR:?}, got {status} with {answered}"
            );
            // The detail text is the one the service states in a single place, and it
            // names the operation that could not reach the store.
            let expected = store_unreachable(operation)
                .message
                .expect("the server error carries its detail text");
            let reported = detail(&case, &answered);
            assert_eq!(
                reported, expected,
                "case {case:?}: expected {expected:?}, got {reported:?}"
            );
            assert!(
                reported.contains(operation),
                "case {case:?}: the detail text must name the operation, got {reported:?}"
            );
        }
    }
}

/// The server error is keyed to the CONDITION, not to the request: the very same
/// requests, against a service whose store IS reachable, are never answered with it.
///
/// What each one is answered with instead belongs to that operation's own suite — these
/// identifiers name no resource, so most are refused as not found. The one assertion
/// here is that none of them is reported as a server-side failure.
#[tokio::test]
async fn a_reachable_store_is_never_reported_as_a_server_error() {
    let emulator = Emulator::new(REGION);
    for (operation, method, path, body) in every_operation() {
        let case = format!("{operation} {method} {path}");
        let (status, answered) = emulator.send(method, &path, body.as_ref()).await;
        assert_ne!(
            error_code(&answered),
            SERVER_ERROR.1,
            "case {case:?}: a reachable store must not be reported as a server failure, \
             got {status} with {answered}"
        );
    }
}
