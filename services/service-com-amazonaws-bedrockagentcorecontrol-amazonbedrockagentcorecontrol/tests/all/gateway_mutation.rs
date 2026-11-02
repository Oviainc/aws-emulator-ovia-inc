// SPDX-License-Identifier: CC0-1.0

//! The two gateway MUTATIONS, end to end over HTTP: `UpdateGateway` and `DeleteGateway`,
//! each read back through `GetGateway`.
//!
//! Three rules are under test, read off the Smithy model rather than off any one recorded
//! request (`smithy_query(query="operation",operation="UpdateGateway,DeleteGateway")`):
//!
//! 1. `UpdateGateway` (`PUT /gateways/{gatewayIdentifier}/`) REPLACES the gateway's
//!    configuration: once it answers, the update's own response and every later read report
//!    exactly the mutable members the request carried — and nothing for the ones it omitted,
//!    which the replacement clears (`protocolType` excepted: it carries the model's `MCP`
//!    default). The gateway's identity — its id, ARN, URL, name, creation instant and
//!    workload identity — is untouched. The rows differ along which member changes and the
//!    kind of value it takes (a string, an enum, a union, a list, a nested structure), along
//!    the published length bounds of `GatewayDescription`, and along clearing a member
//!    rather than setting one.
//! 2. `DeleteGateway` (`DELETE /gateways/{gatewayIdentifier}/`) ACCEPTS the deletion and
//!    the clock carries it out: it answers 202 reporting the gateway `DELETING`, the
//!    gateway stays readable in that status until the span it owes passes (so a caller
//!    acting straight after the call still sees it), and from then on every read of it — and
//!    a further delete — is the modelled `ResourceNotFoundException` (404, with the
//!    `x-amzn-errortype` header restJson1 puts the error code in), its targets are gone with
//!    it, and the name it held is free for a new gateway. The rows differ along what the
//!    deleted gateway carried.
//! 3. A gateway serves a mutation only in the terminal status it settles into: while the
//!    clock still owes it a transition — `CREATING` after a create, `UPDATING` after an
//!    update, `DELETING` after a delete — both mutations are refused `400`
//!    `ValidationException` naming the operation and the status, and nothing is written.
//!    The rows differ along the status and the operation.

use std::sync::Arc;
use std::time::Duration;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region the rows sign for.
const REGION: &str = "us-east-1";

/// Role ARNs inside `RoleArn`'s pattern `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/GatewayRole";
const OTHER_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/GatewayRoleAfterUpdate";

/// KMS key ARNs inside `KmsKeyArn`'s pattern
/// `^arn:aws(|-cn|-us-gov):kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$`.
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:000000000000:key/11111111-2222-3333-4444-555555555555";
const OTHER_KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:000000000000:key/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

/// A Lambda function ARN inside `LambdaFunctionArn`'s pattern, used by the interceptor and
/// the custom-transform configurations.
const LAMBDA_ARN: &str = "arn:aws:lambda:us-east-1:000000000000:function:gateway-hook";

/// A policy engine ARN inside `GatewayPolicyEngineArn`'s pattern
/// `^arn:aws(-[^:]+)?:bedrock-agentcore:[a-z0-9-]+:[0-9]{12}:policy-engine\/[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9_]{10}$`.
const POLICY_ENGINE_ARN: &str =
    "arn:aws:bedrock-agentcore:us-east-1:000000000000:policy-engine/toolguard-abcdefghij";

/// The settle span `src/state.rs` gives a gateway's `CREATING`/`UPDATING` -> `READY`
/// transition and the removal it owes an accepted deletion, plus slack, so a row can wait
/// the clock out.
const PAST_SETTLE: Duration = Duration::from_millis(900);

/// A `gatewayIdentifier` inside `GatewayIdentifier`'s pattern
/// `^([0-9a-z][-]?){1,100}-[0-9a-z]{10}$` that no row ever creates.
const ABSENT_GATEWAY_ID: &str = "no-such-gateway-0123456789";

