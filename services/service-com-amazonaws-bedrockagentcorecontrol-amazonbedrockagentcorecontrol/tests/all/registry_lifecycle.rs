// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the registry lifecycle (`CreateRegistry`, `GetRegistry`).
//!
//! Every test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint validation and the handlers are all exercised. The store is the one shared
//! engine, so what one request writes is what the next request reads.

use std::sync::Arc;
use std::time::Duration;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region the tests sign for.
const REGION: &str = "us-east-1";

/// The account id every emulated ARN carries (`engine::mock_account_id`).
const ACCOUNT: &str = "000000000000";

/// The settle span `src/state.rs` gives the `CREATING` -> `READY` transition, plus
/// slack, so a test can wait the clock out.
const PAST_SETTLE: Duration = Duration::from_millis(3_300);

/// A `clientToken` inside the modelled `ClientToken` constraints (len 33..256,
/// `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$`, `smithy_query operation=CreateRegistry`).
fn client_token(seed: &str) -> String {
    let token = format!("{seed}{}", "0".repeat(40));
    token[..40].to_string()
}

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service in `region`. It is what the router
/// routes on, and what the handlers read the ARN region from.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/bedrock-agentcore/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", authorization(REGION))
        .header("host", "bedrock-agentcore-control.amazonaws.com");
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// `CreateRegistry`: `POST /registries` (`smithy_query operation=CreateRegistry`).
async fn create_registry(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    send(router, "POST", "/registries", Some(body)).await
}

/// `GetRegistry`: `GET /registries/{registryId}`, where the label is a
/// `RegistryIdentifier` — the bare id or the whole ARN.
async fn get_registry(router: &MultiServiceRouter, identifier: &str) -> (u16, Value) {
    send(router, "GET", &format!("/registries/{identifier}"), None).await
}

/// The error code `restJson1` puts in `__type` / `x-amzn-errortype`.
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

/// `RegistryId`'s pattern, `^[a-zA-Z0-9]{12,16}$` (`smithy_query shape=RegistryId`),
/// checked by hand because this crate carries no regex engine.
fn is_modelled_registry_id(id: &str) -> bool {
    (12..=16).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// The ARN form, percent-encoded as a client encodes a non-greedy `@httpLabel`.
fn encoded_label(identifier: &str) -> String {
    identifier.replace(':', "%3A").replace('/', "%2F")
}

/// Assert one `DateTimestamp` member's wire form: a JSON STRING in the canonical RFC
/// 3339 form, naming a real instant. `createdAt` and `updatedAt` target
/// `DateTimestamp`, which carries `@timestampFormat("date-time")` (`smithy_query
/// operation=GetRegistry`), so an epoch-seconds number is not the modelled type.
fn assert_date_time(case: &str, member: &str, value: &Value) {
    let rendered = value.as_str().unwrap_or_else(|| {
        panic!("case {case:?}: {member} must be an RFC 3339 date-time string, got {value:?}")
    });
    let parsed = DateTime::from_str(rendered, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: {member} must parse as a date-time ({error}), got {rendered:?}")
    });
    assert!(
        parsed.secs() > 0,
        "case {case:?}: {member} must name a real instant, got {rendered:?}"
    );
}

