// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for cancelling a Greengrass V2 deployment, and for how every
//! operation keyed by a `deploymentId` refuses an id this service holds no deployment for.
//!
//! The rules under test:
//!
//! * cancelling a stored deployment moves it to the `CANCELED` terminal status its own
//!   reader reports, and asking again for a status it already has changes nothing — the
//!   operation names the end state, not a step; and
//! * a `deploymentId` this service holds no deployment for is refused `404`
//!   `ResourceNotFoundException` with one fixed sentence that reports no value of the
//!   request, whichever operation was asked and whatever the id looked like.
//!
//! Every assertion drives the real `restJson1` front door: the request is built the way
//! an SDK builds it, dispatched through [`MultiServiceRouter`], and judged on the HTTP
//! status and the JSON body. Nothing reaches into the state struct, so a cancel that
//! discarded the write instead of persisting it fails here.
//!
//! The rows differ along what the rules generalise over — the FORM of the id that names
//! nothing (a fresh UUID, a hex token, one character, a long one, the id of a deployment
//! of another target), the operation asked, and how many times a cancel is repeated — not
//! along the target names, which the rules say nothing about.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_greengrassv2_greengrassv2::state::DEPLOYMENT_NOT_FOUND_MESSAGE;
use service_com_amazonaws_greengrassv2_greengrassv2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for; every minted ARN carries it.
const REGION: &str = "us-west-2";

/// The account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// `DeploymentStatus` values this suite expects
/// (`smithy_query(query="shape",shape="DeploymentStatus")`: `ACTIVE`, `COMPLETED`,
/// `CANCELED`, `FAILED`, `INACTIVE`).
const STATUS_ACTIVE: &str = "ACTIVE";
const STATUS_CANCELED: &str = "CANCELED";

/// The emulator front door with only this service mounted, over ONE shared engine —
/// exactly how `core/router` assembles it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one signed request and return its status and parsed JSON body.
async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Vec<u8>) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://greengrass.{REGION}.amazonaws.com{path}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/greengrass/aws4_request, \
                 SignedHeaders=host, Signature=00"
            ),
        )
        .header("content-type", "application/json")
        .body(Bytes::from(body))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or(Value::String(String::from_utf8_lossy(&raw).into_owned()))
    };
    (status, value)
}

/// A string member of a JSON object, or the empty string when absent.
fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The wire error code of a `restJson1` error body.
fn error_code(body: &Value) -> String {
    let code = text(body, "__type");
    if code.is_empty() {
        "<no __type>".to_string()
    } else {
        code
    }
}

/// An IoT thing ARN, which is what a deployment targets.
fn thing_target(name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT}:thing/{name}")
}

/// `CreateDeployment` for `target`; answers the new deployment's id.
async fn create_deployment(router: &MultiServiceRouter, target: &str) -> String {
    let body = serde_json::to_vec(&json!({ "targetArn": target }))
        .expect("a serializable request body");
    let (status, created) = call(router, "POST", "/greengrass/v2/deployments", body).await;
    assert_eq!(
        status, 201,
        "CreateDeployment must answer 201, got {status} {created}"
    );
    let id = text(&created, "deploymentId");
    assert!(
        !id.is_empty(),
        "CreateDeployment must answer a deployment id, got {created}"
    );
    id
}

/// `CancelDeployment` on `deployment_id` (`POST /greengrass/v2/deployments/{id}/cancel`).
async fn cancel(router: &MultiServiceRouter, deployment_id: &str) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/greengrass/v2/deployments/{deployment_id}/cancel"),
        Vec::new(),
    )
    .await
}

/// `GetDeployment` on `deployment_id`.
async fn get_deployment(router: &MultiServiceRouter, deployment_id: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/greengrass/v2/deployments/{deployment_id}"),
        Vec::new(),
    )
    .await
}

