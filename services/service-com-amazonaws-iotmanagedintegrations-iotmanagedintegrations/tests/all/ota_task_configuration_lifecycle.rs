// SPDX-License-Identifier: CC0-1.0

//! The over-the-air (OTA) task-configuration round trip, end to end over the wire:
//! `CreateOtaTaskConfiguration` -> `GetOtaTaskConfiguration`.
//!
//! Every case drives the real restJson1 request path (`MultiServiceRouter::dispatch`), so
//! the routes, status codes, bodies and modelled errors asserted here are the ones a
//! client sees. The suite carries its own request harness, as each suite in this crate
//! does, so a case reads as one HTTP call.
//!
//! RULE under test: a create persists the configuration the request described under a
//! freshly minted `OtaTaskConfigurationId` scoped to the request's region, answers with
//! that id, and a read of that id projects the STORED record — so every member the create
//! carried comes back unchanged and every member it omitted stays absent. The rule is
//! checked over a TABLE of inputs spanning what it generalises over (which members are
//! present, how deeply the `PushConfig` structure is nested, how many abort criteria it
//! lists, and values at the published bounds of each constrained member), not over one
//! recorded request.
//!
//! The neighbouring rules the same surface owes are checked too: an identifier the store
//! does not hold is a modelled `ResourceNotFoundException`; a record is only visible
//! through the region that created it; a replayed `ClientToken` (`@idempotencyToken`)
//! answers with what the first call created instead of creating a second record; a
//! request outside a modelled constraint is refused and persists nothing; and a read
//! changes nothing a later caller sees.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::state::OTA_TASK_CONFIGURATION_RESOURCE_TYPE;
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::{
    ComAmazonawsIotmanagedintegrationsIotmanagedintegrationsState as ServiceState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's requests are signed for.
const REGION: &str = "us-east-1";

/// The route `CreateOtaTaskConfiguration` and `ListOtaTaskConfigurations` are bound to
/// (`smithy_query(query="operation",operation="CreateOtaTaskConfiguration")` ->
/// `POST /ota-task-configurations`).
const COLLECTION: &str = "/ota-task-configurations";

/// `OtaTaskConfigurationName`'s published `@length` maximum, from
/// `smithy_query(query="operation",operation="CreateOtaTaskConfiguration")`
/// (`Name`: len 1..128, pattern `^[A-Za-z0-9-_ ]+$`).
const NAME_MAX_LENGTH: usize = 128;

/// `OtaDescription`'s published `@length` maximum, from the same call
/// (`Description`: len 1..256, pattern `^[0-9A-Za-z_\- ]+$`).
const DESCRIPTION_MAX_LENGTH: usize = 256;

/// `InProgressTimeoutInMinutes`' published `@range`, from
/// `smithy_query(query="shape",shape="OtaTaskTimeoutConfig")` (min 1, max 10080).
const TIMEOUT_IN_MINUTES_RANGE: (i64, i64) = (1, 10_080);

/// The members of the create request that a read projects back. `TaskConfigurationId` and
/// `CreatedAt` are the service's own and are asserted separately.
const ECHOED_MEMBERS: &[&str] = &["Name", "Description", "PushConfig"];

/// One HTTP answer, reduced to what the assertions read.
struct Reply {
    status: u16,
    /// The restJson1 error code, from `x-amzn-errortype`.
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    fn member(&self, name: &str) -> Option<&Value> {
        self.body.get(name).filter(|value| !value.is_null())
    }

    fn string(&self, name: &str) -> Option<&str> {
        self.member(name).and_then(Value::as_str)
    }

    fn seconds(&self, name: &str) -> Option<f64> {
        self.member(name).and_then(Value::as_f64)
    }
}

/// A router holding one `iotmanagedintegrations` service over a fresh shared engine: the
/// same assembly `core/router` performs, with one service mounted.
fn emulator() -> MultiServiceRouter {
    emulator_over(Arc::new(engine::InMemoryEngine::new()))
}

/// [`emulator`] over an engine the caller keeps a handle to, so a test can inspect the
/// service's store directly.
fn emulator_over(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // The signature itself is not under test here, so requests are unauthenticated.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one request and reduce the answer. The `Authorization` credential scope is what
/// routes the request to this service and names the region its records belong to.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> Reply {
    call_signed(router, REGION, method, path_and_query, body).await
}

/// [`call`], signed for an explicit region, so a regional resource can be addressed
/// through another region's endpoint.
async fn call_signed(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> Reply {
    let payload = body.map_or_else(bytes::Bytes::new, |value| {
        bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize the request body"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://iotmanagedintegrations.{region}.amazonaws.com{path_and_query}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{region}/\
                 iotmanagedintegrations/aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Reply {
        status,
        error_type,
        body,
    }
}

/// `CreateOtaTaskConfiguration` on `request`, asserting the modelled 201 and the shape of
/// the generated id, and returning it.
async fn create(router: &MultiServiceRouter, case: &str, request: Value) -> String {
    let reply = call(router, "POST", COLLECTION, Some(request)).await;
    assert_eq!(
        reply.status, 201,
        "case {case:?}: expected HTTP 201 from CreateOtaTaskConfiguration, got {} with body {:?}",
        reply.status, reply.body
    );
    let id = reply
        .string("TaskConfigurationId")
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a TaskConfigurationId, got body {:?}",
                reply.body
            )
        })
        .to_string();
    // `OtaTaskConfigurationId`: len 1..64, pattern ^[a-zA-Z0-9]*$.
    assert!(
        !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric()),
        "case {case:?}: TaskConfigurationId must match ^[a-zA-Z0-9]*$ (len 1..64), got {id:?}"
    );
    id
}