/// A registry is created from the request and read back through `GetRegistry`, for
/// every shape of the configuration `CreateRegistry` accepts.
///
/// The rows differ along the members the request carries — the description, the
/// authorizer type and its configuration, and the approval configuration — which is
/// what the stored registry reports. Source for the value lists:
/// `RegistryAuthorizerType`'s values and `ApprovalConfiguration`'s `autoApproval`
/// default (`smithy_query shape=RegistryAuthorizerType,ApprovalConfiguration`).
#[tokio::test]
async fn a_registry_is_created_and_read_back_for_every_configuration() {
    let router = router();

    // (case, name, the members beyond `name`, expected authorizerType,
    //  expected autoApproval)
    let rows: [(&str, &str, Value, Value, bool); 5] = [
        (
            "only the required name",
            "catalog",
            json!({}),
            Value::Null,
            false,
        ),
        (
            "a described registry authorized by IAM",
            "mcp.servers/eu",
            json!({"description": "the shared catalog", "authorizerType": "AWS_IAM"}),
            json!("AWS_IAM"),
            false,
        ),
        (
            "a registry authorized by a bearer token",
            "a2a_agents",
            json!({
                "authorizerType": "CUSTOM_JWT",
                "authorizerConfiguration": {"customJWTAuthorizer": {
                    "discoveryUrl": "https://example.com/.well-known/openid-configuration",
                    "allowedClients": ["client-one"]
                }}
            }),
            json!("CUSTOM_JWT"),
            false,
        ),
        (
            "records approved on creation",
            "auto.approved",
            json!({"approvalConfiguration": {"autoApproval": true}}),
            Value::Null,
            true,
        ),
        (
            "records that need explicit approval",
            "R2",
            json!({"description": "d", "approvalConfiguration": {"autoApproval": false}}),
            Value::Null,
            false,
        ),
    ];

    // The registries the table creates, so the clock-owed transition is waited out
    // ONCE after the loop rather than per row.
    let mut settling: Vec<(&str, String, Value)> = Vec::new();

    for (case, name, extra, expected_authorizer_type, expected_auto_approval) in rows {
        let mut body = json!({"name": name});
        for (member, value) in extra.as_object().expect("the row's members are an object") {
            body[member] = value.clone();
        }

        let (status, created) = create_registry(&router, &body).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from CreateRegistry, got {status} ({created})"
        );

        let arn = text(&created, "registryArn");
        let id = arn.rsplit('/').next().unwrap_or_default().to_string();
        assert!(
            is_modelled_registry_id(&id),
            "case {case:?}: expected a registryId matching ^[a-zA-Z0-9]{{12,16}}$ in the \
             registryArn, got {arn:?}"
        );
        assert_eq!(
            arn,
            format!("arn:aws:bedrock-agentcore:{REGION}:{ACCOUNT}:registry/{id}"),
            "case {case:?}: expected the modelled RegistryArn for {id}, got {arn:?}"
        );

        let (status, fetched) = get_registry(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetRegistry, got {status} ({fetched})"
        );
        assert_eq!(
            text(&fetched, "name"),
            name,
            "case {case:?}: the name reads back as sent, got {}",
            text(&fetched, "name")
        );
        assert_eq!(
            text(&fetched, "registryId"),
            id,
            "case {case:?}: GetRegistry reports the id the ARN carries, got {}",
            text(&fetched, "registryId")
        );
        assert_eq!(
            text(&fetched, "registryArn"),
            arn,
            "case {case:?}: GetRegistry reports the ARN CreateRegistry handed back, got {}",
            text(&fetched, "registryArn")
        );
        assert_eq!(
            fetched["description"],
            body.get("description").cloned().unwrap_or(Value::Null),
            "case {case:?}: the description reads back as sent, got {}",
            fetched["description"]
        );
        assert_eq!(
            fetched["authorizerType"], expected_authorizer_type,
            "case {case:?}: expected authorizerType {expected_authorizer_type}, got {}",
            fetched["authorizerType"]
        );
        assert_eq!(
            fetched["authorizerConfiguration"],
            body.get("authorizerConfiguration")
                .cloned()
                .unwrap_or(Value::Null),
            "case {case:?}: the authorizerConfiguration reads back as sent, got {}",
            fetched["authorizerConfiguration"]
        );
        assert_eq!(
            fetched["approvalConfiguration"]["autoApproval"],
            json!(expected_auto_approval),
            "case {case:?}: expected autoApproval {expected_auto_approval}, got {}",
            fetched["approvalConfiguration"]
        );
        assert_eq!(
            text(&fetched, "status"),
            "CREATING",
            "case {case:?}: a registry is created in CREATING, got {}",
            text(&fetched, "status")
        );
        assert!(
            fetched["statusReason"].is_null(),
            "case {case:?}: a registry that did not fail reports no statusReason, got {}",
            fetched["statusReason"]
        );
        assert!(
            fetched["updatedAt"] == fetched["createdAt"],
            "case {case:?}: expected createdAt and updatedAt to be the one create instant, got \
             {} and {}",
            fetched["createdAt"],
            fetched["updatedAt"]
        );
        for member in ["createdAt", "updatedAt"] {
            assert_date_time(case, member, &fetched[member]);
        }

        // The whole ARN is a `RegistryIdentifier` too, so it addresses the same record.
        let (status, by_arn) = get_registry(&router, &encoded_label(&arn)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 reading the registry by its ARN, got {status} \
             ({by_arn})"
        );
        assert_eq!(
            text(&by_arn, "registryId"),
            id,
            "case {case:?}: the ARN addresses the record its id addresses, got {}",
            text(&by_arn, "registryId")
        );

        settling.push((case, id, fetched["createdAt"].clone()));
    }

    // The transition to READY is the clock's: it is due at the settle instant whether
    // or not anybody reads the registry.
    tokio::time::sleep(PAST_SETTLE).await;
    for (case, id, created_at) in settling {
        let (status, fetched) = get_registry(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetRegistry, got {status} ({fetched})"
        );
        assert_eq!(
            text(&fetched, "status"),
            "READY",
            "case {case:?}: the clock owes the registry READY once its settle span has \
             passed, got {}",
            text(&fetched, "status")
        );
        assert_eq!(
            fetched["createdAt"], created_at,
            "case {case:?}: a read does not re-date the registry, got {} against {created_at}",
            fetched["createdAt"]
        );
    }
}

