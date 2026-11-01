// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the THREE members that report a function's last update.
//!
//! The rule under test: `LastUpdateStatus`, `LastUpdateStatusReason` and
//! `LastUpdateStatusReasonCode` report one thing — "The LastUpdateStatus,
//! LastUpdateStatusReason, and LastUpdateStatusReasonCode fields in the response from
//! GetFunctionConfiguration indicate when the update is complete"
//! (`smithy_query(query="doc", shape="UpdateFunctionConfigurationRequest")`). So while an
//! accepted update is still being applied the status is the in-flight one AND the pair
//! names the work in flight — Lambda provisions a new instance of the function, which is
//! the same work and the same reason a function still being created reports — and once the
//! clock has settled the update the status is the settled one and neither member is
//! reported, because no work is in flight to give a reason for.
//!
//! The rows differ along what the rule generalises over: the OPERATION that starts the
//! window (`UpdateFunctionConfiguration`, which replaces settings, and
//! `UpdateFunctionCode`, which replaces the deployment package), and the place the members
//! are read from (the operation's own answer, a later `GetFunctionConfiguration`, a later
//! `GetFunction`). A published version is read too: it froze its configuration before any
//! update began, so it is not the thing being updated.
//!
//! Nothing here sleeps: the clock-owed `Pending` -> `Active` and `InProgress` ->
//! `Successful` moves are brought forward by moving the stored due instants into the past.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use smithy_server_runtime::MultiServiceRouter;

use crate::function_lifecycle::{
    create_function, json_call, make_activation_due, package_bytes, router_on, zip_file, Answer,
    ROLE,
};

/// Where the function operations are bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The `LastUpdateStatus` of an update still being applied, from the model's own
/// `LastUpdateStatus` enum (`Successful | Failed | InProgress`).
const IN_PROGRESS: &str = "InProgress";

/// The `LastUpdateStatus` of a settled update, from the same enum.
const SETTLED: &str = "Successful";

/// The reason the in-flight pair reports, and its code: the work in flight is the
/// provisioning of a new instance of the function, which is what the service's own
/// `Creating` reason names.
const IN_FLIGHT_REASON: &str = "The function is being created.";
const IN_FLIGHT_REASON_CODE: &str = "Creating";

/// `GetFunctionConfiguration`, optionally qualified.
async fn get_configuration(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: Option<&str>,
) -> Answer {
    let uri = match qualifier {
        Some(qualifier) => {
            format!("{FUNCTIONS_URI}/{reference}/configuration?Qualifier={qualifier}")
        }
        None => format!("{FUNCTIONS_URI}/{reference}/configuration"),
    };
    json_call(router, "GET", &uri, &Value::Null).await
}

/// `GetFunction`, whose `Configuration` carries the same three members.
async fn get_function(router: &MultiServiceRouter, reference: &str) -> Answer {
    let uri = format!("{FUNCTIONS_URI}/{reference}");
    json_call(router, "GET", &uri, &Value::Null).await
}

/// `PublishVersion`.
async fn publish_version(router: &MultiServiceRouter, reference: &str) -> Answer {
    let uri = format!("{FUNCTIONS_URI}/{reference}/versions");
    json_call(router, "POST", &uri, &Value::Null).await
}

/// Move every in-flight update's settle instant into the past, so the next read
/// materialises the move the clock owes. This is what waiting would do, without waiting.
fn make_update_due(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        if function.last_update_due.is_some() {
            function.last_update_due = Some(past);
        }
    }
}

/// One row: the operation that starts an update window, and the request that drives it.
struct Case {
    /// Names the row, and names its function, so rows never share state.
    case: &'static str,
    /// The method and URI suffix of the operation under test.
    method: &'static str,
    suffix: &'static str,
    /// Its request body.
    body: Value,
}

/// The operations that start an update window: one replaces settings, the other replaces
/// the deployment package, and both provision a new instance of the function.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "update-function-configuration",
            method: "PUT",
            suffix: "configuration",
            body: json!({ "Timeout": 11, "Description": "as updated" }),
        },
        Case {
            case: "update-function-code",
            method: "PUT",
            suffix: "code",
            body: json!({ "ZipFile": zip_file(&package_bytes(96)) }),
        },
    ]
}

/// Assert the three members of `body` (an object carrying a `FunctionConfiguration`).
fn assert_update_members(case: &str, where_from: &str, body: &Value, in_flight: bool) {
    let expected_status = if in_flight { IN_PROGRESS } else { SETTLED };
    assert_eq!(
        body.get("LastUpdateStatus"),
        Some(&json!(expected_status)),
        "case {case:?}: {where_from} must report LastUpdateStatus {expected_status}, got {:?}",
        body.get("LastUpdateStatus")
    );
    let expected_reason = in_flight.then(|| json!(IN_FLIGHT_REASON));
    assert_eq!(
        body.get("LastUpdateStatusReason").cloned(),
        expected_reason,
        "case {case:?}: {where_from} must report LastUpdateStatusReason {expected_reason:?}, \
         got {:?}",
        body.get("LastUpdateStatusReason")
    );
    let expected_code = in_flight.then(|| json!(IN_FLIGHT_REASON_CODE));
    assert_eq!(
        body.get("LastUpdateStatusReasonCode").cloned(),
        expected_code,
        "case {case:?}: {where_from} must report LastUpdateStatusReasonCode \
         {expected_code:?}, got {:?}",
        body.get("LastUpdateStatusReasonCode")
    );
}

#[tokio::test]
async fn an_update_reports_its_reason_while_it_is_in_flight_and_none_once_it_settles() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let code = package_bytes(64);

    for case in cases() {
        let name = case.case;
        let created = create_function(
            &router,
            &json!({
                "FunctionName": name,
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Timeout": 7,
                "MemorySize": 256,
                "Code": { "ZipFile": zip_file(&code) },
            }),
        )
        .await;
        assert_eq!(
            created.status,
            201,
            "case {name:?}: expected 201 from CreateFunction, got {} {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        // A function still being created is not modifiable, so the row starts from the
        // settled record a caller that waited would see: creation completing sets the
        // status to the settled one, with no work in flight to report a reason for.
        make_activation_due(&engine);
        let baseline = get_configuration(&router, name, None).await;
        assert_update_members(name, "the settled baseline read", &baseline.json(), false);

        // The published version froze this configuration before any update began.
        let published = publish_version(&router, name).await;
        assert_eq!(
            published.status,
            201,
            "case {name:?}: expected 201 from PublishVersion, got {} {}",
            published.status,
            String::from_utf8_lossy(&published.body)
        );

        let uri = format!("{FUNCTIONS_URI}/{name}/{}", case.suffix);
        let answer = json_call(&router, case.method, &uri, &case.body).await;
        assert_eq!(
            answer.status,
            200,
            "case {name:?}: expected 200 from the update, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_update_members(name, "the update's own answer", &answer.json(), true);

        // Every read of the unpublished configuration reports what the update persisted.
        let read = get_configuration(&router, name, None).await;
        assert_update_members(name, "GetFunctionConfiguration", &read.json(), true);
        let whole = get_function(&router, name).await;
        assert_update_members(
            name,
            "GetFunction's Configuration",
            &whole.json()["Configuration"],
            true,
        );
        // ... and the published version is not the thing being updated.
        let frozen = get_configuration(&router, name, Some("1")).await;
        assert_update_members(name, "the published version's read", &frozen.json(), false);

        // Once the clock has made the update due, the work in flight is finished.
        make_update_due(&engine);
        let settled = get_configuration(&router, name, None).await;
        assert_update_members(name, "the settled read", &settled.json(), false);
    }
}