/// The one abort-criteria entry `action`/`failure_type` describe, with values inside the
/// published ranges of `AbortConfigCriteria`
/// (`MinNumberOfExecutedThings` 1..100, `ThresholdPercentage` ..100).
fn abort_criteria(action: &str, failure_type: &str, executed: i64, threshold: f64) -> Value {
    json!({
        "Action": action,
        "FailureType": failure_type,
        "MinNumberOfExecutedThings": executed,
        "ThresholdPercentage": threshold,
    })
}

/// RULE (see the module docs): a read projects the stored record, so every member a
/// create carried comes back unchanged and every member it omitted stays absent.
#[tokio::test]
async fn a_read_projects_what_the_create_stored() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));

    // The rows differ along what the rule generalises over: which members are present,
    // how deeply `PushConfig` nests, how many abort criteria it lists, and values at the
    // published bounds of each constrained member. The recorded request this issue was
    // filed about is one row ("name, description and a push config").
    let cases: Vec<(&str, Value)> = vec![
        // No member at all: the model marks none of them `@required`.
        ("every member absent", json!({})),
        ("name only", json!({"Name": "ota config one"})),
        (
            "description only",
            json!({"Description": "fleet rollout stage one"}),
        ),
        (
            "name and description at their published length bounds",
            json!({
                "Name": "n".repeat(NAME_MAX_LENGTH),
                "Description": "d".repeat(DESCRIPTION_MAX_LENGTH),
            }),
        ),
        (
            "a one-character name and description (the length minimum)",
            json!({"Name": "a", "Description": "b"}),
        ),
        (
            "an empty push config structure",
            json!({"Name": "empty push config", "PushConfig": {}}),
        ),
        (
            "a timeout config at the published range minimum",
            json!({
                "Name": "timeout at the minimum",
                "PushConfig": {
                    "TimeoutConfig": {"InProgressTimeoutInMinutes": TIMEOUT_IN_MINUTES_RANGE.0},
                },
            }),
        ),
        (
            "a timeout config at the published range maximum",
            json!({
                "Name": "timeout at the maximum",
                "PushConfig": {
                    "TimeoutConfig": {"InProgressTimeoutInMinutes": TIMEOUT_IN_MINUTES_RANGE.1},
                },
            }),
        ),
        (
            "one abort criteria entry",
            json!({
                "Name": "one abort criteria",
                "PushConfig": {
                    "AbortConfig": {
                        "AbortConfigCriteriaList": [
                            abort_criteria("CANCEL", "FAILED", 1, 10.5),
                        ],
                    },
                },
            }),
        ),
        (
            "three abort criteria entries, reported in the order sent",
            json!({
                "Name": "three abort criteria",
                "PushConfig": {
                    "AbortConfig": {
                        "AbortConfigCriteriaList": [
                            abort_criteria("CANCEL", "ALL", 100, 100.0),
                            abort_criteria("CANCEL", "REJECTED", 50, 25.25),
                            abort_criteria("CANCEL", "TIMED_OUT", 1, 0.5),
                        ],
                    },
                },
            }),
        ),
        (
            "name, description and a push config",
            json!({
                "Name": "fleet lamps config",
                "Description": "rollout for the lamp fleet",
                "PushConfig": {
                    "RolloutConfig": {
                        "MaximumPerMinute": 25,
                        "ExponentialRolloutRate": {
                            "BaseRatePerMinute": 10,
                            "IncrementFactor": 1.5,
                            "RateIncreaseCriteria": {
                                "numberOfNotifiedThings": 20,
                                "numberOfSucceededThings": 10,
                            },
                        },
                    },
                    "TimeoutConfig": {"InProgressTimeoutInMinutes": 60},
                },
            }),
        ),
        (
            "every branch of the push config at once",
            json!({
                "Name": "whole push config",
                "Description": "abort rollout and timeout together",
                "PushConfig": {
                    "AbortConfig": {
                        "AbortConfigCriteriaList": [
                            abort_criteria("CANCEL", "FAILED", 2, 75.5),
                        ],
                    },
                    "RolloutConfig": {
                        "MaximumPerMinute": 1,
                        "ExponentialRolloutRate": {
                            "BaseRatePerMinute": 100,
                            "IncrementFactor": 1.1,
                            "RateIncreaseCriteria": {
                                "numberOfNotifiedThings": 100,
                                "numberOfSucceededThings": 100,
                            },
                        },
                    },
                    "TimeoutConfig": {"InProgressTimeoutInMinutes": 10_080},
                },
            }),
        ),
    ];

    let mut ids = BTreeSet::new();
    for (case, request) in &cases {
        let id = create(&router, case, request.clone()).await;
        assert!(
            ids.insert(id.clone()),
            "case {case:?}: each create must mint a fresh id, got {id:?} twice"
        );

        let read = call(&router, "GET", &format!("{COLLECTION}/{id}"), None).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetOtaTaskConfiguration, got {} with body {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.string("TaskConfigurationId"),
            Some(id.as_str()),
            "case {case:?}: the read must report the created id, got {:?}",
            read.member("TaskConfigurationId")
        );
        assert!(
            read.seconds("CreatedAt").is_some_and(|secs| secs > 0.0),
            "case {case:?}: expected a CreatedAt timestamp, got {:?}",
            read.member("CreatedAt")
        );
        for member in ECHOED_MEMBERS {
            let sent = request.get(member);
            assert_eq!(
                read.member(member),
                sent,
                "case {case:?}: the read must report {member:?} exactly as the create carried \
                 it (expected {sent:?}), got {:?}",
                read.member(member)
            );
        }
    }

    assert_eq!(
        engine
            .state::<ServiceState>()
            .lock()
            .ota_task_configurations
            .len(),
        cases.len(),
        "every create must have persisted exactly one record"
    );
}