#[tokio::test]
async fn cancelling_a_deployment_moves_it_to_canceled_and_stays_there() {
    let router = emulator();

    // Several deployments, so the cancel has to find the one its id names rather than
    // the only one there is — and so the others prove nothing else moved.
    let targets = ["CoreOne", "CoreTwo", "CoreThree"];
    let ids: Vec<String> = {
        let mut ids = Vec::new();
        for target in targets {
            ids.push(create_deployment(&router, &thing_target(target)).await);
        }
        ids
    };

    for (index, id) in ids.iter().enumerate() {
        let (status, body) = get_deployment(&router, id).await;
        assert_eq!(
            (status, text(&body, "deploymentStatus").as_str()),
            (200, STATUS_ACTIVE),
            "case deployment {index} before any cancel: expected an ACTIVE deployment, \
             got {status} {body}"
        );
    }

    // Cancelling the FIRST deployment twice: the second call asks for a status the
    // deployment already has, which is not a failure and changes nothing.
    let target_id = &ids[0];
    for attempt in 1..=2 {
        let (status, body) = cancel(&router, target_id).await;
        assert_eq!(
            status, 200,
            "case cancel attempt {attempt}: expected 200, got {status} {body}"
        );
        assert!(
            !text(&body, "message").is_empty(),
            "case cancel attempt {attempt}: the response must communicate the outcome, \
             got {body}"
        );

        let (status, read) = get_deployment(&router, target_id).await;
        assert_eq!(
            (status, text(&read, "deploymentStatus").as_str()),
            (200, STATUS_CANCELED),
            "case cancel attempt {attempt}: the deployment must read back CANCELED, \
             got {status} {read}"
        );
        // Cancelling changes the status and nothing else about the deployment.
        assert_eq!(
            text(&read, "targetArn"),
            thing_target(targets[0]),
            "case cancel attempt {attempt}: the deployment keeps its target, got {read}"
        );
        assert_eq!(
            text(&read, "deploymentId"),
            *target_id,
            "case cancel attempt {attempt}: the deployment keeps its id, got {read}"
        );
    }

    // Every other deployment is untouched: one cancel moves one deployment.
    for (index, id) in ids.iter().enumerate().skip(1) {
        let (status, body) = get_deployment(&router, id).await;
        assert_eq!(
            (status, text(&body, "deploymentStatus").as_str()),
            (200, STATUS_ACTIVE),
            "case deployment {index} after cancelling another: expected it to stay ACTIVE, \
             got {status} {body}"
        );
    }
}

#[tokio::test]
async fn a_deployment_id_naming_nothing_is_refused_the_same_way_everywhere() {
    let router = emulator();

    // One real deployment exists and is then cancelled, so no row below is refused
    // because the store is empty, and a cancelled deployment is still FOUND.
    let live = create_deployment(&router, &thing_target("LiveCore")).await;
    let (status, body) = cancel(&router, &live).await;
    assert_eq!(
        status, 200,
        "cancelling the live deployment must succeed, got {status} {body}"
    );

    // The rows differ in the FORM of an id that names nothing: the shape
    // `CreateDeployment` mints, a hex token, the shortest id the modelled
    // `NonEmptyString` (length 1..) admits, a long one, and an id of the right shape
    // that was simply never minted.
    let unknown = [
        (
            "a UUID nothing created",
            "44444444-4444-4444-4444-444444444444".to_string(),
        ),
        ("a hex token", "deadbeefcafe".to_string()),
        ("a one-character id", "d".to_string()),
        ("a long id", "d".repeat(256)),
        (
            "the live deployment's id with one character changed",
            flip_last(&live),
        ),
    ];

    for (case, id) in unknown {
        for (operation, answer) in [
            ("CancelDeployment", cancel(&router, &id).await),
            ("GetDeployment", get_deployment(&router, &id).await),
        ] {
            let (status, body) = answer;
            assert_eq!(
                status, 404,
                "case {case:?} on {operation}: expected 404, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?} on {operation}: expected ResourceNotFoundException, got {body}"
            );
            assert_eq!(
                text(&body, "message"),
                DEPLOYMENT_NOT_FOUND_MESSAGE,
                "case {case:?} on {operation}: expected the service's fixed not-found \
                 sentence, got {body}"
            );
            // The refusal names the kind of resource the id addressed, and reports the
            // id as the resource asked about rather than inside the sentence.
            assert_eq!(
                text(&body, "resourceType"),
                "DEPLOYMENT",
                "case {case:?} on {operation}: expected the DEPLOYMENT resource type, got {body}"
            );
            assert_eq!(
                text(&body, "resourceId"),
                id,
                "case {case:?} on {operation}: expected the requested id, got {body}"
            );
        }
    }

    // The stored deployment is still there, so every row above was refused for the id it
    // named and not because the cancel lost the deployment.
    let (status, body) = get_deployment(&router, &live).await;
    assert_eq!(
        (status, text(&body, "deploymentStatus").as_str()),
        (200, STATUS_CANCELED),
        "the cancelled deployment must still be readable, got {status} {body}"
    );
}

/// `id` with its last character replaced by a different one, which yields an id of the
/// same shape that no deployment carries.
fn flip_last(id: &str) -> String {
    let mut flipped: Vec<char> = id.chars().collect();
    if let Some(last) = flipped.last_mut() {
        *last = if *last == 'a' { 'b' } else { 'a' };
    }
    flipped.into_iter().collect()
}
