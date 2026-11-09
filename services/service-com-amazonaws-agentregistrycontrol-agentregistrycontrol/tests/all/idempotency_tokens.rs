// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `@idempotencyToken` contract of `agent-registry-control`'s
//! two creates, driven over HTTP through the real router so every assertion goes through
//! routing, deserialization, the handler and serialization.
//!
//! The rule under test: a `clientToken` is bound to the PARAMETERS of the request that
//! first used it. Presented again with the same parameters it creates nothing and answers
//! what that first call answered; presented with different parameters it is a
//! `ConflictException`, because the token is already bound to another request; and the
//! order a map's entries arrive in is not a parameter, so a replay whose tags arrive in
//! another order is still a replay.
//!
//! Every test asserts over a TABLE of inputs, so it holds for a class of requests rather
//! than for the one body a recording happened to carry.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region in the test's SigV4 credential scope.
const REGION: &str = "us-west-2";

/// A signed-looking request, so the router resolves this service's signing name
/// (`agent-registry`). The signature itself is never checked: the test router runs with
/// authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTAGENTREGISTRY/20260101/us-west-2/agent-registry/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// Longer than the service's own asynchronous-transition span, so a registry has reached
/// `READY` before a record is created in it.
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// What a create answers when a token is presented with other parameters. Wholly fixed
/// text: it names neither the token nor any parameter.
const PARAMETERS_DIFFER: &str = "Idempotency token already used with different request parameters";

/// One HTTP answer, reduced to what these tests assert on.
struct Reply {
    status: u16,
    body: Value,
    /// The wire error code (`x-amzn-errortype`), present only on a failure.
    error_code: Option<String>,
}

impl Reply {
    fn str_field(&self, name: &str) -> &str {
        self.body[name].as_str().unwrap_or_default()
    }

    fn message(&self) -> String {
        self.body["message"]
            .as_str()
            .or_else(|| self.body["Message"].as_str())
            .unwrap_or_default()
            .to_string()
    }
}

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_agentregistrycontrol_agentregistrycontrol::EmulatedService::new(
            engine,
        ),
    );
    router
}

async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Option<Value>) -> Reply {
    let payload = match &body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serializable body")),
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://agent-registry-control.{REGION}.amazonaws.com{path}"
        ))
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(payload)
        .expect("a valid request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.rsplit('#').next().unwrap_or(value).to_string());
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Reply {
        status,
        body,
        error_code,
    }
}

/// A client token of `ClientToken`'s shape (`@length 33..256`,
/// `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$`), derived from `seed` so each row uses its own.
fn token(seed: &str) -> String {
    let padded = format!("{seed}{}", "0".repeat(40));
    padded[..40].to_string()
}

async fn create_registry(router: &MultiServiceRouter, body: Value) -> Reply {
    call(router, "POST", "/registries", Some(body)).await
}

async fn create_record(router: &MultiServiceRouter, registry_id: &str, body: Value) -> Reply {
    call(
        router,
        "POST",
        &format!("/registries/{registry_id}/records"),
        Some(body),
    )
    .await
}

/// How many registries the service holds, read through `GetRegistry` on each ARN a create
/// handed out — so "nothing was created" is checked against the service's own reads.
async fn registry_is_there(router: &MultiServiceRouter, registry_arn: &str) -> bool {
    let registry_id = registry_arn.rsplit('/').next().unwrap_or_default();
    call(router, "GET", &format!("/registries/{registry_id}"), None)
        .await
        .status
        == 200
}