/// RULE: an `Identifier` the store does not hold is answered with the modelled
/// `ResourceNotFoundException` — HTTP 404, the identifier on `ResourceId` and the
/// resource's type on `ResourceType` — and the read persists nothing.
#[tokio::test]
async fn a_read_of_an_unknown_identifier_is_the_modelled_not_found() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));

    let stored = create(&router, "seed", json!({"Name": "seed config"})).await;

    // Identifiers the model admits (^[a-zA-Z0-9]*$, len 1..64) that were never minted:
    // the shortest one, a long one, and one differing from a stored id in its last
    // character.
    let mut near_miss = stored.clone();
    near_miss.pop();
    near_miss.push(if stored.ends_with('z') { 'y' } else { 'z' });
    let cases = [
        ("one character", "a".to_string()),
        ("sixty-four characters", "b".repeat(64)),
        ("one character off a stored id", near_miss),
    ];

    for (case, identifier) in &cases {
        let reply = call(&router, "GET", &format!("{COLLECTION}/{identifier}"), None).await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected HTTP 404 for an identifier that was never minted, got {} \
             with body {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected the modelled ResourceNotFoundException code, got {:?}",
            reply.error_type
        );
        assert_eq!(
            reply.string("ResourceId"),
            Some(identifier.as_str()),
            "case {case:?}: ResourceId must report the identifier the request carried, got {:?}",
            reply.member("ResourceId")
        );
        assert_eq!(
            reply.string("ResourceType"),
            Some(OTA_TASK_CONFIGURATION_RESOURCE_TYPE),
            "case {case:?}: ResourceType must report the resource's type, got {:?}",
            reply.member("ResourceType")
        );
    }

    assert_eq!(
        engine
            .state::<ServiceState>()
            .lock()
            .ota_task_configurations
            .len(),
        1,
        "a refused read must not have written anything"
    );
}