/// What `CreateRegistry` refuses, and with which modelled error: a name this account
/// already holds, and a `CUSTOM_JWT` authorizer with no configuration.
///
/// The rows differ along the rule that refuses the request, and each names the status
/// and error code a client branches on.
#[tokio::test]
async fn create_registry_refuses_a_duplicate_name_and_an_unconfigured_jwt_authorizer() {
    let router = router();

    let (status, created) = create_registry(&router, &json!({"name": "taken"})).await;
    assert_eq!(
        status, 202,
        "the first create of a name succeeds, got {status} ({created})"
    );
    let taken_id = text(&created, "registryArn")
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();

    // (case, the request body, expected status, expected error code)
    let rows: [(&str, Value, u16, &str); 4] = [
        (
            "the same name again",
            json!({"name": "taken"}),
            409,
            "ConflictException",
        ),
        (
            "the same name with a different configuration",
            json!({"name": "taken", "description": "another catalog"}),
            409,
            "ConflictException",
        ),
        (
            "CUSTOM_JWT with no authorizerConfiguration",
            json!({"name": "unconfigured", "authorizerType": "CUSTOM_JWT"}),
            400,
            "ValidationException",
        ),
        (
            "a name that differs only in case is its own registry",
            json!({"name": "Taken"}),
            202,
            "",
        ),
    ];

    for (case, body, expected_status, expected_code) in rows {
        let (status, answered) = create_registry(&router, &body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} ({answered})"
        );
        if expected_status == 202 {
            continue;
        }
        assert_eq!(
            error_code(&answered),
            expected_code,
            "case {case:?}: expected {expected_code}, got {} ({answered})",
            error_code(&answered)
        );
    }

    // The refused creates persisted nothing, and the one registry the name holds is
    // still the one the first create minted.
    let (status, fetched) = get_registry(&router, &taken_id).await;
    assert_eq!(
        status, 200,
        "the registry the first create minted is still readable, got {status} ({fetched})"
    );
    assert_eq!(
        fetched["description"],
        Value::Null,
        "a refused create does not overwrite the stored registry, got {}",
        fetched["description"]
    );
    let (status, missing) = get_registry(&router, "unconfigured00").await;
    assert_eq!(
        status, 404,
        "a refused create persisted no registry, got {status} ({missing})"
    );
}

/// A `clientToken` replayed against `CreateRegistry` answers with the registry it
/// first produced, and an identifier naming no registry is the modelled 404.
#[tokio::test]
async fn a_replayed_client_token_mints_one_registry_and_an_unknown_id_is_not_found() {
    let router = router();

    let token = client_token("idem");
    let body = json!({"name": "once", "clientToken": token});
    let (status, first) = create_registry(&router, &body).await;
    assert_eq!(
        status, 202,
        "the first create succeeds, got {status} ({first})"
    );

    // (case, the replayed body)
    let rows: [(&str, Value); 2] = [
        ("the same request again", body.clone()),
        (
            "the same token with other members changed",
            json!({"name": "once", "clientToken": token, "description": "changed"}),
        ),
    ];

    for (case, replayed) in rows {
        let (status, answered) = create_registry(&router, &replayed).await;
        assert_eq!(
            status, 202,
            "case {case:?}: a replayed token is not an error, got {status} ({answered})"
        );
        assert_eq!(
            text(&answered, "registryArn"),
            text(&first, "registryArn"),
            "case {case:?}: a replayed token answers with the first registry, got {}",
            text(&answered, "registryArn")
        );
    }

    // A different token under the same name is a second create of a held name.
    let (status, conflicted) = create_registry(
        &router,
        &json!({"name": "once", "clientToken": client_token("other")}),
    )
    .await;
    assert_eq!(
        status, 409,
        "a fresh token does not bypass the unique-name rule, got {status} ({conflicted})"
    );

    // (case, the identifier read, expected status)
    let reads: [(&str, String, u16); 3] = [
        (
            "the minted id",
            text(&first, "registryArn")
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_string(),
            200,
        ),
        ("an id no registry holds", "aaaaaaaaaaaa".to_string(), 404),
        (
            "an ARN no registry holds",
            encoded_label(&format!(
                "arn:aws:bedrock-agentcore:{REGION}:{ACCOUNT}:registry/bbbbbbbbbbbb"
            )),
            404,
        ),
    ];

    for (case, identifier, expected_status) in reads {
        let (status, answered) = get_registry(&router, &identifier).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} from GetRegistry, got {status} \
             ({answered})"
        );
        if expected_status == 404 {
            assert_eq!(
                error_code(&answered),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {} ({answered})",
                error_code(&answered)
            );
        }
    }
}