#[tokio::test]
async fn a_create_registry_token_presented_with_other_parameters_is_a_conflict() {
    // Rows differ along WHICH parameter the second call changes: the required member, an
    // optional scalar, a nested configuration structure, one entry of the tag map, a
    // member the first call omitted, and a member the second call omits. Each row's first
    // call is the same request as its second except for that one difference.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "the required name",
            json!({ "name": "idem-one" }),
            json!({ "name": "idem-one-changed" }),
        ),
        (
            "an optional scalar",
            json!({ "name": "idem-two", "description": "first" }),
            json!({ "name": "idem-two", "description": "second" }),
        ),
        (
            "a nested configuration",
            json!({ "name": "idem-three", "discoveryConfiguration": { "authorizerType": "AWS_IAM" } }),
            json!({ "name": "idem-three", "discoveryConfiguration": { "authorizerType": "CUSTOM_JWT" } }),
        ),
        (
            "one entry of the tag map",
            json!({ "name": "idem-four", "tags": { "team": "platform", "env": "test" } }),
            json!({ "name": "idem-four", "tags": { "team": "platform", "env": "prod" } }),
        ),
        (
            "a member the first call omitted",
            json!({ "name": "idem-five" }),
            json!({ "name": "idem-five", "description": "added" }),
        ),
        (
            "a member the second call omits",
            json!({ "name": "idem-six", "description": "dropped" }),
            json!({ "name": "idem-six" }),
        ),
    ];

    for (case, first_body, second_body) in rows {
        let router = router();
        let client_token = token("conflict-probe");
        let mut first = first_body.clone();
        first["clientToken"] = Value::from(client_token.clone());
        let mut second = second_body.clone();
        second["clientToken"] = Value::from(client_token.clone());

        let created = create_registry(&router, first.clone()).await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: the first create answers the modelled 202, got {} {:?}",
            created.status, created.body
        );
        let arn = created.str_field("registryArn").to_string();

        let refused = create_registry(&router, second).await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: the same token with other parameters is the modelled 409, got \
             {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ConflictException"),
            "case {case:?}: the modelled error code is ConflictException, got {:?}",
            refused.error_code
        );
        assert_eq!(
            refused.message(),
            PARAMETERS_DIFFER,
            "case {case:?}: the message is the fixed conflict text, got {:?}",
            refused.message()
        );

        // Nothing of the refused request reached the store, and the first registry is
        // still the one the token names.
        let replay = create_registry(&router, first).await;
        assert_eq!(
            (replay.status, replay.str_field("registryArn")),
            (202, arn.as_str()),
            "case {case:?}: the original request still replays its own answer, got {} {:?}",
            replay.status, replay.body
        );
        assert!(
            registry_is_there(&router, &arn).await,
            "case {case:?}: the registry the first call created must still be readable"
        );
    }
}

#[tokio::test]
async fn a_create_registry_token_presented_with_the_same_parameters_creates_nothing() {
    // Rows differ along the SHAPE of the request that is replayed, including the two forms
    // whose rendering is not a function of the value unless it is canonicalised: a tag map
    // whose entries arrive in another order, and a map of several entries replayed as
    // written. A replay must answer the first call's ARN in every one.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "only the required name",
            json!({ "name": "replay-one" }),
            json!({ "name": "replay-one" }),
        ),
        (
            "a tag map in another order",
            json!({ "name": "replay-two", "tags": { "a": "1", "b": "2", "c": "3" } }),
            json!({ "name": "replay-two", "tags": { "c": "3", "b": "2", "a": "1" } }),
        ),
        (
            "every optional member",
            json!({
                "name": "replay-three",
                "description": "the same",
                "approvalConfiguration": { "autoApprovalRules": ["APPROVE_ALL"] },
                "tags": { "team": "platform" }
            }),
            json!({
                "name": "replay-three",
                "description": "the same",
                "approvalConfiguration": { "autoApprovalRules": ["APPROVE_ALL"] },
                "tags": { "team": "platform" }
            }),
        ),
    ];

    for (case, first_body, replayed_body) in rows {
        let router = router();
        let client_token = token(first_body["name"].as_str().unwrap_or("seed"));
        let mut first = first_body.clone();
        first["clientToken"] = Value::from(client_token.clone());
        let mut replayed = replayed_body.clone();
        replayed["clientToken"] = Value::from(client_token.clone());

        let created = create_registry(&router, first).await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: the first create answers 202, got {} {:?}",
            created.status, created.body
        );
        let arn = created.str_field("registryArn").to_string();

        let replay = create_registry(&router, replayed).await;
        assert_eq!(
            replay.status, 202,
            "case {case:?}: a replay is not an error, got {} {:?}",
            replay.status, replay.body
        );
        assert_eq!(
            replay.str_field("registryArn"),
            arn,
            "case {case:?}: a replay answers the ARN the first call minted, got {:?}",
            replay.body
        );

        // A second registry would have taken the name the first one holds, so a create
        // with a different token proves there is exactly one registry of that name.
        let mut another = first_body.clone();
        another["clientToken"] = Value::from(token("second-token-for-the-same-name"));
        let duplicate = create_registry(&router, another).await;
        assert_eq!(
            duplicate.status, 409,
            "case {case:?}: the replay created no second registry, so the name is still \
             taken, got {} {:?}",
            duplicate.status, duplicate.body
        );
    }
}