/// Every member of `UpdateGatewayRequest` other than `gatewayIdentifier` (the URI label) and
/// `name` (which the model says must not change): the gateway's mutable configuration.
/// Source: `smithy_query(query="operation",operation="UpdateGateway")`.
const MUTABLE_MEMBERS: [&str; 12] = [
    "description",
    "roleArn",
    "protocolType",
    "protocolConfiguration",
    "authorizerType",
    "authorizerConfiguration",
    "kmsKeyArn",
    "customTransformConfiguration",
    "interceptorConfigurations",
    "policyEngineConfiguration",
    "exceptionLevel",
    "wafConfiguration",
];

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service: what the router routes on, and what the
/// handlers read the ARN region from.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/bedrock-agentcore/\
         aws4_request, SignedHeaders=host, Signature=abc"
    )
}

/// One answer: the status, the `x-amzn-errortype` header and the parsed body.
struct Answer {
    status: u16,
    error_type: String,
    body: Value,
}

async fn send(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<&Value>) -> Answer {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", authorization())
        .header("host", "bedrock-agentcore-control.us-east-1.amazonaws.com");
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    Answer {
        status: response.status().as_u16(),
        error_type,
        body: if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        },
    }
}

async fn create_gateway(router: &MultiServiceRouter, body: &Value) -> Answer {
    send(router, "POST", "/gateways/", Some(body)).await
}

async fn get_gateway(router: &MultiServiceRouter, id: &str) -> Answer {
    send(router, "GET", &format!("/gateways/{id}/"), None).await
}

async fn update_gateway(router: &MultiServiceRouter, id: &str, body: &Value) -> Answer {
    send(router, "PUT", &format!("/gateways/{id}/"), Some(body)).await
}

async fn delete_gateway(router: &MultiServiceRouter, id: &str) -> Answer {
    send(router, "DELETE", &format!("/gateways/{id}/"), None).await
}

/// The error code `restJson1` puts in the body's `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A `date-time` member as the instant it carries, in seconds since the epoch.
fn instant(body: &Value, member: &str, case: &str) -> f64 {
    let value = body.get(member).cloned().unwrap_or(Value::Null);
    let raw = value
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: {member} must be an RFC 3339 string, got {value}"));
    DateTime::from_str(raw, Format::DateTime)
        .unwrap_or_else(|error| {
            panic!("case {case:?}: {member} must parse as a date-time ({error}), got {raw:?}")
        })
        .as_secs_f64()
}

/// A `CreateGateway` body carrying a configuration on every optional member an update can
/// replace, so a row that OMITS one observes it being cleared.
fn baseline_body(name: &str) -> Value {
    json!({
        "name": name,
        "roleArn": ROLE_ARN,
        "authorizerType": "AWS_IAM",
        "description": "acc gateway before update",
        "exceptionLevel": "DEBUG",
        "kmsKeyArn": KMS_KEY_ARN,
        "protocolConfiguration": {"mcp": {"instructions": "before the update"}},
    })
}

/// An `UpdateGateway` body: the three members the model makes `@required` on the request
/// (`name`, `roleArn`, `authorizerType`), with `overrides` applied over them.
fn update_body(name: &str, overrides: &Value) -> Value {
    let mut body = json!({
        "name": name,
        "roleArn": ROLE_ARN,
        "authorizerType": "AWS_IAM",
    });
    if let Some(members) = overrides.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    body
}

/// The value a gateway must report for `member` after an update carrying `update`.
fn expected_member(update: &Value, member: &str) -> Value {
    match update.get(member) {
        Some(value) => value.clone(),
        // `protocolType` carries the model's `MCP` default (`default: "MCP"`), so a request
        // that names none still leaves the gateway reporting the protocol it speaks.
        None if member == "protocolType" => json!("MCP"),
        // Every other member the request omits is cleared: `PUT` replaces the whole
        // configuration rather than patching it.
        None => Value::Null,
    }
}