/// RULE: an OTA task configuration is a regional resource, so only the region that
/// created it resolves it; and a read changes nothing a later caller sees.
#[tokio::test]
async fn a_record_resolves_only_through_the_region_that_created_it() {
    let router = emulator();

    let id = create(&router, "regional", json!({"Name": "regional config"})).await;

    for other_region in ["us-west-2", "eu-central-1"] {
        let elsewhere = call_signed(
            &router,
            other_region,
            "GET",
            &format!("{COLLECTION}/{id}"),
            None,
        )
        .await;
        assert_eq!(
            elsewhere.status, 404,
            "case {other_region:?}: a record created in {REGION:?} must not resolve through \
             another region, got {} with body {:?}",
            elsewhere.status, elsewhere.body
        );
    }

    // Reading it twice through its own region reports the same record, unchanged: a read
    // is a read.
    let first = call(&router, "GET", &format!("{COLLECTION}/{id}"), None).await;
    let second = call(&router, "GET", &format!("{COLLECTION}/{id}"), None).await;
    assert_eq!(
        first.status, 200,
        "case \"own region\": expected HTTP 200, got {} with body {:?}",
        first.status, first.body
    );
    assert_eq!(
        first.body, second.body,
        "case \"read twice\": a read must not change what a later caller sees, got {:?} then {:?}",
        first.body, second.body
    );
}

/// RULE: a retry carrying a `ClientToken` an earlier successful create used answers with
/// what that call created and performs no further action; a create under a token nobody
/// used creates its own record.
#[tokio::test]
async fn a_replayed_client_token_answers_with_the_first_record() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));

    // `ClientToken`: len 1..64, pattern ^[a-zA-Z0-9=_-]+$. The rows differ in the token's
    // form and in which members the create carries.
    let cases: Vec<(&str, String, Value)> = vec![
        (
            "a bare token",
            "token1".to_string(),
            json!({"Name": "retried config"}),
        ),
        (
            "a token using every admitted character class",
            "aZ9=_-".to_string(),
            json!({
                "Name": "retried with a push config",
                "Description": "retry safe",
                "PushConfig": {"TimeoutConfig": {"InProgressTimeoutInMinutes": 5}},
            }),
        ),
        (
            "a token at the published length maximum",
            "b".repeat(64),
            json!({}),
        ),
    ];

    let mut minted = BTreeSet::new();
    for (case, token, mut request) in cases {
        request["ClientToken"] = json!(token);
        let first = create(&router, case, request.clone()).await;
        let replayed = create(&router, case, request.clone()).await;
        assert_eq!(
            replayed, first,
            "case {case:?}: a replayed ClientToken must answer with the first record's id"
        );
        assert!(
            minted.insert(first.clone()),
            "case {case:?}: a token nobody used must mint its own record, got {first:?} twice"
        );
    }

    assert_eq!(
        engine
            .state::<ServiceState>()
            .lock()
            .ota_task_configurations
            .len(),
        minted.len(),
        "each replayed create must have performed no further action"
    );
}

/// RULE: a create outside a constraint the model declares is refused with the modelled
/// `ValidationException` and persists nothing — the bound itself is accepted, one past it
/// is not.
#[tokio::test]
async fn a_create_outside_a_modelled_constraint_is_refused_and_stores_nothing() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));

    let cases: Vec<(&str, Value)> = vec![
        (
            "a name one past the published length maximum",
            json!({"Name": "n".repeat(NAME_MAX_LENGTH + 1)}),
        ),
        (
            "a description one past the published length maximum",
            json!({"Description": "d".repeat(DESCRIPTION_MAX_LENGTH + 1)}),
        ),
        (
            "a name outside the published pattern",
            json!({"Name": "not*a*legal*name"}),
        ),
        (
            "a timeout one past the published range maximum",
            json!({
                "PushConfig": {
                    "TimeoutConfig": {
                        "InProgressTimeoutInMinutes": TIMEOUT_IN_MINUTES_RANGE.1 + 1,
                    },
                },
            }),
        ),
        (
            "a timeout one below the published range minimum",
            json!({
                "PushConfig": {
                    "TimeoutConfig": {
                        "InProgressTimeoutInMinutes": TIMEOUT_IN_MINUTES_RANGE.0 - 1,
                    },
                },
            }),
        ),
        (
            "an abort action outside its enum value set",
            json!({
                "PushConfig": {
                    "AbortConfig": {
                        "AbortConfigCriteriaList": [
                            abort_criteria("HALT", "FAILED", 1, 10.5),
                        ],
                    },
                },
            }),
        ),
    ];

    for (case, request) in &cases {
        let reply = call(&router, "POST", COLLECTION, Some(request.clone())).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected the modelled ValidationException code, got {:?}",
            reply.error_type
        );
    }

    assert!(
        engine
            .state::<ServiceState>()
            .lock()
            .ota_task_configurations
            .is_empty(),
        "a refused create must not have persisted a record"
    );
}