#[tokio::test]
async fn a_create_registry_record_token_follows_the_same_rule() {
    let router = router();
    let created = create_registry(
        &router,
        json!({ "name": "record-token-registry", "clientToken": token("record-token-registry") }),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case \"the parent registry\": got {} {:?}",
        created.status, created.body
    );
    let registry_id = created
        .str_field("registryArn")
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    let descriptors = json!({ "custom": { "data": "{\"anything\":true}" } });
    // Rows differ along which parameter of the record create changes.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "the record name",
            json!({ "name": "rec-one", "recordType": "CUSTOM", "descriptors": descriptors.clone() }),
            json!({ "name": "rec-one-changed", "recordType": "CUSTOM", "descriptors": descriptors.clone() }),
        ),
        (
            "the display name",
            json!({ "name": "rec-two", "recordType": "CUSTOM", "descriptors": descriptors.clone(), "displayName": "first" }),
            json!({ "name": "rec-two", "recordType": "CUSTOM", "descriptors": descriptors.clone(), "displayName": "second" }),
        ),
        (
            "the descriptor payload",
            json!({ "name": "rec-three", "recordType": "CUSTOM", "descriptors": { "custom": { "data": "{\"a\":1}" } } }),
            json!({ "name": "rec-three", "recordType": "CUSTOM", "descriptors": { "custom": { "data": "{\"a\":2}" } } }),
        ),
        (
            "the custom metadata",
            json!({ "name": "rec-four", "recordType": "CUSTOM", "descriptors": descriptors.clone(), "customMetadata": { "tier": "gold", "rank": 1 } }),
            json!({ "name": "rec-four", "recordType": "CUSTOM", "descriptors": descriptors.clone(), "customMetadata": { "tier": "silver", "rank": 1 } }),
        ),
    ];

    for (case, first_body, second_body) in rows {
        let client_token = token(first_body["name"].as_str().unwrap_or("seed"));
        let mut first = first_body.clone();
        first["clientToken"] = Value::from(client_token.clone());
        let mut second = second_body.clone();
        second["clientToken"] = Value::from(client_token.clone());

        let created = create_record(&router, &registry_id, first.clone()).await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: the first create answers the modelled 202, got {} {:?}",
            created.status, created.body
        );
        let record_arn = created.str_field("recordArn").to_string();

        let refused = create_record(&router, &registry_id, second).await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: the same token with other parameters is the modelled 409, got \
             {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ConflictException"),
            "case {case:?}: the modelled error code is ConflictException, got {:?}",
            refused.error_code
        );
        assert_eq!(
            refused.message(),
            PARAMETERS_DIFFER,
            "case {case:?}: the message is the fixed conflict text, got {:?}",
            refused.message()
        );

        // And the request as first sent still replays the record it created.
        let replay = create_record(&router, &registry_id, first).await;
        assert_eq!(
            (replay.status, replay.str_field("recordArn")),
            (202, record_arn.as_str()),
            "case {case:?}: the original request still replays its own answer, got {} {:?}",
            replay.status, replay.body
        );
    }

    // A record create whose custom metadata arrives with its members in another order is
    // the same request: the map's iteration order is not one of its parameters.
    let client_token = token("metadata-order-token");
    let first = json!({
        "name": "rec-order",
        "recordType": "CUSTOM",
        "descriptors": descriptors.clone(),
        "customMetadata": { "tier": "gold", "rank": 1, "owner": "agents" },
        "clientToken": client_token
    });
    let reordered = json!({
        "name": "rec-order",
        "recordType": "CUSTOM",
        "descriptors": descriptors,
        "customMetadata": { "owner": "agents", "rank": 1, "tier": "gold" },
        "clientToken": client_token
    });
    let created = create_record(&router, &registry_id, first).await;
    assert_eq!(
        created.status, 202,
        "case \"metadata in another order\": the first create answers 202, got {} {:?}",
        created.status, created.body
    );
    let replay = create_record(&router, &registry_id, reordered).await;
    assert_eq!(
        (replay.status, replay.str_field("recordArn")),
        (202, created.str_field("recordArn")),
        "case \"metadata in another order\": the same parameters in another order replay, \
         got {} {:?}",
        replay.status, replay.body
    );
}