/// `UpdateGateway` writes the mutable configuration through to the stored gateway: its own
/// response and the next `GetGateway` both report exactly what the request carried, and the
/// gateway's identity is untouched.
#[tokio::test]
async fn update_gateway_replaces_the_stored_configuration_for_every_mutable_member() {
    let router = router();

    // (case, the members the update carries beyond the required three)
    let rows: [(&str, Value); 11] = [
        (
            "only the required members: every optional member is cleared",
            json!({}),
        ),
        (
            "a new description",
            json!({"description": "acc gateway after update"}),
        ),
        (
            "a description and exceptionLevel DEBUG",
            json!({"description": "acc gateway after update", "exceptionLevel": "DEBUG"}),
        ),
        (
            // `GatewayDescription` is `len 1..200` (`smithy_query shape=GatewayDescription`):
            // the lower bound.
            "a description at GatewayDescription's minimum length of 1",
            json!({"description": "d"}),
        ),
        (
            "a description at GatewayDescription's maximum length of 200",
            json!({"description": "d".repeat(200)}),
        ),
        (
            "a different KMS key",
            json!({"kmsKeyArn": OTHER_KMS_KEY_ARN}),
        ),
        (
            "a different role, and the protocol named explicitly",
            json!({"roleArn": OTHER_ROLE_ARN, "protocolType": "MCP"}),
        ),
        (
            // `GatewayProtocolConfiguration` is a union with the single member `mcp`.
            "a replaced protocol configuration (a union)",
            json!({"protocolConfiguration": {
                "mcp": {
                    "instructions": "after the update",
                    "searchType": "SEMANTIC",
                    "disableMcpListToolsPagination": true,
                },
            }}),
        ),
        (
            // "If you specify CUSTOM_JWT as the authorizerType, you must provide an
            // authorizerConfiguration" (`smithy_query doc shape=CreateGateway`).
            "the authorizer replaced by CUSTOM_JWT and its configuration",
            json!({
                "authorizerType": "CUSTOM_JWT",
                "authorizerConfiguration": {"customJWTAuthorizer": {
                    "discoveryUrl": "https://example.com/.well-known/openid-configuration",
                    "allowedClients": ["client-1", "client-2"],
                }},
            }),
        ),
        (
            // `GatewayInterceptionPoint` is `REQUEST | RESPONSE`, and
            // `GatewayInterceptionPoints` holds 1..2 of them
            // (`smithy_query shape=GatewayInterceptionPoints`).
            "an interceptor configuration (a list of nested structures)",
            json!({"interceptorConfigurations": [{
                "interceptor": {"lambda": {"arn": LAMBDA_ARN}},
                "interceptionPoints": ["REQUEST", "RESPONSE"],
                "inputConfiguration": {"passRequestHeaders": true},
            }]}),
        ),
        (
            // `GatewayPolicyEngineMode` is `LOG_ONLY | ENFORCE` and `WafFailureMode` is
            // `FAIL_CLOSE | FAIL_OPEN` (`smithy_query shape=GatewayPolicyEngineMode,WafFailureMode`).
            "a policy engine, a custom transform and a WAF failure mode",
            json!({
                "policyEngineConfiguration": {"arn": POLICY_ENGINE_ARN, "mode": "ENFORCE"},
                "customTransformConfiguration": {"lambda": {"arn": LAMBDA_ARN}},
                "wafConfiguration": {"failureMode": "FAIL_OPEN"},
            }),
        ),
    ];

    for (index, (case, overrides)) in rows.into_iter().enumerate() {
        let name = format!("gwu{index}");
        let created = create_gateway(&router, &baseline_body(&name)).await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: the gateway an update needs is created, got {} with {}",
            created.status, created.body
        );
        let id = text(&created.body, "gatewayId");

        // A gateway serves a mutation only once the clock has settled it to READY, and the
        // wait also puts the update's instant strictly after the creation's.
        tokio::time::sleep(PAST_SETTLE).await;

        let update = update_body(&name, &overrides);
        let updated = update_gateway(&router, &id, &update).await;
        assert_eq!(
            updated.status, 202,
            "case {case:?}: expected HTTP 202 from UpdateGateway, got {} with {}",
            updated.status, updated.body
        );

        let fetched = get_gateway(&router, &id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: the updated gateway is readable, got {} with {}",
            fetched.status, fetched.body
        );

        // The rule: the stored configuration IS what the update carried, for every mutable
        // member — in the update's own answer and in the read that follows it.
        for member in MUTABLE_MEMBERS {
            let expected = expected_member(&update, member);
            let reported = updated.body.get(member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                reported, expected,
                "case {case:?}: UpdateGateway must report {member} as {expected}, got {reported}"
            );
            let read_back = fetched.body.get(member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                read_back, expected,
                "case {case:?}: GetGateway must report the {member} UpdateGateway stored \
                 ({expected}), got {read_back}"
            );
        }

        // The gateway's identity is not what an update replaces.
        for member in [
            "gatewayId",
            "gatewayArn",
            "gatewayUrl",
            "name",
            "createdAt",
            "workloadIdentityDetails",
        ] {
            assert_eq!(
                updated.body.get(member),
                created.body.get(member),
                "case {case:?}: an update must not change {member}, got {} for {}",
                updated.body.get(member).cloned().unwrap_or(Value::Null),
                created.body.get(member).cloned().unwrap_or(Value::Null)
            );
            assert_eq!(
                fetched.body.get(member),
                created.body.get(member),
                "case {case:?}: the read must report the stored {member}, got {} for {}",
                fetched.body.get(member).cloned().unwrap_or(Value::Null),
                created.body.get(member).cloned().unwrap_or(Value::Null)
            );
        }

        // `updatedAt` is the instant of the update, so it moved; `createdAt` did not.
        let created_at = instant(&fetched.body, "createdAt", case);
        let updated_at = instant(&fetched.body, "updatedAt", case);
        assert!(
            updated_at > created_at,
            "case {case:?}: expected updatedAt ({updated_at}) to be later than createdAt \
             ({created_at}) after an update"
        );

        // The status an update puts the gateway in, before the clock owes it anything else.
        assert_eq!(
            text(&updated.body, "status"),
            "UPDATING",
            "case {case:?}: an updated gateway is UPDATING, got {}",
            text(&updated.body, "status")
        );
    }
}

/// The `UPDATING` -> `READY` transition is the clock's: a caller acting straight after the
/// update still sees `UPDATING`, and a read neither performs the transition early nor
/// changes what a later caller sees.
#[tokio::test]
async fn an_updated_gateway_settles_back_to_ready_on_the_clock() {
    let router = router();
    let created = create_gateway(&router, &baseline_body("gwsettle")).await;
    assert_eq!(
        created.status, 202,
        "the gateway under test is created, got {} with {}",
        created.status, created.body
    );
    let id = text(&created.body, "gatewayId");

    tokio::time::sleep(PAST_SETTLE).await;
    let ready = get_gateway(&router, &id).await;
    assert_eq!(
        text(&ready.body, "status"),
        "READY",
        "case \"before the update\": the created gateway settled to READY, got {}",
        text(&ready.body, "status")
    );

    let update = update_body("gwsettle", &json!({"description": "after the update"}));
    let updated = update_gateway(&router, &id, &update).await;
    assert_eq!(
        (updated.status, text(&updated.body, "status").as_str()),
        (202, "UPDATING"),
        "case \"the update itself\": expected 202 UPDATING, got {} {} ({})",
        updated.status,
        text(&updated.body, "status"),
        updated.body
    );

    let straight_after = get_gateway(&router, &id).await;
    assert_eq!(
        text(&straight_after.body, "status"),
        "UPDATING",
        "case \"straight after the update\": the transient status is still reported, got {}",
        text(&straight_after.body, "status")
    );

    tokio::time::sleep(PAST_SETTLE).await;
    for case in ["the first read past the span", "a second read past the span"] {
        let settled = get_gateway(&router, &id).await;
        assert_eq!(
            text(&settled.body, "status"),
            "READY",
            "case {case:?}: the gateway is READY again, got {}",
            text(&settled.body, "status")
        );
        assert_eq!(
            text(&settled.body, "description"),
            "after the update",
            "case {case:?}: the description the update stored is still reported, got {}",
            text(&settled.body, "description")
        );
    }
}

/// `DeleteGateway` accepts the deletion of a settled gateway and the clock carries it out:
/// the gateway reports `DELETING` straight after the call, and once the span passes every
/// read of it, every target on it and a further delete are the modelled 404 — whatever the
/// gateway carried.
#[tokio::test]
async fn a_deleted_gateway_reports_deleting_and_then_resolves_to_the_modelled_404() {
    // (case, update it before deleting, create a target on it)
    let rows: [(&str, bool, bool); 3] = [
        ("a gateway as created", false, false),
        ("a gateway reconfigured by an update", true, false),
        ("a gateway carrying a target", false, true),
    ];

    for (index, (case, update, with_target)) in rows.into_iter().enumerate() {
        let router = router();
        let name = format!("gwd{index}");
        let created = create_gateway(&router, &baseline_body(&name)).await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: the gateway to delete is created, got {} with {}",
            created.status, created.body
        );
        let id = text(&created.body, "gatewayId");

        // A mutation reaches a gateway only once the clock has settled it to READY.
        tokio::time::sleep(PAST_SETTLE).await;
        if update {
            let body = update_body(&name, &json!({"description": "before the delete"}));
            let answer = update_gateway(&router, &id, &body).await;
            assert_eq!(
                answer.status, 202,
                "case {case:?}: the update before the delete succeeds, got {} with {}",
                answer.status, answer.body
            );
            tokio::time::sleep(PAST_SETTLE).await;
        }
        let mut target_id = String::new();
        if with_target {
            let body = json!({
                "name": "a-target",
                "targetConfiguration": {
                    "mcp": {"openApiSchema": {"inlinePayload": "{\"openapi\":\"3.0.0\"}"}}
                },
            });
            let answer = send(
                &router,
                "POST",
                &format!("/gateways/{id}/targets/"),
                Some(&body),
            )
            .await;
            assert_eq!(
                answer.status, 202,
                "case {case:?}: the target on the gateway is created, got {} with {}",
                answer.status, answer.body
            );
            target_id = text(&answer.body, "targetId");
        }

        let deleted = delete_gateway(&router, &id).await;
        assert_eq!(
            deleted.status, 202,
            "case {case:?}: expected HTTP 202 from DeleteGateway, got {} with {}",
            deleted.status, deleted.body
        );
        assert_eq!(
            text(&deleted.body, "gatewayId"),
            id,
            "case {case:?}: DeleteGateway reports the gateway it deleted, got {}",
            text(&deleted.body, "gatewayId")
        );
        assert_eq!(
            text(&deleted.body, "status"),
            "DELETING",
            "case {case:?}: a deleted gateway is reported DELETING, got {}",
            text(&deleted.body, "status")
        );

        // Straight after the call the removal is owed but not yet due, so the gateway is
        // still readable — in the status the deletion put it in, whatever status it held
        // before.
        let deleting = get_gateway(&router, &id).await;
        assert_eq!(
            (deleting.status, text(&deleting.body, "status").as_str()),
            (200, "DELETING"),
            "case {case:?}: straight after the delete the gateway reads as DELETING, got {} {} \
             ({})",
            deleting.status,
            text(&deleting.body, "status"),
            deleting.body
        );
        // `DeleteGateway` is `@idempotent`, so a repeat while the deletion is under way
        // answers with the same gateway rather than refusing it.
        let repeat = delete_gateway(&router, &id).await;
        assert_eq!(
            (repeat.status, text(&repeat.body, "status").as_str()),
            (202, "DELETING"),
            "case {case:?}: a repeated DeleteGateway reports the same DELETING gateway, got {} \
             {} ({})",
            repeat.status,
            text(&repeat.body, "status"),
            repeat.body
        );
        // The name is still held while the gateway is: it is one that EXISTS.
        let clash = create_gateway(&router, &baseline_body(&name)).await;
        assert_eq!(
            (clash.status, error_code(&clash.body).as_str()),
            (409, "ConflictException"),
            "case {case:?}: the name of a gateway that is still DELETING is taken, got {} {} \
             ({})",
            clash.status,
            error_code(&clash.body),
            clash.body
        );

        // Once the span passes, the record is gone: every read of it is the modelled 404 —
        // with the error code in the body's `__type` AND in the `x-amzn-errortype` header,
        // and a message rather than an empty envelope.
        tokio::time::sleep(PAST_SETTLE).await;
        let read = get_gateway(&router, &id).await;
        assert_eq!(
            (read.status, error_code(&read.body).as_str(), read.error_type.as_str()),
            (404, "ResourceNotFoundException", "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException from GetGateway once the \
             deletion is due, got {} {} (header {:?}, body {})",
            read.status,
            error_code(&read.body),
            read.error_type,
            read.body
        );
        assert!(
            !text(&read.body, "message").is_empty(),
            "case {case:?}: the refusal carries a message, got {}",
            read.body
        );
        // A read does not resurrect it either: the next read answers the same.
        let read_again = get_gateway(&router, &id).await;
        assert_eq!(
            (read_again.status, error_code(&read_again.body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a second read answers the same refusal, got {} {} ({})",
            read_again.status,
            error_code(&read_again.body),
            read_again.body
        );

        // A delete of a gateway that is gone has nothing left to delete.
        let again = delete_gateway(&router, &id).await;
        assert_eq!(
            (again.status, error_code(&again.body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException from a DeleteGateway once the \
             gateway is gone, got {} {} ({})",
            again.status,
            error_code(&again.body),
            again.body
        );

        // An update of a gateway that is gone is the same refusal, not a resurrection.
        let after = update_gateway(&router, &id, &update_body(&name, &json!({}))).await;
        assert_eq!(
            (after.status, error_code(&after.body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException from UpdateGateway once the \
             gateway is gone, got {} {} ({})",
            after.status,
            error_code(&after.body),
            after.body
        );

        if with_target {
            let target = send(
                &router,
                "GET",
                &format!("/gateways/{id}/targets/{target_id}/"),
                None,
            )
            .await;
            assert_eq!(
                (target.status, error_code(&target.body).as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: a target of a deleted gateway is gone with it, got {} {} ({})",
                target.status,
                error_code(&target.body),
                target.body
            );
        }

        // The name the deleted gateway held is free again: "The name must be unique within
        // your account" holds over the gateways that EXIST.
        let recreated = create_gateway(&router, &baseline_body(&name)).await;
        assert_eq!(
            recreated.status, 202,
            "case {case:?}: the name of a deleted gateway can be created anew, got {} with {}",
            recreated.status, recreated.body
        );
        assert_ne!(
            text(&recreated.body, "gatewayId"),
            id,
            "case {case:?}: the re-created gateway is a NEW resource with its own id, got {}",
            text(&recreated.body, "gatewayId")
        );
    }
}

/// One refusal row: the case, the method, the `gatewayIdentifier` in the URI, the request
/// body, and the status and error code the row expects back.
type RefusalRow<'a> = (&'a str, &'a str, &'a str, Option<Value>, u16, &'a str);

/// What the two mutations refuse, and with which modelled error.
#[tokio::test]
async fn the_gateway_mutations_refuse_what_the_model_states_they_must() {
    let router = router();
    let created = create_gateway(&router, &baseline_body("gwrefuse")).await;
    assert_eq!(
        created.status, 202,
        "the gateway the rows mutate is created, got {} with {}",
        created.status, created.body
    );
    let id = text(&created.body, "gatewayId");
    // Settled first, so each row below is refused for its own reason and not because the
    // clock still owes the gateway a transition (which rule 3's test covers).
    tokio::time::sleep(PAST_SETTLE).await;

    // (case, method, the gateway identifier, body, expected status, expected error code)
    let rows: [RefusalRow<'_>; 5] = [
        (
            "UpdateGateway on an identifier naming no stored gateway",
            "PUT",
            ABSENT_GATEWAY_ID,
            Some(update_body("gwrefuse", &json!({}))),
            404,
            "ResourceNotFoundException",
        ),
        (
            "DeleteGateway on an identifier naming no stored gateway",
            "DELETE",
            ABSENT_GATEWAY_ID,
            None,
            404,
            "ResourceNotFoundException",
        ),
        (
            // "This name must be the same as the one when the gateway was created"
            // (`smithy_query doc shape=UpdateGatewayRequest member=name`).
            "UpdateGateway carrying a different name",
            "PUT",
            &id,
            Some(update_body("gwrenamed", &json!({}))),
            400,
            "ValidationException",
        ),
        (
            // The same prose rule `CreateGateway` carries, about the gateway's authorizer.
            "UpdateGateway with CUSTOM_JWT and no authorizerConfiguration",
            "PUT",
            &id,
            Some(update_body(
                "gwrefuse",
                &json!({"authorizerType": "CUSTOM_JWT"}),
            )),
            400,
            "ValidationException",
        ),
        (
            "UpdateGateway with a roleArn outside RoleArn's pattern",
            "PUT",
            &id,
            Some(json!({
                "name": "gwrefuse",
                "roleArn": "not-an-arn",
                "authorizerType": "AWS_IAM",
            })),
            400,
            "ValidationException",
        ),
    ];

    for (case, method, identifier, body, expected_status, expected_code) in rows {
        let answer = send(
            &router,
            method,
            &format!("/gateways/{identifier}/"),
            body.as_ref(),
        )
        .await;
        assert_eq!(
            (answer.status, error_code(&answer.body).as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected HTTP {expected_status} {expected_code}, got {} {} ({})",
            answer.status,
            error_code(&answer.body),
            answer.body
        );
    }

    // None of the refusals touched the stored gateway: it still reports what it was created
    // with, under its own name.
    let fetched = get_gateway(&router, &id).await;
    assert_eq!(
        (
            fetched.status,
            text(&fetched.body, "name"),
            text(&fetched.body, "description"),
            text(&fetched.body, "authorizerType"),
        ),
        (
            200,
            "gwrefuse".to_string(),
            "acc gateway before update".to_string(),
            "AWS_IAM".to_string(),
        ),
        "case \"after the refusals\": the gateway is unchanged, got {} with {}",
        fetched.status,
        fetched.body
    );

    // A gateway whose deletion has been accepted is on its way out, so it is not one an
    // update can reconfigure: the refusal names the operation and the status, and the
    // gateway keeps the configuration it had.
    let deleted = delete_gateway(&router, &id).await;
    assert_eq!(
        deleted.status, 202,
        "case \"the delete before the last row\": got {} with {}",
        deleted.status, deleted.body
    );
    let while_deleting = update_gateway(&router, &id, &update_body("gwrefuse", &json!({}))).await;
    assert_eq!(
        (while_deleting.status, error_code(&while_deleting.body).as_str()),
        (400, "ValidationException"),
        "case \"UpdateGateway while the gateway is DELETING\": expected HTTP 400 \
         ValidationException, got {} {} ({})",
        while_deleting.status,
        error_code(&while_deleting.body),
        while_deleting.body
    );
    let still_there = get_gateway(&router, &id).await;
    assert_eq!(
        (still_there.status, text(&still_there.body, "description")),
        (200, "acc gateway before update".to_string()),
        "case \"UpdateGateway while the gateway is DELETING\": the refused update wrote \
         nothing, got {} with {}",
        still_there.status,
        still_there.body
    );
}

/// A gateway serves a mutation only in the terminal status it settles into: while the clock
/// still owes it a transition, `UpdateGateway` and `DeleteGateway` are refused `400`
/// `ValidationException` naming the operation and the status it found, and nothing is
/// written.
///
/// The rows differ along the transitional status (`CREATING` after a create, `UPDATING`
/// after an update, `DELETING` after a delete — every non-terminal `GatewayStatus` this
/// service drives, from `smithy_query shape=GatewayStatus`) and along the operation refused.
#[tokio::test]
async fn a_gateway_the_clock_still_owes_a_transition_refuses_both_mutations() {
    // (case, the status to leave the gateway in, that status as the refusal's prose spells
    // it, the method refused, its operation name)
    let rows: [(&str, &str, &str, &str, &str); 5] = [
        ("UpdateGateway while CREATING", "CREATING", "Creating", "PUT", "UpdateGateway"),
        ("DeleteGateway while CREATING", "CREATING", "Creating", "DELETE", "DeleteGateway"),
        ("UpdateGateway while UPDATING", "UPDATING", "Updating", "PUT", "UpdateGateway"),
        ("DeleteGateway while UPDATING", "UPDATING", "Updating", "DELETE", "DeleteGateway"),
        ("UpdateGateway while DELETING", "DELETING", "Deleting", "PUT", "UpdateGateway"),
    ];

    for (index, (case, status, status_prose, method, operation)) in rows.into_iter().enumerate() {
        let router = router();
        let name = format!("gwg{index}");
        let created = create_gateway(&router, &baseline_body(&name)).await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: the gateway under test is created, got {} with {}",
            created.status, created.body
        );
        let id = text(&created.body, "gatewayId");

        // Leave the gateway in the status this row is about.
        match status {
            "CREATING" => {}
            "UPDATING" => {
                tokio::time::sleep(PAST_SETTLE).await;
                let answer =
                    update_gateway(&router, &id, &update_body(&name, &json!({}))).await;
                assert_eq!(
                    answer.status, 202,
                    "case {case:?}: the update that leaves it UPDATING succeeds, got {} with {}",
                    answer.status, answer.body
                );
            }
            _ => {
                tokio::time::sleep(PAST_SETTLE).await;
                let answer = delete_gateway(&router, &id).await;
                assert_eq!(
                    answer.status, 202,
                    "case {case:?}: the delete that leaves it DELETING succeeds, got {} with {}",
                    answer.status, answer.body
                );
            }
        }
        let before = get_gateway(&router, &id).await;
        assert_eq!(
            (before.status, text(&before.body, "status").as_str()),
            (200, status),
            "case {case:?}: the row needs the gateway in {status}, got {} {} ({})",
            before.status,
            text(&before.body, "status"),
            before.body
        );

        let body = (method == "PUT").then(|| update_body(&name, &json!({"description": "x"})));
        let refused = send(&router, method, &format!("/gateways/{id}/"), body.as_ref()).await;
        assert_eq!(
            (
                refused.status,
                error_code(&refused.body).as_str(),
                refused.error_type.as_str()
            ),
            (400, "ValidationException", "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {} {} (header {:?}, \
             body {})",
            refused.status,
            error_code(&refused.body),
            refused.error_type,
            refused.body
        );
        let message = text(&refused.body, "message");
        assert!(
            message.contains(operation) && message.contains(status_prose),
            "case {case:?}: the refusal must name the operation and the status it found (as \
             {status_prose:?}), got {message:?}"
        );

        // The refusal wrote nothing: the gateway is still in the status it was in, with the
        // configuration it had.
        let after = get_gateway(&router, &id).await;
        assert_eq!(
            (after.status, text(&after.body, "status").as_str()),
            (200, status),
            "case {case:?}: the refused mutation left the status alone, got {} {} ({})",
            after.status,
            text(&after.body, "status"),
            after.body
        );
        assert_eq!(
            after.body.get("description"),
            before.body.get("description"),
            "case {case:?}: the refused mutation left the configuration alone, got {} for {}",
            after.body.get("description").cloned().unwrap_or(Value::Null),
            before.body.get("description").cloned().unwrap_or(Value::Null)
        );
    }
}
